use std::env;
use std::ffi::OsStr;
use std::io;
use std::process::ExitCode;

const USAGE_EXIT: u8 = 2;

fn main() -> ExitCode {
    let mut arguments = env::args_os().skip(1);
    let command = arguments.next();
    if command.as_deref() != Some(OsStr::new("verify")) || arguments.next().is_some() {
        eprintln!("usage: cargo xtask verify");
        return ExitCode::from(USAGE_EXIT);
    }

    let mut output = io::stdout().lock();
    match xtask::verify(&mut output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
