use std::env;
use std::ffi::{OsStr, OsString};
use std::io;
use std::process::ExitCode;

const USAGE_EXIT: u8 = 2;

fn main() -> ExitCode {
    let mut arguments = env::args_os().skip(1);
    let command = arguments.next();

    let mut output = io::stdout().lock();
    let result: Result<(), Box<dyn std::error::Error>> = match command.as_deref() {
        Some(value) if value == OsStr::new("verify") => {
            if arguments.next().is_some() {
                return usage();
            }
            xtask::verify(&mut output).map_err(Into::into)
        }
        Some(value) if value == OsStr::new("stage-app") => {
            if arguments.next().is_some() {
                return usage();
            }
            xtask::stage_app(&mut output).map_err(Into::into)
        }
        Some(value) if value == OsStr::new("check-app") => {
            if arguments.next().is_some() {
                return usage();
            }
            xtask::check_app(&mut output).map_err(Into::into)
        }
        Some(value) if value == OsStr::new("stage-target") => {
            let Some(target) = arguments.next() else {
                return usage();
            };
            if arguments.next().is_some() {
                return usage();
            }
            let Some(target) = target.to_str() else {
                return usage();
            };
            xtask::stage_target(target, release_environment(), &mut output)
                .map(|_| ())
                .map_err(Into::into)
        }
        _ => {
            return usage();
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

fn usage() -> ExitCode {
    eprintln!("usage: cargo xtask <verify|stage-app|check-app|stage-target TARGET>");
    ExitCode::from(USAGE_EXIT)
}

fn release_environment() -> xtask::ReleaseEnvironment {
    xtask::ReleaseEnvironment::new(
        bootstrap_environment("PATH"),
        bootstrap_environment("HOME"),
        bootstrap_environment("CARGO_HOME"),
        bootstrap_environment("RUSTUP_HOME"),
        bootstrap_environment("TMPDIR"),
        bootstrap_environment("SDKROOT"),
        bootstrap_environment("DEVELOPER_DIR"),
    )
}

#[expect(
    clippy::disallowed_methods,
    reason = "the xtask binary rim owns ambient release-build acquisition"
)]
fn bootstrap_environment(name: impl AsRef<OsStr>) -> Option<OsString> {
    env::var_os(name)
}
