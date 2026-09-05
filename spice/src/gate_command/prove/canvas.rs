//! Canvas build and browser-shard proof actions.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use gate::{CanvasActionPlan, CanvasShard};
use serde::{Deserialize, Serialize};

use super::*;

const BUILD_DECLARATION: &str = "gate::canvas::build-action-v1";
const SHARD_DECLARATION: &str = "gate::canvas::browser-shard-action-v1";
const CANVAS_BINARY_VARIABLE: &str = "SPICE_CANVAS_BINARY"; // env-policy: allow
const NODE_PROBE: &str = concat!(
    "const {createRequire}=await import('node:module');",
    "const require=createRequire(import.meta.url);",
    "const {chromium,firefox}=await import('playwright');",
    "console.log(JSON.stringify({node:process.version,playwright:require('playwright/package.json').version,",
    "chromium:chromium.executablePath(),firefox:firefox.executablePath()}));",
);

type SharedCanvasBuild = Arc<Mutex<Option<Result<Arc<CanvasBuild>, String>>>>;

#[derive(Clone)]
pub(super) struct CanvasExecution {
    root: PathBuf,
    repository: PathBuf,
    common: PathBuf,
    environment: Vec<(OsString, OsString)>,
    shard: Option<CanvasShard>,
    build: SharedCanvasBuild,
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
    let plan = CanvasActionPlan::load(arm.root(), arm.paths(), arm.changes())
        .map_err(ProveFailure::canvas)?;
    let environment_values = normalized_environment(arm.compiler_environment().iter().cloned());
    let (environment_names, environment) = environment(&environment_values)?;
    let runtime = runtime_authority(repository, &environment_values)?;
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
        repository,
        common,
        &environment_values,
        Arc::clone(&build),
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
                repository,
                common,
                &environment_values,
                Arc::clone(&build),
                &plan,
                shard,
                &build_key,
            )?);
        }
    }
    Ok(actions)
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
    repository: &Path,
    common: &Path,
    environment_values: &[(OsString, OsString)],
    build: SharedCanvasBuild,
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
            repository: repository.to_path_buf(),
            common: common.to_path_buf(),
            environment: environment_values.to_vec(),
            shard: None,
            build,
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
    repository: &Path,
    common: &Path,
    environment_values: &[(OsString, OsString)],
    build: SharedCanvasBuild,
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
            digest_encoded(Domain::GateInventory, shard)?,
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
            repository: repository.to_path_buf(),
            common: common.to_path_buf(),
            environment: environment_values.to_vec(),
            shard: Some(shard.clone()),
            build,
        },
    )
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
        runner_version: RUNNER_VERSION,
        declaration: declaration.to_owned(),
        argv,
        cwd: ".".to_owned(),
        environment_names: environment_names.to_vec(),
        tools: tools.to_vec(),
        input_strategy: "canvas-registry-closure-v1".to_owned(),
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
    const OMIT: [&str; 9] = [
        "PWD",
        "OLDPWD",
        "SHLVL",
        "_",
        "TERM_SESSION_ID",
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_BUILD_DIR",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTC_WRAPPER",
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
    root: &Path,
    environment: &[(OsString, OsString)],
) -> Result<RuntimeAuthority, ProveFailure> {
    let output = command("node", environment)
        .args(["--input-type=module", "--eval", NODE_PROBE])
        .current_dir(root)
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
        .current_dir(root)
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
    })
}

pub(super) fn execute(execution: CanvasExecution, action: &str) -> Result<Digest, ProveFailure> {
    let build = ensure_build(&execution)?;
    if let Some(shard) = execution.shard {
        let mut process = command("node", &execution.environment);
        process
            .current_dir(&build.root)
            .args(["app/browser/canvas.spec.mjs", "--spice-shard", shard.name()])
            .env(CANVAS_BINARY_VARIABLE, &build.binary);
        let output = process
            .output()
            .map_err(|error| ProveFailure::fault("gate::prove-canvas-shard-unavailable", error))?;
        if !output.status.success() {
            return Err(ProveFailure::refusal(
                "gate::prove-canvas-shard-refused",
                process_failure(&output),
            ));
        }
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
    static NEXT_WORKSPACE: AtomicU64 = AtomicU64::new(0);
    let workspace = CanvasWorkspace(std::env::temp_dir().join(format!(
        "spice-canvas-proof-{}-{}",
        std::process::id(),
        NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed),
    )));
    fs::create_dir(&workspace.0)
        .map_err(|error| ProveFailure::fault("gate::prove-canvas-workspace-unavailable", error))?;
    let root = workspace.0.join("root");
    copy_tree(&execution.root, &root)?;
    initialize_git_index(&root)?;
    let modules = root.join("node_modules");
    if !modules.exists() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(execution.repository.join("node_modules"), &modules).map_err(
            |error| ProveFailure::fault("gate::prove-canvas-modules-unavailable", error),
        )?;
    }
    let output = command("node", &execution.environment)
        .current_dir(&root)
        .args([
            OsStr::new("app/browser/canvas-proof-build.mjs"),
            execution.common.as_os_str(),
            execution.repository.as_os_str(),
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

fn copy_tree(source: &Path, destination: &Path) -> Result<(), ProveFailure> {
    fs::create_dir(destination)
        .map_err(|error| ProveFailure::fault("gate::prove-canvas-workspace-unavailable", error))?;
    let mut entries = fs::read_dir(source)
        .map_err(|error| ProveFailure::fault("gate::prove-canvas-source-unreadable", error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ProveFailure::fault("gate::prove-canvas-source-unreadable", error))?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let source = entry.path();
        let destination = destination.join(entry.file_name());
        let kind = entry
            .file_type()
            .map_err(|error| ProveFailure::fault("gate::prove-canvas-source-unreadable", error))?;
        if kind.is_dir() {
            copy_tree(&source, &destination)?;
        } else if kind.is_file() {
            fs::copy(&source, &destination).map_err(|error| {
                ProveFailure::fault("gate::prove-canvas-workspace-unavailable", error)
            })?;
        } else {
            return Err(ProveFailure::refusal(
                "gate::prove-canvas-source-refused",
                format!(
                    "selected Canvas input {} is not a regular file",
                    source.display()
                ),
            ));
        }
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
    use super::*;

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
            ]
            .into_iter(),
        );
        assert!(values.contains(&(OsString::from("HOME"), OsString::from("/home"))));
        assert!(values.iter().any(|(name, _)| name == OsStr::new("PATH")));
        assert!(!values.iter().any(|(name, _)| name == OsStr::new("PWD")));
        assert!(values.contains(&(OsString::from("RUSTC_WORKSPACE_WRAPPER"), OsString::new(),)));
    }
}
