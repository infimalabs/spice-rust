//! Supported Cargo launcher over one private worktree home.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
#[cfg(unix)]
use std::thread::JoinHandle;

use serde::Serialize;
use spice::cargo_isolation::{
    BOOTSTRAP_HOME_VARIABLE, CARGO_HOME_VARIABLE, Host, NATIVE_CARGO_VARIABLE,
    NATIVE_RUSTC_VARIABLE, NATIVE_RUSTDOC_VARIABLE, RUSTC_VARIABLE, RUSTDOC_VARIABLE,
    prepare_manual,
};
#[cfg(unix)]
use spice::cargo_output::{self, OutputLease, Pass};
use spice::repository::Repository;
use wire::{Fault, FaultClass, Outcome, Reply, ReplyPosition};

mod capture;
#[cfg(test)]
mod tests;

use capture::{Stage, Stdout};

/// Result of the isolated native Cargo process boundary.
pub(crate) enum RunResult {
    Child(ExitStatus),
    Rejected { outcome: Outcome<()>, code: i32 },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum StoreAnswer {
    Inspection(spice::cargo_isolation::Inspection),
    Cleanup(spice::cargo_isolation::CleanupReport),
}

#[cfg(unix)]
struct SignalRelay {
    handle: signal_hook::iterator::Handle,
    worker: Option<JoinHandle<()>>,
}

#[cfg(unix)]
impl SignalRelay {
    fn install(group: rustix::process::Pid) -> io::Result<Self> {
        use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};

        let mut signals = signal_hook::iterator::Signals::new([SIGHUP, SIGINT, SIGTERM])?;
        let handle = signals.handle();
        let worker = std::thread::Builder::new()
            .name("spice-cargo-signals".to_owned())
            .spawn(move || forward(group, signals.forever()))?;
        Ok(Self {
            handle,
            worker: Some(worker),
        })
    }
}

/// Forward every relayed signal to Cargo's whole process group.
#[cfg(unix)]
fn forward(group: rustix::process::Pid, signals: impl IntoIterator<Item = std::ffi::c_int>) {
    use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};

    for signal in signals {
        let signal = match signal {
            SIGHUP => rustix::process::Signal::HUP,
            SIGINT => rustix::process::Signal::INT,
            SIGTERM => rustix::process::Signal::TERM,
            _ => continue,
        };
        let _ = rustix::process::kill_process_group(group, signal);
    }
}

#[cfg(unix)]
impl Drop for SignalRelay {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// How one developer Cargo invocation is launched and reported.
struct Invocation {
    arguments: Vec<OsString>,
    surface: &'static str,
    identity: String,
    stdout: Stdout,
    verbose: bool,
}

impl Invocation {
    /// Reporting for this invocation, written to `sink` and retained under
    /// `directory`.
    fn settings(&self, sink: capture::Sink, directory: PathBuf) -> capture::Settings {
        capture::Settings {
            surface: self.surface,
            identity: self.identity.clone(),
            sink,
            heartbeat: crate::gate_command::output::HEARTBEAT,
            verbose: self.verbose,
            directory,
        }
    }
}

/// Cargo subcommands whose product is a rewritten `Cargo.lock`.
const LOCK_WRITING_SUBCOMMANDS: [&str; 5] = ["update", "generate-lockfile", "add", "remove", "rm"];

/// Global Cargo options that take their value as the following argument.
const VALUED_GLOBAL_OPTIONS: [&str; 4] = ["--config", "--color", "-Z", "-C"];

/// Whether `arguments` name a subcommand that rewrites `Cargo.lock`.
///
/// The subcommand is the first argument that is neither a `+toolchain`
/// selector nor a global option; a valued global option written as two
/// arguments skips its value. `--locked` and `--frozen` do not change the
/// answer: Cargo itself refuses the rewrite under them.
fn writes_lock(arguments: &[OsString]) -> bool {
    let mut arguments = arguments.iter().map(|argument| argument.to_string_lossy());
    while let Some(argument) = arguments.next() {
        if argument.starts_with('+') {
            continue;
        }
        if argument.starts_with('-') {
            if VALUED_GLOBAL_OPTIONS.contains(&argument.as_ref()) {
                arguments.next();
            }
            continue;
        }
        return LOCK_WRITING_SUBCOMMANDS.contains(&argument.as_ref());
    }
    false
}

/// Run native Cargo for the arguments after `--`.
///
/// A lock-writing subcommand runs under the bootstrap Cargo home, which may
/// consult the registry and rewrite `Cargo.lock`; the next invocation seeds
/// its generation from that lock. Every other subcommand prepares the exact
/// current lock generation and runs offline within it.
pub(crate) fn run(matches: &clap::ArgMatches) -> RunResult {
    run_invocation(cargo_invocation(matches))
}

/// Run the repository's task binary inside the same admitted Cargo boundary.
pub(crate) fn xtask(matches: &clap::ArgMatches) -> RunResult {
    run_invocation(xtask_invocation(matches))
}

/// Every argument after `--` reaches Cargo verbatim; only flags before it are
/// the wrapper's.
fn cargo_invocation(matches: &clap::ArgMatches) -> Invocation {
    let arguments = matches
        .get_many::<OsString>("CARGO_ARGS")
        .expect("the generated parser requires trailing Cargo arguments")
        .cloned()
        .collect::<Vec<_>>();
    Invocation {
        identity: capture::identity("cargo", &arguments),
        stdout: capture::cargo_stdout(&arguments),
        surface: "dev cargo",
        verbose: matches.get_flag("verbose"),
        arguments,
    }
}

/// The task binary's standard output is its product and is always delivered.
fn xtask_invocation(matches: &clap::ArgMatches) -> Invocation {
    let tasks = matches
        .get_many::<OsString>("XTASK_ARGS")
        .expect("the generated parser requires trailing xtask arguments")
        .cloned()
        .collect::<Vec<_>>();
    let mut arguments = [
        "run",
        "--locked",
        "--package",
        "xtask",
        "--bin",
        "xtask",
        "--",
    ]
    .into_iter()
    .map(OsString::from)
    .collect::<Vec<_>>();
    let identity = capture::identity("xtask", &tasks);
    arguments.extend(tasks);
    Invocation {
        arguments,
        surface: "dev xtask",
        identity,
        stdout: Stdout::Deliver,
        verbose: matches.get_flag("verbose"),
    }
}

fn run_invocation(invocation: Invocation) -> RunResult {
    match run_inner(invocation) {
        Ok(status) => RunResult::Child(status),
        Err((outcome, code)) => RunResult::Rejected { outcome, code },
    }
}

/// A supervising pre-commit worker parses the configured leaf's own progress
/// protocol and captures its output, so its child streams stay verbatim.
#[expect(
    clippy::disallowed_methods,
    reason = "developer Cargo detects authority injected by its supervising pre-commit worker"
)]
fn supervised_by_pre_commit() -> bool {
    std::env::var_os(wire::SpiceVariable::PreCommitSource.as_str()).is_some()
}

/// Developer toolchains and their tests must opt into lane interception state.
fn isolate_developer_environment(command: &mut Command) {
    for variable in spice::cargo_isolation::DEVELOPER_CHILD_EXCLUDED_VARIABLES {
        command.env_remove(variable);
    }
}

#[expect(
    clippy::disallowed_methods,
    reason = "the native binary rim captures the exact Cargo seed environment"
)]
pub(crate) fn host(
    repository: &Repository,
) -> Result<Host, spice::cargo_isolation::IsolationError> {
    Host::from_environment(repository.worktree_root(), std::env::vars_os())
}

/// Inspect the owned store or conservatively reclaim expired unreferenced state.
pub(crate) fn store(matches: &clap::ArgMatches) -> Outcome<StoreAnswer> {
    let working = match std::env::current_dir() {
        Ok(working) => working,
        Err(error) => {
            return Outcome::Fault(Fault {
                class: FaultClass::Environment,
                reason: "cargo-isolation::working-directory-unavailable".to_owned(),
                detail: error.to_string(),
            });
        }
    };
    let repository = match Repository::discover(&working) {
        Ok(repository) => repository,
        Err(error) => {
            return Outcome::Fault(Fault {
                class: FaultClass::Environment,
                reason: spice::repository::ResolveError::REASON.to_owned(),
                detail: error.to_string(),
            });
        }
    };
    let host = match host(&repository) {
        Ok(host) => host,
        Err(error) => return isolation_outcome(error),
    };
    let answer = if matches.get_flag("cleanup") {
        spice::cargo_isolation::cleanup(&host).map(StoreAnswer::Cleanup)
    } else {
        spice::cargo_isolation::inspect(&host).map(StoreAnswer::Inspection)
    };
    match answer {
        Ok(body) => Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body,
        }),
        Err(error) => isolation_outcome(error),
    }
}

fn run_inner(invocation: Invocation) -> Result<ExitStatus, (Outcome<()>, i32)> {
    let working = std::env::current_dir().map_err(|error| {
        rejected(
            FaultClass::Environment,
            "cargo-isolation::working-directory-unavailable",
            error.to_string(),
            1,
        )
    })?;
    let repository = Repository::discover(&working).map_err(|error| {
        rejected(
            FaultClass::Environment,
            spice::repository::ResolveError::REASON,
            error.to_string(),
            1,
        )
    })?;
    #[cfg(unix)]
    let _lease = output_lease(&repository).map_err(|error| {
        rejected(
            FaultClass::Environment,
            "cargo-output::lease-unavailable",
            error.to_string(),
            1,
        )
    })?;
    let host = host(&repository).map_err(isolation_rejection)?;
    let (mut command, _prepared) = if writes_lock(&invocation.arguments) {
        let command = native_command(
            host.cargo(),
            host.bootstrap_home(),
            &host,
            &working,
            &invocation.arguments,
        );
        (command, None)
    } else {
        let mut prepared = prepare_manual(&repository, &host).map_err(isolation_rejection)?;
        let mut command = native_command(
            prepared.cargo(),
            prepared.home(),
            &host,
            &working,
            &invocation.arguments,
        );
        spice::cargo_isolation::bind_windows_msvc(
            repository.worktree_root(),
            host.bootstrap_home(),
            &invocation.arguments,
            crate::bootstrap_environment("CARGO_BUILD_TARGET").as_deref(),
            crate::bootstrap_environment("PATH").unwrap_or_default(),
            &mut command,
        )
        .map_err(isolation_rejection)?;
        prepared
            .retain_pin_for_child(&mut command)
            .map_err(isolation_rejection)?;
        (command, Some(prepared))
    };
    if supervised_by_pre_commit() {
        return run_verbatim(&mut command);
    }
    let settings = invocation.settings(
        std::sync::Arc::new(std::sync::Mutex::new(io::stderr())),
        capture::capture_directory(repository.common_dir()),
    );
    capture::run(&mut command, invocation.stdout, &settings, relay).map_err(|stage| match stage {
        Stage::Log(error) => rejected(
            FaultClass::Environment,
            "cargo-isolation::output-capture-unavailable",
            error.to_string(),
            1,
        ),
        Stage::Spawn(error) => launch_rejection(&error),
        Stage::Relay(error) => relay_rejection(&error),
        Stage::Wait(error) => wait_rejection(&error),
    })
}

/// Hold this worktree's Cargo output lease for the caller's whole run,
/// reclaiming over-bound output first when no other run holds it.
///
/// A reclamation failure is reported and the run continues while protected by
/// the shared lease. Failure to acquire that lease refuses the caller.
///
/// # Errors
///
/// Returns a refusal admitting, opening or locking the worktree lease.
#[cfg(unix)]
pub(crate) fn output_lease(repository: &Repository) -> io::Result<OutputLease> {
    use std::io::Write as _;

    let target = cargo_output::worktree_target(repository.worktree_root());
    let state = repository
        .admit_state()
        .map_err(|error| io::Error::other(error.to_string()))?;
    let (lease, pass) = OutputLease::enter(
        state.worktree_locks(),
        &target,
        cargo_output::LANE_OUTPUT_BOUND,
        std::time::SystemTime::now(),
    )?;
    let mut stderr = io::stderr().lock();
    match pass {
        Pass::Ran(Ok(reclamation))
            if !reclamation.removed.is_empty() || !reclamation.busy.is_empty() =>
        {
            let _ = writeln!(stderr, "[cargo output] {reclamation}");
        }
        Pass::Ran(Err(error)) => {
            let _ = writeln!(stderr, "[cargo output] reclamation failed: {error}");
        }
        Pass::Ran(Ok(_)) | Pass::NotDue | Pass::Shared => {}
    }
    Ok(lease)
}

/// Report this worktree's Cargo output footprint and the bound it is held to.
#[cfg(unix)]
pub(crate) fn footprint() -> Outcome<cargo_output::Footprint> {
    let repository = match std::env::current_dir()
        .map_err(|error| {
            (
                "cargo-output::working-directory-unavailable",
                error.to_string(),
            )
        })
        .and_then(|working| {
            Repository::discover(&working)
                .map_err(|error| (spice::repository::ResolveError::REASON, error.to_string()))
        }) {
        Ok(repository) => repository,
        Err((reason, detail)) => {
            return Outcome::Fault(Fault {
                class: FaultClass::Environment,
                reason: reason.to_owned(),
                detail,
            });
        }
    };
    let target = cargo_output::worktree_target(repository.worktree_root());
    match cargo_output::measure(&target, cargo_output::LANE_OUTPUT_BOUND) {
        Ok(body) => Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body,
        }),
        Err(error) => Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: "cargo-output::unmeasurable".to_owned(),
            detail: format!("{}: {error}", target.display()),
        }),
    }
}

/// Native Cargo over the private home, using the platform's shared descendant
/// ownership boundary.
fn native_command(
    cargo: &Path,
    home: &Path,
    host: &Host,
    working: &Path,
    arguments: &[OsString],
) -> Command {
    let mut command = Command::new(cargo);
    command
        .args(arguments)
        .current_dir(working)
        .env(CARGO_HOME_VARIABLE, home)
        .env(BOOTSTRAP_HOME_VARIABLE, host.bootstrap_home())
        .env(NATIVE_CARGO_VARIABLE, host.cargo())
        .env(NATIVE_RUSTC_VARIABLE, host.rustc())
        .env(NATIVE_RUSTDOC_VARIABLE, host.rustdoc())
        .env(RUSTC_VARIABLE, host.rustc())
        .env(RUSTDOC_VARIABLE, host.rustdoc());
    isolate_developer_environment(&mut command);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    command
}

/// Run the child with inherited streams, as a supervising worker expects.
fn run_verbatim(command: &mut Command) -> Result<ExitStatus, (Outcome<()>, i32)> {
    #[cfg(windows)]
    let mut child = sill::spawn_owned_command(command, sill::ProcessGroup::Inherit)
        .map_err(|error| launch_rejection(&error))?;
    #[cfg(not(windows))]
    let mut child = command.spawn().map_err(|error| launch_rejection(&error))?;
    let relayed = relay(&child);
    if let Err(error) = &relayed {
        let _ = child.kill();
        let _ = child.wait();
        return Err(relay_rejection(error));
    }
    let status = child.wait().map_err(|error| wait_rejection(&error));
    drop(relayed);
    status
}

/// Forward termination signals to the child's process group until dropped.
#[cfg(unix)]
fn relay(child: &Child) -> io::Result<Option<SignalRelay>> {
    i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .map(SignalRelay::install)
        .transpose()
}

#[cfg(not(unix))]
fn relay(_child: &Child) -> io::Result<()> {
    Ok(())
}

/// A launch failure keeps the shell's status conventions: 127 when Cargo is
/// missing, 126 when it is not executable.
fn launch_rejection(error: &io::Error) -> (Outcome<()>, i32) {
    let code = match error.kind() {
        io::ErrorKind::NotFound => 127,
        io::ErrorKind::PermissionDenied => 126,
        _ => 1,
    };
    rejected(
        FaultClass::Environment,
        "cargo-isolation::native-cargo-launch-failed",
        error.to_string(),
        code,
    )
}

fn relay_rejection(error: &io::Error) -> (Outcome<()>, i32) {
    rejected(
        FaultClass::Environment,
        "cargo-isolation::signals-unavailable",
        error.to_string(),
        1,
    )
}

fn wait_rejection(error: &io::Error) -> (Outcome<()>, i32) {
    rejected(
        FaultClass::Environment,
        "cargo-isolation::native-cargo-wait-failed",
        error.to_string(),
        1,
    )
}

fn isolation_rejection(error: spice::cargo_isolation::IsolationError) -> (Outcome<()>, i32) {
    rejected(
        FaultClass::Store,
        error.reason(),
        error.detail().to_owned(),
        1,
    )
}

fn isolation_outcome(error: spice::cargo_isolation::IsolationError) -> Outcome<StoreAnswer> {
    Outcome::Fault(Fault {
        class: FaultClass::Store,
        reason: error.reason().to_owned(),
        detail: error.detail().to_owned(),
    })
}

fn rejected(class: FaultClass, reason: &str, detail: String, code: i32) -> (Outcome<()>, i32) {
    (
        Outcome::Fault(Fault {
            class,
            reason: reason.to_owned(),
            detail,
        }),
        code,
    )
}
