use std::env;
use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use sill::Deadlines;

const USAGE_EXIT: u8 = 2;
const USAGE: &str = concat!(
    "usage: cargo xtask <help|verify [--package PACKAGE]|verify-action battery|",
    "gate ARM|app|activate-hooks|",
    "install [--host HOST] [--port PORT] [--release] [--no-resident]|",
    "proof-image <build [ARCHIVE]|load ARCHIVE|verify [ARCHIVE]>|",
    "stage-target TARGET|",
    "commit-msg MESSAGE_FILE|reference-transaction STATE>"
);

fn help() -> String {
    let arms = gate::GateId::ALL.map(gate::GateId::as_str).join("\n  ");
    format!(
        concat!(
            "{usage}\n\n",
            "verify-action battery is the scheduler-owned child action; ",
            "it is not a completion verifier.\n\n",
            "gate ARM accepts:\n  {arms}"
        ),
        usage = USAGE,
        arms = arms,
    )
}

fn main() -> ExitCode {
    let mut arguments = env::args_os().skip(1);
    let command = arguments.next();

    let mut output = io::stdout().lock();
    // Dispatch only current spellings; removed command names have no alias arm.
    let result: Result<(), Box<dyn std::error::Error>> = match command.as_deref() {
        Some(value) if value == OsStr::new("verify-action") && arguments.len() == 1 => {
            let action = arguments.next().expect("one verifier action was admitted");
            return run_verify_action(&action, &mut output);
        }
        Some(value) if value == OsStr::new("help") && arguments.len() == 0 => {
            writeln!(output, "{}", help()).map_err(Into::into)
        }
        Some(value) if value == OsStr::new("verify") => {
            return run_verify(arguments.collect(), &mut output);
        }
        Some(value) if value == OsStr::new("gate") && arguments.len() == 1 => {
            let arm = arguments.next().expect("one gate arm was admitted");
            let Some(arm) = arm.to_str() else {
                eprintln!("{}", help());
                return ExitCode::from(USAGE_EXIT);
            };
            let Some(gate_id) = gate::GateId::ALL
                .into_iter()
                .find(|gate_id| gate_id.as_str() == arm)
            else {
                eprintln!(
                    "unknown gate arm {arm:?}; run `cargo xtask help` for accepted gate arms"
                );
                return ExitCode::from(USAGE_EXIT);
            };
            xtask::gate_arm(gate_id, &mut output).map_err(Into::into)
        }
        Some(value) if value == OsStr::new("app") && arguments.len() == 0 => {
            xtask::app(&release_environment(), &mut output).map_err(Into::into)
        }
        Some(value) if value == OsStr::new("activate-hooks") && arguments.len() == 0 => {
            // The executable published for the hooks to exec is this one. Cargo
            // keeps it under a content-keyed directory, so this is the only
            // moment its path is known to anything that could copy it.
            env::current_exe()
                .map_err(Into::into)
                .and_then(|entry_point| {
                    xtask::activate_hooks(Path::new("."), &entry_point, &mut output)
                        .map_err(Into::into)
                })
        }
        // The refresh drives `spice::install`, which is gated off the wasm
        // target the workspace check compiles this binary for.
        #[cfg(not(target_arch = "wasm32"))]
        Some(value) if value == OsStr::new("install") => {
            drop(output);
            return run_install(arguments.collect());
        }
        Some(value) if value == OsStr::new("proof-image") => {
            drop(output);
            return run_proof_image(arguments.collect());
        }
        Some(value) if value == OsStr::new("stage-target") && arguments.len() == 1 => {
            let target = arguments.next().expect("one release target was admitted");
            let Some(target) = target.to_str() else {
                eprintln!("{}", help());
                return ExitCode::from(USAGE_EXIT);
            };
            preflight_native_release(target).and_then(|()| {
                xtask::stage_target(target, release_environment(), &mut output)
                    .map(|_| ())
                    .map_err(Into::into)
            })
        }
        Some(value) if value == OsStr::new("commit-msg") && arguments.len() == 1 => {
            let path = arguments
                .next()
                .expect("one commit-message path was admitted");
            match run_commit_message(Path::new(&path)) {
                Ok(true) => {
                    eprintln!(
                        concat!(
                            "spice commit-msg: auto-folded commit message body lines; ",
                            "keep body prose wrapped at {} chars"
                        ),
                        xtask::COMMIT_MESSAGE_WRAP_LIMIT
                    );
                    Ok(())
                }
                Ok(false) => Ok(()),
                Err(error) => Err(error),
            }
        }
        Some(value) if value == OsStr::new("reference-transaction") && arguments.len() == 1 => {
            let state = arguments
                .next()
                .expect("one reference-transaction state was admitted");
            run_reference_transaction(&state)
        }
        _ => {
            eprintln!("{}", help());
            return ExitCode::from(USAGE_EXIT);
        }
    };
    result_exit(result)
}

fn result_exit(result: Result<(), Box<dyn std::error::Error>>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run_verify(arguments: Vec<OsString>, output: &mut impl io::Write) -> ExitCode {
    let result = match arguments.as_slice() {
        [] => xtask::verify(output).map_err(Into::into),
        [option, package] if option == OsStr::new("--package") => {
            let Some(package) = package.to_str() else {
                eprintln!("{}", help());
                return ExitCode::from(USAGE_EXIT);
            };
            xtask::verify_package(package, output).map_err(Into::into)
        }
        _ => {
            eprintln!("{}", help());
            return ExitCode::from(USAGE_EXIT);
        }
    };
    result_exit(result)
}

fn run_verify_action(action: &OsStr, output: &mut impl io::Write) -> ExitCode {
    if action != OsStr::new("battery") {
        eprintln!("{}", help());
        return ExitCode::from(USAGE_EXIT);
    }
    match xtask::verify_battery_child(output) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(xtask::VERIFY_ACTION_REFUSAL_EXIT),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn run_install(arguments: Vec<OsString>) -> ExitCode {
    let Some(request) = install_request(&arguments) else {
        eprintln!("{}", help());
        return ExitCode::from(USAGE_EXIT);
    };
    let mut output = io::stdout().lock();
    match xtask::refresh(&request, &mut output) {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// Read one `cargo xtask install` invocation, or refuse the whole spelling.
#[cfg(not(target_arch = "wasm32"))]
fn install_request(arguments: &[OsString]) -> Option<xtask::RefreshRequest> {
    let mut request = xtask::RefreshRequest {
        restart: true,
        ..xtask::RefreshRequest::default()
    };
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        match argument.to_str()? {
            "--release" => request.release = true,
            "--no-resident" => request.restart = false,
            "--host" => request.host = Some(arguments.next()?.to_str()?.to_owned()),
            "--port" => request.port = Some(arguments.next()?.to_str()?.parse().ok()?),
            _ => return None,
        }
    }
    Some(request)
}

fn run_proof_image(arguments: Vec<OsString>) -> ExitCode {
    let (operation, archive) = match arguments.as_slice() {
        [operation] if operation == OsStr::new("build") => ("build", None),
        [operation] if operation == OsStr::new("verify") => ("verify", None),
        [operation, archive] if operation == OsStr::new("build") => {
            ("build", Some(PathBuf::from(archive)))
        }
        [operation, archive] if operation == OsStr::new("load") => {
            ("load", Some(PathBuf::from(archive)))
        }
        [operation, archive] if operation == OsStr::new("verify") => {
            ("verify", Some(PathBuf::from(archive)))
        }
        _ => {
            eprintln!("{}", help());
            return ExitCode::from(USAGE_EXIT);
        }
    };
    let repository = match spice::repository::Repository::discover(Path::new(".")) {
        Ok(repository) => repository,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    let archive = archive.unwrap_or_else(|| {
        repository
            .worktree_root()
            .join("target/proof-image/spice-proof-image.tar")
    });
    let result = match operation {
        "build" => xtask::build_proof_image(
            repository.worktree_root(),
            &archive,
            &mut stdout,
            &mut stderr,
        ),
        "load" => xtask::load_proof_image(
            repository.worktree_root(),
            &archive,
            &mut stdout,
            &mut stderr,
        ),
        "verify" => xtask::build_proof_image(
            repository.worktree_root(),
            &archive,
            &mut stdout,
            &mut stderr,
        )
        .and_then(|()| {
            xtask::load_proof_image(
                repository.worktree_root(),
                &archive,
                &mut stdout,
                &mut stderr,
            )
        }),
        _ => unreachable!("proof-image operation was admitted above"),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(error.exit_code())
        }
    }
}

fn preflight_native_release(target: &str) -> Result<(), Box<dyn std::error::Error>> {
    if target != "aarch64-apple-darwin" {
        return Ok(());
    }
    let repository = spice::repository::Repository::discover(Path::new("."))?;
    let config = spice::config::load_repository(&repository, &[])?;
    let runner = config.root().proof().runner().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "proof.runner is required before Darwin release staging",
        )
    })?;
    let deadlines = Deadlines::new(
        Duration::from_secs(60),
        Duration::from_secs(10),
        Duration::from_secs(10),
    )?;
    let limits = proof::Limits::new(deadlines, Duration::from_secs(5), 1024 * 1024)?;
    let admitted =
        proof::NativeRunner::attest(runner.as_path(), repository.worktree_root(), limits)?;
    admitted.preflight()?;
    Ok(())
}

fn release_environment() -> xtask::ReleaseEnvironment {
    xtask::ReleaseEnvironment::new(
        bootstrap_environment("PATH"),
        bootstrap_environment("HOME"),
        bootstrap_environment("CARGO_HOME"),
        bootstrap_environment("RUSTUP_HOME"),
        bootstrap_environment("RUSTUP_TOOLCHAIN"),
        bootstrap_environment("TMPDIR"),
        bootstrap_environment("SDKROOT"),
        bootstrap_environment("DEVELOPER_DIR"),
        bootstrap_environment("CARGO_TARGET_DIR"),
        bootstrap_environment("RUSTC"),
        bootstrap_environment("CARGO_BUILD_RUSTC"),
    )
}

#[expect(
    clippy::disallowed_methods,
    reason = "the xtask binary rim owns ambient release-build acquisition"
)]
fn bootstrap_environment(name: impl AsRef<OsStr>) -> Option<OsString> {
    env::var_os(name)
}

fn run_commit_message(path: &Path) -> Result<bool, Box<dyn std::error::Error>> {
    let repository = spice::repository::Repository::discover(Path::new("."))?;
    let resolved = spice::config::load_repository(&repository, &[])?;
    Ok(xtask::commit_message(
        path,
        resolved.root().policy().commit_message(),
    )?)
}

fn run_reference_transaction(state: &OsStr) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    if state == OsStr::new("prepared") {
        io::stdin().read_to_string(&mut input)?;
    }
    let repository = env::current_dir()?;
    xtask::reference_transaction(&repository, state, &input).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::help;

    #[test]
    fn help_exposes_the_one_scheduler_owned_battery_action() {
        let help = help();
        assert!(help.contains("verify-action battery"));
        assert!(help.contains("not a completion verifier"));
    }
}
