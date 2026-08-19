//! Exact-tree task-completion admission over archived verifier evidence.

use std::process::Command;

use super::*;
use crate::gate::GIT;

/// Whether one immutable candidate belongs to the strict source-only verifier tier.
///
/// `baseline.toml` is the tracked floor consumed by the current-source gate and
/// has no role in the installed generic battery. History, rather than only the
/// candidate snapshot, is authoritative here: deleting the floor cannot turn a
/// strict repository into a generic one and bypass completion admission.
pub fn requires_full_verifier_completion_evidence(
    repository: &Repository,
    revision: &str,
) -> Result<bool, RunError> {
    let output = Command::new(GIT)
        .arg("-C")
        .arg(repository.worktree_root())
        .args([
            "log",
            "-1",
            "--format=%H",
            revision,
            "--",
            gate::BASELINE_PATH,
        ])
        .output()
        .map_err(|error| {
            RunError::fault(
                "gate::full-verifier-completion-contract-unavailable",
                format!("inspect tracked source contract: {error}"),
            )
        })?;
    if !output.status.success() {
        return Err(RunError::fault(
            "gate::full-verifier-completion-contract-unavailable",
            format!(
                "inspect tracked source contract at {revision} exited {:?}: {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim(),
            ),
        ));
    }
    let commit = std::str::from_utf8(&output.stdout)
        .map_err(|error| {
            RunError::fault(
                "gate::full-verifier-completion-contract-unavailable",
                format!("tracked source contract identity is not UTF-8: {error}"),
            )
        })?
        .trim();
    if commit.is_empty() {
        return Ok(false);
    }
    if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(RunError::fault(
            "gate::full-verifier-completion-contract-unavailable",
            format!("tracked source contract has a noncanonical commit identity: {commit:?}"),
        ));
    }
    Ok(true)
}

/// Re-admit one archived verifier receipt against an immutable completion commit.
///
/// A full verifier runs over a staged index, while task publication cites the
/// later commit carrying those exact bytes. Their selector scopes deliberately
/// differ, so ordinary exact-evidence admission must continue to refuse. This
/// boundary instead uses the existing staged-to-revision semantic authorities:
/// it revalidates the archived battery, state, compiler, tool, policy, bound,
/// latch, configuration, repository, and complete per-arm result inventory,
/// while requiring the captured revision source and semantic closure to match
/// the receipt exactly.
pub fn inspect_full_verifier_completion_evidence(
    repository: &Repository,
    revision: &str,
) -> Result<FullVerifierEvidenceAdmission, RunError> {
    let paths = repository
        .admit_state()
        .map_err(|error| RunError::fault("gate::full-verifier-state-unavailable", error))?;
    let mut prepared = match crate::gate::prepare_strict_at(
        repository.worktree_root(),
        Selector::revision(revision.to_owned()),
    ) {
        Ok(prepared) => prepared,
        Err(error) => return unavailable(FullVerifierEvidenceStage::Capture, error),
    };
    let verifier_environment = match exact_evidence_environment(repository) {
        Ok(environment) => environment,
        Err(error) => return unavailable(FullVerifierEvidenceStage::Environment, error),
    };
    prepared.bind_exact_evidence_environment(verifier_environment);
    let source = prepared.source().clone();
    let archive_path = paths
        .full_verifier_evidence_archive()
        .join(format!("{source}.json"));
    let archive = storage::LockedArchive::acquire(&paths)
        .map_err(|error| RunError::fault("gate::full-verifier-evidence-unavailable", error))?;
    let document = match archive.read(&archive_path) {
        Ok(Some(document)) => document,
        Ok(None) => {
            return unavailable(
                FullVerifierEvidenceStage::Document,
                format!("no archived full-verifier evidence names completion source {source}"),
            );
        }
        Err(error) => {
            return Err(RunError::fault(
                "gate::full-verifier-evidence-unavailable",
                error,
            ));
        }
    };
    let evidence = match MergeParentEvidence::decode(&document) {
        Ok(evidence) => evidence,
        Err(detail) => return unavailable(FullVerifierEvidenceStage::Bundle, detail),
    };
    let semantic = match semantic::MergeSemanticEvidence::decode(evidence.semantic.as_bytes()) {
        Ok(semantic) => semantic,
        Err(detail) => {
            return unavailable(FullVerifierEvidenceStage::SemanticDocument, detail);
        }
    };
    if semantic.source() != &source {
        return unavailable(
            FullVerifierEvidenceStage::Source,
            format!(
                "archived source {} does not match completion source {source}",
                semantic.source(),
            ),
        );
    }
    let battery = match BatteryReceipt::verify(evidence.battery.as_bytes()) {
        Ok(battery) => battery,
        Err(error) => return unavailable(FullVerifierEvidenceStage::Battery, error),
    };
    if battery.tree() != &source {
        return unavailable(
            FullVerifierEvidenceStage::Source,
            format!(
                "archived battery source {} does not match completion source {source}",
                battery.tree(),
            ),
        );
    }
    let repository_authority = semantic::repository_authority(repository);
    let current_context = match prepared.context() {
        Ok(context) => context,
        Err(error) => return unavailable(FullVerifierEvidenceStage::Closure, error),
    };
    let current_closure = match GateSemanticClosure::capture(
        source.clone(),
        prepared.root(),
        prepared.paths(),
        prepared.changes(),
        prepared.generated_paths(),
    ) {
        Ok(closure) => closure,
        Err(detail) => return unavailable(FullVerifierEvidenceStage::Closure, detail),
    };
    let current_compiler = match prepared
        .merge_semantic_compiler_authorities(evidence.cargo_jobs, &repository_authority)
    {
        Ok(compiler) => compiler.into_iter().collect::<BTreeMap<_, _>>(),
        Err(error) => return unavailable(FullVerifierEvidenceStage::Compiler, error),
    };
    if let Err(detail) = semantic.admit_merge_parent(
        repository,
        &mut prepared,
        &current_closure,
        &current_context,
        &current_compiler,
        evidence.cargo_jobs,
        evidence.battery.as_bytes(),
        evidence.compiler.as_bytes(),
        evidence.state.as_bytes(),
        Digest::compute(Domain::GateReceipt, &document),
    ) {
        return unavailable(FullVerifierEvidenceStage::Semantic, detail);
    }
    Ok(FullVerifierEvidenceAdmission::Admitted(Box::new(
        AdmittedFullVerifierEvidence {
            source,
            evidence: Digest::compute(Domain::GateReceipt, &document),
            battery,
            changes: prepared.changes().to_vec(),
        },
    )))
}
