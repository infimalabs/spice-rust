//! Decision-first output for the two long mutation-era audits, `gate kill`
//! and `gate camp`.
//!
//! Both keep their typed reports, movement, floors, and exit statuses. What
//! changes is what a person reads: live lines that name the campaign and its
//! done/total count, a heartbeat only after a quiet interval, and a closing
//! summary of ordered decisions with the exact next commands. The canonical
//! report text, which carries every attempt's process evidence, is retained
//! as a file and never printed.

use std::sync::Mutex;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use gate::{
    CampProgress, CampReport, CampStanding, KillProgress, KillReport, MutantDisposition,
    ReportAttemptKind, ReportMutantDisposition, ReportMutationMovement, ReportRate, Selector,
};
use serde::Serialize;
use wire::output::{Export, clip};
use wire::{Outcome, Refusal};

use super::output::{self, Elapsed, Progress, Summary};
use super::{Failure, SelectedContract, direct};

/// Mutants each kill list shows by default.
const SHOWN_MUTANTS: usize = 20;
/// Mutants each kill list shows under `--verbose`.
const VERBOSE_MUTANTS: usize = 200;
/// Survivors and unconfirmed mutants named live before the rest are only
/// counted.
const LIVE_FINDINGS: usize = 10;
/// Camping loci shown by default and under `--verbose`.
const SHOWN_LOCI: usize = 20;
const VERBOSE_LOCI: usize = 200;
/// Bytes of one mutant identity or locus name a line carries.
const NAME_BYTES: usize = 160;
/// How often the heartbeat thread asks whether a quiet interval has passed.
const TICK: Duration = Duration::from_secs(1);

/// `gate kill`'s typed report and its decision-first text.
#[derive(Serialize)]
pub(crate) struct KillAnswer {
    #[serde(flatten)]
    report: KillReport,
    #[serde(skip)]
    text: String,
}

impl KillAnswer {
    pub(crate) fn text(&self) -> Option<&str> {
        Some(&self.text)
    }
}

/// `gate camp`'s typed report and its decision-first text.
#[derive(Serialize)]
pub(crate) struct CampAnswer {
    #[serde(flatten)]
    report: CampReport,
    #[serde(skip)]
    text: String,
}

impl CampAnswer {
    pub(crate) fn text(&self) -> Option<&str> {
        Some(&self.text)
    }
}

/// Run the compiled mutation profile against private copies of the index.
pub(crate) fn kill(
    matches: &clap::ArgMatches,
    live: bool,
    ambient: &spice::ambient::Ambient,
) -> Outcome<KillAnswer> {
    let verbose = matches.get_flag("verbose");
    let selected = match SelectedContract::capture(Selector::staged()) {
        Ok(selected) => selected,
        Err(failure) => return failure.outcome(),
    };
    let host = match spice::gate::MutationHost::capture(ambient) {
        Ok(host) => host,
        Err(error) => return super::battery_failure(error),
    };
    let progress = Progress::stderr("gate kill", live);
    progress.line(format_args!("baseline and mutant inventory starting"));
    let started = Instant::now();
    let campaign = Mutex::new(Campaign::new(output::HEARTBEAT, started));
    let run = beating(&progress, &campaign, Campaign::heartbeat, || {
        selected.contract.execute_kill_with_progress(
            selected.root(),
            host.rim(),
            host.compiler_environment(),
            &mut |observation| {
                let line = campaign
                    .lock()
                    .ok()
                    .and_then(|mut campaign| campaign.observe(observation, Instant::now()));
                if let Some(line) = line {
                    progress.line(format_args!("{line}"));
                }
            },
        )
    });
    let report = match run {
        Ok(run) => run.report(),
        Err(error) => return refused("gate::kill-refused", selected.captures(), &error),
    };
    let retained = output::retain(selected.captures(), report.render().as_bytes());
    let text = kill_summary(
        &KillOutcome::of(&report),
        started.elapsed(),
        &retained,
        verbose,
    );
    let refusal = match report.movement() {
        ReportMutationMovement::Regressed { .. } => Some((
            "gate::kill-regressed",
            "remove every confirmed survivor; never raise the survivor floor",
        )),
        ReportMutationMovement::Unconfirmed { .. } => Some((
            "gate::kill-unconfirmed",
            "make the flaky, timed-out, or incomplete attempts repeatable, then run the gate again",
        )),
        ReportMutationMovement::Held { .. }
        | ReportMutationMovement::Shrank { .. }
        | ReportMutationMovement::Unbaselined { .. } => None,
    };
    match refusal {
        Some((reason, summary)) => Outcome::Refusal(Refusal::evidence(reason, text, summary)),
        None => direct(KillAnswer { report, text }),
    }
}

/// Audit the compiled shared bound without changing a tracked floor.
pub(crate) fn camp(matches: &clap::ArgMatches, live: bool) -> Outcome<CampAnswer> {
    let verbose = matches.get_flag("verbose");
    let selected = match SelectedContract::capture(Selector::staged()) {
        Ok(selected) => selected,
        Err(failure) => return failure.outcome(),
    };
    let progress = Progress::stderr("gate camp", live);
    let started = Instant::now();
    let campaign = Mutex::new(CampCampaign::new(output::HEARTBEAT, started));
    let audit = beating(&progress, &campaign, CampCampaign::heartbeat, || {
        selected
            .contract
            .audit_camp_with_progress(selected.root(), &mut |observation| {
                let line = campaign
                    .lock()
                    .ok()
                    .and_then(|mut campaign| campaign.observe(observation, Instant::now()));
                if let Some(line) = line {
                    progress.line(format_args!("{line}"));
                }
            })
    });
    let report = match audit {
        Ok(report) => report,
        Err(error) => return refused("gate::camp-refused", selected.captures(), &error),
    };
    let retained = output::retain(selected.captures(), report.render().as_bytes());
    let text = camp_summary(&report, started.elapsed(), &retained, verbose);
    direct(CampAnswer { report, text })
}

/// A run that ended before a report: its bounded tail and retained text.
fn refused<B>(
    reason: &'static str,
    captures: &std::path::Path,
    error: &dyn std::fmt::Display,
) -> Outcome<B> {
    Failure::Refusal {
        reason,
        expected: "one complete bounded current gate result".to_owned(),
        observed: output::failure_evidence(captures, error.to_string().as_bytes()),
        detail: "restore the declared inputs and tools, then run the gate again".to_owned(),
    }
    .outcome()
}

/// Run `work` while a thread writes each heartbeat the campaign says is due.
fn beating<T, C: Send>(
    progress: &Progress,
    campaign: &Mutex<C>,
    heartbeat: fn(&mut C, Instant) -> Option<String>,
    work: impl FnOnce() -> T,
) -> T {
    let (stop, stopped) = mpsc::channel::<()>();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            while matches!(stopped.recv_timeout(TICK), Err(RecvTimeoutError::Timeout)) {
                let line = campaign
                    .lock()
                    .ok()
                    .and_then(|mut campaign| heartbeat(&mut campaign, Instant::now()));
                if let Some(line) = line {
                    progress.line(format_args!("{line}"));
                }
            }
        });
        let value = work();
        drop(stop);
        value
    })
}

/// Live state of one camping campaign.
pub(super) struct CampCampaign {
    interval: Duration,
    started: Instant,
    quiet_since: Instant,
    total: Option<usize>,
    completed: usize,
    running: Option<String>,
}

impl CampCampaign {
    pub(super) const fn new(interval: Duration, now: Instant) -> Self {
        Self {
            interval,
            started: now,
            quiet_since: now,
            total: None,
            completed: 0,
            running: None,
        }
    }

    pub(super) fn observe(
        &mut self,
        observation: CampProgress<'_>,
        now: Instant,
    ) -> Option<String> {
        self.total = Some(observation.total);
        let line = if let Some(metric) = observation.metric {
            if observation.completed > self.completed {
                self.running = None;
            } else {
                self.running = Some(clip(metric, NAME_BYTES).to_string());
            }
            self.completed = observation.completed;
            None
        } else {
            self.completed = observation.completed;
            Some(format!(
                "campaign: {} distributions, {} loci, {} measurements",
                observation.distributions, observation.loci, observation.total
            ))
        };
        if line.is_some() {
            self.quiet_since = now;
        }
        line
    }

    pub(super) fn heartbeat(&mut self, now: Instant) -> Option<String> {
        if now.saturating_duration_since(self.quiet_since) < self.interval {
            return None;
        }
        self.quiet_since = now;
        let elapsed = Elapsed(now.saturating_duration_since(self.started));
        Some(match (self.total, &self.running) {
            (None, _) => format!("campaign preparation running {elapsed}"),
            (Some(total), Some(metric)) => {
                format!(
                    "{}/{total} measured; {metric} running {elapsed}",
                    self.completed
                )
            }
            (Some(total), None) => {
                format!("{}/{total} measured; auditing {elapsed}", self.completed)
            }
        })
    }
}

/// Live state of one kill campaign, advanced only by observations and the
/// instants the caller supplies.
pub(super) struct Campaign {
    interval: Duration,
    started: Instant,
    /// When the last line was written; a heartbeat waits a full interval.
    quiet_since: Instant,
    total: Option<usize>,
    completed: usize,
    running: Option<String>,
    findings: usize,
}

impl Campaign {
    pub(super) const fn new(interval: Duration, now: Instant) -> Self {
        Self {
            interval,
            started: now,
            quiet_since: now,
            total: None,
            completed: 0,
            running: None,
            findings: 0,
        }
    }

    /// The line one observation earns, if any: the campaign identity once,
    /// and the first few survivors and unconfirmed mutants by name.
    pub(super) fn observe(
        &mut self,
        observation: KillProgress<'_>,
        now: Instant,
    ) -> Option<String> {
        self.total = Some(observation.total);
        self.completed = observation.completed;
        let line = match (observation.mutant, observation.disposition) {
            (None, _) => Some(format!(
                "campaign seed {}: {} mutants x {} attempts",
                observation.seed, observation.total, observation.attempts
            )),
            (Some(mutant), None) => {
                self.running = Some(clip(mutant, NAME_BYTES).to_string());
                None
            }
            (Some(_), Some(MutantDisposition::ConfirmedKilled)) => {
                self.running = None;
                None
            }
            (Some(mutant), Some(disposition)) => {
                self.running = None;
                self.findings += 1;
                let word = match disposition {
                    MutantDisposition::ConfirmedSurvived => "survived",
                    _ => "unconfirmed",
                };
                match self.findings {
                    shown if shown <= LIVE_FINDINGS => Some(format!(
                        "{word} {}/{} {}",
                        self.completed,
                        observation.total,
                        clip(mutant, NAME_BYTES)
                    )),
                    first_withheld if first_withheld == LIVE_FINDINGS + 1 => Some(
                        "further survivors and unconfirmed mutants are counted in the summary"
                            .to_owned(),
                    ),
                    _ => None,
                }
            }
        };
        if line.is_some() {
            self.quiet_since = now;
        }
        line
    }

    /// A heartbeat once a full interval passed without a line.
    pub(super) fn heartbeat(&mut self, now: Instant) -> Option<String> {
        if now.saturating_duration_since(self.quiet_since) < self.interval {
            return None;
        }
        self.quiet_since = now;
        let elapsed = Elapsed(now.saturating_duration_since(self.started));
        Some(match (self.total, &self.running) {
            (None, _) => format!("baseline and mutant inventory running {elapsed}"),
            (Some(total), Some(mutant)) => {
                format!(
                    "{}/{total} done; {mutant} running {elapsed}",
                    self.completed
                )
            }
            (Some(total), None) => format!("{}/{total} done; running {elapsed}", self.completed),
        })
    }
}

fn rate(rate: ReportRate) -> String {
    format!("{}/{}", rate.numerator(), rate.denominator())
}

const fn attempt(kind: ReportAttemptKind) -> &'static str {
    match kind {
        ReportAttemptKind::Killed => "killed",
        ReportAttemptKind::Survived => "survived",
        ReportAttemptKind::TimedOut => "timed-out",
        ReportAttemptKind::LaunchError => "launch-error",
        ReportAttemptKind::WaitError => "wait-error",
        ReportAttemptKind::Undecodable => "undecodable",
    }
}

fn retained_line(summary: &mut Summary, retained: &Result<Export, String>) {
    match retained {
        Ok(export) => summary.closing(
            "report",
            &format!("{} ({} bytes)", export.path, export.bytes),
        ),
        Err(error) => summary.closing("report", &format!("not retained: {error}")),
    }
}

/// The decision-bearing facts of one kill report.
pub(super) struct KillOutcome<'a> {
    pub(super) seed: u64,
    pub(super) attempts: u64,
    pub(super) movement: ReportMutationMovement,
    pub(super) rate: Option<ReportRate>,
    pub(super) proposed: Option<ReportRate>,
    pub(super) mutants: Vec<MutantLine<'a>>,
}

/// One mutant as the summary names it.
pub(super) struct MutantLine<'a> {
    pub(super) identity: &'a str,
    pub(super) disposition: ReportMutantDisposition,
    pub(super) attempts: Vec<ReportAttemptKind>,
}

impl<'a> KillOutcome<'a> {
    fn of(report: &'a KillReport) -> Self {
        Self {
            seed: report.inventory().seed(),
            attempts: report.inventory().attempts(),
            movement: report.movement(),
            rate: report.rate(),
            proposed: report.proposed_floor(),
            mutants: report
                .mutants()
                .iter()
                .map(|mutant| MutantLine {
                    identity: mutant.identity(),
                    disposition: mutant.disposition(),
                    attempts: mutant
                        .attempts()
                        .iter()
                        .map(|attempt| attempt.kind())
                        .collect(),
                })
                .collect(),
        }
    }
}

/// The ordered decisions of one kill campaign.
pub(super) fn kill_summary(
    outcome: &KillOutcome<'_>,
    elapsed: Duration,
    retained: &Result<Export, String>,
    verbose: bool,
) -> String {
    let measured = outcome.rate.map_or_else(|| "undecided".to_owned(), rate);
    let (verdict, subject, next) = match outcome.movement {
        ReportMutationMovement::Held { floor } => (
            "held",
            format!("survivor rate {measured} holds floor {}", rate(floor)),
            "git commit",
        ),
        ReportMutationMovement::Shrank { floor, .. } => (
            "shrank",
            format!("survivor rate {measured} is under floor {}", rate(floor)),
            "git commit",
        ),
        ReportMutationMovement::Regressed { floor, .. } => (
            "regress",
            format!("survivor rate {measured} exceeds floor {}", rate(floor)),
            "kill each listed survivor with a test, git add, then spice gate kill",
        ),
        ReportMutationMovement::Unbaselined { .. } => (
            "new",
            format!("survivor rate {measured} has no tracked floor"),
            "git commit",
        ),
        ReportMutationMovement::Unconfirmed { confirmed, total } => (
            "unsure",
            format!("{confirmed} of {total} mutants confirmed; no rate is decided"),
            "make the listed attempts repeatable, git add, then spice gate kill",
        ),
    };
    let mut summary = Summary::new(format_args!(
        "gate kill: {verdict} (seed {}, {} mutants x {} attempts)",
        outcome.seed,
        outcome.mutants.len(),
        outcome.attempts
    ));
    summary.row(verdict, &subject, Some(elapsed));
    let of = |disposition| {
        outcome
            .mutants
            .iter()
            .filter(move |mutant| mutant.disposition == disposition)
    };
    let killed = of(ReportMutantDisposition::ConfirmedKilled).count();
    summary.row(
        "killed",
        &format!("{killed} mutants confirmed killed"),
        None,
    );
    let limit = if verbose {
        VERBOSE_MUTANTS
    } else {
        SHOWN_MUTANTS
    };
    let survivors = of(ReportMutantDisposition::ConfirmedSurvived).collect::<Vec<_>>();
    summary.row(
        "survived",
        &format!("{} confirmed survivors", survivors.len()),
        None,
    );
    list(&mut summary, &survivors, limit, verbose);
    let unconfirmed = of(ReportMutantDisposition::Unconfirmed).collect::<Vec<_>>();
    summary.row(
        "unsure",
        &format!("{} unconfirmed mutants", unconfirmed.len()),
        None,
    );
    list(&mut summary, &unconfirmed, limit, true);
    retained_line(&mut summary, retained);
    if let Some(proposed) = outcome.proposed {
        summary.closing(
            "proposed floor",
            &format!(
                "{} for rust.mutation-survivors in baseline.toml",
                rate(proposed)
            ),
        );
    }
    summary.closing("next", next);
    summary.finish()
}

/// Name at most `limit` mutants, each with its attempts when `attempts`.
fn list(summary: &mut Summary, mutants: &[&MutantLine<'_>], limit: usize, attempts: bool) {
    for mutant in mutants.iter().take(limit) {
        let name = clip(mutant.identity, NAME_BYTES);
        if attempts {
            let kinds = mutant
                .attempts
                .iter()
                .map(|kind| attempt(*kind))
                .collect::<Vec<_>>()
                .join(",");
            summary.detail(format_args!("{name}  attempts {kinds}"));
        } else {
            summary.detail(format_args!("{name}"));
        }
    }
    if let Some(withheld) = mutants.len().checked_sub(limit).filter(|count| *count > 0) {
        let more = if limit < VERBOSE_MUTANTS {
            "spice gate kill --verbose"
        } else {
            "the retained report"
        };
        summary.detail(format_args!(
            "{withheld} of {} withheld; see {more}",
            mutants.len()
        ));
    }
}

/// The ordered decisions of one camping audit.
pub(super) fn camp_summary(
    report: &CampReport,
    elapsed: Duration,
    retained: &Result<Export, String>,
    verbose: bool,
) -> String {
    let camped = report
        .distributions
        .iter()
        .filter(|distribution| distribution.standing == CampStanding::Camped)
        .count();
    let mut summary = Summary::new(format_args!(
        "gate camp: {} ({} distributions, {camped} camped)",
        report.standing,
        report.distributions.len()
    ));
    let limit = if verbose { VERBOSE_LOCI } else { SHOWN_LOCI };
    let mut shown = 0;
    let mut withheld = 0;
    for finding in &report.findings {
        let near = finding.loci.len();
        summary.row(
            "camped",
            &format!(
                "{}: {near} of {} loci near limit {}",
                clip(&finding.bound, NAME_BYTES),
                finding.population,
                finding.limit
            ),
            (shown == 0 && withheld == 0).then_some(elapsed),
        );
        for locus in &finding.loci {
            if shown == limit {
                withheld += 1;
                continue;
            }
            shown += 1;
            summary.detail(format_args!(
                "{} measured {} {} distance {}/{}",
                clip(&locus.locus, NAME_BYTES),
                locus.measured,
                locus.relation,
                locus.delta,
                locus.denominator
            ));
        }
    }
    if withheld > 0 {
        let more = if verbose {
            "the retained report"
        } else {
            "spice gate camp --verbose"
        };
        summary.detail(format_args!(
            "{withheld} of {} near loci withheld; see {more}",
            shown + withheld
        ));
    }
    let clean = report.distributions.len() - camped;
    summary.row(
        "clean",
        &format!("{clean} distributions clear of their limits"),
        report.findings.is_empty().then_some(elapsed),
    );
    retained_line(&mut summary, retained);
    summary.closing(
        "next",
        if camped == 0 {
            "git commit"
        } else {
            "spread the listed loci away from their shared limit, git add, then spice gate camp"
        },
    );
    summary.finish()
}

#[cfg(test)]
mod tests;
