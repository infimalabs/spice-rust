//! Bounded shard coordinator for workspace Rust test execution.

use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) struct RunPolicy {
    pub(super) concurrency: usize,
    pub(super) shard_deadline: Duration,
    pub(super) coordinator_deadline: Duration,
    pub(super) progress_interval: Duration,
    pub(super) output_drain: Duration,
    pub(super) reap: Duration,
    pub(super) grace: Duration,
}

impl RunPolicy {
    pub(super) fn production(concurrency: usize) -> Result<Self, WorkspaceTestError> {
        if !(1..=WORKSPACE_TEST_CONCURRENCY_CAP).contains(&concurrency) {
            return Err(WorkspaceTestError::new(format!(
                "workspace test concurrency must be between 1 and {}, \
                 got {concurrency}",
                WORKSPACE_TEST_CONCURRENCY_CAP,
            )));
        }
        Ok(Self {
            concurrency,
            shard_deadline: SHARD_DEADLINE,
            coordinator_deadline: COORDINATOR_DEADLINE,
            progress_interval: PROGRESS_INTERVAL,
            output_drain: OUTPUT_DRAIN_DEADLINE,
            reap: REAP_DEADLINE,
            grace: TERMINATION_GRACE,
        })
    }

    pub(super) fn for_shards(mut self, shards: usize) -> Self {
        self.concurrency = self.concurrency.min(if shards == 0 { 1 } else { shards });
        self
    }
}

struct ActiveShard {
    package: String,
    targets: String,
    started: Instant,
    observer: Arc<LastTest>,
    thread: thread::JoinHandle<()>,
}

pub(super) struct Coordinator<'a, W> {
    pending: VecDeque<(usize, PackageInventory)>,
    active: BTreeMap<usize, ActiveShard>,
    results: Vec<ShardResult>,
    root: PathBuf,
    environment: Environment,
    cargo: OsString,
    policy: RunPolicy,
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
        let total = shards.len();
        let (sender, receiver) = mpsc::channel();
        Self {
            pending: shards.into_iter().enumerate().collect(),
            active: BTreeMap::new(),
            results: Vec::new(),
            root,
            environment,
            cargo,
            policy,
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
            self.enforce_coordinator_deadline();
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
        while self.failure.is_none() && self.active.len() < self.policy.concurrency {
            let Some((index, shard)) = self.pending.pop_front() else {
                break;
            };
            let targets = target_list(&shard);
            writeln!(
                self.output,
                "[rust-test] stage=start shard={}/{} package={:?} targets={:?}",
                index + 1,
                self.total,
                shard.name,
                targets
            )
            .map_err(output_error)?;
            self.output.flush().map_err(output_error)?;
            self.launch(index, shard, targets);
        }
        self.peak_concurrency = self.peak_concurrency.max(self.active.len());
        Ok(())
    }

    fn launch(&mut self, index: usize, shard: PackageInventory, targets: String) {
        let sender = self.sender.clone();
        let root = self.root.clone();
        let environment = self.environment.clone();
        let cargo = self.cargo.clone();
        let cancellation = self.cancellation.clone();
        let policy = self.policy;
        let observer = Arc::new(LastTest::default());
        let worker_observer = Arc::clone(&observer);
        let panic_observer = Arc::clone(&observer);
        let panic_shard = shard.clone();
        let panic_targets = targets.clone();
        let package = shard.name.clone();
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
                    worker_observer,
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
                started,
                observer,
                thread: handle,
            },
        );
    }

    fn enforce_coordinator_deadline(&mut self) {
        if self.failure.is_none() && self.started.elapsed() >= self.policy.coordinator_deadline {
            self.fail(format!(
                "coordinator exceeded its {} ms deadline",
                self.policy.coordinator_deadline.as_millis()
            ));
        }
    }

    fn next_wait(&self, next_progress: Instant) -> Duration {
        let now = Instant::now();
        let progress = next_progress.saturating_duration_since(now);
        let deadline =
            (self.started + self.policy.coordinator_deadline).saturating_duration_since(now);
        progress.min(deadline).min(Duration::from_secs(1))
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
                    "elapsed_ms={} last_test={:?}"
                ),
                index + 1,
                self.total,
                active.package,
                active.targets,
                active.started.elapsed().as_millis(),
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

fn target_list(shard: &PackageInventory) -> String {
    shard
        .targets
        .iter()
        .map(TargetIdentity::display)
        .collect::<Vec<_>>()
        .join(",")
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

#[allow(clippy::too_many_arguments)]
fn run_shard(
    index: usize,
    shard: PackageInventory,
    root: PathBuf,
    environment: Environment,
    cargo: OsString,
    cancellation: CancellationToken,
    policy: RunPolicy,
    observer: Arc<LastTest>,
) -> ShardResult {
    let started = Instant::now();
    let targets = target_list(&shard);
    let owner = format!("workspace test package {}", shard.name);
    let spec = shard_spec(
        &cargo,
        &shard,
        &root,
        environment,
        cancellation,
        policy,
        &owner,
    );
    let outcome = spec.and_then(|spec| {
        let observation = Observation::new(
            OutputStream::Stdout,
            RECORD_FRAME_LIMIT,
            RECORD_QUEUE_CAPACITY,
            Arc::clone(&observer) as Arc<dyn RecordObserver>,
        )
        .map_err(|error| WorkspaceTestError::new(error.to_string()))?;
        let running = sill::spawn_observed(spec, observation)
            .map_err(|error| WorkspaceTestError::new(format!("shard launch failed: {error}")))?;
        running
            .wait()
            .map_err(|error| WorkspaceTestError::new(format!("shard wait failed: {error}")))
    });
    shard_result(
        index,
        shard,
        targets,
        started.elapsed(),
        observer.last(),
        outcome,
    )
}

fn shard_spec(
    cargo: &OsStr,
    shard: &PackageInventory,
    root: &Path,
    environment: Environment,
    cancellation: CancellationToken,
    policy: RunPolicy,
    owner: &str,
) -> Result<Spec, WorkspaceTestError> {
    let mut arguments = vec![
        OsString::from("test"),
        OsString::from("--locked"),
        OsString::from("--package"),
        OsString::from(&shard.id),
    ];
    for target in &shard.targets {
        arguments.extend(target_arguments(target)?);
    }
    let mut spec = process_spec(
        cargo,
        arguments,
        root,
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
        group: crate::measure::bounded_process_group(),
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
    shard: PackageInventory,
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
    let binary = last_binary(outcome.stderr());
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
        package: shard.name,
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
    shard: PackageInventory,
    targets: String,
    elapsed: Duration,
    last_test: String,
    error: WorkspaceTestError,
) -> ShardResult {
    ShardResult {
        index,
        package: shard.name,
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
