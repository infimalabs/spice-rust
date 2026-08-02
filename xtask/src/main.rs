use std::env;
use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::Path;
use std::process::ExitCode;

const USAGE_EXIT: u8 = 2;
const USAGE: &str = concat!(
    "usage: cargo xtask <verify|stage-app|check-app|commit-msg MESSAGE_FILE|",
    "reference-transaction STATE>"
);

#[allow(
    clippy::disallowed_methods,
    reason = "PATH is acquired once at the binary rim and passed inward as typed input"
)]
fn main() -> ExitCode {
    let mut arguments = env::args_os().skip(1);
    let command = arguments.next();

    let mut output = io::stdout().lock();
    let result: Result<(), Box<dyn std::error::Error>> = match command.as_deref() {
        Some(value) if value == OsStr::new("verify") && arguments.len() == 0 => {
            let search_path = env::var_os("PATH");
            xtask::verify(search_path.as_deref(), &mut output).map_err(Into::into)
        }
        Some(value) if value == OsStr::new("stage-app") && arguments.len() == 0 => {
            xtask::stage_app(&mut output).map_err(Into::into)
        }
        Some(value) if value == OsStr::new("check-app") && arguments.len() == 0 => {
            xtask::check_app(&mut output).map_err(Into::into)
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
