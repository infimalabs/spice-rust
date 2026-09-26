//! Supported Cargo launcher over one private worktree home.

use std::ffi::OsString;
use std::io;
use std::process::{Command, ExitStatus};
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

/// Prepare the exact current lock generation and run native Cargo within it.
pub(crate) fn run(matches: &clap::ArgMatches) -> RunResult {
    let arguments = matches
        .get_many::<OsString>("CARGO_ARGS")
        .expect("the generated parser requires trailing Cargo arguments")
        .cloned()
        .collect::<Vec<_>>();
    run_arguments(arguments)
}

/// Run the repository's task binary inside the same admitted Cargo boundary.
pub(crate) fn xtask(matches: &clap::ArgMatches) -> RunResult {
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
    arguments.extend(
        matches
            .get_many::<OsString>("XTASK_ARGS")
            .expect("the generated parser requires trailing xtask arguments")
            .cloned(),
    );
    run_arguments(arguments)
}

fn run_arguments(arguments: Vec<OsString>) -> RunResult {
    match run_inner(arguments) {
        Ok(status) => RunResult::Child(status),
        Err((outcome, code)) => RunResult::Rejected { outcome, code },
    }
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
                reason: "cargo-isolation::repository-unresolved".to_owned(),
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

fn run_inner(arguments: Vec<OsString>) -> Result<ExitStatus, (Outcome<()>, i32)> {
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
            "cargo-isolation::repository-unresolved",
            error.to_string(),
            1,
        )
    })?;
    let host = host(&repository).map_err(isolation_rejection)?;
    let mut prepared = prepare_manual(&repository, &host).map_err(isolation_rejection)?;
    let mut command = Command::new(prepared.cargo());
    command
        .args(arguments)
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
    let mut child = command.spawn().map_err(|error| {
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
    })?;
    #[cfg(unix)]
    let relay = {
        let pid = i32::try_from(child.id())
            .ok()
            .and_then(rustix::process::Pid::from_raw);
        match pid.map(SignalRelay::install).transpose() {
            Ok(relay) => relay,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(rejected(
                    FaultClass::Environment,
                    "cargo-isolation::signals-unavailable",
                    error.to_string(),
                    1,
                ));
            }
        }
    };
    let status = child.wait().map_err(|error| {
        rejected(
            FaultClass::Environment,
            "cargo-isolation::native-cargo-wait-failed",
            error.to_string(),
            1,
        )
    })?;
    #[cfg(unix)]
    drop(relay);
    Ok(status)
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
            .env(vendor::CODEX.thread_id_environment(), "fixture-codex")
            .env(vendor::CLAUDE.thread_id_environment(), "fixture-claude");
        isolate_developer_environment(&mut command);

        assert_eq!(
            spice::cargo_isolation::DEVELOPER_CHILD_EXCLUDED_VARIABLES,
            [
                steer::hook::BASH_ENV,
                steer::hook::ZDOTDIR,
                vendor::CODEX.thread_id_environment(),
                vendor::CLAUDE.thread_id_environment(),
                vendor::OMP.thread_id_environment(),
            ]
        );
        for variable in spice::cargo_isolation::DEVELOPER_CHILD_EXCLUDED_VARIABLES {
            assert!(
                command
                    .get_envs()
                    .any(|(name, value)| name == variable && value.is_none()),
                "developer child does not inherit {variable}"
            );
        }

        command.env(vendor::CODEX.thread_id_environment(), "owned-codex");
        assert!(command.get_envs().any(|(name, value)| {
            name == vendor::CODEX.thread_id_environment()
                && value == Some(std::ffi::OsStr::new("owned-codex"))
        }));
    }

    #[test]
    fn launch_failures_preserve_shell_status_conventions() {
        let missing = io::Error::from(io::ErrorKind::NotFound);
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);
        assert_eq!(
            match missing.kind() {
                io::ErrorKind::NotFound => 127,
                _ => 1,
            },
            127
        );
        assert_eq!(
            match denied.kind() {
                io::ErrorKind::PermissionDenied => 126,
                _ => 1,
            },
            126
        );
    }
}
