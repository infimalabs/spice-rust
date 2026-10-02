//! Supported Cargo launcher over one private worktree home.

use std::ffi::OsString;
use std::io;
use std::process::{Child, Command, ExitStatus};
#[cfg(unix)]
use std::thread::JoinHandle;

use serde::Serialize;
use spice::cargo_isolation::{
    BOOTSTRAP_HOME_VARIABLE, CARGO_HOME_VARIABLE, Host, NATIVE_CARGO_VARIABLE,
    NATIVE_RUSTC_VARIABLE, NATIVE_RUSTDOC_VARIABLE, RUSTC_VARIABLE, RUSTDOC_VARIABLE,
    prepare_manual,
};
use spice::repository::Repository;
use wire::{Fault, FaultClass, Outcome, Reply, ReplyPosition};

mod capture;

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
            .spawn(move || {
                for signal in signals.forever() {
                    let signal = match signal {
                        SIGHUP => rustix::process::Signal::HUP,
                        SIGINT => rustix::process::Signal::INT,
                        SIGTERM => rustix::process::Signal::TERM,
                        _ => continue,
                    };
                    let _ = rustix::process::kill_process_group(group, signal);
                }
            })?;
        Ok(Self {
            handle,
            worker: Some(worker),
        })
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

/// Prepare the exact current lock generation and run native Cargo within it.
pub(crate) fn run(matches: &clap::ArgMatches) -> RunResult {
    let arguments = matches
        .get_many::<OsString>("CARGO_ARGS")
        .expect("the generated parser requires trailing Cargo arguments")
        .cloned()
        .collect::<Vec<_>>();
    run_invocation(Invocation {
        identity: capture::identity("cargo", &arguments),
        stdout: capture::cargo_stdout(&arguments),
        surface: "dev cargo",
        verbose: matches.get_flag("verbose"),
        arguments,
    })
}

/// Run the repository's task binary inside the same admitted Cargo boundary.
///
/// The task binary's standard output is its product and is always delivered.
pub(crate) fn xtask(matches: &clap::ArgMatches) -> RunResult {
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
    run_invocation(Invocation {
        arguments,
        surface: "dev xtask",
        identity,
        stdout: Stdout::Deliver,
        verbose: matches.get_flag("verbose"),
    })
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
    let host = host(&repository).map_err(isolation_rejection)?;
    let mut prepared = prepare_manual(&repository, &host).map_err(isolation_rejection)?;
    let mut command = Command::new(prepared.cargo());
    command
        .args(&invocation.arguments)
        .current_dir(&working)
        .env(CARGO_HOME_VARIABLE, prepared.home())
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
    prepared
        .retain_pin_for_child(&mut command)
        .map_err(isolation_rejection)?;
    if supervised_by_pre_commit() {
        return run_verbatim(&mut command);
    }
    let settings = capture::Settings {
        surface: invocation.surface,
        identity: invocation.identity,
        sink: std::sync::Arc::new(std::sync::Mutex::new(io::stderr())),
        heartbeat: crate::gate_command::output::HEARTBEAT,
        verbose: invocation.verbose,
        directory: capture::capture_directory(repository.common_dir()),
    };
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

/// Run the child with inherited streams, as a supervising worker expects.
fn run_verbatim(command: &mut Command) -> Result<ExitStatus, (Outcome<()>, i32)> {
    let mut child = command.spawn().map_err(|error| launch_rejection(&error))?;
    let guard = match relay(&child) {
        Ok(guard) => guard,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(relay_rejection(&error));
        }
    };
    let status = child.wait().map_err(|error| wait_rejection(&error))?;
    drop(guard);
    Ok(status)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn developer_children_begin_outside_supervised_shell_state_and_may_opt_in() {
        let mut command = Command::new("cargo");
        command
            .env(steer::hook::BASH_ENV, "/fixture/bash-env")
            .env(steer::hook::ZDOTDIR, "/fixture/zdotdir")
            .env(vendor::OMP.thread_id_environment(), "fixture-omp");
        isolate_developer_environment(&mut command);

        for variable in spice::cargo_isolation::DEVELOPER_CHILD_EXCLUDED_VARIABLES {
            assert!(
                command
                    .get_envs()
                    .any(|(name, value)| name == variable && value.is_none()),
                "developer child does not inherit {variable}"
            );
        }

        command.env(vendor::OMP.thread_id_environment(), "owned-omp");
        assert!(command.get_envs().any(|(name, value)| {
            name == vendor::OMP.thread_id_environment()
                && value == Some(std::ffi::OsStr::new("owned-omp"))
        }));
    }
}
