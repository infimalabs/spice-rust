//! Evidence admission for one exact staged candidate.

use gate::{AdmissionBundle, AdmissionBundleFields, EvidenceStore};
use gix::bstr::ByteSlice as _;
use serde::{Deserialize, Serialize};
use spice::repository::Repository;
use wire::{Digest, Domain};

use super::WorkerFailure;

const ADMISSION_LIFETIME_SECONDS: i64 = 600;
const ADMISSION_REUSE_WINDOW_SECONDS: i64 = 300;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AdmissionReport {
    source: Digest,
    candidate_tree: String,
    admission: gate::AdmissionId,
    actions: usize,
    live_checks: usize,
}

impl AdmissionReport {
    pub(super) const fn source(&self) -> &Digest {
        &self.source
    }
}

pub(super) struct AdmissionRun {
    report: AdmissionReport,
    staged: spice::change_gate::StagedActionSelection,
}

impl AdmissionRun {
    pub(super) fn into_parts(self) -> (AdmissionReport, spice::change_gate::StagedActionSelection) {
        (self.report, self.staged)
    }
}

pub(super) fn run(
    claim: gate::ClaimCapture,
    staged: spice::change_gate::StagedActionSelection,
) -> Result<AdmissionRun, WorkerFailure> {
    let current = std::env::current_dir().map_err(|error| {
        failure(
            "gate::pre-commit-working-directory-unavailable",
            error,
            true,
        )
    })?;
    let repository = Repository::discover(&current)
        .map_err(|error| failure("gate::pre-commit-repository-unresolved", error, true))?;
    let store =
        EvidenceStore::open(repository.common_dir()).map_err(|error| evidence_failure(&error))?;
    let claim_digest = Digest::compute(
        Domain::GateRepositoryAuthority,
        &wire::encode(&claim)
            .map_err(|error| failure("gate::pre-commit-claim-capture-invalid", error, true))?,
    );
    let preparation =
        super::super::prove::prepare_admission_from_staged(&repository, &store, staged)
            .map_err(prove_failure)?;
    let branch = match branch(&repository) {
        Ok(branch) => branch,
        Err(error) => {
            let _ = preparation.reject_state();
            return Err(error);
        }
    };
    let live_checks = match preparation.execute_live_checks() {
        Ok(checks) => checks,
        Err(error) => {
            let _ = preparation.reject_state();
            return Err(prove_failure(error));
        }
    };
    if let Err(error) = preparation.verify_source() {
        let _ = preparation.reject_state();
        return Err(prove_failure(error));
    }
    let resolved = match store.lookup_batch_exact(preparation.actions()) {
        Ok(resolved) => resolved,
        Err(error) => {
            let _ = preparation.reject_state();
            return Err(evidence_failure(&error));
        }
    };
    let missing = preparation
        .actions()
        .iter()
        .zip(&resolved)
        .filter_map(|(expected, observed)| observed.is_none().then_some(expected.action()))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        let _ = preparation.reject_state();
        return Err(WorkerFailure {
            reason: "gate::pre-commit-proof-missing".to_owned(),
            detail: missing_proof_detail(&missing),
            fault: false,
        });
    }
    let actions = resolved
        .into_iter()
        .map(|action| action.expect("missing receipts were handled above"))
        .collect::<Vec<_>>();
    if let Err(error) = preparation.accept_state() {
        return Err(prove_failure(error));
    }
    preparation.verify_source().map_err(prove_failure)?;
    let (created_at, expires_at) = admission_window()?;
    let live_check_count = live_checks.len();
    let bundle = AdmissionBundle::new(AdmissionBundleFields {
        candidate_tree: preparation.candidate_tree().to_string(),
        branch,
        head: preparation.head(),
        selection: preparation.selection().clone(),
        policy: preparation.policy().clone(),
        gate_set: preparation.gate_set().clone(),
        actions,
        live_checks,
        claim: claim_digest,
        created_at,
        expires_at,
    })
    .map_err(|error| evidence_failure(&error))?;
    let admission = store
        .publish_admission(&bundle)
        .map_err(|error| evidence_failure(&error))?;
    let report = AdmissionReport {
        source: preparation.source().clone(),
        candidate_tree: preparation.candidate_tree().to_string(),
        admission,
        actions: preparation.actions().len(),
        live_checks: live_check_count,
    };
    Ok(AdmissionRun {
        report,
        staged: preparation.into_staged(),
    })
}

fn branch(repository: &Repository) -> Result<String, WorkerFailure> {
    let repository = gix::open(repository.worktree_root())
        .map_err(|error| failure("gate::pre-commit-branch-unavailable", error, false))?;
    let head = repository
        .head()
        .map_err(|error| failure("gate::pre-commit-branch-unavailable", error, false))?;
    let branch = head.referent_name().ok_or_else(|| WorkerFailure {
        reason: "gate::pre-commit-branch-unavailable".to_owned(),
        detail: "HEAD is detached and names no local branch".to_owned(),
        fault: false,
    })?;
    let branch = branch
        .as_bstr()
        .to_str()
        .map_err(|error| failure("gate::pre-commit-branch-unavailable", error, false))?;
    if !branch.starts_with("refs/heads/")
        || gix::validate::reference::branch_name(branch.as_bytes().as_bstr()).is_err()
    {
        return Err(WorkerFailure {
            reason: "gate::pre-commit-branch-unavailable".to_owned(),
            detail: format!("HEAD returned invalid branch {branch:?}"),
            fault: false,
        });
    }
    Ok(branch.to_owned())
}

fn missing_proof_detail(actions: &[&str]) -> String {
    let mut detail = String::new();
    for action in actions {
        detail.push_str(&format!(
            "action={action} reason=missing-proof command=`spice gate prove {action}`\n"
        ));
    }
    detail.push_str("aggregate-command=`spice gate prove --missing`");
    detail
}

fn admission_window() -> Result<(wire::Instant, wire::Instant), WorkerFailure> {
    admission_window_at(jiff::Timestamp::now())
}

fn admission_window_at(
    observed: jiff::Timestamp,
) -> Result<(wire::Instant, wire::Instant), WorkerFailure> {
    let epoch_second = observed
        .as_second()
        .div_euclid(ADMISSION_REUSE_WINDOW_SECONDS)
        * ADMISSION_REUSE_WINDOW_SECONDS;
    let created = jiff::Timestamp::new(epoch_second, 0)
        .map_err(|error| failure("gate::pre-commit-clock-invalid", error, true))?;
    let expires = created
        .checked_add(jiff::SignedDuration::from_secs(ADMISSION_LIFETIME_SECONDS))
        .map_err(|error| failure("gate::pre-commit-clock-invalid", error, true))?;
    let created = wire::Instant::new(created)
        .map_err(|error| failure("gate::pre-commit-clock-invalid", error, true))?;
    let expires = wire::Instant::new(expires)
        .map_err(|error| failure("gate::pre-commit-clock-invalid", error, true))?;
    Ok((created, expires))
}

fn evidence_failure(error: &gate::EvidenceStoreError) -> WorkerFailure {
    let reason = if error.reason() == "gate-evidence::receipt-authority-mismatch" {
        "gate::pre-commit-proof-authority-mismatch"
    } else {
        "gate::pre-commit-evidence-corrupt"
    };
    WorkerFailure {
        reason: reason.to_owned(),
        detail: format!("reason={}; {}", error.reason(), error.detail()),
        fault: false,
    }
}

fn prove_failure(error: super::super::prove::ProveFailure) -> WorkerFailure {
    let reason = match error.reason() {
        "gate::source-moved" | "gate::prove-source-moved" => "gate::pre-commit-staged-source-moved",
        "gate::prove-action-not-reusable"
        | "gate::prove-cargo-action-not-reusable"
        | "gate::prove-canvas-build-not-reusable" => "gate::pre-commit-proof-closure-mismatch",
        reason => reason,
    };
    WorkerFailure {
        reason: reason.to_owned(),
        detail: error.detail().to_owned(),
        fault: error.is_fault(),
    }
}

fn failure(reason: &str, detail: impl std::fmt::Display, fault: bool) -> WorkerFailure {
    WorkerFailure {
        reason: reason.to_owned(),
        detail: detail.to_string(),
        fault,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_proofs_name_each_exact_command_and_the_aggregate() {
        let detail = missing_proof_detail(&["canvas:build", "formatters"]);

        assert!(detail.contains("command=`spice gate prove canvas:build`"));
        assert!(detail.contains("command=`spice gate prove formatters`"));
        assert!(detail.ends_with("aggregate-command=`spice gate prove --missing`"));
    }

    #[test]
    fn admission_retries_share_one_bounded_time_epoch() {
        let first = jiff::Timestamp::new(601, 1).expect("valid first timestamp");
        let second = jiff::Timestamp::new(899, 999_999_999).expect("valid second timestamp");

        let first = admission_window_at(first).expect("first admission window");
        let second = admission_window_at(second).expect("second admission window");

        assert_eq!(first, second);
        assert_eq!(first.0.get().as_second(), 600);
        assert_eq!(first.1.get().as_second(), 1_200);
    }
}
