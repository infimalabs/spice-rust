//! GitHub-hosted disposable Windows execution over a content-addressed source ref.

use std::ffi::OsString;
use std::io::Write;

pub use hosted_windows::client::{HostedExecution, WindowsHostedError};

pub fn windows_hosted(
    arguments: Vec<OsString>,
    output: &mut impl Write,
    error_output: &mut impl Write,
) -> Result<HostedExecution, WindowsHostedError> {
    hosted_windows::client::windows_hosted(arguments, output, error_output)
}

/// Stage the Windows release target on one fresh provider-hosted machine.
pub fn windows_hosted_stage(
    output: &mut impl Write,
    error_output: &mut impl Write,
) -> Result<HostedExecution, WindowsHostedError> {
    hosted_windows::client::require_public_attestation_repository()?;
    let command = hosted_windows::Command {
        program: "@cargo".to_owned(),
        arguments: [
            "run",
            "--locked",
            "-p",
            "xtask",
            "--",
            "stage-target",
            "x86_64-pc-windows-msvc",
            "--unsigned",
        ]
        .map(str::to_owned)
        .into(),
    };
    let execution = hosted_windows::client::windows_hosted_commands_into(
        vec![command],
        Some(std::path::Path::new("dist/stage")),
    )?;
    let first = &execution.outputs[0];
    output
        .write_all(&first.stdout)
        .map_err(WindowsHostedError::Output)?;
    error_output
        .write_all(&first.stderr)
        .map_err(WindowsHostedError::Output)?;
    Ok(execution)
}
