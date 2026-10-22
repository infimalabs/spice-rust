//! Daemonless, read-only transcript forensics at the Rust process rim.
//!
//! Durable task completion belongs to the resident operation path. This
//! read-only surface does not distill or persist completion learnings, and no
//! test-only substitute is retained. The optional learning response shape is
//! wire compatibility data only; current task commands never produce it.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::ArgMatches;
use wire::{Outcome, Refusal, Reply, ReplyPosition};

mod authority;
mod context;
mod dataset;
pub(crate) mod digest;
mod glance;
mod model;
pub(crate) mod project;
mod source;

pub(crate) use model::Report;

/// Resolve one known vendor thread to its preserved transcript.
///
/// The resident uses this same path law to recover structured events that the
/// vendor's selected stdout stream does not carry. Keeping resolution here
/// prevents the app from growing a second interpretation of vendor homes,
/// state indexes, or transcript search precedence.
pub(crate) fn vendor_thread_transcript(
    thread: &str,
    vendor_name: &wire::Name,
    environment: &Environment,
) -> Result<(PathBuf, listen::Dialect), String> {
    let registry = spice::extension::registry()
        .map_err(|error| format!("extension registry is invalid: {error}"))?;
    let vendor = registry
        .vendor(vendor_name.as_str())
        .ok_or_else(|| format!("vendor {} is not registered", vendor_name.as_str()))?;
    let canonical = source::canonical_thread(thread)
        .ok_or_else(|| format!("thread {thread:?} is not a canonical UUID"))?;
    let path = source::find_thread(&canonical, vendor, environment)?
        .into_iter()
        .next_back()
        .ok_or_else(|| format!("vendor {} resolved no transcript paths", vendor.name()))?;
    let metadata =
        std::fs::symlink_metadata(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "{} is not a regular preserved transcript",
            path.display()
        ));
    }
    let path = path
        .canonicalize()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok((path, vendor.session_gate().dialect()))
}

#[derive(Debug, Clone)]
pub(crate) struct Environment {
    vendors: Vec<VendorEnvironment>,
    preferred_vendor: Option<OsString>,
    home: Option<PathBuf>,
    /// The working directory this process was asked from, captured once beside
    /// the vendor rows: a vendor that files sessions by working directory
    /// keys them by the directory that named them, so the directory is
    /// ambient session state in the same sense the home is.
    cwd: Option<PathBuf>,
    /// The temporary directory the process sees, captured beside the home
    /// for the same reason: oh-my-pi names a working directory beneath it
    /// relative to it.
    temp: Option<PathBuf>,
    /// How a briefing glances at a checkout's uncommitted paths and commits.
    checkout: fn(&std::path::Path) -> model::RepositoryGlanceView,
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
        mut vendors: Vec<VendorEnvironment>,
        preferred_vendor: Option<OsString>,
        home: Option<OsString>,
        cwd: Option<PathBuf>,
        temp: Option<PathBuf>,
    ) -> Self {
        vendors.sort_by_key(|row| {
            (
                row.vendor.session_gate().ambient_precedence(),
                row.vendor.session_gate().identity(),
            )
        });
        Self {
            vendors,
            preferred_vendor,
            home: home.map(PathBuf::from),
            cwd,
            temp,
            checkout: glance::repository,
        }
    }

    /// The same ambient session state, asked from another working directory:
    /// a supervisor reads its lane's transcript as that lane's worktree files it.
    pub(crate) fn rooted(mut self, cwd: PathBuf) -> Self {
        self.cwd = Some(cwd);
        self
    }

    /// The same session state, with briefings reporting the checkout's working
    /// state as unread rather than asking Git.
    #[cfg(test)]
    pub(crate) fn unglanced(mut self) -> Self {
        self.checkout = glance::unread;
        self
    }
}

/// Wall time a supervisor waits for its lane's briefing before it hands the
/// agent the refusal instead.
const LANE_BRIEFING_DEADLINE: std::time::Duration = std::time::Duration::from_secs(20);

/// The folded briefing a supervisor hands its lane's agent at a seat start,
/// framed as a private resident notice so transcript readers never take it for
/// an authored ask.
///
/// The seat is the lane rather than a live thread: the transcript read is the
/// session the residency head records for the lane. Any failure still yields
/// a page, one that names the refusal and the commands that read the rest.
#[derive(Clone, Copy)]
enum BriefingMoment {
    Startup,
    Refresh,
}

/// The fresh-session briefing, with an explicit notice when allocation already
/// assigned this lane a task before the agent came online.
pub(crate) fn lane_launch_briefing(
    environment: &Environment,
    lane: wire::CanonicalUuid,
    configuration: Option<&spice::config::RepositoryCapture>,
) -> String {
    render_lane_briefing(environment, lane, configuration, BriefingMoment::Startup)
}

pub(crate) fn lane_briefing(
    environment: &Environment,
    lane: wire::CanonicalUuid,
    configuration: Option<&spice::config::RepositoryCapture>,
) -> String {
    render_lane_briefing(environment, lane, configuration, BriefingMoment::Refresh)
}

/// The short notice a resumed seat receives in place of the briefing page.
///
/// The resumed agent still holds its context, so it learns only that this
/// start is a resume, which claim it holds, and how many steering keys wait.
/// The read skips the transcript fold that makes the full page heavy.
pub(crate) fn lane_resume_notice(
    environment: &Environment,
    lane: wire::CanonicalUuid,
    configuration: Option<&spice::config::RepositoryCapture>,
) -> String {
    let context = DeadlineContext {
        action: "resume".to_owned(),
        requested: vec![format!("lane {lane}")],
        budget: LANE_BRIEFING_DEADLINE,
    };
    let environment = environment.clone();
    let configuration = configuration.cloned();
    let observed = run_deadline(&context, std::time::Instant::now(), move |sender| {
        let authorities = authority::observe_lane(&environment, lane, configuration.as_ref());
        let _ = sender.send(DeadlineEvent::Complete(
            Ok(authorities),
            std::time::Instant::now(),
        ));
    });
    format!(
        "<system-notice>\n{}\n</system-notice>",
        resume_page(observed.as_ref().ok())
    )
}

fn resume_page(authorities: Option<&authority::RecallAuthorities>) -> String {
    const UNREAD: &str = "unread; `spice session briefing` reads it";
    let claim = authorities.map_or_else(
        || UNREAD.to_owned(),
        |read| match (&read.held_work.own_claim, &read.held_work.own_claim_unread) {
            (Some(claim), _) => format!("`{}` in phase `{}`", claim.handle, claim.phase),
            (None, Some(refusal)) => format!("unknown, not none: {refusal}"),
            (None, None) => "none".to_owned(),
        },
    );
    let pending = authorities
        .and_then(|read| read.asks.as_ref())
        .map_or_else(|| UNREAD.to_owned(), |asks| asks.len().to_string());
    format!(
        "# Seat resumed\n\nThis seat resumes its previous session with its context intact, so no briefing page is attached; continue from your context.\n- held claim: {claim}\n- pending steering keys: {pending}"
    )
}

fn render_lane_briefing(
    environment: &Environment,
    lane: wire::CanonicalUuid,
    configuration: Option<&spice::config::RepositoryCapture>,
    moment: BriefingMoment,
) -> String {
    let context = DeadlineContext {
        action: "briefing".to_owned(),
        requested: vec![format!("lane {lane}")],
        budget: LANE_BRIEFING_DEADLINE,
    };
    let environment = environment.clone();
    let configuration = configuration.cloned();
    let started = std::time::Instant::now();
    let built = run_deadline(&context, started, move |sender| {
        let report = lane_recall(&environment, lane, configuration.as_ref(), moment);
        let _ = sender.send(DeadlineEvent::Complete(report, std::time::Instant::now()));
    });
    let page = match built {
        Ok(Report::Recall {
            held_work, digest, ..
        }) => {
            let notices = [
                assignment_notice(moment, held_work.own_claim.as_ref()),
                renewal_notice(moment, &held_work),
            ];
            let mut page = String::new();
            for notice in notices.into_iter().flatten() {
                page.push_str(&notice);
                page.push_str("\n\n");
            }
            page.push_str(digest.trim_end());
            page
        }
        Ok(_) => unreachable!("lane recall builds only a briefing"),
        Err(refusal) => digest::refused(&refusal),
    };
    format!("<system-notice>\n{}\n</system-notice>", page.trim_end())
}

fn assignment_notice(
    moment: BriefingMoment,
    claim: Option<&model::HeldClaimView>,
) -> Option<String> {
    let claim = (matches!(moment, BriefingMoment::Startup)).then_some(claim)??;
    Some(format!(
        "# Task already assigned\n\n`{}` is already assigned to this lane in phase `{}`: {}. Continue this task now; run `spice task show {}` for its contract before asking the allocator for work.",
        claim.handle, claim.phase, claim.title, claim.handle,
    ))
}

/// The startup-only rehydration route a renewal successor owes its context.
///
/// Derived from the lane's durable handoff-pending record, never published as
/// steering, so it carries no acknowledgment key and leaves no inbox residue.
fn renewal_notice(moment: BriefingMoment, held_work: &model::HeldWorkView) -> Option<String> {
    let predecessor = (matches!(moment, BriefingMoment::Startup))
        .then_some(held_work.renewed_from.as_deref())??;
    Some(format!(
        "# Lane renewed\n\nThis is a fresh successor of agent thread `{predecessor}`, which handed this lane off. Rehydrate its context before continuing: resolve its transcript with `spice session thread {predecessor}`, then run `spice session briefing PATH` and `spice session sweep PATH --limit 4` on the returned path."
    ))
}

fn lane_recall(
    environment: &Environment,
    lane: wire::CanonicalUuid,
    configuration: Option<&spice::config::RepositoryCapture>,
    moment: BriefingMoment,
) -> Result<Report, Refusal> {
    let mut authorities = authority::observe_lane(environment, lane, configuration);
    // A fresh launch reads its lane before its own birth is recorded, so the
    // seat it sees is vacant or still its predecessor's; neither is its own.
    if matches!(moment, BriefingMoment::Startup) {
        authorities.held_work.address = Some(model::OwnAddress::Launching);
    }
    let seat = authorities.held_work.thread.clone().zip(
        authorities
            .posture
            .as_ref()
            .and_then(|posture| posture.lane.as_ref())
            .map(|lane| lane.vendor.clone()),
    );
    let mut files = Vec::new();
    if let Some((thread, vendor)) = seat {
        let resolved = vendor
            .parse::<wire::Name>()
            .map_err(|error| error.to_string())
            .and_then(|vendor| vendor_thread_transcript(&thread, &vendor, environment));
        match resolved {
            Ok((path, _)) => files.push(path.display().to_string()),
            Err(detail) => authorities.unknowns.push(model::AuthorityUnknown {
                authority: "transcript".to_owned(),
                probe: "lane-thread".to_owned(),
                detail,
            }),
        }
    }
    let sources = if files.is_empty() {
        Vec::new()
    } else {
        source::resolve_inputs(&files, environment)?
    };
    project::recall(&sources, &files, authorities)
}

pub(crate) fn run(
    matches: &ArgMatches,
    path: &[&str],
    environment: &Environment,
    configuration: Option<&spice::config::RepositoryCapture>,
) -> Outcome<Report> {
    let action = path.get(1).copied().unwrap_or("briefing");
    let result = if action == "context" {
        context::build(matches, path, environment)
    } else if matches!(action, "briefing" | "sweep") {
        deadline_build(matches, path, action, environment, configuration)
    } else {
        build(matches, path, action, environment, configuration)
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
    configuration: Option<&spice::config::RepositoryCapture>,
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
    let owned_configuration = configuration.cloned();
    run_deadline(&context, started, move |sender| {
        deadline_worker(
            owned_matches,
            owned_path,
            owned_action,
            owned_environment,
            owned_configuration,
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
    configuration: Option<spice::config::RepositoryCapture>,
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
    let result = build_resolved(
        &matches,
        &path,
        &action,
        sources,
        &environment,
        configuration.as_ref(),
    );
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
    configuration: Option<&spice::config::RepositoryCapture>,
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
        return Ok(
            match dataset::Aggregate::scan(&resolved, dataset::Recent::NONE) {
                Ok(aggregate) => aggregate.thread(id),
                Err(error) => dataset::Aggregate::thread_without_summary(&resolved, id, &error),
            },
        );
    }

    if HISTORY.contains(&action) {
        crate::read_page::Page::from_matches(matches, path)?;
    }
    let files = project::values(matches, "FILES");
    let sources = source::resolve_inputs(&files, environment)?;
    build_resolved(matches, path, action, sources, environment, configuration)
}

/// Session leaves that page their records through [`crate::read_page::Page`].
const HISTORY: [&str; 10] = [
    "sweep",
    "turns",
    "messages",
    "phases",
    "timeline",
    "slices",
    "commands",
    "compactions",
    "commits",
    "summary",
];

fn build_resolved(
    matches: &ArgMatches,
    path: &[&str],
    action: &str,
    sources: Vec<source::ResolvedSource>,
    environment: &Environment,
    configuration: Option<&spice::config::RepositoryCapture>,
) -> Result<Report, Refusal> {
    match action {
        "briefing" => {
            let files = project::values(matches, "FILES");
            let authorities = authority::observe(&sources, environment, configuration);
            return project::recall(&sources, &files, authorities);
        }
        "tokens" => {
            return Ok(dataset::Aggregate::scan(&sources, dataset::Recent::NONE)?
                .tokens(project::tokens_by_file(matches)));
        }
        _ => {}
    }
    let page = || crate::read_page::Page::from_matches(matches, path);
    match action {
        "sweep" => project::sweep(&sources, &project::values(matches, "FILES"), page()?),
        "turns" => project::turns(&sources, matches, page()?),
        "messages" => project::messages(&sources, matches, page()?),
        "phases" => project::phases(&sources, matches, page()?),
        "timeline" => project::timeline(&sources, matches, page()?),
        "slices" => project::slices(&sources, matches, page()?),
        "commands" => project::commands(&sources, matches, page()?),
        "compactions" => project::compactions(&sources, page()?),
        "commits" => project::commits(&sources, page()?),
        "summary" => project::summary(&sources, page()?),
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
mod contract;

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
    /// scheduled inside a one-millisecond budget, and nothing sends after the
    /// wait starts to race the dropped receiver.
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
            page: wire::output::Bounded {
                rows: Vec::new(),
                omission: None,
            },
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
        let default = deadline_duration_value(None, "briefing").expect("default is valid");
        let explicit = deadline_duration_value(Some("1.2500"), "sweep").expect("explicit is valid");
        assert_eq!(duration_label(default), "30s");
        assert_eq!(duration_label(explicit), "1.25s");
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
        let render_diagnostic = render_refusal.diagnostic().expect("deadline is diagnostic");
        assert_eq!(
            render_diagnostic.observed(),
            Some("action=briefing inputs=resolved[\"/canonical/input.jsonl\"] budget=0.001s")
        );
        assert!(
            !render_diagnostic
                .observed()
                .expect("deadline carries its witness")
                .contains("verbatim transcript secret")
        );
        assert_eq!(render_diagnostic.witness(), Some("session-query"));
        assert_eq!(
            render_diagnostic.expected(),
            Some("one registered read-only transcript query")
        );
    }

    #[test]
    fn held_task_notice_is_emitted_only_for_a_fresh_start() {
        let claim = model::HeldClaimView {
            handle: "SESSION-held".to_owned(),
            project: "spice.session".to_owned(),
            phase: "todo".to_owned(),
            status: "pending".to_owned(),
            title: "Carry the assigned task".to_owned(),
            lease_expiry: None,
            flow: vec!["todo".to_owned(), "review".to_owned()],
            description: None,
            acceptance: Vec::new(),
        };
        assert!(assignment_notice(BriefingMoment::Startup, Some(&claim)).is_some());
        assert_eq!(
            assignment_notice(BriefingMoment::Refresh, Some(&claim)),
            None,
        );
        assert_eq!(assignment_notice(BriefingMoment::Startup, None), None);
    }

    #[test]
    fn only_a_fresh_start_of_a_handoff_pending_lane_carries_renewal_context() {
        let id = |n: u8| -> wire::CanonicalUuid {
            format!("00000000-0000-4000-8000-0000000000{n:02x}")
                .parse()
                .expect("canonical fixture identity")
        };
        let identity = |occupant: u8, thread: &str| wire::RenewalIdentity {
            occupant: id(occupant),
            session: format!("session-{thread}"),
            thread: thread.to_owned(),
        };
        let record = |lane: u8, state| wire::LaneRenewalRecord {
            team: id(9),
            lane: id(lane),
            revision: 1,
            state,
        };
        let renewals = [
            record(
                1,
                wire::LaneRenewalState::Requested {
                    predecessor: identity(11, "requested"),
                    team_slot: 0,
                },
            ),
            record(
                2,
                wire::LaneRenewalState::HandoffPending {
                    predecessor: identity(12, "handed-off"),
                    team_slot: 1,
                },
            ),
            record(
                3,
                wire::LaneRenewalState::SuccessorStarted {
                    predecessor: identity(13, "replaced"),
                    successor: identity(23, "successor"),
                    team_slot: 2,
                },
            ),
        ];
        assert_eq!(authority::handoff_predecessor(&renewals, id(1)), None);
        assert_eq!(
            authority::handoff_predecessor(&renewals, id(2)).as_deref(),
            Some("handed-off"),
        );
        assert_eq!(authority::handoff_predecessor(&renewals, id(3)), None);
        assert_eq!(authority::handoff_predecessor(&renewals, id(4)), None);

        let mut held = model::HeldWorkView {
            thread: Some("handed-off".to_owned()),
            lane: Some(id(2).to_string()),
            own_claim: None,
            own_claim_unread: None,
            peer_claims: None,
            renewed_from: Some("handed-off".to_owned()),
            address: None,
        };
        let notice = renewal_notice(BriefingMoment::Startup, &held).expect("successor startup");
        assert!(
            notice.contains("spice session thread handed-off"),
            "{notice}"
        );
        assert_eq!(renewal_notice(BriefingMoment::Refresh, &held), None);
        held.renewed_from = None;
        assert_eq!(renewal_notice(BriefingMoment::Startup, &held), None);
    }

    #[test]
    fn a_resumed_seat_reports_no_claim_only_after_a_successful_claim_read() {
        let read = |unread: Option<&str>| authority::RecallAuthorities {
            asks: Some(Vec::new()),
            held_work: model::HeldWorkView {
                thread: None,
                lane: None,
                own_claim: None,
                own_claim_unread: unread.map(str::to_owned),
                peer_claims: None,
                renewed_from: None,
                address: None,
            },
            posture: None,
            peers: Vec::new(),
            directions: Vec::new(),
            board: None,
            repository: None,
            unknowns: Vec::new(),
        };
        let refusal = "board entry is invalid: fixture refusal";
        let unknown = resume_page(Some(&read(Some(refusal))));
        assert!(unknown.contains(refusal), "{unknown}");
        assert!(!unknown.contains("held claim: none"), "{unknown}");
        let known = resume_page(Some(&read(None)));
        assert!(known.contains("held claim: none"), "{known}");
    }
}
