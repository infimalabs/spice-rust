//! Fast reusable-action manifest authority and resolution.

use super::*;

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct FingerprintEnvironments {
    gate: Digest,
    cargo: Option<Digest>,
    canvas: Option<Digest>,
}

pub(super) type PreparedActions = (
    Vec<ProveActionReport>,
    Vec<PreparedAction>,
    Vec<gate::ReceiptExpectation>,
);

#[cfg(test)]
pub(super) enum ActionResolution {
    Fingerprint(Vec<ProveActionReport>),
    Planned(PreparedActions),
}

pub(super) fn authority(
    repository: &Repository,
    anchor: &spice::gate::PreparedArm<'_>,
    context: &gate::BatteryContext,
    candidate_tree: &str,
    policy: Digest,
    gate_set: Digest,
) -> Result<ActionFingerprintAuthority, ProveFailure> {
    let gate_environment = normalized_action_environment(anchor.compiler_environment());
    let (_, gate_environment) = environment(&gate_environment)?;
    let proof_tools = anchor.proof_tool_authority().map_err(ProveFailure::run)?;
    let (tools, canvas_environment) = if canvas_authority_present(anchor, repository) {
        let (tools, environment) =
            canvas::fingerprint_authorities(anchor, context, repository.worktree_root())?;
        (
            digest_encoded(Domain::GateTool, &(tools, &proof_tools))?,
            Some(environment),
        )
    } else {
        (
            digest_encoded(Domain::GateTool, &(context.tools(), &proof_tools))?,
            None,
        )
    };
    let needs_cargo = anchor.root().join("Cargo.lock").is_file();
    // A running appliance can change these compiled contracts while the staged
    // tree and external tools stay fixed. Its cache must name that generation.
    let tools = digest_encoded(
        Domain::GateTool,
        &(
            tools,
            RUNNER_VERSION,
            needs_cargo.then_some((
                cargo::RUNNER,
                cargo::INPUT_STRATEGY,
                cargo::TEST_SHARD_DECLARATION,
            )),
        ),
    )?;
    let (cargo_environment, cargo_source) = if needs_cargo {
        let cargo = cargo_authority(anchor.root(), anchor.compiler_environment())?;
        let (_, environment) = environment(&cargo.action_environment)?;
        let (_, external_source) = cargo_source_authority(
            repository.worktree_root(),
            anchor.root(),
            &cargo.execution_environment,
        )?;
        (
            Some(environment),
            Some(digest_encoded(Domain::GateInventory, &external_source)?),
        )
    } else {
        (None, None)
    };
    let environments = FingerprintEnvironments {
        gate: gate_environment,
        cargo: cargo_environment,
        canvas: canvas_environment,
    };
    let environment = digest_encoded(Domain::GateConfiguration, &environments)?;
    ActionFingerprintAuthority::new(ActionFingerprintAuthorityFields {
        candidate_tree: candidate_tree.to_owned(),
        baseline: anchor
            .staged_authority()
            .and_then(gate::tree::StagedAuthority::baseline_identity),
        source: anchor.source().clone(),
        policy,
        gate_set,
        tools,
        environment,
        cargo_source,
    })
    .map_err(ProveFailure::store)
}

pub(super) fn current_authority(
    repository: &Repository,
    anchor: &spice::gate::PreparedArm<'_>,
    candidate_tree: &spice::gate::CandidateTree,
) -> Result<ActionFingerprintAuthority, ProveFailure> {
    let context = anchor.context().map_err(ProveFailure::run)?;
    let policy = context.policy().clone();
    let gate_set = gate::canonical_gate_set_digest()
        .map_err(|error| ProveFailure::fault("gate::prove-manifest-invalid", error))?;
    authority(
        repository,
        anchor,
        &context,
        &candidate_tree.to_string(),
        policy,
        gate_set,
    )
}

pub(super) fn admission_authority(
    repository: &Repository,
    anchor: &spice::gate::PreparedArm<'_>,
    candidate_tree: &str,
    policy: Digest,
    gate_set: Digest,
) -> Result<ActionFingerprintAuthority, ProveFailure> {
    let context = anchor.context().map_err(ProveFailure::run)?;
    authority(
        repository,
        anchor,
        &context,
        candidate_tree,
        policy,
        gate_set,
    )
}

fn canvas_authority_present(
    anchor: &spice::gate::PreparedArm<'_>,
    repository: &Repository,
) -> bool {
    anchor.root().join("package-lock.json").is_file()
        && repository
            .worktree_root()
            .join("node_modules/playwright")
            .is_dir()
}

#[cfg(test)]
pub(super) fn resolve(
    authority: Option<&ActionFingerprintAuthority>,
    lookup: impl FnOnce(
        &ActionFingerprintAuthority,
    ) -> Result<Option<Vec<ProveActionReport>>, ProveFailure>,
    prepare: impl FnOnce() -> Result<PreparedActions, ProveFailure>,
) -> Result<ActionResolution, ProveFailure> {
    if let Some(authority) = authority
        && let Some(actions) = lookup(authority)?
    {
        return Ok(ActionResolution::Fingerprint(actions));
    }
    prepare().map(ActionResolution::Planned)
}

pub(super) fn lookup_complete(
    store: &EvidenceStore,
    authority: &ActionFingerprintAuthority,
) -> Result<Option<Vec<ProveActionReport>>, ProveFailure> {
    let Some(resolution) = store
        .lookup_fingerprint_receipts(authority)
        .map_err(evidence_lookup_failure)?
    else {
        return Ok(None);
    };
    let (manifest, resolved) = resolution.into_parts();
    if resolved.iter().any(Option::is_none) {
        return Ok(None);
    }
    let mut reports = manifest
        .actions()
        .iter()
        .zip(resolved)
        .map(|(expected, action)| {
            let action = action.expect("missing fingerprint receipts were handled above");
            ProveActionReport {
                action: expected.action().to_owned(),
                action_key: expected.action_key().clone(),
                disposition: ProveDisposition::Present,
                receipt: action.receipt().clone(),
            }
        })
        .collect::<Vec<_>>();
    reports.sort_by(|left, right| proof_action_order(&left.action, &right.action));
    Ok(Some(reports))
}

fn evidence_lookup_failure(error: gate::EvidenceStoreError) -> ProveFailure {
    let reason = if error.reason().starts_with("gate-evidence::fingerprint") {
        "gate::prove-fingerprint-invalid"
    } else {
        "gate::prove-receipt-invalid"
    };
    ProveFailure::refusal(
        reason,
        format!(
            "reason={}; {}\ncommand=`spice gate prove --missing`",
            error.reason(),
            error.detail(),
        ),
    )
}
