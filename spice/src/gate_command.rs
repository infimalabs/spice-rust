//! The journal-free binary rim for the four executable gate leaves.
//!
//! Selection always precedes contract loading. Consequently tracked policy,
//! tracked floors, source bytes, and the root handed to every bounded process
//! all come from one captured Git source rather than from a moving checkout.

use std::path::Path;

use gate::{
    BatteryReceipt, BatteryReport, CampReport, GateId, KillReport, ReportMutationMovement, Selector,
};
#[cfg(test)]
use gate::{GateReport, Standing};
use wire::intent::{CommandAnswer, GateFindings};
use wire::{Digest, Domain, Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition};

use crate::cli::Contract;
use spice::repository::Repository;

mod pre_commit;

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
    let report = match spice::gate::run_arm_report(selector, gate_id) {
        Ok(report) => report,
        Err(error) => return focused_failure(error, gate_id),
    };
    let (report_digest, evidence) = match focused_report_evidence(&report) {
        Ok(evidence) => evidence,
        Err(outcome) => return outcome,
    };
    let repository = match current_repository() {
        Ok(repository) => repository,
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

/// Run the release-scoped native pre-commit battery without command recursion.
pub(crate) fn pre_commit() -> Outcome<BatteryReport> {
    pre_commit::run()
}

pub(crate) fn pre_commit_worker_entry(
    arguments: impl Iterator<Item = std::ffi::OsString>,
) -> Option<i32> {
    pre_commit::worker_entry(arguments)
}

fn pre_commit_report(
    result: Result<BatteryReport, spice::gate::RunError>,
) -> Outcome<BatteryReport> {
    match result {
        Ok(report) if report.admits() => direct(report),
        Ok(report) => {
            let evidence = report.refusal_fold().to_string();
            Outcome::Refusal(Refusal::evidence(
                "gate::battery-regressed",
                evidence,
                "repair the typed failing results; never launch a predecessor gate. A merge is recorded by plain git commit like any other commit: the gate measures each file against the parent that carries it, so what refuses here is what this merge itself wrote",
            ))
        }
        Err(error) => battery_failure(error),
    }
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

/// Freshly adjudicate the captured index and return its complete receipt.
pub(crate) fn receipt() -> Outcome<BatteryReceipt> {
    match spice::gate::receipt(Selector::staged()) {
        Ok(receipt) => direct(receipt),
        Err(error) => battery_failure(error),
    }
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
    Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(
            "gate::instrument-cannot-run".to_owned(),
            None,
            Some(format!(
                "registered gate instrument {gate_id} to complete one focused run"
            )),
            Some(format!("{}: {}", error.reason(), error.detail())),
        )
        .with_detail("repair the named capture input, then rerun this registered instrument"),
    ))
}

fn direct<B>(body: B) -> Outcome<B> {
    Outcome::Reply(Reply {
        position: ReplyPosition::Direct {},
        body,
    })
}

fn focused_report_evidence(
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

fn current_repository() -> Result<Repository, Outcome<CommandAnswer>> {
    let current = std::env::current_dir().map_err(|error| {
        Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: "gate::working-directory-unavailable".to_owned(),
            detail: error.to_string(),
        })
    })?;
    Repository::discover(&current).map_err(|error| {
        Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: "gate::repository-unresolved".to_owned(),
            detail: error.to_string(),
        })
    })
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
