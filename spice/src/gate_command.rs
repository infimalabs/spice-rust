//! The journal-free binary rim for the four executable gate leaves.
//!
//! Selection always precedes contract loading. Consequently tracked policy,
//! tracked floors, source bytes, and the root handed to every bounded process
//! all come from one captured Git source rather than from a moving checkout.

use std::env;
use std::io;
use std::path::Path;

use gate::{BatteryReport, CampReport, GateId, KillReport, ReportMutationMovement, Selector};
#[cfg(test)]
use gate::{GateReport, Standing};
use wire::intent::{CommandAnswer, GateFindings};
use wire::{Digest, Domain, Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition};

use crate::cli::Contract;
use spice::repository::Repository;

mod commit_message;
mod pre_commit;
mod reference_transaction;

/// Run the ordinary gate against the captured index or one selected commit.
pub(crate) fn run(matches: &clap::ArgMatches) -> Outcome<BatteryReport> {
    let selector = matches
        .get_one::<String>("at")
        .map_or_else(Selector::staged, |revision| {
            Selector::revision(revision.clone())
        });
    match spice::gate::run(selector) {
        Ok(report) => direct(report),
        Err(error) => battery_failure(error),
    }
}

/// Run the ordinary gate and file its projected findings as deferred board work.
pub(crate) fn findings(matches: &clap::ArgMatches, contract: &Contract) -> Outcome<CommandAnswer> {
    let instrument = matches
        .get_one::<String>("INSTRUMENT")
        .expect("required instrument is parsed");
    let Some(gate_id) = GateId::BATTERY
        .into_iter()
        .find(|candidate| candidate.as_str() == instrument)
    else {
        return Outcome::Refusal(Refusal::invocation(
            "gate::instrument-unregistered",
            format!(
                "spice gate findings INSTRUMENT requires one registered instrument; `{instrument}` is not registered"
            ),
        ));
    };
    let selector = matches
        .get_one::<String>("at")
        .map_or_else(Selector::staged, |revision| {
            Selector::revision(revision.clone())
        });
    let repository = match focused_repository(gate_id) {
        Ok(repository) => repository,
        Err(outcome) => return outcome,
    };
    let before = match spice::gate::capture_gate_finding_anchor(&repository, &selector) {
        Ok(source) => source,
        Err(error) => return focused_failure(error, gate_id),
    };
    let report = match spice::gate::run_arm_report(selector.clone(), gate_id) {
        Ok(report) => report,
        Err(error) => return focused_failure(error, gate_id),
    };
    let after = match spice::gate::capture_gate_finding_anchor(&repository, &selector) {
        Ok(source) => source,
        Err(error) => return focused_failure(error, gate_id),
    };
    if before != after {
        return Outcome::Refusal(Refusal::evidence(
            "gate::finding-source-moved",
            format!("before={before:?}; after={after:?}"),
            "rerun the focused instrument after the Git source stops moving",
        ));
    }
    let source = before.bind(report.context().source().clone());
    let (report_digest, evidence) = match focused_report_evidence(&report) {
        Ok(evidence) => evidence,
        Err(outcome) => return outcome,
    };
    crate::command_client::execute_typed(
        &repository,
        contract,
        wire::CommandKind::GateFindings,
        Vec::new(),
        &GateFindings {
            report: report_digest,
            evidence,
            source,
        },
    )
}

#[cfg(test)]
fn run_report(result: Result<GateReport, gate::RunError>) -> Outcome<GateReport> {
    let report = match result {
        Ok(report) => report,
        Err(error) => return Failure::run("gate::run-refused", error).outcome(),
    };
    if report.standing() == Standing::Refused {
        return Outcome::Refusal(Refusal::evidence(
            "gate::regressed",
            report.render(),
            "remove the regression; never lower baseline.toml to admit it",
        ));
    }
    direct(report)
}

/// Run the release-scoped native staged-change gate without command recursion.
pub(crate) fn pre_commit() -> Outcome<pre_commit::Report> {
    pre_commit::run()
}

/// Run selected evidence for an allocator-authenticated merge resolution.
pub(crate) fn resolution_evidence(repository: &Repository) -> Outcome<()> {
    pre_commit::run_resolution(repository)
}

/// Run the current source repository's strict ordinary change selection.
///
/// This leaf is the policy-free destination for source-development delegates:
/// Cargo may build this product command, but no xtask command or hook owns a
/// second selection implementation.
#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case", tag = "evidence")]
pub(crate) enum CurrentSourceSelectionReport {
    /// Full validated evidence for a direct operator invocation.
    Focused {
        report: spice::change_gate::ChangeGateReport,
    },
    /// Compact success identity for the bounded configured-command supervisor.
    ///
    /// The source gate has already constructed and validated the full report;
    /// its supervisor consumes only the process disposition and exact source.
    Supervised { source: Digest },
}

pub(crate) fn selection() -> Outcome<CurrentSourceSelectionReport> {
    let supervised = configured_selection_progress();
    let mut progress = SelectionProgress::new(supervised);
    match spice::change_gate::run_current_source(&mut progress) {
        Ok(report) if report.admits() && supervised => {
            direct(CurrentSourceSelectionReport::Supervised {
                source: report.selection().source().clone(),
            })
        }
        Ok(report) if report.admits() => direct(CurrentSourceSelectionReport::Focused { report }),
        Ok(report) => {
            progress.extend_from_slice(report.refusal_fold().as_bytes());
            Outcome::Refusal(Refusal::evidence(
                "gate::selection-regressed",
                selection_detail(progress.buffered(), "selected gate regressed"),
                "repair the selected change before committing it",
            ))
        }
        Err(error) if error.is_fault() => Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: error.reason().to_owned(),
            detail: selection_detail(progress.buffered(), error.detail()),
        }),
        Err(error) => Outcome::Refusal(Refusal::evidence(
            error.reason(),
            selection_detail(progress.buffered(), error.detail()),
            "repair the named selection input, then rerun the source gate",
        )),
    }
}

struct SelectionProgress {
    buffered: Vec<u8>,
    live: Option<io::Stderr>,
}

impl SelectionProgress {
    fn new(live: bool) -> Self {
        Self {
            buffered: Vec::new(),
            live: live.then(io::stderr),
        }
    }

    fn buffered(&self) -> &[u8] {
        &self.buffered
    }

    fn extend_from_slice(&mut self, bytes: &[u8]) {
        self.buffered.extend_from_slice(bytes);
    }
}

impl io::Write for SelectionProgress {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(live) = &mut self.live {
            live.write_all(bytes)?;
        }
        self.buffered.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.live.as_mut().map_or(Ok(()), io::Write::flush)
    }
}

#[expect(
    clippy::disallowed_methods,
    reason = "the configured source-gate leaf detects authority injected by its supervising worker"
)]
fn configured_selection_progress() -> bool {
    env::var_os(wire::SpiceVariable::PreCommitSource.as_str()).is_some()
}

fn selection_detail(progress: &[u8], detail: &str) -> String {
    if progress.is_empty() {
        return detail.to_owned();
    }
    format!("{}{detail}", String::from_utf8_lossy(progress))
}

pub(crate) use commit_message::Run as CommitMessageRun;

/// Apply the current repository commit-message policy to Git's message file.
pub(crate) fn commit_message(matches: &clap::ArgMatches) -> CommitMessageRun {
    commit_message::run(matches)
}

pub(crate) fn pre_commit_worker_entry(
    arguments: impl Iterator<Item = std::ffi::OsString>,
) -> Option<i32> {
    let arguments = arguments.collect::<Vec<_>>();
    let has_json = arguments
        .iter()
        .skip(1)
        .any(|argument| argument.as_os_str() == std::ffi::OsStr::new("--json"));
    if !has_json && let Some(code) = reference_transaction::entry(arguments.iter().cloned()) {
        return Some(code);
    }
    let reference_arguments = parser_admitted_json_reference_arguments(&arguments);
    reference_arguments
        .and_then(|arguments| reference_transaction::entry(arguments.into_iter()))
        .or_else(|| pre_commit::worker_entry(arguments.into_iter()))
}

fn parser_admitted_json_reference_arguments(
    arguments: &[std::ffi::OsString],
) -> Option<Vec<std::ffi::OsString>> {
    let (program, operands) = arguments.split_first()?;
    let normalized = std::iter::once(program.clone())
        .chain(
            operands
                .iter()
                .filter(|argument| argument.as_os_str() != std::ffi::OsStr::new("--json"))
                .cloned(),
        )
        .collect::<Vec<_>>();
    if normalized.get(1).map(std::ffi::OsString::as_os_str) != Some(std::ffi::OsStr::new("gate"))
        || normalized.get(2).map(std::ffi::OsString::as_os_str)
            != Some(std::ffi::OsStr::new("reference-transaction"))
    {
        return None;
    }
    crate::cli::command()
        .try_get_matches_from(arguments.iter().cloned())
        .ok()?;
    Some(normalized)
}

/// Audit the compiled shared bound without changing a tracked floor.
pub(crate) fn camp() -> Outcome<CampReport> {
    let selected = match SelectedContract::capture(Selector::staged()) {
        Ok(selected) => selected,
        Err(failure) => return failure.outcome(),
    };
    match selected.contract.audit_camp(selected.root()) {
        Ok(report) => direct(report),
        Err(error) => Failure::run("gate::camp-refused", error).outcome(),
    }
}

/// Run the compiled mutation profile against private copies of the index.
pub(crate) fn kill() -> Outcome<KillReport> {
    let selected = match SelectedContract::capture(Selector::staged()) {
        Ok(selected) => selected,
        Err(failure) => return failure.outcome(),
    };
    match selected.contract.execute_kill(selected.root()) {
        Ok(run) => kill_report(run.report()),
        Err(error) => Failure::run("gate::kill-refused", error).outcome(),
    }
}

fn kill_report(report: KillReport) -> Outcome<KillReport> {
    let refusal = match report.movement() {
        ReportMutationMovement::Regressed { .. } => Some((
            "gate::kill-regressed",
            "confirmed survivor debt to hold or shrink",
            "remove every confirmed survivor; never raise the survivor floor",
        )),
        ReportMutationMovement::Unconfirmed { .. } => Some((
            "gate::kill-unconfirmed",
            "every declared attempt to confirm each mutant",
            "make the flaky, timed-out, or incomplete attempts repeatable, then run the gate again",
        )),
        ReportMutationMovement::Held { .. }
        | ReportMutationMovement::Shrank { .. }
        | ReportMutationMovement::Unbaselined { .. } => None,
    };
    let Some((reason, _expected, summary)) = refusal else {
        return direct(report);
    };
    Outcome::Refusal(Refusal::evidence(reason, report.render(), summary))
}

fn battery_failure<B>(error: spice::gate::RunError) -> Outcome<B> {
    if error.is_fault() {
        return Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: error.reason().to_owned(),
            detail: error.detail().to_owned(),
        });
    }
    Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(
            error.reason().to_owned(),
            None,
            Some("one complete canonical Rust battery capture".to_owned()),
            Some(error.detail().to_owned()),
        )
        .with_detail("repair the named capture input; the Rust battery is the sole execution path"),
    ))
}

fn focused_failure<B>(error: spice::gate::RunError, gate_id: GateId) -> Outcome<B> {
    focused_unavailable(error.reason(), error.detail(), gate_id)
}

fn focused_unavailable<B>(reason: &str, detail: &str, gate_id: GateId) -> Outcome<B> {
    Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(
            "gate::instrument-cannot-run".to_owned(),
            None,
            Some(format!(
                "registered gate instrument {gate_id} to complete one focused run"
            )),
            Some(format!("{reason}: {detail}")),
        )
        .with_detail("repair the named capture input, then rerun this registered instrument"),
    ))
}

fn focused_repository(gate_id: GateId) -> Result<Repository, Outcome<CommandAnswer>> {
    let current = std::env::current_dir().map_err(|error| {
        focused_unavailable(
            "gate::working-directory-unavailable",
            &error.to_string(),
            gate_id,
        )
    })?;
    Repository::discover(&current).map_err(|error| {
        focused_unavailable("gate::repository-unresolved", &error.to_string(), gate_id)
    })
}

fn direct<B>(body: B) -> Outcome<B> {
    Outcome::Reply(Reply {
        position: ReplyPosition::Direct {},
        body,
    })
}

pub(crate) fn focused_report_evidence(
    report: &gate::FocusedArmReport,
) -> Result<(Digest, String), Outcome<CommandAnswer>> {
    let bytes = report.encode().map_err(|error| {
        Outcome::Fault(Fault {
            class: FaultClass::Protocol,
            reason: "gate::report-unencodable".to_owned(),
            detail: error.to_string(),
        })
    })?;
    let digest = Digest::compute(Domain::GateFocusedReport, &bytes);
    let evidence = String::from_utf8(bytes).map_err(|error| {
        Outcome::Fault(Fault {
            class: FaultClass::Protocol,
            reason: "gate::report-non-utf8".to_owned(),
            detail: error.to_string(),
        })
    })?;
    Ok((digest, evidence))
}

/// One immutable selected tree and the current contract loaded from its bytes.
struct SelectedContract {
    tree: gate::SelectedTree,
    contract: gate::Contract,
}

impl SelectedContract {
    fn capture(selector: Selector) -> Result<Self, Failure> {
        let current = std::env::current_dir().map_err(|error| Failure::Fault {
            reason: "gate::working-directory-unavailable",
            detail: error.to_string(),
        })?;
        let repository = Repository::discover(&current).map_err(|error| Failure::Fault {
            reason: "gate::repository-unresolved",
            detail: error.to_string(),
        })?;
        let tree = selector
            .materialize(repository.worktree_root())
            .map_err(|error| Failure::Refusal {
                reason: "gate::selection-refused",
                expected: "one stable captured index or one immutable commit".to_owned(),
                observed: error.to_string(),
                detail: "resolve the named Git source before running the gate".to_owned(),
            })?;
        let contract = gate::Contract::load(tree.root()).map_err(|error| Failure::Refusal {
            reason: "gate::contract-refused",
            expected: "one complete current tracked gate contract".to_owned(),
            observed: error.to_string(),
            detail: "repair the selected spice-rust.toml, baseline.toml, or compiled tool"
                .to_owned(),
        })?;
        Ok(Self { tree, contract })
    }

    fn root(&self) -> &Path {
        self.tree.root()
    }
}

enum Failure {
    Refusal {
        reason: &'static str,
        expected: String,
        observed: String,
        detail: String,
    },
    Fault {
        reason: &'static str,
        detail: String,
    },
}

impl Failure {
    fn run(reason: &'static str, error: impl std::fmt::Display) -> Self {
        Self::Refusal {
            reason,
            expected: "one complete bounded current gate result".to_owned(),
            observed: error.to_string(),
            detail: "restore the declared inputs and tools, then run the gate again".to_owned(),
        }
    }

    fn outcome<B>(self) -> Outcome<B> {
        match self {
            Self::Refusal {
                reason,
                expected,
                observed,
                detail,
            } => Outcome::Refusal(Refusal::bare(
                wire::RefusalDiagnostic::new(
                    reason.to_owned(),
                    None,
                    Some(expected),
                    Some(observed),
                )
                .with_detail(detail),
            )),
            Self::Fault { reason, detail } => Outcome::Fault(Fault {
                class: FaultClass::Environment,
                reason: reason.to_owned(),
                detail,
            }),
        }
    }
}

#[cfg(all(test, unix))]
mod fixtures;
