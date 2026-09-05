//! Canvas build and browser-shard proof actions.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
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
const PLAYWRIGHT_PACKAGE: &str = "playwright";
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

#[derive(Deserialize)]
struct PackageLock {
    #[serde(rename = "lockfileVersion")]
    lockfile_version: u64,
    packages: BTreeMap<String, LockedPackage>,
}

#[derive(Deserialize)]
struct LockedPackage {
    version: Option<String>,
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "optionalDependencies")]
    optional_dependencies: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct InstalledPackage {
    name: String,
    version: String,
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "optionalDependencies")]
    optional_dependencies: BTreeMap<String, String>,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct PackageSnapshotAuthority<'a> {
    packages: &'a [SnapshotPackage],
    files: Vec<SnapshotFileAuthority<'a>>,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct SnapshotFileAuthority<'a> {
    path: &'a str,
    mode: u32,
    content: Digest,
}

#[derive(Clone, Serialize)]
#[serde(deny_unknown_fields)]
struct SnapshotPackage {
    path: String,
    name: String,
    version: String,
}

struct SnapshotFile {
    path: String,
    mode: u32,
    bytes: Vec<u8>,
}

struct PlaywrightSnapshot {
    files: Vec<SnapshotFile>,
    authority: Digest,
}

impl PlaywrightSnapshot {
    fn capture(selected_root: &Path, installed_root: &Path) -> Result<Self, ProveFailure> {
        let first = Self::capture_once(selected_root, installed_root)?;
        let second = Self::capture_once(selected_root, installed_root)?;
        if first.authority != second.authority {
            return Err(package_refusal(
                "installed Playwright package sources moved while they were being captured",
            ));
        }
        Ok(second)
    }

    fn capture_once(selected_root: &Path, installed_root: &Path) -> Result<Self, ProveFailure> {
        let lock_path = selected_root.join("package-lock.json");
        require_regular_file(&lock_path, "selected package lock")?;
        let lock_bytes = fs::read(&lock_path).map_err(|error| {
            package_refusal(format!("cannot read {}: {error}", lock_path.display()))
        })?;
        let lock: PackageLock = serde_json::from_slice(&lock_bytes).map_err(|error| {
            package_refusal(format!("cannot decode {}: {error}", lock_path.display()))
        })?;
        if lock.lockfile_version != 3 {
            return Err(package_refusal(format!(
                "{} has unsupported lockfileVersion {}; expected 3",
                lock_path.display(),
                lock.lockfile_version,
            )));
        }

        let modules = installed_root.join("node_modules");
        require_directory(&modules, "installed package root")?;
        let mut pending = VecDeque::from([PLAYWRIGHT_PACKAGE.to_owned()]);
        let mut visited = BTreeSet::new();
        let mut packages = Vec::new();
        while let Some(name) = pending.pop_front() {
            validate_package_name(&name)?;
            if !visited.insert(name.clone()) {
                continue;
            }
            let relative = PathBuf::from("node_modules").join(&name);
            let spelling = relative_path_spelling(&relative)?;
            let locked = lock.packages.get(&spelling).ok_or_else(|| {
                package_refusal(format!("package lock is missing closure entry {spelling}"))
            })?;
            let locked_version = locked.version.as_deref().ok_or_else(|| {
                package_refusal(format!("package lock entry {spelling} has no version"))
            })?;
            let installed_path = installed_root.join(&relative);
            require_directory(&installed_path, "installed package")?;
            let package_json = installed_path.join("package.json");
            require_regular_file(&package_json, "installed package identity")?;
            let identity: InstalledPackage =
                serde_json::from_slice(&fs::read(&package_json).map_err(|error| {
                    package_refusal(format!("cannot read {}: {error}", package_json.display()))
                })?)
                .map_err(|error| {
                    package_refusal(format!("cannot decode {}: {error}", package_json.display()))
                })?;
            if identity.name != name || identity.version != locked_version {
                return Err(package_refusal(format!(
                    "installed package {spelling} identifies as {}@{}, but the lock requires {name}@{locked_version}",
                    identity.name, identity.version,
                )));
            }
            if identity.dependencies != locked.dependencies
                || identity.optional_dependencies != locked.optional_dependencies
            {
                return Err(package_refusal(format!(
                    "installed package {spelling} dependency declarations disagree with the package lock",
                )));
            }
            packages.push(SnapshotPackage {
                path: spelling,
                name: identity.name,
                version: identity.version,
            });
            pending.extend(locked.dependencies.keys().cloned());
            for optional in locked.optional_dependencies.keys() {
                validate_package_name(optional)?;
                let optional_path = modules.join(optional);
                match fs::symlink_metadata(&optional_path) {
                    Ok(_) => pending.push_back(optional.clone()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(package_refusal(format!(
                            "cannot inspect optional package {}: {error}",
                            optional_path.display(),
                        )));
                    }
                }
            }
        }
        packages.sort_by(|left, right| left.path.cmp(&right.path));

        let mut files = Vec::new();
        for package in &packages {
            collect_snapshot_files(
                installed_root,
                &installed_root.join(&package.path),
                &mut files,
            )?;
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        if files.windows(2).any(|pair| pair[0].path == pair[1].path) {
            return Err(package_refusal(
                "installed Playwright closure contains duplicate file paths",
            ));
        }
        let authority = PackageSnapshotAuthority {
            packages: &packages,
            files: files
                .iter()
                .map(|file| SnapshotFileAuthority {
                    path: &file.path,
                    mode: file.mode,
                    content: Digest::compute(Domain::GateTool, &file.bytes),
                })
                .collect(),
        };
        Ok(Self {
            authority: digest_encoded(Domain::GateTool, &authority)?,
            files,
        })
    }

    fn materialize(&self, root: &Path) -> Result<(), ProveFailure> {
        let modules = root.join("node_modules");
        match fs::symlink_metadata(&modules) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(package_refusal(format!(
                    "snapshot destination already exists: {}",
                    modules.display(),
                )));
            }
            Err(error) => {
                return Err(package_refusal(format!(
                    "cannot inspect snapshot destination {}: {error}",
                    modules.display(),
                )));
            }
        }
        for file in &self.files {
            let destination = root.join(&file.path);
            let parent = destination.parent().ok_or_else(|| {
                package_refusal(format!("snapshot file has no parent: {}", file.path))
            })?;
            fs::create_dir_all(parent).map_err(|error| {
                package_refusal(format!("cannot create {}: {error}", parent.display()))
            })?;
            fs::write(&destination, &file.bytes).map_err(|error| {
                package_refusal(format!("cannot write {}: {error}", destination.display()))
            })?;
            set_snapshot_mode(&destination, file.mode)?;
        }
        Ok(())
    }
}

fn collect_snapshot_files(
    installed_root: &Path,
    directory: &Path,
    files: &mut Vec<SnapshotFile>,
) -> Result<(), ProveFailure> {
    let mut entries = fs::read_dir(directory)
        .map_err(|error| package_refusal(format!("cannot read {}: {error}", directory.display())))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            package_refusal(format!("cannot read {}: {error}", directory.display()))
        })?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            package_refusal(format!("cannot inspect {}: {error}", path.display()))
        })?;
        if metadata.file_type().is_dir() {
            collect_snapshot_files(installed_root, &path, files)?;
            continue;
        }
        if !metadata.file_type().is_file() {
            return Err(package_refusal(format!(
                "installed package member {} is not a regular file",
                path.display(),
            )));
        }
        let relative = path.strip_prefix(installed_root).map_err(|_| {
            package_refusal(format!(
                "installed package member escapes its root: {}",
                path.display(),
            ))
        })?;
        files.push(SnapshotFile {
            path: relative_path_spelling(relative)?,
            mode: snapshot_mode(&metadata),
            bytes: fs::read(&path).map_err(|error| {
                package_refusal(format!("cannot read {}: {error}", path.display()))
            })?,
        });
    }
    Ok(())
}

fn validate_package_name(name: &str) -> Result<(), ProveFailure> {
    let path = Path::new(name);
    let components = path.components().collect::<Vec<_>>();
    let scoped = name.starts_with('@');
    let expected = if scoped { 2 } else { 1 };
    if components.len() != expected
        || components
            .iter()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        || name.contains('\\')
    {
        return Err(package_refusal(format!(
            "package name is not a confined npm path: {name:?}",
        )));
    }
    Ok(())
}

fn relative_path_spelling(path: &Path) -> Result<String, ProveFailure> {
    path.components()
        .map(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .map(|parts| parts.join("/"))
        .ok_or_else(|| {
            package_refusal(format!(
                "package snapshot path is not confined UTF-8: {}",
                path.display(),
            ))
        })
}

fn require_directory(path: &Path, description: &str) -> Result<(), ProveFailure> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| package_refusal(format!("cannot inspect {}: {error}", path.display())))?;
    if !metadata.file_type().is_dir() {
        return Err(package_refusal(format!(
            "{description} is not a directory: {}",
            path.display(),
        )));
    }
    Ok(())
}

fn require_regular_file(path: &Path, description: &str) -> Result<(), ProveFailure> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| package_refusal(format!("cannot inspect {}: {error}", path.display())))?;
    if !metadata.file_type().is_file() {
        return Err(package_refusal(format!(
            "{description} is not a regular file: {}",
            path.display(),
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn snapshot_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;

    metadata.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn snapshot_mode(metadata: &fs::Metadata) -> u32 {
    u32::from(metadata.permissions().readonly())
}

#[cfg(unix)]
fn set_snapshot_mode(path: &Path, mode: u32) -> Result<(), ProveFailure> {
    use std::os::unix::fs::PermissionsExt as _;

    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| package_refusal(format!("cannot set mode on {}: {error}", path.display())))
}

#[cfg(not(unix))]
fn set_snapshot_mode(path: &Path, mode: u32) -> Result<(), ProveFailure> {
    let mut permissions = fs::metadata(path)
        .map_err(|error| package_refusal(format!("cannot inspect {}: {error}", path.display())))?
        .permissions();
    permissions.set_readonly(mode != 0);
    fs::set_permissions(path, permissions)
        .map_err(|error| package_refusal(format!("cannot set mode on {}: {error}", path.display())))
}

fn package_refusal(detail: impl ToString) -> ProveFailure {
    ProveFailure::refusal("gate::prove-canvas-package-refused", detail)
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
        repository,
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
                repository,
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
            repository: repository.to_path_buf(),
            common: common.to_path_buf(),
            environment: environment_values.to_vec(),
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
    repository: &Path,
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
            playwright,
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
    let workspace = CanvasWorkspace(exclusive_workspace("spice-canvas-proof")?);
    fs::create_dir(&workspace.0)
        .map_err(|error| ProveFailure::fault("gate::prove-canvas-workspace-unavailable", error))?;
    let root = workspace.0.join("root");
    copy_tree(&execution.root, &root)?;
    initialize_git_index(&root)?;
    execution.playwright.materialize(&root)?;
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

    struct PackageFixture {
        root: PathBuf,
        selected: PathBuf,
        installed: PathBuf,
    }

    impl PackageFixture {
        fn new() -> Self {
            static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "spice-canvas-package-test-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
            ));
            let selected = root.join("selected");
            let installed = root.join("installed");
            fs::create_dir_all(&selected).expect("create selected fixture root");
            fs::create_dir_all(&installed).expect("create installed fixture root");
            fs::write(
                selected.join("package-lock.json"),
                br#"{
                    "lockfileVersion": 3,
                    "packages": {
                        "": {},
                        "node_modules/playwright": {
                            "version": "1.0.0",
                            "dependencies": {"playwright-core": "1.0.0"},
                            "optionalDependencies": {"fsevents": "1.0.0"}
                        },
                        "node_modules/playwright-core": {"version": "1.0.0"},
                        "node_modules/fsevents": {"version": "1.0.0"}
                    }
                }"#,
            )
            .expect("write package lock");
            for (name, body) in [
                (
                    "playwright",
                    b"export const value = 'playwright';".as_slice(),
                ),
                (
                    "playwright-core",
                    b"export const value = 'playwright-core';".as_slice(),
                ),
                ("fsevents", b"export const value = 'fsevents';".as_slice()),
            ] {
                let package = installed.join("node_modules").join(name);
                fs::create_dir_all(&package).expect("create package fixture");
                let dependencies = if name == "playwright" {
                    r#","dependencies":{"playwright-core":"1.0.0"},"optionalDependencies":{"fsevents":"1.0.0"}"#
                } else {
                    ""
                };
                fs::write(
                    package.join("package.json"),
                    format!(r#"{{"name":"{name}","version":"1.0.0"{dependencies}}}"#),
                )
                .expect("write package identity");
                fs::write(package.join("index.js"), body).expect("write package source");
            }
            Self {
                root,
                selected,
                installed,
            }
        }

        fn source(&self, package: &str) -> PathBuf {
            self.installed
                .join("node_modules")
                .join(package)
                .join("index.js")
        }
    }

    impl Drop for PackageFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn refusal_reason(failure: ProveFailure) -> String {
        match failure {
            ProveFailure::Refusal { reason, .. } => reason,
            ProveFailure::Fault { reason, detail } => {
                panic!("expected refusal, got fault {reason}: {detail}")
            }
        }
    }

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

    #[test]
    fn package_snapshot_authority_is_root_independent_and_content_sensitive() {
        let first = PackageFixture::new();
        let second = PackageFixture::new();
        let original = success(PlaywrightSnapshot::capture(
            &first.selected,
            &first.installed,
        ));
        let equivalent = success(PlaywrightSnapshot::capture(
            &second.selected,
            &second.installed,
        ));
        assert_eq!(original.authority, equivalent.authority);

        fs::write(second.source("playwright"), b"same version, changed bytes")
            .expect("mutate package source");
        let changed = success(PlaywrightSnapshot::capture(
            &second.selected,
            &second.installed,
        ));
        assert_ne!(original.authority, changed.authority);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            let source = first.source("playwright-core");
            fs::set_permissions(&source, fs::Permissions::from_mode(0o755))
                .expect("change package source mode");
            let changed_mode = success(PlaywrightSnapshot::capture(
                &first.selected,
                &first.installed,
            ));
            assert_ne!(original.authority, changed_mode.authority);
        }
    }

    #[test]
    fn package_snapshot_materializes_only_captured_bytes() {
        let fixture = PackageFixture::new();
        let snapshot = success(PlaywrightSnapshot::capture(
            &fixture.selected,
            &fixture.installed,
        ));
        fs::write(fixture.source("playwright"), b"live mutation")
            .expect("mutate live package source");
        let materialized = fixture.root.join("materialized");
        fs::create_dir(&materialized).expect("create materialization root");
        success(snapshot.materialize(&materialized));
        assert_eq!(
            fs::read(materialized.join("node_modules/playwright/index.js"))
                .expect("read materialized source"),
            b"export const value = 'playwright';",
        );
        assert_eq!(
            fs::read(fixture.source("playwright")).expect("read live source"),
            b"live mutation",
        );
    }

    #[test]
    fn package_snapshot_refuses_lock_identity_disagreement() {
        let fixture = PackageFixture::new();
        fs::write(
            fixture
                .installed
                .join("node_modules/playwright/package.json"),
            r#"{"name":"playwright","version":"2.0.0"}"#,
        )
        .expect("replace package identity");
        let failure = PlaywrightSnapshot::capture(&fixture.selected, &fixture.installed)
            .err()
            .expect("version disagreement must refuse");
        assert_eq!(
            refusal_reason(failure),
            "gate::prove-canvas-package-refused"
        );
    }

    #[test]
    fn package_snapshot_refuses_incomplete_dependency_resolution() {
        let fixture = PackageFixture::new();
        fs::write(
            fixture
                .installed
                .join("node_modules/playwright/package.json"),
            r#"{
                "name":"playwright",
                "version":"1.0.0",
                "dependencies":{"playwright-core":"1.0.0","undeclared":"1.0.0"},
                "optionalDependencies":{"fsevents":"1.0.0"}
            }"#,
        )
        .expect("replace package dependencies");
        let failure = PlaywrightSnapshot::capture(&fixture.selected, &fixture.installed)
            .err()
            .expect("incomplete dependency resolution must refuse");
        assert_eq!(
            refusal_reason(failure),
            "gate::prove-canvas-package-refused"
        );
    }

    #[cfg(unix)]
    #[test]
    fn package_snapshot_refuses_symlinked_members() {
        let fixture = PackageFixture::new();
        let source = fixture.source("playwright");
        fs::remove_file(&source).expect("remove package source");
        std::os::unix::fs::symlink("package.json", &source).expect("symlink package source");
        let failure = PlaywrightSnapshot::capture(&fixture.selected, &fixture.installed)
            .err()
            .expect("symlinked member must refuse");
        assert_eq!(
            refusal_reason(failure),
            "gate::prove-canvas-package-refused"
        );
    }
}
