use super::*;

#[cfg(unix)]
mod descendants;

pub(crate) fn run_process<I, S, P>(
    label: &str,
    program: P,
    arguments: I,
    cancellation: &CancellationToken,
    cargo_jobs: usize,
    overrides: &[(&str, &OsStr)],
) -> Result<CommandRun, RunError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
    P: Into<OsString>,
{
    let cwd = std::env::current_dir().map_err(RunError::Launch)?;
    run_process_at(
        label,
        program,
        arguments,
        &cwd,
        cancellation,
        cargo_jobs,
        overrides,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "observed action processes retain the ordinary rim plus progress source and authority"
)]
pub(crate) fn run_process_with_progress<I, S, P>(
    label: &str,
    program: P,
    arguments: I,
    cancellation: &CancellationToken,
    cargo_jobs: usize,
    overrides: &[(&str, &OsStr)],
    source: ProgressSource,
    progress: ActionProgress,
) -> Result<CommandRun, RunError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
    P: Into<OsString>,
{
    let cwd = std::env::current_dir().map_err(RunError::Launch)?;
    run_process_at_with_progress(
        label,
        program,
        arguments,
        &cwd,
        cancellation,
        cargo_jobs,
        overrides,
        source,
        progress,
    )
}

pub(crate) fn run_process_at<I, S, P>(
    label: &str,
    program: P,
    arguments: I,
    cwd: &Path,
    cancellation: &CancellationToken,
    cargo_jobs: usize,
    overrides: &[(&str, &OsStr)],
) -> Result<CommandRun, RunError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
    P: Into<OsString>,
{
    run_bounded_process(
        label,
        program,
        arguments,
        cwd,
        cancellation,
        Some(cargo_jobs),
        overrides,
        ACTION_PROCESS_BOUNDS,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "observed action processes retain the ordinary rim plus stream and progress authority"
)]
pub(crate) fn run_process_at_with_progress<I, S, P>(
    label: &str,
    program: P,
    arguments: I,
    cwd: &Path,
    cancellation: &CancellationToken,
    cargo_jobs: usize,
    overrides: &[(&str, &OsStr)],
    source: ProgressSource,
    progress: ActionProgress,
) -> Result<CommandRun, RunError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
    P: Into<OsString>,
{
    let spec = bounded_process_spec(
        label,
        program,
        arguments,
        cwd,
        cancellation,
        Some(cargo_jobs),
        overrides,
        ACTION_PROCESS_BOUNDS,
    )?;
    let observation = Observation::new(
        source.stream(),
        ACTION_PROCESS_BOUNDS.output_limit,
        FULL_VERIFIER_LIVE_QUEUE_CAPACITY,
        Arc::new(OutputProgressObserver {
            progress,
            source,
            observed: Mutex::new(BTreeSet::new()),
        }),
    )
    .map_err(|error| RunError::Launch(io::Error::new(io::ErrorKind::InvalidInput, error)))?;
    let running = sill::spawn_observed(spec, observation).map_err(RunError::Launch)?;
    let outcome = wait_bounded(running, cancellation, ACTION_PROCESS_BOUNDS)?;
    command_run_from_outcome(label, &outcome)
}

pub(super) struct OutputProgressObserver {
    progress: ActionProgress,
    source: ProgressSource,
    observed: Mutex<BTreeSet<Vec<u8>>>,
}

impl OutputProgressObserver {
    #[cfg(test)]
    pub(super) fn fixture(source: ProgressSource) -> Self {
        Self {
            progress: Arc::new(|| {}),
            source,
            observed: Mutex::new(BTreeSet::new()),
        }
    }

    pub(super) fn newly_advances(&self, record: &[u8]) -> Result<bool, ObserveError> {
        if !self.source.advances(record) {
            return Ok(false);
        }
        if matches!(self.source, ProgressSource::CargoStderr) {
            return Ok(true);
        }
        self.observed
            .lock()
            .map_err(|error| ObserveError::new(error.to_string()))
            .map(|mut observed| observed.insert(record.to_vec()))
    }
}

impl RecordObserver for OutputProgressObserver {
    fn observe(&self, record: ObservedRecord<'_>) -> Result<(), ObserveError> {
        if self.newly_advances(record.bytes())? {
            (self.progress)();
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ProgressSource {
    CargoStderr,
    #[cfg(not(target_arch = "wasm32"))]
    WorkspaceStdout,
    BrowserStdout,
}

impl ProgressSource {
    const fn stream(self) -> OutputStream {
        match self {
            Self::CargoStderr => OutputStream::Stderr,
            Self::BrowserStdout => OutputStream::Stdout,
            #[cfg(not(target_arch = "wasm32"))]
            Self::WorkspaceStdout => OutputStream::Stdout,
        }
    }

    pub(super) fn advances(self, record: &[u8]) -> bool {
        match self {
            Self::CargoStderr => true,
            #[cfg(not(target_arch = "wasm32"))]
            Self::WorkspaceStdout => {
                record.starts_with(b"[rust-test] stage=start ")
                    || record.starts_with(b"[rust-test] stage=terminal ")
            }
            Self::BrowserStdout => record.starts_with(b"CANVAS-SHARD {\"kind\":\"scenario-end\""),
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the process boundary keeps cwd, cancellation, environment, and bounds explicit"
)]
fn run_bounded_process<I, S, P>(
    label: &str,
    program: P,
    arguments: I,
    cwd: &Path,
    cancellation: &CancellationToken,
    cargo_jobs: Option<usize>,
    overrides: &[(&str, &OsStr)],
    bounds: ProcessBounds,
) -> Result<CommandRun, RunError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
    P: Into<OsString>,
{
    let spec = bounded_process_spec(
        label,
        program,
        arguments,
        cwd,
        cancellation,
        cargo_jobs,
        overrides,
        bounds,
    )?;
    let running = sill::spawn(spec).map_err(RunError::Launch)?;
    let outcome = wait_bounded(running, cancellation, bounds)?;
    command_run_from_outcome(label, &outcome)
}

fn wait_bounded(
    mut running: sill::Running,
    cancellation: &CancellationToken,
    bounds: ProcessBounds,
) -> Result<sill::WaitOutcome, RunError> {
    loop {
        if cancellation.is_cancelled() || running.runtime_remaining().is_zero() {
            return wait_after_stop_request(running, bounds);
        }
        let interval = SCHEDULER_POLL.min(running.runtime_remaining());
        match running
            .wait_for(interval)
            .map_err(|error| RunError::Cleanup(error.to_string()))?
        {
            WaitStep::Exited(outcome) => return Ok(outcome),
            WaitStep::Running(next) => running = *next,
        }
    }
}

fn wait_after_stop_request(
    running: sill::Running,
    bounds: ProcessBounds,
) -> Result<sill::WaitOutcome, RunError> {
    #[cfg(not(unix))]
    let _ = bounds;
    #[cfg(unix)]
    let mut faults = Vec::new();
    #[cfg(unix)]
    let descendants = match descendants::DescendantGroups::discover(running.id()) {
        Ok(descendants) => {
            if let Err(error) = descendants.terminate() {
                faults.push(format!(
                    "discoverable descendant termination failed: {error}"
                ));
            }
            Some(descendants)
        }
        Err(error) => {
            faults.push(format!("descendant discovery failed: {error}"));
            None
        }
    };

    let outcome = running.wait();

    #[cfg(unix)]
    if let Some(descendants) = descendants
        && let Err(error) = descendants.reap(bounds.grace, bounds.reap)
    {
        faults.push(format!("descendant reap failed: {error}"));
    }
    #[cfg(unix)]
    if !faults.is_empty() {
        if let Err(error) = outcome {
            faults.push(format!("action process-group reap failed: {error}"));
        }
        return Err(RunError::Cleanup(faults.join("; ")));
    }

    outcome.map_err(|error| RunError::Cleanup(error.to_string()))
}

#[derive(Debug)]
pub(super) enum StreamingRunError {
    Process(RunError),
    Output(io::Error),
}

#[derive(Debug)]
struct LiveProgressObserver {
    records: mpsc::SyncSender<LiveVerifierRecord>,
}

struct LiveVerifierRecord {
    bytes: Vec<u8>,
    renews_lease: bool,
}

impl RecordObserver for LiveProgressObserver {
    fn observe(&self, record: ObservedRecord<'_>) -> Result<(), ObserveError> {
        if is_live_progress_record(record.bytes()) {
            self.records
                .send(LiveVerifierRecord {
                    bytes: record.bytes().to_vec(),
                    renews_lease: is_lease_renewal_record(record.bytes()),
                })
                .map_err(|error| ObserveError::new(error.to_string()))?;
        }
        Ok(())
    }
}

struct ProgressLease {
    inactivity: Duration,
    last_observed: Instant,
}

impl ProgressLease {
    fn new(inactivity: Duration) -> Self {
        Self {
            inactivity,
            last_observed: Instant::now(),
        }
    }

    fn renew(&mut self) {
        self.last_observed = Instant::now();
    }

    fn remaining(&self) -> Duration {
        self.inactivity.saturating_sub(self.last_observed.elapsed())
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the streaming process boundary retains the ordinary rim plus its explicit sink"
)]
pub(super) fn run_bounded_process_streaming<I, S, P>(
    label: &str,
    program: P,
    arguments: I,
    cwd: &Path,
    cancellation: &CancellationToken,
    cargo_jobs: Option<usize>,
    overrides: &[(&str, &OsStr)],
    bounds: ProgressLeaseBounds,
    output: &mut impl Write,
) -> Result<CommandRun, StreamingRunError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
    P: Into<OsString>,
{
    if bounds.inactivity.is_zero() || bounds.inactivity > bounds.process.runtime {
        return Err(StreamingRunError::Process(RunError::Launch(
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the verifier progress inactivity lease must be nonzero and no longer than its absolute runtime",
            ),
        )));
    }
    let process_cancellation = CancellationToken::new();
    let spec = bounded_process_spec(
        label,
        program,
        arguments,
        cwd,
        &process_cancellation,
        cargo_jobs,
        overrides,
        bounds.process,
    )
    .map_err(StreamingRunError::Process)?;
    let (records, received) = mpsc::sync_channel(FULL_VERIFIER_LIVE_QUEUE_CAPACITY);
    let observation = Observation::new(
        OutputStream::Stdout,
        FULL_VERIFIER_LIVE_FRAME_LIMIT,
        FULL_VERIFIER_LIVE_QUEUE_CAPACITY,
        Arc::new(LiveProgressObserver { records }),
    )
    .map_err(|error| {
        StreamingRunError::Process(RunError::Launch(io::Error::new(
            io::ErrorKind::InvalidInput,
            error.to_string(),
        )))
    })?;
    let mut running = sill::spawn_observed(spec, observation)
        .map_err(RunError::Launch)
        .map_err(StreamingRunError::Process)?;
    let mut lease = ProgressLease::new(bounds.inactivity);
    let mut inactivity_expired = false;

    let outcome = loop {
        let progress = match drain_live_progress(&received, output) {
            Ok(progress) => progress,
            Err(error) => {
                cancellation.cancel();
                process_cancellation.cancel();
                drop(received);
                let _cleanup = wait_after_stop_request(running, bounds.process);
                return Err(StreamingRunError::Output(error));
            }
        };
        if progress {
            lease.renew();
        }
        if cancellation.is_cancelled() {
            process_cancellation.cancel();
            break wait_after_stop_request(running, bounds.process)
                .map_err(StreamingRunError::Process)?;
        }
        if running.runtime_remaining().is_zero() {
            break wait_after_stop_request(running, bounds.process)
                .map_err(StreamingRunError::Process)?;
        }
        let lease_remaining = lease.remaining();
        if lease_remaining.is_zero() {
            inactivity_expired = true;
            process_cancellation.cancel();
            break wait_after_stop_request(running, bounds.process)
                .map_err(StreamingRunError::Process)?;
        }
        let observation = FULL_VERIFIER_LIVE_POLL
            .min(running.runtime_remaining())
            .min(lease_remaining);
        match running
            .wait_for(observation)
            .map_err(|error| RunError::Cleanup(error.to_string()))
            .map_err(StreamingRunError::Process)?
        {
            WaitStep::Exited(outcome) => break outcome,
            WaitStep::Running(next) => running = *next,
        }
    };
    drain_final_live_progress(&received, output).map_err(StreamingRunError::Output)?;
    let run = command_run_from_outcome(label, &outcome).map_err(StreamingRunError::Process)?;
    Ok(if inactivity_expired {
        force_timed_out(run)
    } else {
        run
    })
}

fn force_timed_out(run: CommandRun) -> CommandRun {
    let output = match run {
        CommandRun::Exited(output)
        | CommandRun::Cancelled(output)
        | CommandRun::TimedOut(output) => output,
    };
    CommandRun::TimedOut(output)
}

fn drain_live_progress(
    records: &mpsc::Receiver<LiveVerifierRecord>,
    output: &mut impl Write,
) -> io::Result<bool> {
    let mut wrote = false;
    let mut renews_lease = false;
    while let Ok(record) = records.try_recv() {
        output.write_all(&record.bytes)?;
        renews_lease |= record.renews_lease;
        wrote = true;
    }
    if wrote {
        output.flush()?;
    }
    Ok(renews_lease)
}

/// Observation completion and process completion are separate threads. After
/// the child exits, wait one bounded live poll after the last record so a final
/// terminal record cannot remain between the observer and its queue.
fn drain_final_live_progress(
    records: &mpsc::Receiver<LiveVerifierRecord>,
    output: &mut impl Write,
) -> io::Result<bool> {
    let mut wrote = drain_live_progress(records, output)?;
    while let Ok(record) = records.recv_timeout(FULL_VERIFIER_LIVE_POLL) {
        output.write_all(&record.bytes)?;
        wrote = true;
    }
    if wrote {
        output.flush()?;
    }
    Ok(wrote)
}

#[expect(
    clippy::too_many_arguments,
    reason = "the process specification binds every authority at one construction point"
)]
fn bounded_process_spec<I, S, P>(
    label: &str,
    program: P,
    arguments: I,
    cwd: &Path,
    cancellation: &CancellationToken,
    cargo_jobs: Option<usize>,
    overrides: &[(&str, &OsStr)],
    bounds: ProcessBounds,
) -> Result<Spec, RunError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
    P: Into<OsString>,
{
    let mut environment = ambient_environment()?
        .entries()
        .iter()
        .cloned()
        .collect::<BTreeMap<_, _>>();
    if let Some(cargo_jobs) = cargo_jobs {
        environment.insert(
            OsString::from("CARGO_BUILD_JOBS"),
            OsString::from(cargo_jobs.to_string()),
        );
    }
    for (key, value) in overrides {
        environment.insert(OsString::from(key), (*value).to_owned());
    }
    let environment = Environment::new(environment).map_err(|error| {
        RunError::Launch(io::Error::new(
            io::ErrorKind::InvalidInput,
            error.to_string(),
        ))
    })?;
    let deadlines =
        Deadlines::new(bounds.runtime, bounds.output_drain, bounds.reap).map_err(|error| {
            RunError::Launch(io::Error::new(
                io::ErrorKind::InvalidInput,
                error.to_string(),
            ))
        })?;
    let wait = WaitPolicy::new(
        format!("verifier action {label}"),
        Cancellation::Token(cancellation.clone()),
        RestartDisposition::Never,
    )
    .map_err(|error| {
        RunError::Launch(io::Error::new(
            io::ErrorKind::InvalidInput,
            error.to_string(),
        ))
    })?;
    Ok(Spec {
        argv: Argv::new(program, arguments).map_err(|error| {
            RunError::Launch(io::Error::new(
                io::ErrorKind::InvalidInput,
                error.to_string(),
            ))
        })?,
        cwd: cwd.to_owned(),
        environment,
        group: ProcessGroup::New,
        deadlines,
        wait,
        output: SillOutput::new(
            StdinPolicy::Null,
            StreamPolicy::Capture {
                limit: bounds.output_limit,
            },
            StreamPolicy::Capture {
                limit: bounds.output_limit,
            },
        ),
        redaction: Redaction::None,
        grace: bounds.grace,
    })
}

fn command_run_from_outcome(
    label: &str,
    outcome: &sill::WaitOutcome,
) -> Result<CommandRun, RunError> {
    if let Some(failure) = outcome.observation_failure() {
        return Err(RunError::Cleanup(format!(
            "{label} live output observation failed: {failure}"
        )));
    }
    if !outcome.outputs_drained() {
        return Err(RunError::Cleanup(format!(
            "{label} output did not drain before cleanup"
        )));
    }
    let stdout = captured(label, "stdout", outcome.stdout())?;
    let stderr = captured(label, "stderr", outcome.stderr())?;
    let status = outcome.completion().status();
    let output = ProcessOutput {
        status: Status {
            success: status.success(),
            code: status.code(),
        },
        stdout,
        stderr,
    };
    Ok(match outcome.completion() {
        Completion::Exited(_) => CommandRun::Exited(output),
        Completion::Cancelled(_) => CommandRun::Cancelled(output),
        Completion::TimedOut(_) => CommandRun::TimedOut(output),
    })
}

#[expect(
    clippy::disallowed_methods,
    reason = "the verifier's native process rim captures the exact child environment"
)]
fn ambient_environment() -> Result<Environment, RunError> {
    Environment::new(std::env::vars_os()).map_err(|error| {
        RunError::Launch(io::Error::new(
            io::ErrorKind::InvalidInput,
            error.to_string(),
        ))
    })
}

fn captured(
    label: &str,
    stream: &str,
    output: Option<&sill::CapturedOutput>,
) -> Result<Vec<u8>, RunError> {
    let output = output.ok_or_else(|| {
        RunError::Cleanup(format!("{label} {stream} capture is unexpectedly absent"))
    })?;
    if output.truncated() || !output.complete() {
        return Err(RunError::Cleanup(format!(
            "{label} {stream} capture exceeded its bounded complete-output contract"
        )));
    }
    Ok(output.bytes().to_vec())
}

pub(super) fn active_peer_cargo_jobs() -> io::Result<usize> {
    let output = Command::new("ps")
        .args(["-Ao", "pid=,ppid=,comm="])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "ps exited {:?} while counting peer Cargo jobs",
            output.status.code()
        )));
    }
    let listing = String::from_utf8(output.stdout).map_err(io::Error::other)?;
    Ok(count_peer_cargo(&listing, std::process::id()))
}

pub(super) fn count_peer_cargo(listing: &str, current: u32) -> usize {
    let processes = listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((
                fields.next()?.parse::<u32>().ok()?,
                fields.next()?.parse::<u32>().ok()?,
                fields.next()?.to_owned(),
            ))
        })
        .collect::<Vec<_>>();
    let parents = processes
        .iter()
        .map(|(pid, parent, _)| (*pid, *parent))
        .collect::<BTreeMap<_, _>>();
    let mut ancestors = BTreeSet::new();
    let mut cursor = current;
    while let Some(parent) = parents.get(&cursor).copied() {
        if parent == 0 || !ancestors.insert(parent) {
            break;
        }
        cursor = parent;
    }
    processes
        .iter()
        .filter(|(pid, _, command)| {
            !ancestors.contains(pid)
                && !descends_from(*pid, current, &parents)
                && PathBuf::from(command)
                    .file_name()
                    .is_some_and(|name| name == OsStr::new("cargo"))
        })
        .count()
}

fn descends_from(process: u32, ancestor: u32, parents: &BTreeMap<u32, u32>) -> bool {
    let mut cursor = process;
    let mut visited = BTreeSet::new();
    while let Some(parent) = parents.get(&cursor).copied() {
        if parent == ancestor {
            return true;
        }
        if parent == 0 || !visited.insert(parent) {
            return false;
        }
        cursor = parent;
    }
    false
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) struct SignalGuard {
    handle: signal_hook::iterator::Handle,
    join: Option<thread::JoinHandle<()>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl SignalGuard {
    pub(super) fn install(
        cancellation: CancellationToken,
        interrupted: Arc<AtomicBool>,
    ) -> Result<Self, VerifyError> {
        use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};

        let mut signals = signal_hook::iterator::Signals::new([SIGHUP, SIGINT, SIGTERM])
            .map_err(VerifyError::InterruptInstall)?;
        let handle = signals.handle();
        let join = thread::Builder::new()
            .name("verify-signal-bridge".to_owned())
            .spawn(move || {
                if signals.forever().next().is_some() {
                    interrupted.store(true, Ordering::Release);
                    cancellation.cancel();
                }
            })
            .map_err(VerifyError::InterruptInstall)?;
        Ok(Self {
            handle,
            join: Some(join),
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for SignalGuard {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub(super) struct SignalGuard;

#[cfg(target_arch = "wasm32")]
impl SignalGuard {
    pub(super) fn install(
        _cancellation: CancellationToken,
        _interrupted: Arc<AtomicBool>,
    ) -> Result<Self, VerifyError> {
        Ok(Self)
    }
}
