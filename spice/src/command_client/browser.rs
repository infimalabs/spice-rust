//! Browser discovery admission and platform launch.

use super::*;

/// Open the browser surface named by the current owner-only discovery file.
///
/// This deliberately shares [`DiscoveryRead`] with resident commands. Canvas
/// acquisition therefore has one file-identity, ownership, mode, wire-shape,
/// and loopback-origin admission path rather than a browser-specific reader.
pub(crate) fn open_canvas(discovery_path: &Path) -> Outcome<wire::CanvasOpen> {
    open_canvas_with(discovery_path, || {}, system_browser)
}

pub(crate) fn open_canvas_with(
    discovery_path: &Path,
    before_current_check: impl FnOnce(),
    launch: impl FnOnce(&str) -> Result<(), String>,
) -> Outcome<wire::CanvasOpen> {
    let discovery = match DiscoveryRead::open(discovery_path) {
        Ok(discovery) => discovery,
        Err(outcome) => return rebody(outcome),
    };
    if let Err(outcome) = dialable_endpoint(&discovery.value.endpoint) {
        return rebody(outcome);
    }
    before_current_check();
    if let Err(outcome) = discovery.verify_current() {
        return rebody(outcome);
    }
    if let Err(detail) = launch(&discovery.value.endpoint) {
        return Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: "canvas::launch-failed".to_owned(),
            detail,
        });
    }
    Outcome::Reply(Reply {
        position: ReplyPosition::Direct {},
        body: wire::CanvasOpen {
            endpoint: discovery.value.endpoint,
            epoch: discovery.value.epoch,
        },
    })
}

fn system_browser(endpoint: &str) -> Result<(), String> {
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
pub(super) fn browser_command(endpoint: &str) -> Result<ProcessCommand, String> {
    let mut command = ProcessCommand::new("/usr/bin/open");
    command.arg("-u").arg(endpoint);
    Ok(command)
}

#[cfg(target_os = "linux")]
pub(super) fn browser_command(endpoint: &str) -> Result<ProcessCommand, String> {
    let mut command = ProcessCommand::new("/usr/bin/xdg-open");
    command.arg(endpoint);
    Ok(command)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) fn browser_command(_endpoint: &str) -> Result<ProcessCommand, String> {
    Err("canvas browser launch is unsupported on this build target".to_owned())
}

fn rebody<B>(outcome: Outcome<CommandAnswer>) -> Outcome<B> {
    match outcome {
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
        Outcome::Reply(_) => Outcome::Fault(Fault {
            class: FaultClass::Protocol,
            reason: "canvas::admission-replied".to_owned(),
            detail: "canvas discovery admission returned a reply instead of a failure".to_owned(),
        }),
    }
}
