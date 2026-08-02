use std::env;
use std::ffi::OsStr;
use std::io;
use std::process::ExitCode;

const USAGE_EXIT: u8 = 2;

#[allow(
    clippy::disallowed_methods,
    reason = "PATH is acquired once at the binary rim and passed inward as typed input"
)]
fn main() -> ExitCode {
    let mut arguments = env::args_os().skip(1);
    let command = arguments.next();
    if arguments.next().is_some() {
        eprintln!("usage: cargo xtask <verify|stage-app|check-app>");
        return ExitCode::from(USAGE_EXIT);
    }

    let mut output = io::stdout().lock();
    let result: Result<(), Box<dyn std::error::Error>> = match command.as_deref() {
        Some(value) if value == OsStr::new("verify") => {
            let search_path = env::var_os("PATH");
            xtask::verify(search_path.as_deref(), &mut output).map_err(Into::into)
        }
        Some(value) if value == OsStr::new("stage-app") => {
            xtask::stage_app(&mut output).map_err(Into::into)
        }
        Some(value) if value == OsStr::new("check-app") => {
            xtask::check_app(&mut output).map_err(Into::into)
        }
        _ => {
            eprintln!("usage: cargo xtask <verify|stage-app|check-app>");
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
