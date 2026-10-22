//! `spice app`: the console in a native window.
//!
//! The window is one more client of the resident. The command joins a
//! resident already serving this repository, or starts one exactly as `spice
//! up` does and drains it when the window closes; the endpoint it shows has
//! passed the same discovery admission a browser launch applies, and the
//! bearer token, when one is required, travels only in the URL fragment the
//! webview loads, never in argv, environment, or any log.

use std::path::Path;

use clap::ArgMatches;
use wire::{
    AppClosed, AppResident, Fault, FaultClass, Outcome, Refusal, RefusalDiagnostic, Reply,
    ReplyPosition,
};

use crate::command_client::{open_app_result_with, system_browser};
use crate::resident;

/// The window's title; the console names the repository inside.
const TITLE: &str = "Spice";

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
    let session = match join_or_start(&current, leaf) {
        Ok(session) => session,
        Err(outcome) => return outcome,
    };
    let url = token.as_deref().map_or_else(
        || session.opened.endpoint.clone(),
        |token| format!("{}#{token}", session.opened.endpoint),
    );
    let shown = desktop::show(TITLE, &url, |link| {
        let _ = system_browser(link);
    });
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

/// The admitted endpoint the window shows and whether this command owns the
/// resident behind it.
struct Session {
    opened: wire::AppOpen,
    started: bool,
}

/// Join the resident already serving `current`, or start one without a
/// browser launch when none is live.
fn join_or_start(current: &Path, leaf: &ArgMatches) -> Result<Session, Outcome<AppClosed>> {
    let repository = spice::repository::Repository::discover(current).map_err(|error| {
        environment_fault(spice::repository::ResolveError::REASON, &error.to_string())
    })?;
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
