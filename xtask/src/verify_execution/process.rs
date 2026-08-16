#[cfg(unix)]
mod descendants;

pub(super) fn run_process<I, S, P>(
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

pub(super) fn run_process_at<I, S, P>(
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
enum StreamingRunError {
    Process(RunError),
    Output(io::Error),
}

#[derive(Debug)]
struct LiveProgressObserver {
    records: mpsc::SyncSender<Vec<u8>>,
}

impl RecordObserver for LiveProgressObserver {
    fn observe(&self, record: ObservedRecord<'_>) -> Result<(), ObserveError> {
        if is_live_progress_record(record.bytes()) {
            self.records
                .send(record.bytes().to_vec())
                .map_err(|error| ObserveError::new(error.to_string()))?;
        }
        Ok(())
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the streaming process boundary retains the ordinary rim plus its explicit sink"
)]
fn run_bounded_process_streaming<I, S, P>(
    label: &str,
    program: P,
    arguments: I,
    cwd: &Path,
    cancellation: &CancellationToken,
    cargo_jobs: Option<usize>,
    overrides: &[(&str, &OsStr)],
    bounds: ProcessBounds,
    output: &mut impl Write,
) -> Result<CommandRun, StreamingRunError>
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

    let outcome = loop {
        if let Err(error) = drain_live_progress(&received, output) {
            cancellation.cancel();
            drop(received);
            let _cleanup = wait_after_stop_request(running, bounds);
            return Err(StreamingRunError::Output(error));
        }
        if cancellation.is_cancelled() || running.runtime_remaining().is_zero() {
            break wait_after_stop_request(running, bounds).map_err(StreamingRunError::Process)?;
        }
        let observation = FULL_VERIFIER_LIVE_POLL.min(running.runtime_remaining());
        match running
            .wait_for(observation)
            .map_err(|error| RunError::Cleanup(error.to_string()))
            .map_err(StreamingRunError::Process)?
        {
            WaitStep::Exited(outcome) => break outcome,
            WaitStep::Running(next) => running = *next,
        }
    };
    drain_live_progress(&received, output).map_err(StreamingRunError::Output)?;
    command_run_from_outcome(label, &outcome).map_err(StreamingRunError::Process)
}

fn drain_live_progress(
    records: &mpsc::Receiver<Vec<u8>>,
    output: &mut impl Write,
) -> io::Result<()> {
    let mut wrote = false;
    while let Ok(record) = records.try_recv() {
        output.write_all(&record)?;
        wrote = true;
    }
    if wrote {
        output.flush()?;
    }
    Ok(())
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

fn active_peer_cargo_jobs() -> io::Result<usize> {
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

fn count_peer_cargo(listing: &str, current: u32) -> usize {
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

fn descends_from(pid: u32, ancestor: u32, parents: &BTreeMap<u32, u32>) -> bool {
    let mut seen = BTreeSet::new();
    let mut cursor = pid;
    while let Some(parent) = parents.get(&cursor).copied() {
        if parent == ancestor {
            return true;
        }
        if parent == 0 || !seen.insert(parent) {
            break;
        }
        cursor = parent;
    }
    false
}

#[cfg(not(target_arch = "wasm32"))]
struct SignalGuard {
    handle: signal_hook::iterator::Handle,
    join: Option<thread::JoinHandle<()>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl SignalGuard {
    fn install(
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
struct SignalGuard;

#[cfg(target_arch = "wasm32")]
impl SignalGuard {
    fn install(
        _cancellation: CancellationToken,
        _interrupted: Arc<AtomicBool>,
    ) -> Result<Self, VerifyError> {
        Ok(Self)
    }
}
