//! The hidden entries this executable runs under: a lane supervisor, or the
//! keeper of one of a lane's jobs.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::Path;

use super::terminal::job;

/// Dispatch a hidden entry before normal CLI parsing.
pub(crate) fn entry(arguments: impl IntoIterator<Item = OsString>) -> Option<i32> {
    let mut arguments = arguments.into_iter();
    let _executable = arguments.next()?;
    let role = arguments.next()?;
    let (name, run): (&str, fn(&Path) -> io::Result<()>) = if role == OsStr::new(super::ENTRY) {
        ("lane supervisor", super::run)
    } else if role == OsStr::new(job::ENTRY) {
        ("lane job", job::keep)
    } else {
        return None;
    };
    let Some(request) = arguments.next() else {
        eprintln!("{name} request path is missing");
        return Some(2);
    };
    if arguments.next().is_some() {
        eprintln!("{name} received unexpected arguments");
        return Some(2);
    }
    Some(match run(Path::new(&request)) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{name} failed: {error}");
            1
        }
    })
}
