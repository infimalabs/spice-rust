//! `spice app`: the console in a native window.
//!
//! The window is one more client of the resident. The command joins a
//! resident already serving this repository, or starts one exactly as `spice
//! up` does and drains it when the window closes; the endpoint it shows has
//! passed the same discovery admission a browser launch applies, and the
//! bearer token, when one is required, travels only in the URL fragment the
//! webview loads, never in argv, environment, or any log.
//!
//! On macOS the window itself is drawn by a child running this same
//! appliance from the shell's app bundle, because dictation attaches only to
//! a client LaunchServices knows by bundle identity. The child learns the URL
//! over its stdin and treats that pipe as its lease, closing the window when
//! this process is gone; resident start, join, and drain never leave here.

use std::ffi::OsString;
use std::path::Path;

use clap::ArgMatches;
use wire::{
    AppClosed, AppResident, Fault, FaultClass, Outcome, Refusal, RefusalDiagnostic, Reply,
    ReplyPosition,
};

use crate::command_client::{open_app_result_with, system_browser};
use crate::resident;

/// The window's title: the product, then the repository it serves, so the
/// Window menu and Mission Control tell several repositories apart.
fn title(common: &Path) -> String {
    let named = if common.file_name() == Some(std::ffi::OsStr::new(".git")) {
        common.parent()
    } else {
        Some(common)
    };
    match named.and_then(Path::file_name) {
        Some(name) => {
            let name = name.to_string_lossy();
            format!("Spice: {}", name.strip_suffix(".git").unwrap_or(&name))
        }
        None => "Spice".to_owned(),
    }
}
/// The hidden entry the window process runs under.
const WINDOW_ENTRY: &str = "__app-window";

/// Show the console in a native window and answer once it closes.
pub(crate) fn run(leaf: &ArgMatches) -> Outcome<AppClosed> {
    let current = match std::env::current_dir() {
        Ok(current) => current,
        Err(error) => return environment_fault("resident::cwd-unavailable", &error.to_string()),
    };
    let listen = match resident::requested_listen(leaf) {
        Ok(listen) => listen,
        Err(outcome) => return typed(outcome),
    };
    let token = match resident::requested_access(leaf, listen) {
        Ok(access) => access.token,
        Err(outcome) => return typed(outcome),
    };
    let repository = match spice::repository::Repository::discover(&current) {
        Ok(repository) => repository,
        Err(error) => {
            return environment_fault(spice::repository::ResolveError::REASON, &error.to_string());
        }
    };
    let session = match join_or_start(&repository, leaf) {
        Ok(session) => session,
        Err(outcome) => return outcome,
    };
    let url = token.as_deref().map_or_else(
        || session.opened.endpoint.clone(),
        |token| format!("{}#{token}", session.opened.endpoint),
    );
    let shown = show_window(&url, repository.common_dir());
    let resident = if session.started {
        match drain(&current) {
            Ok(()) => AppResident::Drained,
            Err(outcome) => return outcome,
        }
    } else {
        AppResident::Running
    };
    if let Err(error) = shown {
        return window_failure(&error);
    }
    Outcome::Reply(Reply {
        position: ReplyPosition::Direct {},
        body: AppClosed {
            endpoint: session.opened.endpoint,
            epoch: session.opened.epoch,
            resident,
        },
    })
}

/// Draw the window for the URL one line of stdin names, titled and keyed by
/// the Git common directory its one argument names; the hidden entry
/// `show_window` spawns.
pub(crate) fn window_entry(arguments: impl Iterator<Item = OsString>) -> Option<i32> {
    use std::io::Read;

    let mut arguments = arguments.skip(1);
    if arguments.next()? != WINDOW_ENTRY {
        return None;
    }
    let (Some(common), None) = (arguments.next(), arguments.next()) else {
        eprintln!(
            "the window entry takes one argument, the Git common directory; its URL arrives on stdin"
        );
        return Some(2);
    };
    let common = std::path::PathBuf::from(common);
    let mut url = String::new();
    match std::io::stdin().read_line(&mut url) {
        Ok(_) if !url.trim().is_empty() => {}
        Ok(_) => {
            eprintln!("the window entry expects its URL on stdin");
            return Some(2);
        }
        Err(error) => {
            eprintln!("the window entry could not read its URL: {error}");
            return Some(2);
        }
    }
    // The parent holds the pipe open for the window's whole life; its end is
    // the signal that nothing is left to drain the resident for this window.
    std::thread::spawn(|| {
        let _ = std::io::stdin().read_to_end(&mut Vec::new());
        std::process::exit(0);
    });
    let key = common.to_string_lossy();
    Some(
        match desktop::show(&title(&common), &key, url.trim(), external_link) {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("{error}");
                1
            }
        },
    )
}

fn external_link(link: &str) {
    let _ = system_browser(link);
}

/// Show the window from a process carrying a bundle identity and wait for
/// it to close.
#[cfg(target_os = "macos")]
fn show_window(url: &str, common: &Path) -> Result<(), desktop::WindowError> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let platform = |detail: String| desktop::WindowError::Platform(detail);
    let executable = std::env::current_exe()
        .map_err(|error| platform(format!("the running executable is unresolved: {error}")))?;
    let bundled = desktop::bundled_executable(&executable)?;
    let mut child = Command::new(bundled)
        .arg(WINDOW_ENTRY)
        .arg(common)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| platform(format!("the window process could not start: {error}")))?;
    let mut lease = child
        .stdin
        .take()
        .expect("the window process was given a piped stdin");
    if let Err(error) = writeln!(lease, "{url}") {
        let _ = child.kill();
        let _ = child.wait();
        return Err(platform(format!("the window process took no URL: {error}")));
    }
    let output = child
        .wait_with_output()
        .map_err(|error| platform(format!("the window process could not be awaited: {error}")))?;
    drop(lease);
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    Err(platform(if detail.is_empty() {
        format!("the window process ended with {}", output.status)
    } else {
        detail.to_owned()
    }))
}

#[cfg(not(target_os = "macos"))]
fn show_window(url: &str, common: &Path) -> Result<(), desktop::WindowError> {
    desktop::show(
        &title(common),
        &common.to_string_lossy(),
        url,
        external_link,
    )
}

/// The admitted endpoint the window shows and whether this command owns the
/// resident behind it.
struct Session {
    opened: wire::AppOpen,
    started: bool,
}

/// Join the resident already serving `repository`, or start one without a
/// browser launch when none is live.
fn join_or_start(
    repository: &spice::repository::Repository,
    leaf: &ArgMatches,
) -> Result<Session, Outcome<AppClosed>> {
    let serve = repository
        .admit_resident_client()
        .map_err(|error| crate::state_failure(&error).outcome())?;
    if let Ok(opened) = open_app_result_with(&serve, || {}, |_| Ok(())) {
        return Ok(Session {
            opened,
            started: false,
        });
    }
    match resident::launch(leaf, false) {
        Outcome::Reply(reply) => Ok(Session {
            opened: reply.body,
            started: true,
        }),
        Outcome::Refusal(refusal) => Err(Outcome::Refusal(refusal)),
        Outcome::Fault(fault) => Err(Outcome::Fault(fault)),
    }
}

/// Stop the resident this command started, through ordinary `spice down`.
fn drain(current: &Path) -> Result<(), Outcome<AppClosed>> {
    let contract = spice::cli::contract(&["down"]).expect("down is a registered command");
    match resident::down_from(current, contract) {
        Outcome::Reply(_) => Ok(()),
        Outcome::Refusal(refusal) => Err(Outcome::Refusal(refusal)),
        Outcome::Fault(fault) => Err(Outcome::Fault(fault)),
    }
}

fn typed(outcome: Outcome<()>) -> Outcome<AppClosed> {
    match outcome {
        Outcome::Reply(_) => unreachable!("unit command failures never carry replies"),
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

/// An unsupported target is a refusal naming the browser route; a window
/// system that would not start is an environment fault.
fn window_failure(error: &desktop::WindowError) -> Outcome<AppClosed> {
    match error {
        desktop::WindowError::Unsupported => Outcome::Refusal(Refusal::bare(
            RefusalDiagnostic::new(
                "app::window-unsupported".to_owned(),
                Some("target-os".to_owned()),
                Some("macos".to_owned()),
                Some(std::env::consts::OS.to_owned()),
            )
            .with_detail(error.to_string()),
        )),
        desktop::WindowError::Platform(_) => {
            environment_fault("app::window-unavailable", &error.to_string())
        }
    }
}

fn environment_fault(reason: &str, detail: &str) -> Outcome<AppClosed> {
    Outcome::Fault(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}
