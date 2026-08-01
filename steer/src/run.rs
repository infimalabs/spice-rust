//! The wrapper that lets a lane hear before its command runs.
//!
//! This module names nothing but the standard library, and the bound is
//! measured rather than asserted: `steer/tests/bound.rs` reads this file and
//! refuses any import that leaves std. The reason is cost, not purity. This
//! code sits in front of every command an agent runs, so whatever it touches is
//! paid for on every invocation. It reads one rendered file, writes those bytes
//! to the stream, and hands the command through with one spawn — no repository
//! discovery, no database, no second process.
//!
//! It also refuses nothing. A wrapper that declined to run commands would break
//! the agent it wraps, so a missing rendering or an unreadable one is ordinary
//! and the command still runs. Regression is the enforcement here.

use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, ExitStatus};

/// Writes a lane's rendered direction to the stream, reporting whether anything
/// was waiting.
///
/// An absent rendering writes nothing and reports `false`. Absence is not a
/// failure: the file is a rendering of the journal, disposition lives in the
/// journal itself, and a lane with nothing pending has nothing to render.
///
/// # Errors
///
/// Returns the error when a rendering that exists cannot be read, or when the
/// stream cannot take it.
pub fn emit(rendering: &Path, stream: &mut impl Write) -> io::Result<bool> {
    let bytes = match fs::read(rendering) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if bytes.is_empty() {
        return Ok(false);
    }
    stream.write_all(&bytes)?;
    stream.flush()?;
    Ok(true)
}

/// Runs the wrapped command and waits for it.
///
/// The command's own streams are inherited: this hands the operator's command
/// through rather than capturing it, so what the caller sees is what the
/// command wrote.
///
/// # Errors
///
/// Returns the error when the command cannot be spawned.
pub fn hand<I, S>(program: &OsStr, arguments: I) -> io::Result<ExitStatus>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(program).args(arguments).status()
}
