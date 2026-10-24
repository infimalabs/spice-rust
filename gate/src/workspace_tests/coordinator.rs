//! Bounded shard coordinator for workspace Rust test execution.

use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const TEMP_ROOT_ATTEMPTS: usize = 100;
const SHARD_PROCESS_POLL: Duration = Duration::from_millis(50);
const TEMP_ENVIRONMENT: [&str; 3] = ["TMPDIR", "TMP", "TEMP"];
/// Each Cargo shard starts test binaries that would otherwise each claim every
/// host CPU. Retain modest intra-binary overlap while bounding the multiplier
/// across the admitted process lanes.
const MAX_TEST_HARNESS_THREADS_PER_SHARD: usize = 3;

#[derive(Clone, Copy, Debug)]
pub(super) struct RunPolicy {
    pub(super) concurrency: usize,
    pub(super) shared_outputs: bool,
    pub(super) cargo_jobs: Option<usize>,
    pub(super) test_threads: Option<usize>,
    pub(super) shard_deadline: Duration,
    pub(super) progress_interval: Duration,
    pub(super) output_drain: Duration,
    pub(super) reap: Duration,
    pub(super) grace: Duration,
}

impl RunPolicy {
    pub(super) fn production(concurrency: usize) -> Result<Self, WorkspaceTestError> {
        Self::production_with_cap(concurrency, WORKSPACE_TEST_CONCURRENCY_CAP, false)
    }

    pub(super) fn production_shared(concurrency: usize) -> Result<Self, WorkspaceTestError> {
        Self::production_with_cap(
            concurrency,
            crate::PREBUILT_WORKSPACE_TEST_CONCURRENCY_CAP,
            true,
        )
    }

    fn production_with_cap(
        concurrency: usize,
        cap: usize,
        shared_outputs: bool,
    ) -> Result<Self, WorkspaceTestError> {
        if !(1..=cap).contains(&concurrency) {
            return Err(WorkspaceTestError::new(format!(
                "workspace test concurrency must be between 1 and {}, \
                 got {concurrency}",
                cap,
            )));
        }
        Ok(Self {
            concurrency,
            shared_outputs,
            cargo_jobs: None,
            test_threads: None,
            shard_deadline: SHARD_DEADLINE,
            progress_interval: PROGRESS_INTERVAL,
            output_drain: OUTPUT_DRAIN_DEADLINE,
            reap: REAP_DEADLINE,
            grace: TERMINATION_GRACE,
        })
    }

    pub(super) const fn production_budgeted(budget: WorkspaceTestBudget) -> Self {
        Self {
            concurrency: budget.concurrency,
            shared_outputs: false,
            cargo_jobs: Some(budget.cargo_jobs),
            test_threads: Some(budget.test_threads),
            shard_deadline: SHARD_DEADLINE,
            progress_interval: PROGRESS_INTERVAL,
            output_drain: OUTPUT_DRAIN_DEADLINE,
            reap: REAP_DEADLINE,
            grace: TERMINATION_GRACE,
        }
    }

    /// Rebalance the admitted aggregate harness grant toward direct process lanes.
    pub(super) fn production_prebuilt_budgeted(budget: WorkspaceTestBudget) -> Self {
        let aggregate_threads = budget.concurrency.saturating_mul(budget.test_threads);
        let (concurrency, test_threads) = (1..=crate::PREBUILT_WORKSPACE_TEST_CONCURRENCY_CAP
            .min(aggregate_threads))
            .map(|concurrency| {
                let test_threads =
                    (aggregate_threads / concurrency).clamp(1, MAX_TEST_HARNESS_THREADS_PER_SHARD);
                (concurrency, test_threads)
            })
            .max_by_key(|(concurrency, test_threads)| {
                (concurrency.saturating_mul(*test_threads), *concurrency)
            })
            .expect("an admitted workspace-test budget retains direct capacity");
        Self {
            concurrency,
            shared_outputs: true,
            cargo_jobs: Some(budget.cargo_jobs),
            test_threads: Some(test_threads),
            shard_deadline: SHARD_DEADLINE,
            progress_interval: PROGRESS_INTERVAL,
            output_drain: OUTPUT_DRAIN_DEADLINE,
            reap: REAP_DEADLINE,
            grace: TERMINATION_GRACE,
        }
    }

    pub(super) fn for_shards(mut self, shards: usize) -> Self {
        self.concurrency = self.concurrency.min(if shards == 0 { 1 } else { shards });
        self
    }
}

#[derive(Clone, Debug)]
enum ExecutionShard {
    Cargo(PackageInventory),
    Prebuilt(PrebuiltTest),
}

impl ExecutionShard {
    fn package(package: PackageInventory) -> Self {
        Self::Cargo(package)
    }

    fn prebuilt(test: PrebuiltTest) -> Self {
        Self::Prebuilt(test)
    }

    fn label(&self) -> String {
        match self {
            Self::Cargo(package) => package.name.clone(),
            Self::Prebuilt(test) => test.package.clone(),
        }
    }

    fn target_list(&self) -> String {
        match self {
            Self::Cargo(package) => package
                .targets
                .iter()
                .map(TargetIdentity::display)
                .collect::<Vec<_>>()
                .join(","),
            Self::Prebuilt(test) => format!(
                "{}#partition-{}/{}",
                test.target.display(),
                test.partition_index() + 1,
                test.partition_count(),
            ),
        }
    }

    fn is_prebuilt(&self) -> bool {
        matches!(self, Self::Prebuilt(_))
    }

    fn requires_exclusive_lane(&self) -> bool {
        matches!(
            self,
            Self::Prebuilt(test) if test.requires_exclusive_lane()
        )
    }

    fn binary(&self) -> Option<&Path> {
        match self {
            Self::Cargo(_) => None,
            Self::Prebuilt(test) => Some(&test.executable),
        }
    }
}

struct ActiveShard {
    package: String,
    targets: String,
    output_lane: usize,
    started: Instant,
    observer: Arc<LastTest>,
    exclusive: bool,
    thread: thread::JoinHandle<()>,
}

pub(super) struct Coordinator<'a, W> {
    pending: VecDeque<(usize, ExecutionShard)>,
    available_lanes: VecDeque<usize>,
    active: BTreeMap<usize, ActiveShard>,
    results: Vec<ShardResult>,
    root: PathBuf,
    environment: Environment,
    cargo: OsString,
    policy: RunPolicy,
    launcher: Arc<dyn ShardLauncher>,
    cancellation: CancellationToken,
    sender: mpsc::Sender<ShardResult>,
    receiver: mpsc::Receiver<ShardResult>,
    output: &'a mut W,
    started: Instant,
    peak_concurrency: usize,
    failure: Option<String>,
    total: usize,
}

impl<W> Drop for Coordinator<'_, W> {
    fn drop(&mut self) {
        self.pending.clear();
        self.cancellation.cancel();
        for active in std::mem::take(&mut self.active).into_values() {
            let _ = active.thread.join();
        }
    }
}

impl<'a, W: Write> Coordinator<'a, W> {
    pub(super) fn new(
        shards: Vec<PackageInventory>,
        root: PathBuf,
        environment: Environment,
        cargo: OsString,
        policy: RunPolicy,
        output: &'a mut W,
    ) -> Self {
        Self::from_execution_shards(
            shards.into_iter().map(ExecutionShard::package).collect(),
            root,
            environment,
            cargo,
            policy,
            Arc::new(SystemShards),
            output,
        )
    }

    pub(super) fn new_prebuilt(
        tests: Vec<PrebuiltTest>,
        root: PathBuf,
        environment: Environment,
        cargo: OsString,
        policy: RunPolicy,
        output: &'a mut W,
    ) -> Self {
        Self::from_execution_shards(
            tests.into_iter().map(ExecutionShard::prebuilt).collect(),
            root,
            environment,
            cargo,
            policy,
            Arc::new(SystemShards),
            output,
        )
    }

    fn from_execution_shards(
        shards: Vec<ExecutionShard>,
        root: PathBuf,
        environment: Environment,
        cargo: OsString,
        policy: RunPolicy,
        launcher: Arc<dyn ShardLauncher>,
        output: &'a mut W,
    ) -> Self {
        let total = shards.len();
        let (sender, receiver) = mpsc::channel();
        let available_lanes = (0..policy.concurrency).collect();
        Self {
            pending: shards.into_iter().enumerate().collect(),
            available_lanes,
            active: BTreeMap::new(),
            results: Vec::new(),
            root,
            environment,
            cargo,
            policy,
            launcher,
            cancellation: CancellationToken::new(),
            sender,
            receiver,
            output,
            started: Instant::now(),
            peak_concurrency: 0,
            failure: None,
            total,
        }
    }

    pub(super) fn run(mut self) -> Result<(), WorkspaceTestError> {
        if self.policy.concurrency == 0 || self.policy.progress_interval.is_zero() {
            return Err(WorkspaceTestError::new(
                "coordinator concurrency and progress interval must be nonzero",
            ));
        }
        self.start_available()?;
        let mut next_progress = self.started + self.policy.progress_interval;
        while !self.active.is_empty() {
            let wait = self.next_wait(next_progress);
            match self.receiver.recv_timeout(wait) {
                Ok(result) => {
                    self.accept_result(result)?;
                    self.start_available()?;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.fail("all shard workers disconnected before reporting".to_owned());
                }
            }
            if Instant::now() >= next_progress {
                self.emit_progress()?;
                next_progress += self.policy.progress_interval;
            }
        }
        self.finish()
    }

    fn start_available(&mut self) -> Result<(), WorkspaceTestError> {
        if self.active.values().any(|active| active.exclusive) {
            return Ok(());
        }
        while self.failure.is_none() && self.active.len() < self.policy.concurrency {
            let Some(position) = self.startable_position() else {
                break;
            };
            let (index, shard) = self
                .pending
                .remove(position)
                .expect("startable position names one pending shard");
            let exclusive = shard.requires_exclusive_lane();
            let Some(output_lane) = self.available_lanes.pop_front() else {
                self.pending.insert(position, (index, shard));
                break;
            };
            let targets = shard.target_list();
            writeln!(
                self.output,
                concat!(
                    "[rust-test] stage=start shard={}/{} package={:?} ",
                    "targets={:?} output_lane={}"
                ),
                index + 1,
                self.total,
                shard.label(),
                targets,
                output_lane + 1
            )
            .map_err(output_error)?;
            self.output.flush().map_err(output_error)?;
            self.launch(index, shard, targets, output_lane);
            if exclusive {
                break;
            }
        }
        self.peak_concurrency = self.peak_concurrency.max(self.active.len());
        Ok(())
    }

    fn startable_position(&self) -> Option<usize> {
        let (_, first) = self.pending.front()?;
        if self.active.is_empty() {
            return Some(0);
        }
        if first.requires_exclusive_lane() {
            return None;
        }
        self.pending
            .iter()
            .position(|(_, shard)| !shard.requires_exclusive_lane())
    }

    fn launch(&mut self, index: usize, shard: ExecutionShard, targets: String, output_lane: usize) {
        let sender = self.sender.clone();
        let root = self.root.clone();
        let environment = self.environment.clone();
        let cargo = self.cargo.clone();
        let cancellation = self.cancellation.clone();
        let policy = self.policy;
        let launcher = Arc::clone(&self.launcher);
        let observer = Arc::new(LastTest::default());
        let worker_observer = Arc::clone(&observer);
        let panic_observer = Arc::clone(&observer);
        let panic_shard = shard.clone();
        let panic_targets = targets.clone();
        let package = shard.label();
        let exclusive = shard.requires_exclusive_lane();
        let started = Instant::now();
        let handle = thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_shard(
                    index,
                    shard,
                    root,
                    environment,
                    cargo,
                    cancellation,
                    policy,
                    launcher.as_ref(),
                    worker_observer,
                    output_lane,
                )
            }))
            .unwrap_or_else(|_| {
                failed_shard(
                    index,
                    panic_shard,
                    panic_targets,
                    started.elapsed(),
                    panic_observer.last(),
                    WorkspaceTestError::new("shard worker panicked"),
                )
            });
            let _ = sender.send(result);
        });
        self.active.insert(
            index,
            ActiveShard {
                package,
                targets,
                output_lane,
                started,
                observer,
                exclusive,
                thread: handle,
            },
        );
    }

    fn next_wait(&self, next_progress: Instant) -> Duration {
        let now = Instant::now();
        let progress = next_progress.saturating_duration_since(now);
        progress.min(Duration::from_secs(1))
    }

    fn accept_result(&mut self, result: ShardResult) -> Result<(), WorkspaceTestError> {
        let active = self.active.remove(&result.index).ok_or_else(|| {
            WorkspaceTestError::new(format!("unknown shard {} reported", result.index + 1))
        })?;
        if active.thread.join().is_err() {
            self.fail(format!(
                "package {:?} shard worker panicked",
                active.package
            ));
        }
        self.available_lanes.push_back(active.output_lane);
        self.emit_terminal(&result)?;
        if !result.success && self.failure.is_none() {
            self.fail(format!(
                concat!(
                    "package {:?} ended as {} at binary {:?}, last test {:?}; ",
                    "stdout-tail={:?}; stderr-tail={:?}"
                ),
                result.package,
                result.disposition,
                result.binary,
                result.last_test,
                result.stdout_tail,
                result.stderr_tail,
            ));
        }
        self.results.push(result);
        Ok(())
    }

    fn emit_terminal(&mut self, result: &ShardResult) -> Result<(), WorkspaceTestError> {
        writeln!(
            self.output,
            concat!(
                "[rust-test] stage=terminal shard={}/{} package={:?} targets={:?} ",
                "elapsed_ms={} disposition={} binary={:?} last_test={:?}"
            ),
            result.index + 1,
            self.total,
            result.package,
            result.targets,
            result.elapsed.as_millis(),
            result.disposition,
            result.binary,
            result.last_test,
        )
        .map_err(output_error)?;
        if !result.success {
            writeln!(
                self.output,
                "[rust-test] stdout-tail={:?}",
                result.stdout_tail
            )
            .map_err(output_error)?;
            writeln!(
                self.output,
                "[rust-test] stderr-tail={:?}",
                result.stderr_tail
            )
            .map_err(output_error)?;
        }
        self.output.flush().map_err(output_error)
    }

    fn emit_progress(&mut self) -> Result<(), WorkspaceTestError> {
        for (index, active) in &self.active {
            writeln!(
                self.output,
                concat!(
                    "[rust-test] stage=progress shard={}/{} package={:?} targets={:?} ",
                    "elapsed_ms={} output_lane={} last_test={:?}"
                ),
                index + 1,
                self.total,
                active.package,
                active.targets,
                active.started.elapsed().as_millis(),
                active.output_lane + 1,
                active.observer.last(),
            )
            .map_err(output_error)?;
        }
        self.output.flush().map_err(output_error)
    }

    fn fail(&mut self, detail: String) {
        self.failure.get_or_insert(detail);
        self.pending.clear();
        self.cancellation.cancel();
    }

    fn finish(mut self) -> Result<(), WorkspaceTestError> {
        let slowest = self.results.iter().max_by_key(|result| result.elapsed);
        let (slowest_package, slowest_elapsed) = slowest.map_or(("none", 0), |result| {
            (result.package.as_str(), result.elapsed.as_millis())
        });
        let disposition = if self.failure.is_some() {
            "failed"
        } else {
            "success"
        };
        writeln!(
            self.output,
            concat!(
                "[rust-test] coordinator disposition={} elapsed_ms={} peak_concurrency={} ",
                "slowest_package={:?} slowest_elapsed_ms={}"
            ),
            disposition,
            self.started.elapsed().as_millis(),
            self.peak_concurrency,
            slowest_package,
            slowest_elapsed,
        )
        .map_err(output_error)?;
        self.output.flush().map_err(output_error)?;
        match self.failure.take() {
            Some(detail) => Err(WorkspaceTestError::new(detail)),
            None if self.pending.is_empty() && self.results.len() == self.total => Ok(()),
            None => Err(WorkspaceTestError::new(format!(
                "coordinator covered {} of {} shards",
                self.results.len(),
                self.total
            ))),
        }
    }
}

pub(super) fn output_error(error: io::Error) -> WorkspaceTestError {
    WorkspaceTestError::new(format!("report output failed: {error}"))
}

#[derive(Default)]
pub(super) struct LastTest {
    line: Mutex<Option<String>>,
}

impl LastTest {
    pub(super) fn last(&self) -> String {
        self.line
            .lock()
            .ok()
            .and_then(|line| line.clone())
            .unwrap_or_else(|| "none".to_owned())
    }

    pub(super) fn observe_line(&self, line: String) -> Result<(), ObserveError> {
        if line.starts_with("test ") && !line.starts_with("test result:") {
            let mut last = self
                .line
                .lock()
                .map_err(|_| ObserveError::new("last-test observation lock poisoned"))?;
            *last = Some(line);
        }
        Ok(())
    }
}

impl RecordObserver for LastTest {
    fn observe(&self, record: ObservedRecord<'_>) -> Result<(), ObserveError> {
        let line = String::from_utf8_lossy(record.bytes())
            .trim_end()
            .to_owned();
        self.observe_line(line)
    }
}

struct ShardResult {
    index: usize,
    package: String,
    targets: String,
    elapsed: Duration,
    disposition: String,
    binary: String,
    last_test: String,
    stdout_tail: String,
    stderr_tail: String,
    success: bool,
}

/// The process boundary one shard crosses.
///
/// A launcher runs the composed specification to completion, reports each
/// observed stdout line to `observer`, and stops the shard's whole process
/// tree when `cancellation` fires or the specification's deadline elapses.
pub(super) trait ShardLauncher: Send + Sync {
    fn run(
        &self,
        spec: Spec,
        observer: Arc<LastTest>,
        cancellation: &CancellationToken,
        policy: RunPolicy,
    ) -> Result<WaitOutcome, WorkspaceTestError>;
}

/// Shards run as real host processes.
struct SystemShards;

impl ShardLauncher for SystemShards {
    fn run(
        &self,
        spec: Spec,
        observer: Arc<LastTest>,
        cancellation: &CancellationToken,
        policy: RunPolicy,
    ) -> Result<WaitOutcome, WorkspaceTestError> {
        let observation = Observation::new(
            OutputStream::Stdout,
            RECORD_FRAME_LIMIT,
            RECORD_QUEUE_CAPACITY,
            policy.output_drain,
            observer as Arc<dyn RecordObserver>,
        )
        .map_err(|error| WorkspaceTestError::new(error.to_string()))?;
        let running = crate::measure::spawn_observed(spec, Some(observation))
            .map_err(|error| WorkspaceTestError::new(format!("shard launch failed: {error}")))?;
        wait_for_shard_process(running, cancellation, policy)
    }
}

#[allow(clippy::too_many_arguments)]
fn run_shard(
    index: usize,
    shard: ExecutionShard,
    root: PathBuf,
    environment: Environment,
    cargo: OsString,
    cancellation: CancellationToken,
    policy: RunPolicy,
    launcher: &dyn ShardLauncher,
    observer: Arc<LastTest>,
    output_lane: usize,
) -> ShardResult {
    let started = Instant::now();
    let targets = shard.target_list();
    let owner = format!("workspace test package(s) {}", shard.label());
    let environment = if shard.is_prebuilt() {
        Ok(environment)
    } else if policy.shared_outputs && policy.concurrency > 1 {
        isolate_shared_target_output(environment, output_lane)
    } else if policy.concurrency == 1 {
        Ok(environment)
    } else {
        isolate_shard_outputs(environment, output_lane)
    };
    let outcome = (|| {
        let temporary = ShardTemporaryRoot::create()?;
        let environment = environment?.with_temporary_root(temporary.path())?;
        let spec = shard_spec(
            &cargo,
            &shard,
            &root,
            environment,
            cancellation.clone(),
            policy,
            &owner,
        )?;
        launcher.run(spec, Arc::clone(&observer), &cancellation, policy)
    })();
    shard_result(
        index,
        shard,
        targets,
        started.elapsed(),
        observer.last(),
        outcome,
    )
}

fn wait_for_shard_process(
    mut running: sill::Running,
    cancellation: &CancellationToken,
    policy: RunPolicy,
) -> Result<WaitOutcome, WorkspaceTestError> {
    loop {
        if cancellation.is_cancelled() || running.runtime_remaining().is_zero() {
            return wait_after_shard_stop(running, policy);
        }
        let interval = SHARD_PROCESS_POLL.min(running.runtime_remaining());
        match running.wait_for(interval).map_err(shard_wait_error)? {
            WaitStep::Exited(outcome) => return Ok(outcome),
            WaitStep::Running(next) => running = *next,
        }
    }
}

#[cfg(not(unix))]
fn wait_after_shard_stop(
    running: sill::Running,
    _policy: RunPolicy,
) -> Result<WaitOutcome, WorkspaceTestError> {
    running.wait().map_err(shard_wait_error)
}

#[cfg(unix)]
fn wait_after_shard_stop(
    running: sill::Running,
    policy: RunPolicy,
) -> Result<WaitOutcome, WorkspaceTestError> {
    {
        let mut faults = Vec::new();
        let descendants = match sill::DescendantGroups::discover(running.id()) {
            Ok(mut descendants) => {
                if let Err(error) = descendants.terminate() {
                    faults.push(format!(
                        "discoverable shard descendant termination failed: {error}"
                    ));
                }
                Some(descendants)
            }
            Err(error) => {
                faults.push(format!("shard descendant discovery failed: {error}"));
                None
            }
        };
        let outcome = match descendants {
            Some(mut descendants) => {
                let root_terminal = AtomicBool::new(false);
                let (outcome, descendant_reap) = thread::scope(|scope| {
                    let reaper = scope.spawn(|| {
                        descendants.reap_while_root_terminates(
                            policy.grace,
                            policy.reap,
                            policy.output_drain,
                            &root_terminal,
                        )
                    });
                    let outcome = running.wait();
                    root_terminal.store(true, AtomicOrdering::Release);
                    (outcome, reaper.join())
                });
                match descendant_reap {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        faults.push(format!("shard descendant reap failed: {error}"));
                    }
                    Err(_) => faults.push("shard descendant reap worker panicked".to_owned()),
                }
                outcome
            }
            None => running.wait(),
        };
        if !faults.is_empty() {
            if let Err(error) = outcome {
                faults.push(format!("shard process-group reap failed: {error}"));
            }
            return Err(WorkspaceTestError::new(faults.join("; ")));
        }
        outcome.map_err(shard_wait_error)
    }
}

fn shard_wait_error(error: sill::WaitError) -> WorkspaceTestError {
    WorkspaceTestError::new(format!("shard wait failed: {error}"))
}

trait PrivateTemporaryEnvironment {
    fn with_temporary_root(self, root: &Path) -> Result<Self, WorkspaceTestError>
    where
        Self: Sized;
}

impl PrivateTemporaryEnvironment for Environment {
    fn with_temporary_root(self, root: &Path) -> Result<Self, WorkspaceTestError> {
        let mut entries = self
            .entries()
            .iter()
            .cloned()
            .collect::<BTreeMap<OsString, OsString>>();
        for variable in TEMP_ENVIRONMENT {
            entries.insert(OsString::from(variable), root.as_os_str().to_owned());
        }
        Environment::new(entries).map_err(|error| WorkspaceTestError::new(error.to_string()))
    }
}

struct ShardTemporaryRoot(PathBuf);

impl ShardTemporaryRoot {
    fn create() -> Result<Self, WorkspaceTestError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..TEMP_ROOT_ATTEMPTS {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| WorkspaceTestError::new(error.to_string()))?
                .as_nanos();
            let path = shard_temporary_parent().join(format!(
                "swt-{stamp:x}-{:x}",
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(WorkspaceTestError::new(format!(
                        "workspace test temporary root could not be created: {error}"
                    )));
                }
            }
        }
        Err(WorkspaceTestError::new(format!(
            "no exclusive workspace test temporary root was available in {TEMP_ROOT_ATTEMPTS} attempts"
        )))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

fn shard_temporary_parent() -> PathBuf {
    std::env::temp_dir()
}

impl Drop for ShardTemporaryRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Give one concurrent worker lane sole ownership of every Cargo output publication.
///
/// # Errors
///
/// Returns an environment error if the rewritten target authority is invalid.
pub(super) fn isolate_shard_outputs(
    environment: Environment,
    output_lane: usize,
) -> Result<Environment, WorkspaceTestError> {
    // Without a completed shared prebuild, Cargo 1.97's build intermediates and
    // final target publications both need lane ownership. Stable suffixes on
    // both authorities permit independent compilers and retain warm output for
    // later shards in the same lane across repeated gate attempts.
    let mut entries = environment
        .entries()
        .iter()
        .cloned()
        .collect::<BTreeMap<OsString, OsString>>();
    for variable in [CARGO_TARGET_DIR, CARGO_BUILD_BUILD_DIR] {
        if let Some(root) = entries.get(OsStr::new(variable)).cloned() {
            let isolated = PathBuf::from(root)
                .join("spice-workspace-test-shards")
                .join(format!("shard-{}", output_lane + 1));
            entries.insert(OsString::from(variable), isolated.into_os_string());
        }
    }
    Environment::new(entries).map_err(|error| WorkspaceTestError::new(error.to_string()))
}

/// Give one shared-build worker lane sole ownership of final Cargo publications.
///
/// Cargo's `build.build-dir` remains shared so every lane consumes the completed
/// prebuild. `target-dir` is lane-local because `CARGO_BIN_EXE_*` paths published
/// there must remain present while sibling Cargo processes run their tests.
fn isolate_shared_target_output(
    environment: Environment,
    output_lane: usize,
) -> Result<Environment, WorkspaceTestError> {
    let mut entries = environment
        .entries()
        .iter()
        .cloned()
        .collect::<BTreeMap<OsString, OsString>>();
    if let Some(root) = entries.get(OsStr::new(CARGO_TARGET_DIR)).cloned() {
        let isolated = PathBuf::from(root)
            .join("spice-workspace-test-shards")
            .join(format!("shard-{}", output_lane + 1));
        entries.insert(OsString::from(CARGO_TARGET_DIR), isolated.into_os_string());
    }
    Environment::new(entries).map_err(|error| WorkspaceTestError::new(error.to_string()))
}

fn shard_spec(
    cargo: &OsStr,
    shard: &ExecutionShard,
    root: &Path,
    environment: Environment,
    cancellation: CancellationToken,
    policy: RunPolicy,
    owner: &str,
) -> Result<Spec, WorkspaceTestError> {
    let threads = policy.test_threads.map_or_else(
        || execution_test_threads(shard),
        |cap| cap.min(execution_test_threads(shard)),
    );
    let (program, arguments, cwd) = match shard {
        ExecutionShard::Cargo(package) => {
            let mut arguments = vec![
                OsString::from("test"),
                OsString::from("--locked"),
                OsString::from("--package"),
                OsString::from(&package.id),
            ];
            if let Some(jobs) = policy.cargo_jobs {
                arguments.extend([OsString::from("--jobs"), OsString::from(jobs.to_string())]);
            }
            for target in &package.targets {
                arguments.extend(target_arguments(target)?);
            }
            if let Some(features) =
                qualified_required_features(package.targets.iter().flat_map(|target| {
                    target
                        .required_features
                        .iter()
                        .map(|feature| (package.name.as_str(), feature.as_str()))
                }))
            {
                arguments.extend([OsString::from("--features"), OsString::from(features)]);
            }
            if policy.shared_outputs || policy.test_threads.is_some() {
                arguments.extend([
                    OsString::from("--"),
                    OsString::from(format!("--test-threads={threads}")),
                ]);
            }
            (cargo, arguments, root)
        }
        ExecutionShard::Prebuilt(test) => {
            let arguments = test.direct_arguments(threads);
            (
                test.executable.as_os_str(),
                arguments,
                test.package_root.as_path(),
            )
        }
    };
    let mut spec = process_spec(
        program,
        arguments,
        cwd,
        environment,
        policy.shard_deadline,
        Cancellation::Token(cancellation),
        owner,
    )?;
    spec.deadlines = Deadlines::new(policy.shard_deadline, policy.output_drain, policy.reap)
        .map_err(|error| WorkspaceTestError::new(error.to_string()))?;
    spec.grace = policy.grace;
    Ok(spec)
}

fn execution_test_threads(shard: &ExecutionShard) -> usize {
    let is_sill = match shard {
        ExecutionShard::Cargo(package) => package.name == "sill",
        ExecutionShard::Prebuilt(test) => test.package == "sill",
    };
    if is_sill {
        return 1;
    }
    default_test_threads()
}

fn default_test_threads() -> usize {
    thread::available_parallelism()
        .map(std::num::NonZero::get)
        .unwrap_or(1)
        .clamp(1, MAX_TEST_HARNESS_THREADS_PER_SHARD)
}

pub(super) fn target_arguments(
    target: &TargetIdentity,
) -> Result<Vec<OsString>, WorkspaceTestError> {
    let (selector, named) = match target.kind.as_str() {
        "bin" => ("--bin", true),
        "test" => ("--test", true),
        "example" => ("--example", true),
        "bench" => ("--bench", true),
        "lib" | "cdylib+rlib" | "proc-macro" => ("--lib", false),
        kind => {
            return Err(WorkspaceTestError::new(format!(
                "package {:?} target {:?} has unsupported Cargo kind {kind:?}",
                target.package, target.name
            )));
        }
    };
    let mut arguments = vec![OsString::from(selector)];
    if named {
        arguments.push(OsString::from(&target.name));
    }
    Ok(arguments)
}

pub(super) fn process_spec<I, S>(
    program: &OsStr,
    arguments: I,
    root: &Path,
    environment: Environment,
    runtime: Duration,
    cancellation: Cancellation,
    owner: &str,
) -> Result<Spec, WorkspaceTestError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    Ok(Spec {
        argv: Argv::new(program, arguments)
            .map_err(|error| WorkspaceTestError::new(error.to_string()))?,
        cwd: root.to_owned(),
        environment,
        group: ProcessGroup::New,
        deadlines: Deadlines::new(runtime, OUTPUT_DRAIN_DEADLINE, REAP_DEADLINE)
            .map_err(|error| WorkspaceTestError::new(error.to_string()))?,
        wait: WaitPolicy::new(owner, cancellation, RestartDisposition::Never)
            .map_err(|error| WorkspaceTestError::new(error.to_string()))?,
        output: Output::new(
            StdinPolicy::Inherit,
            StreamPolicy::Capture {
                limit: CAPTURE_LIMIT,
            },
            StreamPolicy::Capture {
                limit: CAPTURE_LIMIT,
            },
        ),
        redaction: Redaction::None,
        grace: TERMINATION_GRACE,
    })
}

fn shard_result(
    index: usize,
    shard: ExecutionShard,
    targets: String,
    elapsed: Duration,
    last_test: String,
    outcome: Result<WaitOutcome, WorkspaceTestError>,
) -> ShardResult {
    let Ok(outcome) = outcome else {
        return failed_shard(
            index,
            shard,
            targets,
            elapsed,
            last_test,
            outcome.unwrap_err(),
        );
    };
    let stdout_tail = captured_excerpt(outcome.stdout());
    let stderr_tail = captured_excerpt(outcome.stderr());
    let binary = shard.binary().map_or_else(
        || last_binary(outcome.stderr()),
        |executable| executable.display().to_string(),
    );
    let capture_failed = outcome.observation_failure().is_some()
        || !outcome.outputs_drained()
        || outcome.stdout().is_some_and(|output| output.truncated())
        || outcome.stderr().is_some_and(|output| output.truncated());
    let (disposition, success) = match outcome.completion() {
        Completion::Exited(status) if status.success() && !capture_failed => {
            ("success".to_owned(), true)
        }
        Completion::Exited(status) if capture_failed => (
            format!("capture-failed after {}", exit_name(*status)),
            false,
        ),
        Completion::Exited(status) => (exit_name(*status), false),
        Completion::TimedOut(_) => ("timeout".to_owned(), false),
        Completion::Cancelled(_) => ("cancelled".to_owned(), false),
    };
    ShardResult {
        index,
        package: shard.label(),
        targets,
        elapsed,
        disposition,
        binary,
        last_test,
        stdout_tail,
        stderr_tail,
        success,
    }
}

fn failed_shard(
    index: usize,
    shard: ExecutionShard,
    targets: String,
    elapsed: Duration,
    last_test: String,
    error: WorkspaceTestError,
) -> ShardResult {
    ShardResult {
        index,
        package: shard.label(),
        targets,
        elapsed,
        disposition: format!("supervision-error: {error}"),
        binary: "not-observed".to_owned(),
        last_test,
        stdout_tail: String::new(),
        stderr_tail: String::new(),
        success: false,
    }
}

pub(super) fn captured_excerpt(output: Option<&sill::CapturedOutput>) -> String {
    let Some(output) = output else {
        return "not-captured".to_owned();
    };
    let bytes = output.bytes();
    let start = bytes.len().saturating_sub(FAILURE_EXCERPT_LIMIT);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

fn last_binary(stderr: Option<&sill::CapturedOutput>) -> String {
    let Some(stderr) = stderr else {
        return "not-observed".to_owned();
    };
    String::from_utf8_lossy(stderr.bytes())
        .lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix("Running ").map(str::to_owned))
        .unwrap_or_else(|| "not-observed".to_owned())
}

pub(super) fn completion_name(completion: &Completion) -> String {
    match completion {
        Completion::Exited(status) => exit_name(*status),
        Completion::TimedOut(_) => "timeout".to_owned(),
        Completion::Cancelled(_) => "cancelled".to_owned(),
    }
}

fn exit_name(status: std::process::ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| "signal".to_owned(), |code| format!("exit code={code}"))
}

#[cfg(all(test, unix))]
mod tests;
