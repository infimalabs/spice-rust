//! Proof-aware execution of selected reusable gate actions.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant as MonotonicInstant;

use gate::{
    ActionFingerprintAuthority, ActionFingerprintAuthorityFields, ActionFingerprintManifest,
    ActionFingerprintManifestFields, ActionInputClosure, ActionInputInventory, ActionKey,
    ActionKeyAuthority, ActionNonFileInput, ActionNonFileKind, ActionReuseClass, ActionSpec,
    ActionSpecFields, ActionToolRequirement, ArmDisposition, BatteryManifest,
    CargoExternalSourceAuthority, CargoGateShard, CargoGateShardKind, CargoGateShardPlanning,
    EvidenceProducerRecord, EvidenceStore, FocusedArmReport, GateId, PassReceipt,
    PassReceiptFields,
};
use serde::Serialize;
use sill::Environment;
use wire::{Digest, Domain, Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition};

use spice::change_gate::{ChangeGateError, StagedActionSelection};
use spice::repository::Repository;

#[path = "prove/action.rs"]
mod action;
mod canvas;
#[path = "prove/cargo.rs"]
mod cargo;
#[cfg(test)]
#[path = "prove/cargo_authority_tests.rs"]
mod cargo_authority_tests;
#[path = "prove/fingerprint.rs"]
mod fingerprint;
#[path = "prove/inspect.rs"]
mod inspect;
#[path = "prove/single_flight.rs"]
mod single_flight;
#[path = "prove/tools.rs"]
mod tools;

#[cfg(test)]
use cargo::require_reusable_shard as require_reusable_cargo_shard;
use cargo::{
    authority as cargo_authority, environment_value, request as cargo_request,
    source_authority as cargo_source_authority,
};
use fingerprint::PreparedActions;
pub(super) use inspect::{fingerprint_generation_detail, missing_proof_detail};
use tools::action_requirements as action_tools;

const RUNNER_VERSION: u64 = 1;

#[derive(Clone, Copy)]
struct Request<'a> {
    action: Option<&'a str>,
    missing: bool,
}

impl Request<'_> {
    const fn inspect(self) -> bool {
        self.action.is_none() && !self.missing
    }

    const fn uses_fingerprint(self) -> bool {
        self.action.is_none()
    }
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProveReport {
    candidate_tree: String,
    source: Digest,
    actions: Vec<ProveActionReport>,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct ProveActionReport {
    action: String,
    action_key: ActionKey,
    disposition: ProveDisposition,
    receipt: gate::ReceiptId,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ProveDisposition {
    Present,
    Published,
}

struct PreparedAction {
    action: String,
    action_key: ActionKey,
    action_spec: Digest,
    input_manifest: Digest,
    execution: PreparedExecution,
}

impl PreparedAction {
    fn expectation(&self) -> Result<gate::ReceiptExpectation, ProveFailure> {
        gate::ReceiptExpectation::new(
            self.action.clone(),
            self.action_key.clone(),
            self.action_spec.clone(),
            self.input_manifest.clone(),
        )
        .map_err(ProveFailure::store)
    }
}

pub(super) struct AdmissionPreparation {
    candidate_tree: String,
    staged: StagedActionSelection,
    selection: Digest,
    policy: Digest,
    gate_set: Digest,
    authority: ActionFingerprintAuthority,
}

pub(super) struct AdmissionLookup {
    candidate_tree: String,
    anchor: spice::gate::PreparedArm<'static>,
    authority: ActionFingerprintAuthority,
    policy: Digest,
    gate_set: Digest,
}

impl AdmissionLookup {
    pub(super) const fn authority(&self) -> &ActionFingerprintAuthority {
        &self.authority
    }

    pub(super) fn reject_state(&self) -> Result<(), ProveFailure> {
        self.anchor.reject_state().map_err(ProveFailure::run)
    }

    pub(super) fn execute_live_checks(
        &self,
    ) -> Result<Vec<gate::AdmissionLiveCheck>, ProveFailure> {
        [GateId::PlanPhase, GateId::Staging]
            .into_iter()
            .map(|gate_id| {
                let arm = self
                    .anchor
                    .derive_live_arm(gate_id)
                    .map_err(ProveFailure::run)?;
                execute_live_arm(arm, gate_id)
            })
            .collect()
    }

    pub(super) fn restore(
        self,
        manifest: &ActionFingerprintManifest,
    ) -> Result<AdmissionPreparation, ProveFailure> {
        let selection = manifest.selection().clone();
        let selection_digest = digest_encoded(Domain::GateSelection, &selection)?;
        let staged = spice::change_gate::restore_staged_actions(
            self.anchor,
            selection,
            manifest.cargo_tests(),
        )
        .map_err(ProveFailure::selection)?;
        Ok(AdmissionPreparation {
            candidate_tree: self.candidate_tree,
            staged,
            selection: selection_digest,
            policy: self.policy,
            gate_set: self.gate_set,
            authority: self.authority,
        })
    }
}

impl AdmissionPreparation {
    pub(super) fn candidate_tree(&self) -> &str {
        &self.candidate_tree
    }

    pub(super) fn source(&self) -> &Digest {
        self.staged.selection().source()
    }

    pub(super) fn selection(&self) -> &Digest {
        &self.selection
    }

    pub(super) fn policy(&self) -> &Digest {
        &self.policy
    }

    pub(super) fn gate_set(&self) -> &Digest {
        &self.gate_set
    }

    pub(super) fn head(&self) -> Option<String> {
        self.staged
            .anchor()
            .staged_authority()
            .and_then(gate::tree::StagedAuthority::baseline_identity)
    }

    pub(super) fn verify_source(&self) -> Result<(), ProveFailure> {
        self.staged
            .anchor()
            .verify_source()
            .map_err(ProveFailure::run)
    }

    pub(super) fn verify_fingerprint_authority(
        &self,
        repository: &Repository,
    ) -> Result<(), ProveFailure> {
        let observed = fingerprint::admission_authority(
            repository,
            self.staged.anchor(),
            &self.candidate_tree,
            self.policy.clone(),
            self.gate_set.clone(),
        )?;
        if observed != self.authority {
            return Err(ProveFailure::refusal(
                "gate::prove-authority-moved",
                "proof fingerprint authority changed after lookup",
            ));
        }
        Ok(())
    }

    pub(super) fn verify_current(&self, repository: &Repository) -> Result<(), ProveFailure> {
        self.verify_source()?;
        self.verify_fingerprint_authority(repository)
    }

    pub(super) fn execute_live_checks(
        &self,
    ) -> Result<Vec<gate::AdmissionLiveCheck>, ProveFailure> {
        let mut checks = Vec::new();
        for gate_id in self.staged.selection().gate_ids().iter().copied() {
            if gate::gate_action_reuse_class(gate_id) != Some(ActionReuseClass::LiveOnly) {
                continue;
            }
            let arm = self
                .staged
                .anchor()
                .derive_reached_arm(self.staged.selection(), gate_id)
                .map_err(ProveFailure::run)?;
            checks.push(execute_live_arm(arm, gate_id)?);
        }
        checks.sort_by(|left, right| left.action().cmp(right.action()));
        Ok(checks)
    }

    pub(super) fn accept_state(&self) -> Result<(), ProveFailure> {
        self.staged
            .anchor()
            .accept_state(gate::StateRunScope::Partial)
            .map_err(ProveFailure::run)
    }

    pub(super) fn reject_state(&self) -> Result<(), ProveFailure> {
        self.staged.reject_state().map_err(ProveFailure::run)
    }

    pub(super) fn into_staged(self) -> StagedActionSelection {
        self.staged
    }
}

fn execute_live_arm(
    mut arm: spice::gate::PreparedArm<'static>,
    gate_id: GateId,
) -> Result<gate::AdmissionLiveCheck, ProveFailure> {
    let report = arm.execute().map_err(ProveFailure::run)?;
    if !report.admits() {
        return Err(ProveFailure::refusal(
            "gate::pre-commit-live-check-regressed",
            refusal_detail(&report),
        ));
    }
    let result = Digest::compute(
        Domain::GateFocusedReport,
        &report.encode().map_err(|error| {
            ProveFailure::fault("gate::pre-commit-live-result-unencodable", error)
        })?,
    );
    gate::AdmissionLiveCheck::new(gate_id.to_string(), result).map_err(ProveFailure::store)
}

pub(super) fn prepare_admission_from_anchor(
    repository: &Repository,
    anchor: spice::gate::PreparedArm<'static>,
) -> Result<AdmissionLookup, ProveFailure> {
    let staged_authority = anchor
        .staged_authority()
        .expect("staged action preparation retains staged authority");
    let candidate_tree = staged_authority.candidate_tree_identity().to_owned();
    let context = anchor.context().map_err(ProveFailure::run)?;
    let policy = context.policy().clone();
    let gate_set = gate::canonical_gate_set_digest()
        .map_err(|error| ProveFailure::fault("gate::prove-manifest-invalid", error))?;
    let authority = fingerprint::admission_authority(
        repository,
        &anchor,
        &candidate_tree,
        policy.clone(),
        gate_set.clone(),
    )?;
    Ok(AdmissionLookup {
        candidate_tree,
        anchor,
        authority,
        policy,
        gate_set,
    })
}

enum PreparedExecution {
    Gate {
        arm: Box<spice::gate::PreparedArm<'static>>,
        tools: Vec<gate::ToolEvidence>,
    },
    Cargo(Box<cargo::Execution>),
    Canvas(Box<canvas::CanvasExecution>),
}

struct PendingReceipt {
    action: String,
    action_key: ActionKey,
    receipt: PassReceipt,
}

pub(crate) fn prove(matches: &clap::ArgMatches) -> Outcome<ProveReport> {
    let request = Request {
        action: matches.get_one::<String>("ACTION").map(String::as_str),
        missing: matches.get_flag("missing"),
    };
    if let Some(action) = request.action
        && !registered_request(action)
    {
        return refusal(
            "gate::prove-action-unregistered",
            format!("{action:?} is not a registered gate action or Cargo shard"),
        );
    }
    match run(request) {
        Ok(report) => Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body: report,
        }),
        Err(failure) => failure.outcome(),
    }
}

fn run(request: Request<'_>) -> Result<ProveReport, ProveFailure> {
    let current = std::env::current_dir()
        .map_err(|error| ProveFailure::fault("gate::prove-working-directory-unavailable", error))?;
    let repository = Repository::discover(&current)
        .map_err(|error| ProveFailure::fault("gate::prove-repository-unresolved", error))?;
    let candidate_tree =
        spice::gate::capture_candidate_tree(&repository).map_err(ProveFailure::run)?;
    let anchor = spice::change_gate::prepare_staged_anchor().map_err(ProveFailure::selection)?;
    let store = EvidenceStore::open(repository.common_dir()).map_err(ProveFailure::store)?;
    let fingerprint = request
        .uses_fingerprint()
        .then(|| fingerprint::current_authority(&repository, &anchor, &candidate_tree))
        .transpose()?;
    if request.inspect() {
        return inspect::current(
            &repository,
            &store,
            &candidate_tree,
            &anchor,
            fingerprint
                .as_ref()
                .expect("inspection always derives fingerprint authority"),
        );
    }
    let staged = if let Some(authority) = fingerprint.as_ref() {
        match fingerprint::lookup_complete(&store, authority) {
            Ok(Some(actions)) => {
                validate_source(&repository, &anchor, &candidate_tree)?;
                anchor.reject_state().map_err(ProveFailure::run)?;
                return Ok(ProveReport {
                    candidate_tree: candidate_tree.to_string(),
                    source: anchor.source().clone(),
                    actions,
                });
            }
            Ok(None) => {}
            Err(error) => {
                let _ = anchor.reject_state();
                return Err(error);
            }
        }
        match store
            .lookup_action_fingerprints(authority)
            .map_err(ProveFailure::store)
        {
            Ok(Some(manifest)) => spice::change_gate::restore_staged_actions(
                anchor,
                manifest.selection().clone(),
                manifest.cargo_tests(),
            )
            .map_err(ProveFailure::selection)?,
            Ok(None) => spice::change_gate::prepare_staged_actions_from_anchor(anchor)
                .map_err(ProveFailure::selection)?,
            Err(error) => {
                let _ = anchor.reject_state();
                return Err(error);
            }
        }
    } else {
        spice::change_gate::prepare_staged_actions_from_anchor(anchor)
            .map_err(ProveFailure::selection)?
    };
    let (mut actions, pending, expectations) = match prepare_actions(
        &staged,
        &store,
        request,
        repository.worktree_root(),
        repository.common_dir(),
    ) {
        Ok(prepared) => prepared,
        Err(error) => {
            let _ = staged.reject_state();
            return Err(error);
        }
    };
    if let Some(authority) = fingerprint.as_ref()
        && let Err(error) = ActionFingerprintManifest::new(ActionFingerprintManifestFields {
            authority: authority.clone(),
            selection: staged.selection().clone(),
            cargo_tests: staged.cargo_tests(),
            actions: expectations.clone(),
        })
        .map_err(ProveFailure::store)
        .and_then(|manifest| {
            store
                .publish_action_fingerprints(&manifest)
                .map(|_| ())
                .map_err(ProveFailure::store)
        })
    {
        let _ = staged.reject_state();
        return Err(error);
    }
    let published = single_flight::run(
        &store,
        pending,
        || validate_source(&repository, staged.anchor(), &candidate_tree),
        || staged.reject_state().map_err(ProveFailure::run),
    )?;
    actions.extend(published);
    actions.sort_by(|left, right| proof_action_order(&left.action, &right.action));
    Ok(ProveReport {
        candidate_tree: candidate_tree.to_string(),
        source: staged.selection().source().clone(),
        actions,
    })
}

#[cfg(test)]
fn complete_actions<A, R, O, E>(
    actions: Vec<A>,
    mut execute: impl FnMut(A) -> Result<R, E>,
    mut validate: impl FnMut() -> Result<(), E>,
    cleanup: impl FnOnce() -> Result<(), E>,
    mut publish: impl FnMut(R) -> Result<O, E>,
) -> Result<Vec<O>, E> {
    let completed = actions
        .into_iter()
        .try_fold(Vec::new(), |mut reports, action| {
            let receipt = execute(action)?;
            validate()?;
            reports.push(publish(receipt)?);
            Ok(reports)
        });
    let cleaned = cleanup();
    let reports = completed?;
    cleaned?;
    Ok(reports)
}

fn validate_source(
    repository: &Repository,
    anchor: &spice::gate::PreparedArm<'_>,
    candidate_tree: &spice::gate::CandidateTree,
) -> Result<(), ProveFailure> {
    anchor
        .verify_refreshable_source()
        .map_err(ProveFailure::run)?;
    let observed = spice::gate::capture_candidate_tree(repository).map_err(ProveFailure::run)?;
    if &observed != candidate_tree {
        return Err(ProveFailure::refusal(
            "gate::prove-source-moved",
            format!("before={candidate_tree}; after={observed}"),
        ));
    }
    Ok(())
}

fn prepare_actions(
    staged: &StagedActionSelection,
    store: &EvidenceStore,
    request: Request<'_>,
    writable_root: &Path,
    common: &Path,
) -> Result<PreparedActions, ProveFailure> {
    let prepared_actions =
        prepare_action_authorities(staged, store, request, writable_root, common)?;
    let mut expectations = prepared_actions
        .iter()
        .map(PreparedAction::expectation)
        .collect::<Result<Vec<_>, _>>()?;
    expectations.sort_by(|left, right| left.action().cmp(right.action()));
    let mut present = Vec::new();
    let mut pending = Vec::new();
    for prepared in prepared_actions {
        classify_prepared(store, request.missing, prepared, &mut present, &mut pending)?;
    }

    if let Some(action) = request.action
        && !request_matches_any(action, &present, &pending)
    {
        return Err(ProveFailure::refusal(
            "gate::prove-action-unregistered",
            format!("{action:?} is not a selected registered gate, Cargo shard, or Canvas action"),
        ));
    }
    present.sort_by(|left, right| proof_action_order(&left.action, &right.action));
    pending.sort_by(|left, right| proof_action_order(&left.action, &right.action));
    Ok((present, pending, expectations))
}

fn prepare_action_authorities(
    staged: &StagedActionSelection,
    store: &EvidenceStore,
    request: Request<'_>,
    writable_root: &Path,
    common: &Path,
) -> Result<Vec<PreparedAction>, ProveFailure> {
    let requested = requested_actions(staged.selection(), request)?;
    let manifest = BatteryManifest::canonical()
        .map_err(|error| ProveFailure::fault("gate::prove-manifest-invalid", error))?;
    let gate_set = gate::canonical_gate_set_digest()
        .map_err(|error| ProveFailure::fault("gate::prove-manifest-invalid", error))?;
    let baseline = staged
        .anchor()
        .staged_authority()
        .and_then(gate::tree::StagedAuthority::baseline_identity);
    let mut prepared = Vec::new();
    for gate_id in requested {
        prepared.push(action::prepare(
            staged,
            &manifest,
            &gate_set,
            baseline.as_deref(),
            gate_id,
        )?);
    }

    if cargo_request(request) {
        prepared.extend(cargo::prepare(
            staged,
            &manifest,
            &gate_set,
            request,
            writable_root,
        )?);
    }

    if canvas::request(request) {
        prepared.extend(canvas::prepare(
            staged,
            store,
            &gate_set,
            request,
            writable_root,
            common,
        )?);
    }
    Ok(prepared)
}

fn classify_prepared(
    store: &EvidenceStore,
    missing: bool,
    prepared: PreparedAction,
    present: &mut Vec<ProveActionReport>,
    pending: &mut Vec<PreparedAction>,
) -> Result<(), ProveFailure> {
    let current = if missing {
        store
            .lookup(&prepared.action_key)
            .map_err(ProveFailure::store)?
    } else {
        None
    };
    if action_needed(missing, current.is_some()) {
        pending.push(prepared);
    } else {
        let receipt = current.expect("a filtered action has a current receipt");
        present.push(ProveActionReport {
            action: prepared.action,
            action_key: prepared.action_key,
            disposition: ProveDisposition::Present,
            receipt: receipt.id().map_err(ProveFailure::store)?,
        });
    }
    Ok(())
}

fn normalized_action_environment(values: &[(OsString, OsString)]) -> Vec<(OsString, OsString)> {
    const LOCATION_ONLY: [&str; 5] = [
        "PWD",
        "CARGO_HOME",
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_BUILD_DIR",
        gate::TEST_CARGO_METADATA_MARKER,
    ];
    const DISABLED_WRAPPERS: [&str; 2] = ["RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"];
    let mut normalized = values
        .iter()
        .filter(|(name, _)| {
            !LOCATION_ONLY
                .iter()
                .any(|candidate| name == OsStr::new(candidate))
                && !DISABLED_WRAPPERS
                    .iter()
                    .any(|candidate| name == OsStr::new(candidate))
        })
        .cloned()
        .collect::<Vec<_>>();
    normalized.extend(DISABLED_WRAPPERS.map(|name| (OsString::from(name), OsString::new())));
    normalized.sort();
    normalized
}

fn execute(action: PreparedAction) -> Result<PendingReceipt, ProveFailure> {
    let started_at = now()?;
    let started = MonotonicInstant::now();
    let typed_result = match action.execution {
        PreparedExecution::Gate {
            arm,
            tools: expected_tools,
        } => tools::execute(*arm, expected_tools)?,
        PreparedExecution::Cargo(execution) => cargo::execute(*execution)?,
        PreparedExecution::Canvas(execution) => canvas::execute(*execution, &action.action)?,
    };
    let duration_millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let finished_at = now()?;
    let receipt = PassReceipt::new(PassReceiptFields {
        action_key: action.action_key.clone(),
        action_spec: action.action_spec,
        input_manifest: action.input_manifest,
        typed_result,
        started_at,
        finished_at,
        duration_millis,
        artifacts: Vec::new(),
        producer: EvidenceProducerRecord::new(None, None, None, None, None),
    })
    .map_err(ProveFailure::store)?;
    Ok(PendingReceipt {
        action: action.action,
        action_key: action.action_key,
        receipt,
    })
}

fn requested_actions(
    selection: &gate::ChangeSelectionReport,
    request: Request<'_>,
) -> Result<Vec<GateId>, ProveFailure> {
    let named = request.action.and_then(gate_id);
    if request.action.is_some() && named.is_none() {
        return Ok(Vec::new());
    }
    if let Some(gate_id) = named
        && !selection.gate_ids().contains(&gate_id)
    {
        return Err(ProveFailure::refusal(
            "gate::prove-action-not-selected",
            format!("action {gate_id} is not selected by the staged change"),
        ));
    }
    let mut requested = Vec::new();
    for gate_id in selection.gate_ids().iter().copied() {
        if gate_id == GateId::RustTypecheck {
            continue;
        }
        if named.is_some_and(|named| named != gate_id) {
            continue;
        }
        match gate::gate_action_reuse_class(gate_id) {
            Some(ActionReuseClass::Cacheable | ActionReuseClass::AncestrySensitive) => {
                requested.push(gate_id);
            }
            Some(ActionReuseClass::LiveOnly) | None if named == Some(gate_id) => {
                return Err(ProveFailure::refusal(
                    "gate::prove-action-not-reusable",
                    format!("action {gate_id} is live-only and cannot publish reusable proof"),
                ));
            }
            Some(ActionReuseClass::LiveOnly) | None => {}
        }
    }
    Ok(requested)
}

fn registered_request(action: &str) -> bool {
    gate_id(action).is_some()
        || action == "cargo-tests"
        || action.starts_with("cargo:typecheck:")
        || action.starts_with("cargo:test:")
        || canvas::registered(action)
}

fn request_matches_any(
    requested: &str,
    present: &[ProveActionReport],
    pending: &[PreparedAction],
) -> bool {
    let exact = present.iter().any(|row| row.action == requested)
        || pending.iter().any(|row| row.action == requested);
    exact
        || (requested == "rust-typecheck"
            && present
                .iter()
                .map(|row| row.action.as_str())
                .chain(pending.iter().map(|row| row.action.as_str()))
                .any(|action| action.starts_with("cargo:typecheck:")))
        || (requested == "cargo-tests"
            && present
                .iter()
                .map(|row| row.action.as_str())
                .chain(pending.iter().map(|row| row.action.as_str()))
                .any(|action| action.starts_with("cargo:test:")))
        || canvas::matches_group(requested, present, pending)
}

fn declaration(manifest: &BatteryManifest, gate_id: GateId) -> Result<&str, ProveFailure> {
    let position = gate_position(gate_id);
    match manifest
        .arms()
        .get(position)
        .map(gate::BatteryManifestArm::disposition)
    {
        Some(ArmDisposition::Compiled {
            gate_id: observed,
            declaration,
            ..
        }) if *observed == gate_id => Ok(declaration),
        _ => Err(ProveFailure::fault(
            "gate::prove-manifest-invalid",
            format!("canonical manifest has no row for action {gate_id}"),
        )),
    }
}

fn environment(
    values: &[(std::ffi::OsString, std::ffi::OsString)],
) -> Result<(Vec<String>, Digest), ProveFailure> {
    let mut values = values
        .iter()
        .map(|(name, value)| (name.as_os_str(), value.as_os_str()))
        .collect::<Vec<_>>();
    values.sort();
    let mut bytes = Vec::new();
    let mut names = Vec::with_capacity(values.len());
    for (name, value) in values {
        let text = name.to_str().ok_or_else(|| {
            ProveFailure::refusal(
                "gate::prove-environment-invalid",
                "compiler environment contains a non-UTF-8 name",
            )
        })?;
        names.push(text.to_owned());
        append_bytes(&mut bytes, name.as_encoded_bytes());
        append_bytes(&mut bytes, value.as_encoded_bytes());
    }
    names.dedup();
    Ok((names, Digest::compute(Domain::GateConfiguration, &bytes)))
}

fn append_bytes(target: &mut Vec<u8>, value: &[u8]) {
    target.extend_from_slice(&u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    target.extend_from_slice(value);
}

fn digest_encoded<T: Serialize>(domain: Domain, value: &T) -> Result<Digest, ProveFailure> {
    let bytes = wire::encode(value)
        .map_err(|error| ProveFailure::fault("gate::prove-authority-unencodable", error))?;
    Ok(Digest::compute(domain, &bytes))
}

fn non_file(
    kind: ActionNonFileKind,
    name: &str,
    digest: Digest,
) -> Result<ActionNonFileInput, ProveFailure> {
    ActionNonFileInput::new(kind, name, digest).map_err(ProveFailure::action_input)
}

fn gate_id(action: &str) -> Option<GateId> {
    GateId::BATTERY
        .into_iter()
        .find(|candidate| candidate.as_str() == action)
}

const fn action_needed(missing: bool, receipt_present: bool) -> bool {
    !missing || !receipt_present
}

fn gate_position(gate_id: GateId) -> usize {
    GateId::BATTERY
        .into_iter()
        .position(|candidate| candidate == gate_id)
        .expect("registered gate ids have canonical positions")
}

fn proof_action_order(left: &str, right: &str) -> std::cmp::Ordering {
    proof_action_position(left)
        .cmp(&proof_action_position(right))
        .then_with(|| left.cmp(right))
}

fn proof_action_position(action: &str) -> usize {
    if action.starts_with("cargo:typecheck:") {
        return gate_position(GateId::RustTypecheck);
    }
    if action.starts_with("cargo:test:") {
        return GateId::BATTERY.len();
    }
    if action == gate::CANVAS_BUILD_ACTION {
        return GateId::BATTERY.len() + 1;
    }
    if action.starts_with(gate::CANVAS_ACTION_PREFIX) {
        return GateId::BATTERY.len() + 2;
    }
    gate_id(action).map_or(GateId::BATTERY.len() + 1, gate_position)
}

fn now() -> Result<wire::Instant, ProveFailure> {
    wire::Instant::new(jiff::Timestamp::now())
        .map_err(|error| ProveFailure::fault("gate::prove-clock-invalid", error))
}

fn refusal_detail(report: &FocusedArmReport) -> String {
    format!(
        "action={} state={} reason={}",
        report.gate_id(),
        report.result().state().as_str(),
        report.result().reason(),
    )
}

fn refusal<B>(reason: &str, detail: impl Into<String>) -> Outcome<B> {
    Outcome::Refusal(Refusal::evidence(
        reason,
        detail,
        "repair the named proof request or staged action inputs, then rerun it",
    ))
}

pub(super) enum ProveFailure {
    Refusal { reason: String, detail: String },
    Fault { reason: String, detail: String },
}

impl ProveFailure {
    pub(super) fn reason(&self) -> &str {
        match self {
            Self::Refusal { reason, .. } | Self::Fault { reason, .. } => reason,
        }
    }

    pub(super) fn detail(&self) -> &str {
        match self {
            Self::Refusal { detail, .. } | Self::Fault { detail, .. } => detail,
        }
    }

    pub(super) const fn is_fault(&self) -> bool {
        matches!(self, Self::Fault { .. })
    }

    fn refusal(reason: impl Into<String>, detail: impl ToString) -> Self {
        Self::Refusal {
            reason: reason.into(),
            detail: detail.to_string(),
        }
    }

    fn fault(reason: impl Into<String>, detail: impl ToString) -> Self {
        Self::Fault {
            reason: reason.into(),
            detail: detail.to_string(),
        }
    }

    fn run(error: spice::gate::RunError) -> Self {
        if error.is_fault() {
            Self::fault(error.reason(), error.detail())
        } else {
            Self::refusal(error.reason(), error.detail())
        }
    }

    fn selection(error: ChangeGateError) -> Self {
        if error.is_fault() {
            Self::fault(error.reason(), error.detail())
        } else {
            Self::refusal(error.reason(), error.detail())
        }
    }

    fn store(error: gate::EvidenceStoreError) -> Self {
        Self::fault(error.reason(), error.detail())
    }

    fn action_input(error: gate::ActionInputError) -> Self {
        Self::refusal("gate::prove-action-input-refused", error)
    }

    fn cargo(error: gate::WorkspaceTestError) -> Self {
        Self::refusal("gate::prove-cargo-shard-refused", error)
    }

    fn cargo_isolation(error: spice::cargo_isolation::IsolationError) -> Self {
        Self::refusal(error.reason(), error.detail())
    }

    fn canvas(error: gate::CanvasActionError) -> Self {
        Self::refusal("gate::prove-canvas-action-refused", error)
    }

    fn outcome<B>(self) -> Outcome<B> {
        match self {
            Self::Refusal { reason, detail } => refusal(&reason, detail),
            Self::Fault { reason, detail } => Outcome::Fault(Fault {
                class: FaultClass::Environment,
                reason,
                detail,
            }),
        }
    }
}

#[cfg(test)]
mod tests;
