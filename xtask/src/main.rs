use std::env;
use std::ffi::{OsStr, OsString};
use std::io::{self, Read};
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use sill::Deadlines;

const USAGE_EXIT: u8 = 2;
const USAGE: &str = concat!(
    "usage: cargo xtask <verify|stage-app|dev-app|check-app|stage-target TARGET|",
    "commit-msg MESSAGE_FILE|reference-transaction STATE>"
);

fn main() -> ExitCode {
    let mut arguments = env::args_os().skip(1);
    let command = arguments.next();

    let mut output = io::stdout().lock();
    let result: Result<(), Box<dyn std::error::Error>> = match command.as_deref() {
        Some(value) if value == OsStr::new("verify") && arguments.len() == 0 => {
            xtask::verify(&mut output).map_err(Into::into)
        }
        Some(value) if value == OsStr::new("stage-app") && arguments.len() == 0 => {
            xtask::stage_app(&release_environment(), &mut output).map_err(Into::into)
        }
        Some(value) if value == OsStr::new("dev-app") && arguments.len() == 0 => {
            xtask::dev_app(&release_environment(), &mut output).map_err(Into::into)
        }
        Some(value) if value == OsStr::new("check-app") && arguments.len() == 0 => {
            xtask::check_app(&release_environment(), &mut output).map_err(Into::into)
        }
        Some(value) if value == OsStr::new("stage-target") && arguments.len() == 1 => {
            let target = arguments.next().expect("one release target was admitted");
            let Some(target) = target.to_str() else {
                eprintln!("{USAGE}");
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
            eprintln!("{USAGE}");
            return ExitCode::from(USAGE_EXIT);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
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
