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
    let spec = Spec {
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
    };
    let outcome = sill::spawn(spec)
        .map_err(RunError::Launch)?
        .wait()
        .map_err(|error| RunError::Cleanup(error.to_string()))?;
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
                && PathBuf::from(command)
                    .file_name()
                    .is_some_and(|name| name == OsStr::new("cargo"))
        })
        .count()
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
