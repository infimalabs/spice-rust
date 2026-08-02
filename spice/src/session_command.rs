//! Daemonless, read-only transcript forensics at the Rust process rim.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::ArgMatches;
use wire::{Outcome, Refusal, Reply, ReplyPosition};

mod dataset;
mod model;
mod project;
mod source;

use model::Report;

#[derive(Debug, Clone)]
pub(crate) struct Environment {
    codex_thread: Option<OsString>,
    claude_session: Option<OsString>,
    preferred_vendor: Option<OsString>,
    codex_home: Option<PathBuf>,
    claude_home: Option<PathBuf>,
    home: Option<PathBuf>,
}

impl Environment {
    pub(crate) fn new(
        codex_thread: Option<OsString>,
        claude_session: Option<OsString>,
        preferred_vendor: Option<OsString>,
        codex_home: Option<OsString>,
        claude_home: Option<OsString>,
        home: Option<OsString>,
    ) -> Self {
        Self {
            codex_thread,
            claude_session,
            preferred_vendor,
            codex_home: codex_home.map(PathBuf::from),
            claude_home: claude_home.map(PathBuf::from),
            home: home.map(PathBuf::from),
        }
    }
}

pub(crate) fn run(
    matches: &ArgMatches,
    path: &[&str],
    environment: &Environment,
) -> Outcome<Report> {
    let action = path.get(1).copied().unwrap_or("briefing");
    let result = if matches!(action, "briefing" | "sweep") {
        deadline_build(matches, path, action, environment)
    } else {
        build(matches, path, environment)
    };
    outcome(result)
}

fn outcome(result: Result<Report, Refusal>) -> Outcome<Report> {
    match result {
        Ok(body) => Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body,
        }),
        Err(refusal) => Outcome::Refusal(refusal),
    }
}

fn deadline_build(
    matches: &ArgMatches,
    path: &[&str],
    action: &str,
    environment: &Environment,
) -> Result<Report, Refusal> {
    let duration = deadline_duration(matches, action)?;
    let owned_matches = matches.clone();
    let owned_path = path
        .iter()
        .map(|part| (*part).to_owned())
        .collect::<Vec<_>>();
    let owned_environment = environment.clone();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name(format!("spice-session-{action}-deadline"))
        .spawn(move || {
            let path = owned_path.iter().map(String::as_str).collect::<Vec<_>>();
            let _ = sender.send(build(&owned_matches, &path, &owned_environment));
        })
        .map_err(|error| {
            refusal(
                "session::deadline-worker-unavailable",
                Some(error.to_string()),
                "restore process thread capacity and retry the same immutable inputs",
            )
        })?;
    match receiver.recv_timeout(duration) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(refusal(
            "session::deadline-exceeded",
            Some(format!(
                "{action} end-to-end render exceeded {}s",
                duration.as_secs_f64()
            )),
            "increase --deadline-seconds or narrow the immutable transcript inputs and filters",
        )),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(refusal(
            "session::deadline-worker-interrupted",
            Some(action.to_owned()),
            "retry the same immutable inputs; if interruption repeats, preserve the \
             transcript and inspect the process fault",
        )),
    }
}

fn build(
    matches: &ArgMatches,
    path: &[&str],
    environment: &Environment,
) -> Result<Report, Refusal> {
    let action = path.get(1).copied().unwrap_or("briefing");
    if action == "thread" {
        let raw = matches
            .get_one::<String>("THREAD_ID")
            .expect("the generated session thread syntax requires THREAD_ID");
        let id = source::canonical_thread(raw).ok_or_else(|| {
            refusal(
                "session::thread-invalid",
                Some(raw.clone()),
                "pass one dashed or dashless hexadecimal UUID thread identity",
            )
        })?;
        let resolved = source::resolve_thread(&id, environment)?;
        let data = dataset::load(std::slice::from_ref(&resolved))?;
        let source = data
            .sources
            .first()
            .cloned()
            .expect("one resolved thread yields one source");
        return Ok(project::thread(source, &data, id));
    }

    let files = project::values(matches, "FILES");
    let sources = source::resolve_inputs(&files, environment)?;
    let data = dataset::load(&sources)?;
    let filters = project::Filters::from_matches(matches)?;
    match action {
        "briefing" => project::recall(&data, matches, &filters),
        "sweep" => project::sweep(&data, matches, &filters),
        "summary" => project::summary(&data, matches),
        "tokens" => Ok(project::tokens(&data, matches)),
        "timeline" => project::timeline(&data, matches, &filters),
        "turns" => project::turns(&data, matches, &filters),
        "compactions" => project::compactions(&data, matches),
        "slices" => project::slices(&data, matches),
        "phases" => project::phases(&data, matches, &filters),
        "messages" => project::messages(&data, matches),
        "commits" => project::commits(&data, matches),
        "commands" => project::commands(&data, matches),
        _ => Err(refusal(
            "session::query-unregistered",
            Some(path.join(" ")),
            "select one generated session query leaf",
        )),
    }
}

fn deadline_duration(matches: &ArgMatches, action: &str) -> Result<std::time::Duration, Refusal> {
    let value = matches
        .try_get_one::<String>("deadline-seconds")
        .ok()
        .flatten()
        .map_or("30", String::as_str);
    let seconds = value.parse::<f64>().map_err(|_| {
        refusal(
            "session::deadline-invalid",
            Some(value.to_owned()),
            "pass a positive finite number of seconds",
        )
    })?;
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err(refusal(
            "session::deadline-invalid",
            Some(format!("{action} deadline={value}")),
            "increase --deadline-seconds and retry the same immutable inputs",
        ));
    }
    std::time::Duration::try_from_secs_f64(seconds).map_err(|_| {
        refusal(
            "session::deadline-invalid",
            Some(format!("{action} deadline={value}")),
            "pass a positive finite number of seconds representable by the host clock",
        )
    })
}

fn refusal(reason: &str, observed: Option<String>, remedy: &str) -> Refusal {
    Refusal {
        reason: reason.to_owned(),
        witness: Some("session-query".to_owned()),
        expected: Some("one registered read-only transcript query".to_owned()),
        observed,
        remedy: remedy.to_owned(),
    }
}
