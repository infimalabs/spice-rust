//! Canvas build and browser-shard proof actions.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use gate::{CanvasActionPlan, CanvasActionPlanning, CanvasShard};
use serde::{Deserialize, Serialize};

use super::*;

const BUILD_DECLARATION: &str = "gate::canvas::build-action-v3";
const SHARD_DECLARATION: &str = "gate::canvas::browser-shard-action-v1";
const CANVAS_RUNNER_VERSION: u64 = 3;
const CANVAS_BINARY_VARIABLE: &str = "SPICE_CANVAS_BINARY"; // env-policy: allow
const CANVAS_SHARD_VARIABLE: &str = "SPICE_CANVAS_SHARD"; // env-policy: allow
const NODE_PROBE: &str = concat!(
    "const {createRequire}=await import('node:module');",
    "const require=createRequire(import.meta.url);",
    "const {chromium,firefox}=await import('playwright');",
    "console.log(JSON.stringify({node:process.version,playwright:require('playwright/package.json').version,",
    "chromium:chromium.executablePath(),firefox:firefox.executablePath()}));",
);

type SharedCanvasBuild = Arc<Mutex<Option<Result<Arc<CanvasBuild>, String>>>>;

mod packages;
mod process;

use packages::PlaywrightSnapshot;

#[derive(Clone)]
pub(super) struct CanvasExecution {
    root: PathBuf,
    common: PathBuf,
    environment: Vec<(OsString, OsString)>,
    build_inputs: Vec<String>,
    shard_inputs: Vec<String>,
    shard: Option<CanvasShard>,
    build: SharedCanvasBuild,
    playwright: Arc<PlaywrightSnapshot>,
}

struct CanvasBuild {
    _workspace: CanvasWorkspace,
    root: PathBuf,
    binary: PathBuf,
}

struct CanvasWorkspace(PathBuf);

impl Drop for CanvasWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RuntimeProbe {
    node: String,
    playwright: String,
    chromium: PathBuf,
    firefox: PathBuf,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct RuntimeAuthority {
    git_version: String,
    node_version: String,
    playwright_version: String,
    git_executable: Digest,
    node_executable: Digest,
    chromium_executable: Digest,
    firefox_executable: Digest,
    playwright_packages: Digest,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct CanvasResult<'a> {
    action: &'a str,
    completion: &'static str,
}

pub(super) fn request(request: Request<'_>) -> bool {
    (request.missing && request.action.is_none())
        || request.action.is_some_and(|action| {
            action == gate::CANVAS_ACTION_GROUP
                || action == gate::CANVAS_BUILD_ACTION
                || action.starts_with(gate::CANVAS_ACTION_PREFIX)
        })
}

pub(super) fn registered(action: &str) -> bool {
    action == gate::CANVAS_ACTION_GROUP
        || action == gate::CANVAS_BUILD_ACTION
        || action.starts_with(gate::CANVAS_ACTION_PREFIX)
}

pub(super) fn matches_group(
    requested: &str,
    present: &[ProveActionReport],
    pending: &[PreparedAction],
) -> bool {
    requested == gate::CANVAS_ACTION_GROUP
        && present
            .iter()
            .map(|row| row.action.as_str())
            .chain(pending.iter().map(|row| row.action.as_str()))
            .any(|action| action == gate::CANVAS_BUILD_ACTION)
}

pub(super) fn prepare(
    staged: &StagedActionSelection,
    store: &EvidenceStore,
    gate_set: &Digest,
    request: Request<'_>,
    repository: &Path,
    common: &Path,
) -> Result<Vec<PreparedAction>, ProveFailure> {
    let selected = staged
        .selection()
        .gate_ids()
        .contains(&GateId::CanvasWebTypecheck);
    if !selected {
        if request.action.is_some_and(registered) {
            return Err(ProveFailure::refusal(
                "gate::prove-action-not-selected",
                "Canvas actions are not selected by the staged change",
            ));
        }
        return Ok(Vec::new());
    }

    let arm = staged
        .anchor()
        .derive_reached_arm(staged.selection(), GateId::CanvasWebTypecheck)
        .map_err(ProveFailure::run)?;
    let context = arm.context().map_err(ProveFailure::run)?;
    let (plan, environment_values) = canvas_plan(&arm)?;
    let (environment_names, environment) = environment(&environment_values)?;
    let playwright = Arc::new(PlaywrightSnapshot::capture(arm.root(), repository)?);
    let runtime = runtime_authority(&environment_values, &playwright)?;
    let tool_authority = digest_encoded(Domain::GateTool, &(context.tools(), &runtime))?;
    let mut tools = action_tools(context.tools())?;
    tools.extend(
        [
            ActionToolRequirement::new("chromium", "playwright-chromium"),
            ActionToolRequirement::new("firefox", "playwright-firefox"),
            ActionToolRequirement::new("git", "/usr/bin/git"),
            ActionToolRequirement::new("node", "node"),
            ActionToolRequirement::new("playwright", "playwright"),
        ]
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map_err(ProveFailure::action_input)?,
    );
    tools.sort();
    tools.dedup();
    let build = Arc::new(Mutex::new(None));
    let prepared_build = prepare_build(
        &arm,
        &context,
        gate_set,
        &tool_authority,
        &environment,
        &environment_names,
        &tools,
        common,
        &environment_values,
        Arc::clone(&build),
        Arc::clone(&playwright),
        &plan,
    )?;
    let build_key = prepared_build.action_key.clone();
    if request
        .action
        .is_some_and(|action| action.starts_with(gate::CANVAS_ACTION_PREFIX))
        && store
            .lookup(&build_key)
            .map_err(ProveFailure::store)?
            .is_none()
    {
        return Err(ProveFailure::refusal(
            "gate::prove-canvas-build-proof-missing",
            format!(
                "{} requires a current {}; run `spice gate prove {}` first",
                request.action.expect("a direct shard request is present"),
                gate::CANVAS_BUILD_ACTION,
                gate::CANVAS_BUILD_ACTION,
            ),
        ));
    }
    let mut actions = Vec::new();
    if request.action.is_none()
        || request.action == Some(gate::CANVAS_ACTION_GROUP)
        || request.action == Some(gate::CANVAS_BUILD_ACTION)
    {
        actions.push(prepared_build);
    }
    if request.action != Some(gate::CANVAS_BUILD_ACTION) {
        for shard in selected_shards(&plan, request) {
            actions.push(prepare_shard(
                &arm,
                &context,
                gate_set,
                &tool_authority,
                &environment,
                &environment_names,
                &tools,
                common,
                &environment_values,
                Arc::clone(&build),
                Arc::clone(&playwright),
                &plan,
                shard,
                &build_key,
            )?);
        }
    }
    Ok(actions)
}

fn canvas_plan(
    arm: &spice::gate::PreparedArm<'_>,
) -> Result<(CanvasActionPlan, Vec<(OsString, OsString)>), ProveFailure> {
    let cargo = super::cargo_authority(arm.root(), arm.compiler_environment())?;
    let environment_values = normalized_environment(cargo.environment.iter().cloned());
    let cargo_home = super::environment_value(&environment_values, "CARGO_HOME")?;
    let lockfile = fs::read(arm.root().join("Cargo.lock"))
        .map_err(|error| ProveFailure::refusal("gate::prove-cargo-source-unavailable", error))?;
    let source =
        spice::cargo_isolation::source_generation_authority(Path::new(cargo_home), &lockfile)
            .map_err(|error| {
                ProveFailure::refusal("gate::prove-cargo-source-unavailable", error)
            })?;
    let external_source = gate::CargoExternalSourceAuthority::new(
        source.generation().to_owned(),
        source.manifest().to_owned(),
    )
    .map_err(ProveFailure::cargo)?;
    let environment = Environment::new(environment_values.iter().cloned())
        .map_err(|error| ProveFailure::refusal("gate::prove-canvas-environment-refused", error))?;
    let plan = CanvasActionPlan::load(CanvasActionPlanning {
        root: arm.root(),
        paths: arm.paths(),
        changed_paths: arm.changes(),
        environment: &environment,
        cargo: cargo.cargo.as_os_str(),
        external_source: Some(&external_source),
    })
    .map_err(ProveFailure::canvas)?;
    require_reusable_build(&plan)?;
    Ok((plan, environment_values))
}

fn selected_shards<'a>(plan: &'a CanvasActionPlan, request: Request<'_>) -> Vec<&'a CanvasShard> {
    match request.action {
        None | Some(gate::CANVAS_ACTION_GROUP) => plan.shards().iter().collect(),
        Some(action) => plan
            .shards()
            .iter()
            .filter(|shard| shard.action_id() == action)
            .collect(),
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_build(
    arm: &spice::gate::PreparedArm<'_>,
    context: &gate::BatteryContext,
    gate_set: &Digest,
    tools_digest: &Digest,
    environment: &Digest,
    environment_names: &[String],
    tools: &[ActionToolRequirement],
    common: &Path,
    environment_values: &[(OsString, OsString)],
    build: SharedCanvasBuild,
    playwright: Arc<PlaywrightSnapshot>,
    plan: &CanvasActionPlan,
) -> Result<PreparedAction, ProveFailure> {
    let declaration = Digest::compute(Domain::GateBatteryManifest, BUILD_DECLARATION.as_bytes());
    let non_files = canvas_non_files(
        gate_set,
        context.policy(),
        tools_digest,
        environment,
        declaration,
    )?;
    let input_manifest = arm
        .compile_explicit_action_input(plan.build_inputs().to_vec(), non_files)
        .map_err(ProveFailure::run)?;
    prepared_canvas_action(
        gate::CANVAS_BUILD_ACTION.to_owned(),
        BUILD_DECLARATION,
        vec![
            "node".to_owned(),
            "app/browser/canvas-proof-build.mjs".to_owned(),
        ],
        input_manifest,
        context.policy(),
        gate_set,
        tools_digest,
        environment,
        environment_names,
        tools,
        CanvasExecution {
            root: arm.root().to_path_buf(),
            common: common.to_path_buf(),
            environment: environment_values.to_vec(),
            build_inputs: present_inputs(plan.build_inputs(), arm.paths()),
            shard_inputs: Vec::new(),
            shard: None,
            build,
            playwright,
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn prepare_shard(
    arm: &spice::gate::PreparedArm<'_>,
    context: &gate::BatteryContext,
    gate_set: &Digest,
    tools_digest: &Digest,
    environment: &Digest,
    environment_names: &[String],
    tools: &[ActionToolRequirement],
    common: &Path,
    environment_values: &[(OsString, OsString)],
    build: SharedCanvasBuild,
    playwright: Arc<PlaywrightSnapshot>,
    plan: &CanvasActionPlan,
    shard: &CanvasShard,
    build_key: &ActionKey,
) -> Result<PreparedAction, ProveFailure> {
    let mut non_files = canvas_non_files(
        gate_set,
        context.policy(),
        tools_digest,
        environment,
        Digest::compute(Domain::GateBatteryManifest, SHARD_DECLARATION.as_bytes()),
    )?;
    non_files.extend([
        non_file(
            ActionNonFileKind::Inventory,
            "canvas-shard-registry-entry",
            shard_registry_authority(shard)?,
        )?,
        non_file(
            ActionNonFileKind::Declaration,
            "canvas-build-proof",
            Digest::compute(Domain::GateActionKey, build_key.as_str().as_bytes()),
        )?,
    ]);
    let input_manifest = arm
        .compile_explicit_action_input(plan.shard_inputs().to_vec(), non_files)
        .map_err(ProveFailure::run)?;
    let action = shard.action_id();
    prepared_canvas_action(
        action.clone(),
        SHARD_DECLARATION,
        vec![
            "node".to_owned(),
            "app/browser/canvas.spec.mjs".to_owned(),
            "--spice-shard".to_owned(),
            shard.name().to_owned(),
        ],
        input_manifest,
        context.policy(),
        gate_set,
        tools_digest,
        environment,
        environment_names,
        tools,
        CanvasExecution {
            root: arm.root().to_path_buf(),
            common: common.to_path_buf(),
            environment: environment_values.to_vec(),
            build_inputs: present_inputs(plan.build_inputs(), arm.paths()),
            shard_inputs: present_inputs_except(
                plan.shard_inputs(),
                plan.build_inputs(),
                arm.paths(),
            ),
            shard: Some(shard.clone()),
            build,
            playwright,
        },
    )
}

fn require_reusable_build(plan: &CanvasActionPlan) -> Result<(), ProveFailure> {
    if let Some(reason) = plan.build_non_cacheable_reason() {
        return Err(ProveFailure::refusal(
            "gate::prove-canvas-build-not-reusable",
            format!("Canvas build has no complete reusable source authority: {reason}"),
        ));
    }
    Ok(())
}

fn present_inputs(inputs: &[String], selected_paths: &[String]) -> Vec<String> {
    let selected = selected_paths.iter().collect::<BTreeSet<_>>();
    inputs
        .iter()
        .filter(|path| selected.contains(path))
        .cloned()
        .collect()
}

fn present_inputs_except(
    inputs: &[String],
    excluded: &[String],
    selected_paths: &[String],
) -> Vec<String> {
    let excluded = excluded.iter().collect::<BTreeSet<_>>();
    present_inputs(inputs, selected_paths)
        .into_iter()
        .filter(|path| !excluded.contains(path))
        .collect()
}

fn shard_registry_authority(shard: &CanvasShard) -> Result<Digest, ProveFailure> {
    digest_encoded(Domain::GateInventory, shard)
}

#[allow(clippy::too_many_arguments)]
fn prepared_canvas_action(
    action: String,
    declaration: &str,
    argv: Vec<String>,
    input_manifest: gate::ActionInputManifest,
    policy: &Digest,
    gate_set: &Digest,
    tools_digest: &Digest,
    environment: &Digest,
    environment_names: &[String],
    tools: &[ActionToolRequirement],
    execution: CanvasExecution,
) -> Result<PreparedAction, ProveFailure> {
    let input_manifest = input_manifest
        .digest()
        .map_err(ProveFailure::action_input)?;
    let spec = ActionSpec::new(ActionSpecFields {
        action_id: action.clone(),
        runner: "spice-canvas-proof-action".to_owned(),
        runner_version: CANVAS_RUNNER_VERSION,
        declaration: declaration.to_owned(),
        argv,
        cwd: ".".to_owned(),
        environment_names: environment_names.to_vec(),
        tools: tools.to_vec(),
        input_strategy: "canvas-registry-closure-v2".to_owned(),
        reuse: ActionReuseClass::Cacheable,
    })
    .map_err(ProveFailure::action_input)?;
    let action_spec = spec.digest().map_err(ProveFailure::action_input)?;
    let action_key = gate::derive_authority_bound_action_key(
        &spec,
        &ActionKeyAuthority {
            input_manifest: input_manifest.clone(),
            policy: policy.clone(),
            gate_set: gate_set.clone(),
            tools: tools_digest.clone(),
            environment: environment.clone(),
            baseline: None,
        },
    )
    .map_err(ProveFailure::action_input)?;
    Ok(PreparedAction {
        action,
        action_key,
        action_spec,
        input_manifest,
        execution: PreparedExecution::Canvas(Box::new(execution)),
    })
}

fn canvas_non_files(
    gate_set: &Digest,
    policy: &Digest,
    tools: &Digest,
    environment: &Digest,
    declaration: Digest,
) -> Result<Vec<ActionNonFileInput>, ProveFailure> {
    Ok(vec![
        non_file(
            ActionNonFileKind::Declaration,
            "compiled-declaration",
            declaration,
        )?,
        non_file(
            ActionNonFileKind::GateSet,
            "canonical-gate-set",
            gate_set.clone(),
        )?,
        non_file(ActionNonFileKind::Policy, "compiled-policy", policy.clone())?,
        non_file(ActionNonFileKind::Toolchain, "canvas-tools", tools.clone())?,
        non_file(
            ActionNonFileKind::Environment,
            "canvas-environment",
            environment.clone(),
        )?,
    ])
}

fn normalized_environment(
    values: impl Iterator<Item = (OsString, OsString)>,
) -> Vec<(OsString, OsString)> {
    const OMIT: [&str; 11] = [
        "PWD",
        "OLDPWD",
        "SHLVL",
        "_",
        "TERM_SESSION_ID",
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_BUILD_DIR",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTC_WRAPPER",
        CANVAS_BINARY_VARIABLE,
        CANVAS_SHARD_VARIABLE,
    ];
    let mut values = values
        .filter(|(name, _)| !OMIT.iter().any(|omit| name == OsStr::new(omit)))
        .collect::<Vec<_>>();
    for name in [
        "CARGO_HOME",
        "HOME",
        "PATH",
        "RUSTUP_HOME",
        "SHELL",
        spice::cargo_isolation::BOOTSTRAP_HOME_VARIABLE,
        spice::cargo_isolation::NATIVE_CARGO_VARIABLE,
        spice::cargo_isolation::NATIVE_RUSTC_VARIABLE,
        spice::cargo_isolation::NATIVE_RUSTDOC_VARIABLE,
        wire::SpiceVariable::NativeShell.as_str(),
        wire::SpiceVariable::RtkExecutable.as_str(),
        "TMPDIR",
    ] {
        if !values
            .iter()
            .any(|(observed, _)| observed == OsStr::new(name))
            && let Some(value) = ambient_variable(name)
        {
            values.push((OsString::from(name), value));
        }
    }
    values.sort();
    values.extend([
        (OsString::from("RUSTC_WORKSPACE_WRAPPER"), OsString::new()),
        (OsString::from("RUSTC_WRAPPER"), OsString::new()),
    ]);
    values.sort();
    values
}

#[expect(
    clippy::disallowed_methods,
    reason = "the proof command rim binds these three Canvas process inputs into the action key"
)]
fn ambient_variable(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

fn runtime_authority(
    environment: &[(OsString, OsString)],
    playwright: &PlaywrightSnapshot,
) -> Result<RuntimeAuthority, ProveFailure> {
    let probe_workspace = CanvasWorkspace(exclusive_workspace("spice-canvas-tool-probe")?);
    fs::create_dir(&probe_workspace.0)
        .map_err(|error| ProveFailure::fault("gate::prove-canvas-workspace-unavailable", error))?;
    playwright.materialize(&probe_workspace.0)?;
    let output = command("node", environment)
        .args(["--input-type=module", "--eval", NODE_PROBE])
        .current_dir(&probe_workspace.0)
        .output()
        .map_err(|error| ProveFailure::fault("gate::prove-canvas-tool-unavailable", error))?;
    if !output.status.success() {
        return Err(ProveFailure::refusal(
            "gate::prove-canvas-tool-refused",
            String::from_utf8_lossy(&output.stderr),
        ));
    }
    let probe: RuntimeProbe = serde_json::from_slice(&output.stdout)
        .map_err(|error| ProveFailure::fault("gate::prove-canvas-tool-invalid", error))?;
    let node_path = command("node", environment)
        .args(["--print", "process.execPath"])
        .current_dir(&probe_workspace.0)
        .output()
        .map_err(|error| ProveFailure::fault("gate::prove-canvas-tool-unavailable", error))?;
    if !node_path.status.success() {
        return Err(ProveFailure::refusal(
            "gate::prove-canvas-tool-refused",
            process_failure(&node_path),
        ));
    }
    let node_path = String::from_utf8(node_path.stdout)
        .map_err(|error| ProveFailure::fault("gate::prove-canvas-tool-invalid", error))?;
    let node = fs::read(node_path.trim())
        .map_err(|error| ProveFailure::fault("gate::prove-canvas-tool-unreadable", error))?;
    let chromium = fs::read(&probe.chromium)
        .map_err(|error| ProveFailure::fault("gate::prove-canvas-tool-unreadable", error))?;
    let firefox = fs::read(&probe.firefox)
        .map_err(|error| ProveFailure::fault("gate::prove-canvas-tool-unreadable", error))?;
    let git = fs::read("/usr/bin/git")
        .map_err(|error| ProveFailure::fault("gate::prove-canvas-tool-unreadable", error))?;
    let git_version = Command::new("/usr/bin/git")
        .arg("--version")
        .output()
        .map_err(|error| ProveFailure::fault("gate::prove-canvas-tool-unavailable", error))?;
    if !git_version.status.success() {
        return Err(ProveFailure::refusal(
            "gate::prove-canvas-tool-refused",
            process_failure(&git_version),
        ));
    }
    Ok(RuntimeAuthority {
        git_version: String::from_utf8(git_version.stdout)
            .map_err(|error| ProveFailure::fault("gate::prove-canvas-tool-invalid", error))?
            .trim()
            .to_owned(),
        node_version: probe.node,
        playwright_version: probe.playwright,
        git_executable: Digest::compute(Domain::GateTool, &git),
        node_executable: Digest::compute(Domain::GateTool, &node),
        chromium_executable: Digest::compute(Domain::GateTool, &chromium),
        firefox_executable: Digest::compute(Domain::GateTool, &firefox),
        playwright_packages: playwright.authority.clone(),
    })
}

pub(super) fn execute(execution: CanvasExecution, action: &str) -> Result<Digest, ProveFailure> {
    let build = ensure_build(&execution)?;
    if let Some(shard) = execution.shard {
        copy_paths(&execution.root, &build.root, &execution.shard_inputs)?;
        let encoded_shard = serde_json::to_string(&shard)
            .map_err(|error| ProveFailure::fault("gate::prove-canvas-shard-unencodable", error))?;
        let mut environment = execution
            .environment
            .iter()
            .filter(|(name, _)| {
                name != OsStr::new(CANVAS_BINARY_VARIABLE)
                    && name != OsStr::new(CANVAS_SHARD_VARIABLE)
            })
            .cloned()
            .collect::<Vec<_>>();
        environment.push((
            OsString::from(CANVAS_BINARY_VARIABLE),
            build.binary.as_os_str().to_owned(),
        ));
        environment.push((
            OsString::from(CANVAS_SHARD_VARIABLE),
            OsString::from(encoded_shard),
        ));
        let environment = sill::Environment::new(environment).map_err(|error| {
            ProveFailure::fault("gate::prove-canvas-shard-environment-invalid", error)
        })?;
        process::run(
            shard.name(),
            OsString::from("node"),
            [
                OsString::from("app/browser/canvas.spec.mjs"),
                OsString::from("--spice-shard"),
                OsString::from(shard.name()),
            ],
            &build.root,
            environment,
            shard.timeout_millis(),
        )?;
    }
    let result = CanvasResult {
        action,
        completion: "passed",
    };
    digest_encoded(Domain::GateFocusedReport, &result)
}

fn ensure_build(execution: &CanvasExecution) -> Result<Arc<CanvasBuild>, ProveFailure> {
    let mut shared = execution
        .build
        .lock()
        .map_err(|error| ProveFailure::fault("gate::prove-canvas-build-state-poisoned", error))?;
    if let Some(result) = shared.as_ref() {
        return result
            .clone()
            .map_err(|detail| ProveFailure::refusal("gate::prove-canvas-build-refused", detail));
    }
    let result = run_build(execution).map_err(|failure| match failure {
        ProveFailure::Refusal { detail, .. } | ProveFailure::Fault { detail, .. } => detail,
    });
    *shared = Some(result.clone());
    result.map_err(|detail| ProveFailure::refusal("gate::prove-canvas-build-refused", detail))
}

fn run_build(execution: &CanvasExecution) -> Result<Arc<CanvasBuild>, ProveFailure> {
    let workspace = CanvasWorkspace(exclusive_workspace("spice-canvas-proof")?);
    fs::create_dir(&workspace.0)
        .map_err(|error| ProveFailure::fault("gate::prove-canvas-workspace-unavailable", error))?;
    let root = workspace.0.join("root");
    copy_paths(&execution.root, &root, &execution.build_inputs)?;
    initialize_git_index(&root)?;
    execution.playwright.materialize(&root)?;
    let output = command("node", &execution.environment)
        .current_dir(&root)
        .args([
            OsStr::new("app/browser/canvas-proof-build.mjs"),
            execution.common.as_os_str(),
        ])
        .env_remove(CANVAS_BINARY_VARIABLE)
        .output()
        .map_err(|error| ProveFailure::fault("gate::prove-canvas-build-unavailable", error))?;
    if !output.status.success() {
        return Err(ProveFailure::refusal(
            "gate::prove-canvas-build-refused",
            process_failure(&output),
        ));
    }
    let binary = root.join("target/debug/spice");
    let app = root.join("dist/stage-app/current");
    if !binary.is_file() || !app.is_file() {
        return Err(ProveFailure::refusal(
            "gate::prove-canvas-build-incomplete",
            format!(
                "Canvas build outputs are incomplete: resident={} app={}",
                binary.is_file(),
                app.is_file(),
            ),
        ));
    }
    Ok(Arc::new(CanvasBuild {
        _workspace: workspace,
        root,
        binary,
    }))
}

fn exclusive_workspace(prefix: &str) -> Result<PathBuf, ProveFailure> {
    static NEXT_WORKSPACE: AtomicU64 = AtomicU64::new(0);
    for _ in 0..100 {
        let root = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed),
        ));
        match fs::symlink_metadata(&root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(root),
            Ok(_) => {}
            Err(error) => {
                return Err(ProveFailure::fault(
                    "gate::prove-canvas-workspace-unavailable",
                    error,
                ));
            }
        }
    }
    Err(ProveFailure::fault(
        "gate::prove-canvas-workspace-unavailable",
        "no exclusive temporary workspace name was available",
    ))
}

fn initialize_git_index(root: &Path) -> Result<(), ProveFailure> {
    for arguments in [["init", "--quiet"].as_slice(), ["add", "--all"].as_slice()] {
        let output = Command::new("/usr/bin/git")
            .args(arguments)
            .current_dir(root)
            .output()
            .map_err(|error| ProveFailure::fault("gate::prove-canvas-git-unavailable", error))?;
        if !output.status.success() {
            return Err(ProveFailure::refusal(
                "gate::prove-canvas-git-refused",
                process_failure(&output),
            ));
        }
    }
    Ok(())
}

fn copy_paths(source: &Path, destination: &Path, paths: &[String]) -> Result<(), ProveFailure> {
    fs::create_dir_all(destination)
        .map_err(|error| ProveFailure::fault("gate::prove-canvas-workspace-unavailable", error))?;
    for relative in paths {
        let source = source.join(relative);
        let metadata = fs::symlink_metadata(&source)
            .map_err(|error| ProveFailure::fault("gate::prove-canvas-source-unreadable", error))?;
        if !metadata.file_type().is_file() {
            return Err(ProveFailure::refusal(
                "gate::prove-canvas-source-refused",
                format!(
                    "selected Canvas input {} is not a regular file",
                    source.display()
                ),
            ));
        }
        let destination = destination.join(relative);
        let parent = destination.parent().ok_or_else(|| {
            ProveFailure::refusal(
                "gate::prove-canvas-source-refused",
                format!("selected Canvas input {relative:?} has no parent"),
            )
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            ProveFailure::fault("gate::prove-canvas-workspace-unavailable", error)
        })?;
        fs::copy(&source, &destination).map_err(|error| {
            ProveFailure::fault("gate::prove-canvas-workspace-unavailable", error)
        })?;
    }
    Ok(())
}

fn command(program: &str, environment: &[(OsString, OsString)]) -> Command {
    let mut command = Command::new(program);
    command.env_clear().envs(environment.iter().cloned());
    command
}

fn process_failure(output: &std::process::Output) -> String {
    format!(
        "status={} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    fn success<T>(result: Result<T, ProveFailure>) -> T {
        match result {
            Ok(value) => value,
            Err(ProveFailure::Refusal { reason, detail })
            | Err(ProveFailure::Fault { reason, detail }) => {
                panic!("unexpected failure {reason}: {detail}")
            }
        }
    }

    #[test]
    fn registry_timeout_changes_the_shard_contract_authority() {
        let first: CanvasShard = serde_json::from_value(serde_json::json!({
            "name": "fixture",
            "timeout_ms": 10,
            "scenarios": ["one"]
        }))
        .expect("decode first shard");
        let changed: CanvasShard = serde_json::from_value(serde_json::json!({
            "name": "fixture",
            "timeout_ms": 11,
            "scenarios": ["one"]
        }))
        .expect("decode changed shard");

        let first = success(shard_registry_authority(&first));
        let changed = success(shard_registry_authority(&changed));
        assert_ne!(first, changed);

        let spec = ActionSpec::new(ActionSpecFields {
            action_id: "canvas:shard:fixture".to_owned(),
            runner: "spice-canvas-proof-action".to_owned(),
            runner_version: CANVAS_RUNNER_VERSION,
            declaration: SHARD_DECLARATION.to_owned(),
            argv: vec!["node".to_owned(), "fixture.mjs".to_owned()],
            cwd: ".".to_owned(),
            environment_names: Vec::new(),
            tools: Vec::new(),
            input_strategy: "canvas-registry-closure-v2".to_owned(),
            reuse: ActionReuseClass::Cacheable,
        })
        .expect("valid Canvas action contract");
        let key = |input_manifest| {
            gate::derive_authority_bound_action_key(
                &spec,
                &ActionKeyAuthority {
                    input_manifest,
                    policy: Digest::compute(Domain::GatePolicy, b"policy"),
                    gate_set: Digest::compute(Domain::GateSet, b"gate-set"),
                    tools: Digest::compute(Domain::GateTool, b"tools"),
                    environment: Digest::compute(Domain::GateInventory, b"environment"),
                    baseline: None,
                },
            )
            .expect("derive Canvas action key")
        };
        assert_ne!(key(first), key(changed));
        assert_eq!(CANVAS_RUNNER_VERSION, 3);
    }

    #[test]
    fn canvas_requests_are_exact() {
        assert!(registered("canvas"));
        assert!(registered("canvas:build"));
        assert!(registered("canvas:shard:interaction"));
        assert!(!registered("canvas-shard:interaction"));
        assert!(!registered("canvas:unknown"));
        assert!(!request(Request {
            action: Some("formatters"),
            missing: true,
        }));
    }

    #[test]
    fn runtime_output_paths_do_not_enter_environment_authority() {
        let values = normalized_environment(
            [
                (OsString::from("PWD"), OsString::from("/first")),
                (OsString::from("HOME"), OsString::from("/home")),
                (
                    OsString::from(CANVAS_BINARY_VARIABLE),
                    OsString::from("/ambient/spice"),
                ),
                (
                    OsString::from(CANVAS_SHARD_VARIABLE),
                    OsString::from("ambient-shard"),
                ),
            ]
            .into_iter(),
        );
        assert!(values.contains(&(OsString::from("HOME"), OsString::from("/home"))));
        assert!(values.iter().any(|(name, _)| name == OsStr::new("PATH")));
        assert!(!values.iter().any(|(name, _)| name == OsStr::new("PWD")));
        assert!(
            !values
                .iter()
                .any(|(name, _)| name == OsStr::new(CANVAS_BINARY_VARIABLE))
        );
        assert!(
            !values
                .iter()
                .any(|(name, _)| name == OsStr::new(CANVAS_SHARD_VARIABLE))
        );
        assert!(values.contains(&(OsString::from("RUSTC_WORKSPACE_WRAPPER"), OsString::new(),)));
    }

    #[test]
    fn canvas_workspace_materializes_only_admitted_inputs_across_build_and_shard_overlays() {
        let source = CanvasWorkspace(success(exclusive_workspace("canvas-copy-source")));
        let destination = CanvasWorkspace(success(exclusive_workspace("canvas-copy-destination")));
        fs::create_dir(&source.0).expect("create source workspace");
        fs::write(source.0.join("kept.txt"), "kept\n").expect("write admitted source");
        fs::write(source.0.join("shard.mjs"), "shard\n").expect("write admitted shard source");
        fs::write(source.0.join("browser-only.mjs"), "ignored\n").expect("write unadmitted source");

        success(copy_paths(
            &source.0,
            &destination.0,
            &["kept.txt".to_owned()],
        ));
        success(copy_paths(
            &source.0,
            &destination.0,
            &["shard.mjs".to_owned()],
        ));

        assert_eq!(
            fs::read_to_string(destination.0.join("kept.txt")).expect("read admitted copy"),
            "kept\n"
        );
        assert_eq!(
            fs::read_to_string(destination.0.join("shard.mjs")).expect("read admitted shard copy"),
            "shard\n"
        );
        assert!(!destination.0.join("browser-only.mjs").exists());
    }

    #[test]
    fn canvas_workspace_omits_deleted_manifest_inputs() {
        assert_eq!(
            present_inputs(
                &["present.txt".to_owned(), "removed.txt".to_owned()],
                &["present.txt".to_owned()],
            ),
            ["present.txt"]
        );
        assert_eq!(
            present_inputs_except(
                &["build.mjs".to_owned(), "shard.mjs".to_owned()],
                &["build.mjs".to_owned()],
                &["build.mjs".to_owned(), "shard.mjs".to_owned()],
            ),
            ["shard.mjs"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn canvas_build_process_reads_only_the_private_selected_root() {
        let selected = CanvasWorkspace(success(exclusive_workspace("canvas-selected-source")));
        let checkout = CanvasWorkspace(success(exclusive_workspace("canvas-live-checkout")));
        let tools = CanvasWorkspace(success(exclusive_workspace("canvas-build-tools")));
        for root in [&selected.0, &checkout.0] {
            fs::create_dir_all(root.join(".cargo")).expect("create Cargo configuration directory");
        }
        fs::create_dir_all(selected.0.join("app/browser"))
            .expect("create selected launcher directory");
        fs::create_dir_all(checkout.0.join(".git")).expect("create checkout Git directory");
        fs::create_dir(&tools.0).expect("create tool directory");
        fs::write(selected.0.join("Cargo.lock"), "selected-lock\n")
            .expect("write selected lockfile");
        fs::write(
            selected.0.join(".cargo/config.toml"),
            "selected-configuration\n",
        )
        .expect("write selected Cargo configuration");
        fs::write(
            selected.0.join("app/browser/canvas-proof-build.mjs"),
            "// selected launcher\n",
        )
        .expect("write selected launcher");
        fs::write(checkout.0.join("Cargo.lock"), "initial-live-lock\n")
            .expect("write live lockfile");
        fs::write(
            checkout.0.join(".cargo/config.toml"),
            "initial-live-configuration\n",
        )
        .expect("write live Cargo configuration");

        let node = tools.0.join("node");
        fs::write(
            &node,
            concat!(
                "#!/bin/sh\n",
                "{\n",
                "  /bin/pwd\n",
                "  for argument in \"$@\"; do /usr/bin/printf 'arg=%s\\n' \"$argument\"; done\n",
                "  /usr/bin/printf 'lock='; /bin/cat Cargo.lock\n",
                "  /usr/bin/printf 'config='; /bin/cat .cargo/config.toml\n",
                "} > canvas-test-ledger\n",
                "/bin/mkdir -p target/debug dist/stage-app\n",
                "/usr/bin/touch target/debug/spice dist/stage-app/current\n",
            ),
        )
        .expect("write test Node executable");
        fs::set_permissions(&node, fs::Permissions::from_mode(0o700))
            .expect("make test Node executable");

        let make_execution = || CanvasExecution {
            root: selected.0.clone(),
            common: checkout.0.join(".git"),
            environment: vec![(OsString::from("PATH"), tools.0.as_os_str().to_owned())],
            build_inputs: vec![
                ".cargo/config.toml".to_owned(),
                "Cargo.lock".to_owned(),
                "app/browser/canvas-proof-build.mjs".to_owned(),
            ],
            shard_inputs: Vec::new(),
            shard: None,
            build: Arc::new(Mutex::new(None)),
            playwright: Arc::new(PlaywrightSnapshot::empty()),
        };
        let first = make_execution();
        let first_result = success(execute(first.clone(), gate::CANVAS_BUILD_ACTION));
        let first_ledger = build_ledger(&first);

        fs::write(checkout.0.join("Cargo.lock"), "mutated-live-lock\n")
            .expect("mutate live lockfile");
        fs::write(
            checkout.0.join(".cargo/config.toml"),
            "mutated-live-configuration\n",
        )
        .expect("mutate live Cargo configuration");
        let second = make_execution();
        let second_result = success(execute(second.clone(), gate::CANVAS_BUILD_ACTION));
        let second_ledger = build_ledger(&second);

        assert_eq!(first_result, second_result);
        for ledger in [&first_ledger, &second_ledger] {
            assert!(ledger.contains("arg=app/browser/canvas-proof-build.mjs\n"));
            assert!(ledger.contains(&format!("arg={}\n", checkout.0.join(".git").display())));
            assert!(ledger.contains("lock=selected-lock\n"));
            assert!(ledger.contains("config=selected-configuration\n"));
            assert!(!ledger.contains("live-lock"));
            assert!(!ledger.contains("live-configuration"));
            assert!(!ledger.contains(&format!("arg={}\n", checkout.0.display())));
        }
        for root in [&selected.0, &checkout.0] {
            assert!(!root.join("target").exists());
            assert!(!root.join("dist").exists());
        }
    }

    fn build_ledger(execution: &CanvasExecution) -> String {
        let shared = execution.build.lock().expect("lock Canvas build state");
        let build = shared
            .as_ref()
            .expect("Canvas build ran")
            .as_ref()
            .expect("Canvas build succeeded");
        fs::read_to_string(build.root.join("canvas-test-ledger")).expect("read Canvas build ledger")
    }
}
