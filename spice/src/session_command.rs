//! Daemonless, read-only transcript forensics at the Rust process rim.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::ArgMatches;
use wire::{Outcome, Refusal, Reply, ReplyPosition};

mod dataset;
pub(crate) mod learnings;
mod model;
mod project;
mod source;

use model::Report;

#[derive(Debug, Clone)]
pub(crate) struct Environment {
    vendors: Vec<VendorEnvironment>,
    preferred_vendor: Option<OsString>,
    home: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub(crate) struct VendorEnvironment {
    vendor: &'static spice::extension::Vendor,
    identity: Option<OsString>,
    home: Option<PathBuf>,
}

impl VendorEnvironment {
    pub(crate) fn new(
        vendor: &'static spice::extension::Vendor,
        identity: Option<OsString>,
        home: Option<OsString>,
    ) -> Self {
        Self {
            vendor,
            identity,
            home: home.map(PathBuf::from),
        }
    }
}

impl Environment {
    pub(crate) fn new(
        vendors: Vec<VendorEnvironment>,
        preferred_vendor: Option<OsString>,
        home: Option<OsString>,
    ) -> Self {
        Self {
            vendors,
            preferred_vendor,
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
        build(matches, path, action, environment)
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

struct DeadlineContext {
    action: String,
    requested: Vec<String>,
    budget: std::time::Duration,
}

enum DeadlineEvent<T> {
    Resolved(Vec<String>),
    Complete(Result<T, Refusal>, std::time::Instant),
}

fn deadline_build(
    matches: &ArgMatches,
    path: &[&str],
    action: &str,
    environment: &Environment,
) -> Result<Report, Refusal> {
    let context = DeadlineContext {
        action: action.to_owned(),
        requested: requested_inputs(matches, environment),
        budget: deadline_duration(matches, action)?,
    };
    let started = std::time::Instant::now();
    let owned_matches = matches.clone();
    let owned_path = path
        .iter()
        .map(|part| (*part).to_owned())
        .collect::<Vec<_>>();
    let owned_action = action.to_owned();
    let owned_environment = environment.clone();
    run_deadline(&context, started, move |sender| {
        deadline_worker(
            owned_matches,
            owned_path,
            owned_action,
            owned_environment,
            sender,
        );
    })
}

fn run_deadline<T, F>(
    context: &DeadlineContext,
    started: std::time::Instant,
    worker: F,
) -> Result<T, Refusal>
where
    T: Send + 'static,
    F: FnOnce(std::sync::mpsc::Sender<DeadlineEvent<T>>) + Send + 'static,
{
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name(format!("spice-session-{}-deadline", context.action))
        .spawn(move || worker(sender))
        .map_err(|error| {
            refusal(
                "session::deadline-worker-unavailable",
                Some(error.to_string()),
                "restore process thread capacity and retry the same immutable inputs",
            )
        })?;
    wait_for_deadline(context, started, receiver)
}

fn deadline_worker(
    matches: ArgMatches,
    path: Vec<String>,
    action: String,
    environment: Environment,
    sender: std::sync::mpsc::Sender<DeadlineEvent<Report>>,
) {
    let files = project::values(&matches, "FILES");
    let sources = match source::resolve_inputs(&files, &environment) {
        Ok(sources) => sources,
        Err(error) => {
            let _ = sender.send(DeadlineEvent::Complete(
                Err(error),
                std::time::Instant::now(),
            ));
            return;
        }
    };
    let resolved = sources
        .iter()
        .map(|source| source.path.display().to_string())
        .collect();
    if sender.send(DeadlineEvent::Resolved(resolved)).is_err() {
        return;
    }
    let path = path.iter().map(String::as_str).collect::<Vec<_>>();
    let result = build_resolved(&matches, &path, &action, sources);
    let _ = sender.send(DeadlineEvent::Complete(result, std::time::Instant::now()));
}

fn wait_for_deadline<T>(
    context: &DeadlineContext,
    started: std::time::Instant,
    receiver: std::sync::mpsc::Receiver<DeadlineEvent<T>>,
) -> Result<T, Refusal> {
    let mut resolved = None;
    loop {
        let remaining = context.budget.saturating_sub(started.elapsed());
        match receiver.recv_timeout(remaining) {
            Ok(DeadlineEvent::Resolved(inputs)) => resolved = Some(inputs),
            Ok(DeadlineEvent::Complete(result, finished))
                if finished.saturating_duration_since(started) <= context.budget =>
            {
                return result;
            }
            Ok(DeadlineEvent::Complete(_, _)) => {
                return Err(deadline_exceeded(context, resolved.as_deref()));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                return Err(deadline_exceeded(context, resolved.as_deref()));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(refusal(
                    "session::deadline-worker-interrupted",
                    Some(context.action.clone()),
                    "retry the same immutable inputs; if interruption repeats, preserve the \
                     transcript and inspect the process fault",
                ));
            }
        }
    }
}

fn deadline_exceeded(context: &DeadlineContext, resolved: Option<&[String]>) -> Refusal {
    let (state, inputs) = resolved.map_or(("requested", context.requested.as_slice()), |inputs| {
        ("resolved", inputs)
    });
    refusal(
        "session::deadline-exceeded",
        Some(format!(
            "action={} inputs={state}{} budget={}",
            context.action,
            input_list(inputs),
            duration_label(context.budget)
        )),
        "increase --deadline-seconds or narrow the immutable transcript inputs and filters",
    )
}

fn requested_inputs(matches: &ArgMatches, environment: &Environment) -> Vec<String> {
    let files = project::values(matches, "FILES");
    if !files.is_empty() {
        return files;
    }
    for row in &environment.vendors {
        if let Some(value) = &row.identity {
            return vec![format!(
                "{}={}",
                row.vendor.thread_id_environment(),
                value.to_string_lossy()
            )];
        }
    }
    vec!["working-directory transcript discovery".to_owned()]
}

fn input_list(inputs: &[String]) -> String {
    let rendered = inputs
        .iter()
        .map(|input| format!("{input:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{rendered}]")
}

fn duration_label(duration: std::time::Duration) -> String {
    if duration.subsec_nanos() == 0 {
        return format!("{}s", duration.as_secs());
    }
    let fraction = format!("{:09}", duration.subsec_nanos());
    format!(
        "{}.{fraction}s",
        duration.as_secs(),
        fraction = fraction.trim_end_matches('0')
    )
}

fn build(
    matches: &ArgMatches,
    path: &[&str],
    action: &str,
    environment: &Environment,
) -> Result<Report, Refusal> {
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
    build_resolved(matches, path, action, sources)
}

fn build_resolved(
    matches: &ArgMatches,
    path: &[&str],
    action: &str,
    sources: Vec<source::ResolvedSource>,
) -> Result<Report, Refusal> {
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
        .map(String::as_str);
    deadline_duration_value(value, action)
}

fn deadline_duration_value(
    value: Option<&str>,
    action: &str,
) -> Result<std::time::Duration, Refusal> {
    let value = value.unwrap_or("30");
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

fn refusal(reason: &str, observed: Option<String>, detail: &str) -> Refusal {
    Refusal::bare(
        wire::RefusalDiagnostic::new(
            reason.to_owned(),
            Some("session-query".to_owned()),
            Some("one registered read-only transcript query".to_owned()),
            observed,
        )
        .with_detail(detail),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(action: &str, requested: &[&str], budget: std::time::Duration) -> DeadlineContext {
        DeadlineContext {
            action: action.to_owned(),
            requested: requested.iter().map(|input| (*input).to_owned()).collect(),
            budget,
        }
    }

    /// The refusal a worker earns by never completing, having resolved `resolved`.
    ///
    /// The resolved-input event is queued before the wait begins, so the wait
    /// observes it by construction rather than by a worker thread being
    /// scheduled inside a one-millisecond budget. A held worker thread used to
    /// send it while the clock ran, and losing that race surfaced two ways: a
    /// send that beat the dropped receiver but not the budget left the refusal
    /// naming requested inputs instead of resolved ones, and a send arriving
    /// after the drop panicked the worker, which stranded the release this
    /// helper then sent it. Nothing here sends after the wait starts, so
    /// neither remains reachable.
    ///
    /// `sender` stays live across the wait because a dropped one turns the
    /// budget into a disconnected-worker refusal, which is a different path
    /// from the timeout these tests exist to pin.
    fn timeout(context: &DeadlineContext, resolved: Option<&[&str]>) -> Refusal {
        let (sender, receiver) = std::sync::mpsc::channel::<DeadlineEvent<Report>>();
        if let Some(resolved) = resolved {
            sender
                .send(DeadlineEvent::Resolved(
                    resolved.iter().map(|input| (*input).to_owned()).collect(),
                ))
                .expect("the resolved-input event reaches the deadline receiver");
        }
        let refusal = wait_for_deadline(context, std::time::Instant::now(), receiver)
            .expect_err("an open worker with no complete event exceeds its budget");
        // Dropping here rather than at the end of scope is what makes that
        // liveness a compile error to break: handing `sender` to a worker above
        // would leave this reading a moved value.
        drop(sender);
        refusal
    }

    /// A worker spawned by `run_deadline` hands its report back to the caller.
    ///
    /// The timeout helper builds its own channel and waits directly, so this is
    /// the only test that drives the spawn, the sender handed to the worker, and
    /// the completion branch that returns the value the worker sent. Its budget is
    /// the default thirty seconds against a worker whose whole body is one send,
    /// so reaching the completion branch does not turn on scheduling latency the
    /// way a one-millisecond budget would.
    #[test]
    fn a_worker_completing_inside_its_budget_returns_the_report_it_built() {
        let context = context(
            "sweep",
            &["thread-identity"],
            std::time::Duration::from_secs(30),
        );
        let built = Report::Sweep {
            sources: Vec::new(),
            windows: Vec::new(),
            residues: Vec::new(),
        };
        let sent = built.clone();
        let returned = run_deadline(&context, std::time::Instant::now(), move |sender| {
            sender
                .send(DeadlineEvent::Complete(Ok(sent), std::time::Instant::now()))
                .expect("the completion event reaches the deadline receiver");
        })
        .expect("a worker completing inside its budget is not refused");
        assert_eq!(returned, built);
    }

    #[test]
    fn source_discovery_and_report_construction_share_one_deadline() {
        let budget = std::time::Duration::from_millis(1);
        let discovering = context("sweep", &["thread-identity"], budget);
        let discovery_refusal = timeout(&discovering, None);
        assert_eq!(discovery_refusal.reason(), "session::deadline-exceeded");
        assert_eq!(
            discovery_refusal
                .diagnostic()
                .and_then(wire::RefusalDiagnostic::observed),
            Some("action=sweep inputs=requested[\"thread-identity\"] budget=0.001s")
        );

        let rendering = context("briefing", &["input.jsonl"], budget);
        let render_refusal = timeout(&rendering, Some(&["/canonical/input.jsonl"]));
        assert_eq!(render_refusal.reason(), "session::deadline-exceeded");
        assert_eq!(
            render_refusal
                .diagnostic()
                .and_then(wire::RefusalDiagnostic::observed),
            Some("action=briefing inputs=resolved[\"/canonical/input.jsonl\"] budget=0.001s")
        );
    }

    #[test]
    fn default_and_explicit_deadline_witnesses_are_canonical() {
        let default = deadline_duration_value(None, "briefing").expect("default is valid");
        let explicit = deadline_duration_value(Some("1.2500"), "sweep").expect("explicit is valid");
        assert_eq!(duration_label(default), "30s");
        assert_eq!(duration_label(explicit), "1.25s");
    }

    #[test]
    fn a_deadline_refusal_carries_source_identity_but_no_partial_report_data() {
        let context = context(
            "briefing",
            &["private-thread-identity"],
            std::time::Duration::from_millis(1),
        );
        let refusal = timeout(&context, Some(&["/private/session.jsonl"]));
        let diagnostic = refusal.diagnostic().expect("deadline is diagnostic");
        let observed = diagnostic
            .observed()
            .expect("the deadline carries its witness");
        assert!(observed.contains("/private/session.jsonl"));
        assert!(!observed.contains("verbatim transcript secret"));
        assert_eq!(diagnostic.witness(), Some("session-query"));
        assert_eq!(
            diagnostic.expected(),
            Some("one registered read-only transcript query")
        );
    }
}
