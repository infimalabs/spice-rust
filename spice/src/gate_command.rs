//! The journal-free binary rim for the four executable gate leaves.
//!
//! Selection always precedes contract loading. Consequently tracked policy,
//! tracked floors, source bytes, and the root handed to every bounded process
//! all come from one captured Git source rather than from a moving checkout.

use std::path::Path;

use gate::{
    CampReport, GateReport, KillReport, ReportMutationMovement, Selector, Standing, Verdict,
};
use wire::{Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition};

use spice::repository::Repository;

/// Run the ordinary gate against the captured index or one selected commit.
pub(crate) fn run(matches: &clap::ArgMatches) -> Outcome<GateReport> {
    let selector = matches
        .get_one::<String>("at")
        .map_or_else(Selector::staged, |revision| {
            Selector::revision(revision.clone())
        });
    let selected = match SelectedContract::capture(selector) {
        Ok(selected) => selected,
        Err(failure) => return failure.outcome(),
    };
    let current = match selected.contract.adjudicate(selected.root()) {
        Ok(current) => current,
        Err(error) => return Failure::run("gate::run-refused", error).outcome(),
    };
    let report = current.report();
    if report.standing() == Standing::Refused {
        return Outcome::Refusal(Refusal {
            reason: "gate::regressed".to_owned(),
            witness: Some(report.verdict().digest().to_string()),
            expected: Some("every current gate finding to hold or shrink".to_owned()),
            observed: Some(report.render()),
            remedy: "remove the regression; never lower baseline.toml to admit it".to_owned(),
        });
    }
    direct(report)
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
    let Some((reason, expected, remedy)) = refusal else {
        return direct(report);
    };
    Outcome::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: None,
        expected: Some(expected.to_owned()),
        observed: Some(report.render()),
        remedy: remedy.to_owned(),
    })
}

/// Freshly adjudicate the captured index and return its complete receipt.
pub(crate) fn receipt() -> Outcome<Verdict> {
    let selected = match SelectedContract::capture(Selector::staged()) {
        Ok(selected) => selected,
        Err(failure) => return failure.outcome(),
    };
    match selected.contract.adjudicate(selected.root()) {
        Ok(current) => direct(current.receipt().clone()),
        Err(error) => Failure::run("gate::receipt-refused", error).outcome(),
    }
}

fn direct<B>(body: B) -> Outcome<B> {
    Outcome::Reply(Reply {
        position: ReplyPosition::Direct {},
        body,
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
                remedy: "resolve the named Git source before running the gate".to_owned(),
            })?;
        let contract = gate::Contract::load(tree.root()).map_err(|error| Failure::Refusal {
            reason: "gate::contract-refused",
            expected: "one complete current tracked gate contract".to_owned(),
            observed: error.to_string(),
            remedy: "repair the selected spice.toml, baseline.toml, or compiled tool".to_owned(),
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
        remedy: String,
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
            remedy: "restore the declared inputs and tools, then run the gate again".to_owned(),
        }
    }

    fn outcome<B>(self) -> Outcome<B> {
        match self {
            Self::Refusal {
                reason,
                expected,
                observed,
                remedy,
            } => Outcome::Refusal(Refusal {
                reason: reason.to_owned(),
                witness: None,
                expected: Some(expected),
                observed: Some(observed),
                remedy,
            }),
            Self::Fault { reason, detail } => Outcome::Fault(Fault {
                class: FaultClass::Environment,
                reason: reason.to_owned(),
                detail,
            }),
        }
    }
}
