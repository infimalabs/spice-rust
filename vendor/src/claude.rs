//! Claude Code's current stream-json launch contract.

use std::ffi::OsString;

use listen::Dialect;
use sill::{Running, SpecError};

use crate::{LaunchError, Process, Record, RecordStream, Resume};

/// Build the exact current Claude Code invocation without performing I/O.
///
/// # Errors
///
/// Returns [`SpecError`] when `binary` is empty.
pub fn record(
    binary: impl Into<OsString>,
    prompt: impl Into<OsString>,
    resume: Resume,
) -> Result<Record, SpecError> {
    let mut arguments = vec![
        OsString::from("--print"),
        OsString::from("--verbose"),
        OsString::from("--output-format"),
        OsString::from("stream-json"),
    ];
    if let Some(session) = resume.session_id() {
        arguments.push(OsString::from("--resume"));
        arguments.push(OsString::from(session));
    }
    arguments.push(prompt.into());

    Record::new(
        binary,
        arguments,
        Dialect::Claude,
        RecordStream::Stdout,
        resume,
    )
}

/// Launch Claude Code through the sole `sill` process door.
///
/// # Errors
///
/// Returns [`LaunchError`] when the contract is invalid or `sill` cannot
/// create the child.
pub fn launch(
    binary: impl Into<OsString>,
    prompt: impl Into<OsString>,
    resume: Resume,
    process: &Process,
) -> Result<Running, LaunchError> {
    let record = record(binary, prompt, resume).map_err(LaunchError::Contract)?;
    sill::spawn(record.spec(process)).map_err(LaunchError::Spawn)
}
