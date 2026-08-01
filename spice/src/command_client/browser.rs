use std::process::{Command, Stdio};

pub(super) fn system_browser(endpoint: &str) -> Result<(), String> {
    let mut command = browser_command(endpoint)?;
    let status = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("the platform browser launcher did not start: {error}"))?;
    if !status.success() {
        return Err(format!(
            "the platform browser launcher did not accept the current endpoint: {status}"
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub(super) fn browser_command(endpoint: &str) -> Result<Command, String> {
    let mut command = Command::new("/usr/bin/open");
    command.arg("-u").arg(endpoint);
    Ok(command)
}

#[cfg(target_os = "linux")]
pub(super) fn browser_command(endpoint: &str) -> Result<Command, String> {
    let mut command = Command::new("/usr/bin/xdg-open");
    command.arg(endpoint);
    Ok(command)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) fn browser_command(_endpoint: &str) -> Result<Command, String> {
    Err("canvas browser launch is unsupported on this build target".to_owned())
}
