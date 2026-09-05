//! Proof-aware execution of selected reusable gate actions.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Instant as MonotonicInstant;

use gate::{
    ActionInputClosure, ActionInputInventory, ActionKey, ActionKeyAuthority, ActionNonFileInput,
    ActionNonFileKind, ActionReuseClass, ActionSpec, ActionSpecFields, ActionToolRequirement,
    ArmDisposition, BatteryManifest, BatteryToolEvidence, CargoGateShard, CargoGateShardKind,
    CargoGateShardPlanning, EvidenceProducerRecord, EvidenceStore, FocusedArmReport, GateId,
    PassReceipt, PassReceiptFields,
};
use serde::Serialize;
use sill::Environment;
use wire::{Digest, Domain, Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition};

use spice::change_gate::{ChangeGateError, StagedActionSelection};
use spice::repository::Repository;

const RUNNER_VERSION: u64 = 1;

#[derive(Clone, Copy)]
struct Request<'a> {
    action: Option<&'a str>,
    missing: bool,
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

enum PreparedExecution {
    Gate(Box<spice::gate::PreparedArm<'static>>),
    Cargo(Box<CargoExecution>),
}

struct CargoExecution {
    shard: CargoGateShard,
    root: PathBuf,
    writable_root: PathBuf,
    environment: Environment,
    cargo: OsString,
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
    if request.action.is_none() && !request.missing {
        return refusal(
            "gate::prove-action-required",
            "name one ACTION to force, or use --missing to prove every selected reusable action without a current receipt",
        );
    }
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
    let staged = spice::change_gate::prepare_staged_actions().map_err(ProveFailure::selection)?;
    let store = EvidenceStore::open(repository.common_dir()).map_err(ProveFailure::store)?;

    let (mut actions, pending) =
        match prepare_actions(&staged, &store, request, repository.worktree_root()) {
            Ok(prepared) => prepared,
            Err(error) => {
                let _ = staged.reject_state();
                return Err(error);
            }
        };
    let published = complete_transaction(
        pending,
        execute,
        || validate_source(&repository, &staged, &candidate_tree),
        || staged.reject_state().map_err(ProveFailure::run),
        |pending| publish(&store, pending),
    )?;
    actions.extend(published);
    actions.sort_by(|left, right| proof_action_order(&left.action, &right.action));
    Ok(ProveReport {
        candidate_tree: candidate_tree.to_string(),
        source: staged.selection().source().clone(),
        actions,
    })
}

fn publish(
    store: &EvidenceStore,
    pending: Vec<PendingReceipt>,
) -> Result<Vec<ProveActionReport>, ProveFailure> {
    let publications = store
        .publish_batch(pending.iter().map(|pending| &pending.receipt))
        .map_err(ProveFailure::store)?;
    Ok(pending
        .into_iter()
        .zip(publications)
        .map(|(pending, publication)| ProveActionReport {
            action: pending.action,
            action_key: pending.action_key,
            disposition: ProveDisposition::Published,
            receipt: publication.receipt().clone(),
        })
        .collect())
}

fn complete_transaction<A, R, O, E>(
    actions: Vec<A>,
    mut execute: impl FnMut(A) -> Result<R, E>,
    validate: impl FnOnce() -> Result<(), E>,
    cleanup: impl FnOnce() -> Result<(), E>,
    publish: impl FnOnce(Vec<R>) -> Result<Vec<O>, E>,
) -> Result<Vec<O>, E> {
    let executed = actions
        .into_iter()
        .map(&mut execute)
        .collect::<Result<Vec<_>, _>>();
    let validated = if executed.is_ok() { validate() } else { Ok(()) };
    let cleaned = cleanup();
    let receipts = executed?;
    validated?;
    cleaned?;
    publish(receipts)
}

fn validate_source(
    repository: &Repository,
    staged: &StagedActionSelection,
    candidate_tree: &spice::gate::CandidateTree,
) -> Result<(), ProveFailure> {
    staged.anchor().verify_source().map_err(ProveFailure::run)?;
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
) -> Result<(Vec<ProveActionReport>, Vec<PreparedAction>), ProveFailure> {
    let requested = requested_actions(staged.selection(), request)?;
    let manifest = BatteryManifest::canonical()
        .map_err(|error| ProveFailure::fault("gate::prove-manifest-invalid", error))?;
    let gate_set = gate::canonical_gate_set_digest()
        .map_err(|error| ProveFailure::fault("gate::prove-manifest-invalid", error))?;
    let baseline = staged
        .anchor()
        .staged_authority()
        .and_then(gate::tree::StagedAuthority::baseline_identity);
    let mut present = Vec::new();
    let mut pending = Vec::new();

    for gate_id in requested {
        let prepared = prepare_action(staged, &manifest, &gate_set, baseline.as_deref(), gate_id)?;
        classify_prepared(store, request.missing, prepared, &mut present, &mut pending)?;
    }

    if cargo_request(request) {
        let prepared = prepare_cargo_actions(staged, &manifest, &gate_set, request, writable_root)?;
        for action in prepared {
            classify_prepared(store, request.missing, action, &mut present, &mut pending)?;
        }
    }

    if let Some(action) = request.action
        && !request_matches_any(action, &present, &pending)
    {
        return Err(ProveFailure::refusal(
            "gate::prove-action-unregistered",
            format!("{action:?} is not a selected registered gate or Cargo action"),
        ));
    }
    present.sort_by(|left, right| proof_action_order(&left.action, &right.action));
    pending.sort_by(|left, right| proof_action_order(&left.action, &right.action));
    Ok((present, pending))
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

fn prepare_action(
    staged: &StagedActionSelection,
    manifest: &BatteryManifest,
    gate_set: &Digest,
    baseline: Option<&str>,
    gate_id: GateId,
) -> Result<PreparedAction, ProveFailure> {
    let arm = staged
        .anchor()
        .derive_reached_arm(staged.selection(), gate_id)
        .map_err(ProveFailure::run)?;
    let context = arm.context().map_err(ProveFailure::run)?;
    let declaration = declaration(manifest, gate_id)?;
    let reuse = gate::gate_action_reuse_class(gate_id)
        .expect("requested actions are classified as reusable");
    let baseline = action_baseline(gate_id, reuse, baseline)?;
    let tools = action_tools(context.tools())?;
    let (environment_names, environment) = environment(arm.compiler_environment())?;
    let tool_authority = digest_encoded(Domain::GateTool, &context.tools())?;
    let inventory = action_inventory(
        staged,
        gate_id,
        declaration,
        gate_set,
        &context,
        &tool_authority,
        &environment,
        baseline.as_ref(),
    )?;
    let input_manifest = match arm
        .compile_action_input(staged.selection(), gate_id, inventory)
        .map_err(ProveFailure::run)?
    {
        ActionInputClosure::Reusable(manifest) => manifest,
        ActionInputClosure::NonCacheable(reason) => {
            return Err(ProveFailure::refusal(
                "gate::prove-action-not-reusable",
                format!("action {gate_id} has no complete reusable closure: {reason:?}"),
            ));
        }
    };
    let input_manifest = input_manifest
        .digest()
        .map_err(ProveFailure::action_input)?;
    let spec = ActionSpec::new(ActionSpecFields {
        action_id: format!("gate:{gate_id}"),
        runner: "spice-focused-gate".to_owned(),
        runner_version: RUNNER_VERSION,
        declaration: declaration.to_owned(),
        argv: vec!["gate".to_owned(), gate_id.to_string()],
        cwd: ".".to_owned(),
        environment_names,
        tools,
        input_strategy: "selected-reach-v1".to_owned(),
        reuse,
    })
    .map_err(ProveFailure::action_input)?;
    let action_spec = spec.digest().map_err(ProveFailure::action_input)?;
    let action_key = gate::derive_authority_bound_action_key(
        &spec,
        &ActionKeyAuthority {
            input_manifest: input_manifest.clone(),
            policy: context.policy().clone(),
            gate_set: gate_set.clone(),
            tools: tool_authority,
            environment,
            baseline,
        },
    )
    .map_err(ProveFailure::action_input)?;
    Ok(PreparedAction {
        action: gate_id.to_string(),
        action_key,
        action_spec,
        input_manifest,
        execution: PreparedExecution::Gate(Box::new(arm)),
    })
}

fn prepare_cargo_actions(
    staged: &StagedActionSelection,
    manifest: &BatteryManifest,
    gate_set: &Digest,
    request: Request<'_>,
    writable_root: &Path,
) -> Result<Vec<PreparedAction>, ProveFailure> {
    let typecheck_selected = staged
        .selection()
        .gate_ids()
        .contains(&GateId::RustTypecheck);
    if !typecheck_selected && !staged.cargo_tests() {
        return Ok(Vec::new());
    }
    if !typecheck_selected {
        return Err(ProveFailure::refusal(
            "gate::prove-cargo-authority-unavailable",
            "Cargo tests were selected without the canonical Rust toolchain arm",
        ));
    }

    let arm = staged
        .anchor()
        .derive_reached_arm(staged.selection(), GateId::RustTypecheck)
        .map_err(ProveFailure::run)?;
    let context = arm.context().map_err(ProveFailure::run)?;
    let cargo_environment = normalized_cargo_environment(arm.compiler_environment());
    let environment = Environment::new(cargo_environment.iter().cloned())
        .map_err(|error| ProveFailure::refusal("gate::prove-cargo-environment-refused", error))?;
    let base = arm.materialize_staged_base().map_err(ProveFailure::run)?;
    let plan = gate::plan_cargo_gate_shards_at(CargoGateShardPlanning {
        root: arm.root(),
        base: base.as_ref().map(gate::SelectedTree::root),
        source: staged.selection().source().clone(),
        changed_sources: staged.selection().changed_sources(),
        selected_paths: arm.paths(),
        environment: environment.clone(),
        cargo: OsStr::new("cargo"),
        include_tests: staged.cargo_tests(),
    })
    .map_err(ProveFailure::cargo)?;

    let shards = selected_cargo_shards(
        request,
        typecheck_selected.then_some(plan.typechecks()),
        staged.cargo_tests().then_some(plan.tests()),
    );
    let declaration = declaration(manifest, GateId::RustTypecheck)?;
    shards
        .into_iter()
        .map(|shard| {
            prepare_cargo_action(
                &arm,
                &context,
                gate_set,
                declaration,
                writable_root,
                &cargo_environment,
                environment.clone(),
                shard.clone(),
            )
        })
        .collect()
}

fn selected_cargo_shards<'a>(
    request: Request<'_>,
    typechecks: Option<&'a [CargoGateShard]>,
    tests: Option<&'a [CargoGateShard]>,
) -> Vec<&'a CargoGateShard> {
    let typechecks = typechecks.unwrap_or_default();
    let tests = tests.unwrap_or_default();
    match request.action {
        None => typechecks.iter().chain(tests).collect(),
        Some("rust-typecheck") => typechecks.iter().collect(),
        Some("cargo-tests") => tests.iter().collect(),
        Some(action) if action.starts_with("cargo:typecheck:") => typechecks
            .iter()
            .filter(|shard| shard.action_id() == action)
            .collect(),
        Some(action) if action.starts_with("cargo:test:") => tests
            .iter()
            .filter(|shard| shard.action_id() == action)
            .collect(),
        Some(_) => Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_cargo_action(
    arm: &spice::gate::PreparedArm<'_>,
    context: &gate::BatteryContext,
    gate_set: &Digest,
    typecheck_declaration: &str,
    writable_root: &Path,
    cargo_environment: &[(OsString, OsString)],
    execution_environment: Environment,
    shard: CargoGateShard,
) -> Result<PreparedAction, ProveFailure> {
    let tools = action_tools(context.tools())?;
    let (environment_names, environment) = environment(cargo_environment)?;
    let tool_authority = digest_encoded(Domain::GateTool, &context.tools())?;
    let configuration = digest_encoded(Domain::GateConfiguration, &context.configuration())?;
    let inventory = digest_encoded(Domain::GateInventory, &shard)?;
    let declaration = match shard.kind() {
        CargoGateShardKind::Typecheck => typecheck_declaration,
        CargoGateShardKind::Test => "gate::cargo-workspace-tests::proof-shard-v1",
    };
    let non_files = vec![
        non_file(
            ActionNonFileKind::Declaration,
            "compiled-declaration",
            Digest::compute(Domain::GateBatteryManifest, declaration.as_bytes()),
        )?,
        non_file(
            ActionNonFileKind::GateSet,
            "canonical-gate-set",
            gate_set.clone(),
        )?,
        non_file(
            ActionNonFileKind::Policy,
            "compiled-policy",
            context.policy().clone(),
        )?,
        non_file(
            ActionNonFileKind::Policy,
            "resolved-configuration",
            configuration,
        )?,
        non_file(
            ActionNonFileKind::Toolchain,
            "discovered-tools",
            tool_authority.clone(),
        )?,
        non_file(
            ActionNonFileKind::Environment,
            "compiler-environment",
            environment.clone(),
        )?,
        non_file(ActionNonFileKind::Inventory, "cargo-shard", inventory)?,
    ];
    let input_manifest = arm
        .compile_explicit_action_input(shard.input_paths().to_vec(), non_files)
        .map_err(ProveFailure::run)?;
    let input_manifest = input_manifest
        .digest()
        .map_err(ProveFailure::action_input)?;
    let mut argv = Vec::with_capacity(shard.argv().len() + 1);
    argv.push("cargo".to_owned());
    argv.extend_from_slice(shard.argv());
    let spec = ActionSpec::new(ActionSpecFields {
        action_id: shard.action_id().to_owned(),
        runner: "spice-cargo-proof-shard".to_owned(),
        runner_version: RUNNER_VERSION,
        declaration: declaration.to_owned(),
        argv,
        cwd: ".".to_owned(),
        environment_names,
        tools,
        input_strategy: "cargo-package-closure-v1".to_owned(),
        reuse: ActionReuseClass::Cacheable,
    })
    .map_err(ProveFailure::action_input)?;
    let action_spec = spec.digest().map_err(ProveFailure::action_input)?;
    let action_key = gate::derive_authority_bound_action_key(
        &spec,
        &ActionKeyAuthority {
            input_manifest: input_manifest.clone(),
            policy: context.policy().clone(),
            gate_set: gate_set.clone(),
            tools: tool_authority,
            environment,
            baseline: None,
        },
    )
    .map_err(ProveFailure::action_input)?;
    Ok(PreparedAction {
        action: shard.action_id().to_owned(),
        action_key,
        action_spec,
        input_manifest,
        execution: PreparedExecution::Cargo(Box::new(CargoExecution {
            shard,
            root: arm.root().to_path_buf(),
            writable_root: writable_root.to_path_buf(),
            environment: execution_environment,
            cargo: OsString::from("cargo"),
        })),
    })
}

fn normalized_cargo_environment(values: &[(OsString, OsString)]) -> Vec<(OsString, OsString)> {
    const OUTPUT_ONLY: [&str; 3] = ["PWD", "CARGO_TARGET_DIR", "CARGO_BUILD_BUILD_DIR"];
    const DISABLED_WRAPPERS: [&str; 2] = ["RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"];
    let mut normalized = values
        .iter()
        .filter(|(name, _)| {
            !OUTPUT_ONLY
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

#[allow(clippy::too_many_arguments)]
fn action_inventory(
    staged: &StagedActionSelection,
    gate_id: GateId,
    declaration: &str,
    gate_set: &Digest,
    context: &gate::BatteryContext,
    tools: &Digest,
    environment: &Digest,
    baseline: Option<&Digest>,
) -> Result<ActionInputInventory, ProveFailure> {
    let configuration = digest_encoded(Domain::GateConfiguration, &context.configuration())?;
    let reach = staged
        .selection()
        .reach_for(gate_id)
        .expect("requested actions come from the canonical selection");
    let inventory = digest_encoded(Domain::GateInventory, reach)?;
    let mut non_files = vec![
        non_file(
            ActionNonFileKind::Declaration,
            "compiled-declaration",
            Digest::compute(Domain::GateBatteryManifest, declaration.as_bytes()),
        )?,
        non_file(
            ActionNonFileKind::GateSet,
            "canonical-gate-set",
            gate_set.clone(),
        )?,
        non_file(
            ActionNonFileKind::Policy,
            "compiled-policy",
            context.policy().clone(),
        )?,
        non_file(
            ActionNonFileKind::Policy,
            "resolved-configuration",
            configuration,
        )?,
        non_file(
            ActionNonFileKind::Toolchain,
            "discovered-tools",
            tools.clone(),
        )?,
        non_file(
            ActionNonFileKind::Environment,
            "compiler-environment",
            environment.clone(),
        )?,
        non_file(ActionNonFileKind::Inventory, "selected-reach", inventory)?,
        non_file(
            ActionNonFileKind::BoundLedger,
            "gate-state-ledger",
            context.state().clone(),
        )?,
    ];
    if let Some(identity) = baseline {
        non_files.push(non_file(
            ActionNonFileKind::Baseline,
            "committed-baseline",
            identity.clone(),
        )?);
    }
    ActionInputInventory::complete(Vec::new(), non_files).map_err(ProveFailure::action_input)
}

fn action_baseline(
    gate_id: GateId,
    reuse: ActionReuseClass,
    baseline: Option<&str>,
) -> Result<Option<Digest>, ProveFailure> {
    match reuse {
        ActionReuseClass::AncestrySensitive => baseline
            .map(|identity| Digest::compute(Domain::GateBaseline, identity.as_bytes()))
            .map(Some)
            .ok_or_else(|| {
                ProveFailure::refusal(
                    "gate::prove-baseline-unavailable",
                    format!("action {gate_id} requires a committed staged baseline"),
                )
            }),
        ActionReuseClass::Cacheable => Ok(None),
        ActionReuseClass::LiveOnly => unreachable!("live-only actions were filtered"),
    }
}

fn execute(action: PreparedAction) -> Result<PendingReceipt, ProveFailure> {
    let started_at = now()?;
    let started = MonotonicInstant::now();
    let typed_result = match action.execution {
        PreparedExecution::Gate(mut arm) => {
            let report = arm.execute().map_err(ProveFailure::run)?;
            if !report.admits() {
                return Err(ProveFailure::refusal(
                    "gate::prove-action-regressed",
                    refusal_detail(&report),
                ));
            }
            Digest::compute(
                Domain::GateFocusedReport,
                &report.encode().map_err(|error| {
                    ProveFailure::fault("gate::prove-result-unencodable", error)
                })?,
            )
        }
        PreparedExecution::Cargo(execution) => {
            let CargoExecution {
                shard,
                root,
                writable_root,
                environment,
                cargo,
            } = *execution;
            let result = gate::run_cargo_gate_shard_at(
                &root,
                &writable_root,
                environment,
                &cargo,
                &shard,
                &mut std::io::stderr(),
            )
            .map_err(ProveFailure::cargo)?;
            Digest::compute(
                Domain::GateFocusedReport,
                &wire::encode(&result).map_err(|error| {
                    ProveFailure::fault("gate::prove-result-unencodable", error)
                })?,
            )
        }
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

fn cargo_request(request: Request<'_>) -> bool {
    request.missing
        || request.action.is_some_and(|action| {
            action == "rust-typecheck"
                || action == "cargo-tests"
                || action.starts_with("cargo:typecheck:")
                || action.starts_with("cargo:test:")
        })
}

fn registered_request(action: &str) -> bool {
    gate_id(action).is_some()
        || action == "cargo-tests"
        || action.starts_with("cargo:typecheck:")
        || action.starts_with("cargo:test:")
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

fn action_tools(tools: &[BatteryToolEvidence]) -> Result<Vec<ActionToolRequirement>, ProveFailure> {
    let mut requirements = tools
        .iter()
        .map(|tool| ActionToolRequirement::new(tool.name(), tool.program()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(ProveFailure::action_input)?;
    requirements.sort();
    requirements.dedup();
    Ok(requirements)
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

enum ProveFailure {
    Refusal { reason: String, detail: String },
    Fault { reason: String, detail: String },
}

impl ProveFailure {
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
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[test]
    fn registered_gate_lookup_is_exact() {
        assert_eq!(gate_id("formatters"), Some(GateId::Formatters));
        assert_eq!(gate_id("Formatters"), None);
        assert_eq!(gate_id("formatters "), None);
    }

    #[test]
    fn missing_filters_only_an_action_with_a_current_receipt() {
        assert!(action_needed(false, false));
        assert!(action_needed(false, true));
        assert!(action_needed(true, false));
        assert!(!action_needed(true, true));
    }

    #[test]
    fn cargo_selectors_are_explicit_and_keep_canonical_report_order() {
        assert!(cargo_request(Request {
            action: Some("rust-typecheck"),
            missing: false,
        }));
        assert!(cargo_request(Request {
            action: Some("cargo:test:fixture::lib:fixture"),
            missing: false,
        }));
        assert!(!cargo_request(Request {
            action: Some("formatters"),
            missing: false,
        }));
        assert_eq!(
            proof_action_order("taste", "cargo:typecheck:fixture::lib:fixture"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            proof_action_order(
                "cargo:typecheck:fixture::lib:fixture",
                "canvas-web-typecheck"
            ),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            proof_action_order("private-internals", "cargo:test:fixture::lib:fixture"),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn cargo_environment_omits_worktree_outputs_and_matches_forced_wrappers() {
        let normalized = normalized_cargo_environment(&[
            (OsString::from("PWD"), OsString::from("/worktree-a")),
            (
                OsString::from("CARGO_TARGET_DIR"),
                OsString::from("/worktree-a/target"),
            ),
            (
                OsString::from("CARGO_BUILD_BUILD_DIR"),
                OsString::from("/worktree-a/build"),
            ),
            (OsString::from("RUSTC_WRAPPER"), OsString::from("sccache")),
            (OsString::from("RUSTFLAGS"), OsString::from("--cfg=proof")),
        ]);
        assert_eq!(
            normalized,
            [
                (OsString::from("RUSTC_WORKSPACE_WRAPPER"), OsString::new()),
                (OsString::from("RUSTC_WRAPPER"), OsString::new()),
                (OsString::from("RUSTFLAGS"), OsString::from("--cfg=proof")),
            ]
        );
    }

    #[test]
    fn interrupted_execution_cleans_up_without_reaching_publication() {
        let events = RefCell::new(Vec::new());
        let result: Result<Vec<u8>, &str> = complete_transaction(
            vec![1_u8, 2, 3],
            |action| {
                events.borrow_mut().push(format!("execute-{action}"));
                if action == 2 {
                    Err("interrupted")
                } else {
                    Ok(action)
                }
            },
            || {
                events.borrow_mut().push("validate".to_owned());
                Ok(())
            },
            || {
                events.borrow_mut().push("cleanup".to_owned());
                Ok(())
            },
            |receipts| {
                events.borrow_mut().push("publish".to_owned());
                Ok(receipts)
            },
        );

        assert_eq!(result, Err("interrupted"));
        assert_eq!(events.into_inner(), ["execute-1", "execute-2", "cleanup"]);
    }

    #[test]
    fn source_validation_precedes_cleanup_and_publication() {
        let events = RefCell::new(Vec::new());
        let result = complete_transaction(
            vec![1_u8, 2],
            |action| {
                events.borrow_mut().push(format!("execute-{action}"));
                Ok::<_, &str>(action)
            },
            || {
                events.borrow_mut().push("validate".to_owned());
                Ok(())
            },
            || {
                events.borrow_mut().push("cleanup".to_owned());
                Ok(())
            },
            |receipts| {
                events.borrow_mut().push(format!("publish-{receipts:?}"));
                Ok(receipts)
            },
        );

        assert_eq!(result, Ok(vec![1, 2]));
        assert_eq!(
            events.into_inner(),
            [
                "execute-1",
                "execute-2",
                "validate",
                "cleanup",
                "publish-[1, 2]",
            ]
        );
    }

    #[test]
    fn aggregate_publication_failure_returns_no_partial_reports() {
        let result: Result<Vec<u8>, &str> = complete_transaction(
            vec![1_u8, 2],
            Ok::<_, &str>,
            || Ok(()),
            || Ok(()),
            |receipts| {
                assert_eq!(receipts, [1, 2]);
                Err("later publication failed")
            },
        );

        assert_eq!(result, Err("later publication failed"));
    }
}
