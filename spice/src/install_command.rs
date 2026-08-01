use std::path::PathBuf;
use std::str::FromStr;

use clap::ArgMatches;
use serde::Serialize;
use spice::install::{self, InstallError, InstallPlan, InstallReceipt, InstallStatus};
use spice::repository::Repository;
use wire::{Digest, Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Action {
    Preview,
    Applied,
    Unapplied,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct InstallReport {
    action: Action,
    digest: Digest,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<InstallStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completed: Option<usize>,
    plan: InstallPlan,
}

pub(crate) fn run(matches: &ArgMatches) -> Outcome<InstallReport> {
    let repository = match repository() {
        Ok(repository) => repository,
        Err(outcome) => return map_outcome(outcome),
    };
    if matches.get_flag("unapply") {
        return match install::unapply(&repository) {
            Ok(receipt) => reply(receipt_report(Action::Unapplied, &receipt)),
            Err(error) => map_outcome(failure(&error)),
        };
    }

    let binary = match current_binary() {
        Ok(binary) => binary,
        Err(outcome) => return map_outcome(outcome),
    };
    let plan = match install::plan(&repository, Some(&binary)) {
        Ok(plan) => plan,
        Err(error) => return map_outcome(failure(&error)),
    };
    let digest = match plan.digest() {
        Ok(digest) => digest,
        Err(error) => return map_outcome(failure(&error)),
    };
    let Some(requested) = matches.get_one::<String>("apply") else {
        return reply(InstallReport {
            action: Action::Preview,
            digest,
            status: None,
            completed: None,
            plan,
        });
    };
    let requested = match Digest::from_str(requested) {
        Ok(requested) => requested,
        Err(error) => {
            return map_outcome(refusal(
                "install::digest-refused",
                Some(error.to_string()),
                "apply the canonical digest emitted by the current install preview",
            ));
        }
    };
    if requested != digest {
        return map_outcome(refusal(
            "install::plan-moved",
            Some(format!("requested {requested}; current {digest}")),
            "preview the current plan and apply its exact digest",
        ));
    }
    match install::apply(&repository, Some(&binary), &requested) {
        Ok(receipt) => reply(receipt_report(Action::Applied, &receipt)),
        Err(error) => map_outcome(failure(&error)),
    }
}

fn repository() -> Result<Repository, Outcome<()>> {
    let current = crate::working_directory()?;
    Repository::discover(current).map_err(|error| {
        fault(
            FaultClass::Environment,
            "install::repository-unresolved",
            error.to_string(),
        )
    })
}

fn current_binary() -> Result<PathBuf, Outcome<()>> {
    std::env::current_exe().map_err(|error| {
        fault(
            FaultClass::Environment,
            "install::binary-unresolved",
            error.to_string(),
        )
    })
}

fn receipt_report(action: Action, receipt: &InstallReceipt) -> InstallReport {
    InstallReport {
        action,
        digest: receipt.digest().clone(),
        status: Some(receipt.status()),
        completed: Some(receipt.completed()),
        plan: receipt.plan().clone(),
    }
}

fn reply(report: InstallReport) -> Outcome<InstallReport> {
    Outcome::Reply(Reply {
        position: ReplyPosition::Direct {},
        body: report,
    })
}

fn failure(error: &InstallError) -> Outcome<()> {
    let detail = error.to_string();
    match error {
        InstallError::DigestMismatch { .. } => refusal(
            "install::plan-moved",
            Some(detail),
            "preview the current plan and apply its exact digest",
        ),
        InstallError::RecordMissing => refusal(
            "install::record-absent",
            None,
            "apply an installation plan before unapplying it",
        ),
        InstallError::Drift { .. } => refusal(
            "install::drift",
            Some(detail),
            "restore the recorded installed bytes before retrying reversal",
        ),
        InstallError::ExclusionsNotUtf8
        | InstallError::DivergentExclusions
        | InstallError::BinaryContextChanged
        | InstallError::LegacyRecord(_)
        | InstallError::ApplyInProgress
        | InstallError::UnapplyInProgress => refusal(
            "install::state-refused",
            Some(detail),
            "repair or complete the named installation state before retrying",
        ),
        InstallError::Repository(_)
        | InstallError::BinaryNotRegular(_)
        | InstallError::BinaryEmpty(_) => fault(
            FaultClass::Environment,
            "install::environment-unavailable",
            detail,
        ),
        InstallError::Io { .. }
        | InstallError::Atomic(_)
        | InstallError::Decode(_)
        | InstallError::InvalidRecord(_)
        | InstallError::PathShape(_) => fault(FaultClass::Store, "install::store-invalid", detail),
        InstallError::Encode(_) | InstallError::Interrupted { .. } => {
            fault(FaultClass::Protocol, "install::protocol-invalid", detail)
        }
    }
}

fn refusal(reason: &str, observed: Option<String>, remedy: &str) -> Outcome<()> {
    Outcome::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: None,
        expected: None,
        observed,
        remedy: remedy.to_owned(),
    })
}

fn fault(class: FaultClass, reason: &str, detail: String) -> Outcome<()> {
    Outcome::Fault(Fault {
        class,
        reason: reason.to_owned(),
        detail,
    })
}

fn map_outcome<B>(outcome: Outcome<()>) -> Outcome<B> {
    match outcome {
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
        Outcome::Reply(_) => unreachable!("error paths cannot carry replies"),
    }
}
