//! Digest-bound repository installation and exact recorded reversal.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wire::{Digest, Domain, Fault, FaultClass, Refusal};

use crate::repository::Repository;

mod record;
mod support;

use record::{
    preflight_unapply, prepare_unapplied, restore_snapshot, rollback_unapply,
    validate_existing_record, validate_installed_state,
};
use support::*;

const PLAN_VERSION: u16 = 2;
const RECORD_VERSION: u16 = 2;
const DATA_MODE: u32 = 0o644;
const EXECUTABLE_MODE: u32 = 0o755;
const RECORD_MODE: u32 = 0o600;
const TRACKED_POLICY: &str = include_str!("../../spice.toml");
const BASELINE: &[u8] = include_bytes!("../../baseline.toml");
const SKILL: &[u8] = include_bytes!("../../.agents/skills/spice/SKILL.md");
const HOOK_ENTRY: &[u8] = include_bytes!("../../steer/hooks/entry.sh");
const HOOK_STAGE: &[u8] = include_bytes!("../../steer/hooks/static/stage.sh");
const TASTE_NAMES: [&str; 6] = [
    "polling",
    "fallbacks",
    "compatibility",
    "shims",
    "aliases",
    "legacy",
];

/// What one plan operation will do to its destination.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Change {
    Create,
    Replace,
    Remove,
    Retain,
}

/// Stable native identity of one observed filesystem object.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Generation {
    pub device: u64,
    pub inode: u64,
}

/// The resolved existing ancestor that anchors a destination parent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ParentObservation {
    pub ancestor: PathBuf,
    pub generation: Generation,
    pub missing: PathBuf,
}

/// The state observed at a filesystem destination before the plan was built.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Observation {
    Absent,
    File {
        digest: Digest,
        mode: u32,
        generation: Generation,
    },
}

/// One explicit value state for the Git hook selection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum HookValue {
    Absent,
    Value { value: String },
}

impl HookValue {
    fn from_option(value: Option<String>) -> Self {
        match value {
            Some(value) => Self::Value { value },
            None => Self::Absent,
        }
    }

    fn as_deref(&self) -> Option<&str> {
        match self {
            Self::Absent => None,
            Self::Value { value } => Some(value),
        }
    }

    fn into_option(self) -> Option<String> {
        match self {
            Self::Absent => None,
            Self::Value { value } => Some(value),
        }
    }
}

/// One exact filesystem operation in an installation plan.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub path: PathBuf,
    pub parent: ParentObservation,
    pub change: Change,
    pub observed: Observation,
    pub desired: Digest,
    pub mode: u32,
}

/// The Git hook-selection edit included in the plan.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HookSelection {
    pub config: PathBuf,
    pub parent: ParentObservation,
    pub observed: Observation,
    pub previous: HookValue,
    pub selected: HookValue,
    pub mode: u32,
    pub change: Change,
}

/// State of the plan returned by the command.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Preview,
    Applied,
    Unapplied,
}

/// The complete deterministic install answer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub version: u16,
    pub action: Action,
    pub repository: PathBuf,
    pub operations: Vec<Operation>,
    pub hook: HookSelection,
    pub digest: Digest,
}

/// A typed installation failure already classified for the process rim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    Refusal(Refusal),
    Fault(Fault),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refusal(error) => write!(formatter, "{}: {}", error.reason, error.remedy),
            Self::Fault(error) => write!(formatter, "{}: {}", error.reason, error.detail),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u16,
    repository: PathBuf,
    home: PathBuf,
    plan: Digest,
    operations: Vec<Operation>,
    selection: HookSelection,
    hook: RecordedHook,
    writes: Vec<RecordedWrite>,
}

#[derive(Deserialize)]
struct RecordHeader {
    version: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RecordedHook {
    config: PathBuf,
    previous: Option<String>,
    selected: String,
    prior: Snapshot,
    installed: Digest,
    mode: u32,
    changed: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RecordedWrite {
    path: PathBuf,
    prior: Snapshot,
    installed: Digest,
    mode: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum Snapshot {
    Absent,
    File {
        bytes: Vec<u8>,
        mode: u32,
        generation: Generation,
    },
}

#[derive(Clone, Debug)]
struct Destination {
    logical: PathBuf,
    path: PathBuf,
    root: PathBuf,
    parent: ParentObservation,
}

#[derive(Clone, Debug)]
struct Artifact {
    destination: Destination,
    operation: Operation,
    desired: Vec<u8>,
    prior: Snapshot,
}

#[derive(Clone, Debug)]
struct Prepared {
    repository: Repository,
    home: PathBuf,
    plan: Plan,
    artifacts: Vec<Artifact>,
    config_bytes: Vec<u8>,
    config_mode: u32,
    config_prior: Snapshot,
    hook_destination: Destination,
    record: Destination,
}

#[derive(Serialize)]
struct PlanIdentity<'a> {
    version: u16,
    repository: &'a Path,
    operations: &'a [Operation],
    hook: &'a HookSelection,
}

/// Preview an installation without changing repository or user state.
pub fn preview(start: &Path, executable: &Path, home: &Path) -> Result<Plan, Error> {
    prepare(start, executable, home).map(|prepared| prepared.plan)
}

/// Apply exactly the plan whose digest the caller accepted.
pub fn apply(start: &Path, executable: &Path, home: &Path, accepted: &str) -> Result<Plan, Error> {
    let mut prepared = prepare(start, executable, home)?;
    if prepared.plan.digest.as_str() != accepted {
        return Err(refusal(
            "install::plan-changed",
            Some(prepared.plan.digest.to_string()),
            Some(accepted.to_owned()),
            "run spice install again and apply the digest it prints",
        ));
    }
    if prepared.record.path.exists() {
        validate_existing_record(&prepared)?;
        prepared.plan.action = Action::Applied;
        return Ok(prepared.plan);
    }
    apply_prepared(&prepared)?;
    prepared.plan.action = Action::Applied;
    Ok(prepared.plan)
}

/// Reverse only the exact files and hook selection named by the install record.
pub fn unapply(start: &Path, home: &Path) -> Result<Plan, Error> {
    let repository = Repository::discover(start).map_err(repository_error)?;
    let home = canonical_directory(home, "install::home")?;
    let record_destination = destination(
        repository.common_dir().join(".spice/install"),
        repository.common_dir(),
    )?;
    let record = read_record(&record_destination.path)?;
    let preflight = preflight_unapply(&repository, &home, &record)?;
    let mut restored = Vec::new();
    for index in (0..record.writes.len()).rev() {
        let write = &record.writes[index];
        if let Err(error) = restore_snapshot(&preflight.destinations[index], &write.prior) {
            rollback_unapply(&preflight, &restored);
            return Err(error);
        }
        restored.push(index);
    }
    if record.hook.changed
        && let Err(error) = restore_snapshot(&preflight.hook, &record.hook.prior)
    {
        rollback_unapply(&preflight, &restored);
        return Err(error);
    }
    if let Err(source) = fs::remove_file(&record_destination.path) {
        if record.hook.changed {
            let _ = restore_snapshot(&preflight.hook, &preflight.hook_observed);
        }
        rollback_unapply(&preflight, &restored);
        return Err(io_fault(
            "remove install record",
            &record_destination.path,
            source,
        ));
    }
    let prepared = prepare_unapplied(
        &repository,
        record,
        &preflight.installed,
        &preflight.hook_observed,
    )?;
    Ok(prepared)
}

fn prepare(start: &Path, executable: &Path, home: &Path) -> Result<Prepared, Error> {
    let repository = Repository::discover(start).map_err(repository_error)?;
    let executable = canonical_file(executable, "install::executable")?;
    let home = canonical_directory(home, "install::home")?;
    let repository_id = Digest::compute(
        Domain::InstallRepository,
        repository.worktree_root().as_os_str().as_encoded_bytes(),
    );
    let artifacts = artifacts(&repository, &executable, &home, &repository_id)?;
    let hook = hook_selection(&repository)?;
    let operations = artifacts
        .iter()
        .map(|artifact| artifact.operation.clone())
        .collect::<Vec<_>>();
    let identity = PlanIdentity {
        version: PLAN_VERSION,
        repository: repository.worktree_root(),
        operations: &operations,
        hook: &hook.selection,
    };
    let bytes = wire::encode(&identity).map_err(|source| encode_fault("install plan", source))?;
    let digest = Digest::compute(Domain::InstallPlan, &bytes);
    let plan = Plan {
        version: PLAN_VERSION,
        action: Action::Preview,
        repository: repository.worktree_root().to_path_buf(),
        operations,
        hook: hook.selection,
        digest,
    };
    let record = destination(
        repository.common_dir().join(".spice/install"),
        repository.common_dir(),
    )?;
    Ok(Prepared {
        repository,
        home,
        plan,
        artifacts,
        config_bytes: hook.bytes,
        config_mode: hook.mode,
        config_prior: hook.prior,
        hook_destination: hook.destination,
        record,
    })
}

struct PreparedHook {
    destination: Destination,
    selection: HookSelection,
    bytes: Vec<u8>,
    mode: u32,
    prior: Snapshot,
}

fn hook_selection(repository: &Repository) -> Result<PreparedHook, Error> {
    let config_destination = destination(
        repository.common_dir().join("config"),
        repository.common_dir(),
    )?;
    let snapshot = snapshot(&config_destination.path)?;
    let observed = observation(&snapshot);
    let (bytes, mode) = match &snapshot {
        Snapshot::Absent => (Vec::new(), DATA_MODE),
        Snapshot::File { bytes, mode, .. } => (bytes.clone(), *mode),
    };
    let text = String::from_utf8(bytes.clone()).map_err(|_| {
        refusal(
            "install::git-config-encoding",
            Some("UTF-8 Git configuration".to_owned()),
            Some(config_destination.path.display().to_string()),
            "repair the Git configuration before installing",
        )
    })?;
    let parsed = parse_hook(&text)?;
    let hook_entry = destination(
        repository.common_dir().join(".spice/hooks/pre-commit"),
        repository.common_dir(),
    )?;
    let selected = hook_entry
        .path
        .parent()
        .expect("the installed hook has a parent")
        .display()
        .to_string();
    let change = if parsed.value.as_deref() == Some(&selected) {
        Change::Retain
    } else if parsed.value.is_some() {
        Change::Replace
    } else {
        Change::Create
    };
    Ok(PreparedHook {
        selection: HookSelection {
            config: config_destination.path.clone(),
            parent: config_destination.parent.clone(),
            observed,
            previous: HookValue::from_option(parsed.value),
            selected: HookValue::Value { value: selected },
            mode,
            change,
        },
        bytes,
        mode,
        prior: snapshot,
        destination: config_destination,
    })
}

fn artifacts(
    repository: &Repository,
    executable: &Path,
    home: &Path,
    repository_id: &Digest,
) -> Result<Vec<Artifact>, Error> {
    let root = repository.worktree_root();
    let suffix = &repository_id.as_str()[..12];
    let policy = seeded_policy(root.join("spice.toml"))?;
    let destinations = install_destinations(repository, home)?;
    let installed_executable = destinations[0].path.clone();
    let executable_bytes = fs::read(executable)
        .map_err(|source| io_fault("read Rust executable", executable, source))?;
    let pre_commit = pre_commit(&installed_executable);
    let systemd = systemd_unit(&installed_executable, root);
    let launchd = launchd_unit(&installed_executable, root, suffix);
    let desired = vec![
        (executable_bytes, EXECUTABLE_MODE),
        (HOOK_ENTRY.to_vec(), EXECUTABLE_MODE),
        (HOOK_STAGE.to_vec(), EXECUTABLE_MODE),
        (pre_commit.into_bytes(), EXECUTABLE_MODE),
        (SKILL.to_vec(), DATA_MODE),
        (policy, DATA_MODE),
        (seeded_baseline(root.join("baseline.toml"))?, DATA_MODE),
        (systemd.into_bytes(), DATA_MODE),
        (launchd.into_bytes(), DATA_MODE),
    ];
    destinations
        .into_iter()
        .zip(desired)
        .map(|(destination, (bytes, mode))| artifact(destination, bytes, mode))
        .collect()
}

fn install_destinations(repository: &Repository, home: &Path) -> Result<Vec<Destination>, Error> {
    let root = repository.worktree_root();
    let common = repository.common_dir();
    let hooks = common.join(".spice/hooks");
    let repository_id = Digest::compute(
        Domain::InstallRepository,
        root.as_os_str().as_encoded_bytes(),
    );
    let suffix = &repository_id.as_str()[..12];
    [
        (home.join(".local/bin/spice-rust"), home),
        (hooks.join("entry.sh"), common),
        (hooks.join("static/stage.sh"), common),
        (hooks.join("pre-commit"), common),
        (root.join(".agents/skills/spice/SKILL.md"), root),
        (root.join("spice.toml"), root),
        (root.join("baseline.toml"), root),
        (
            home.join(format!(".config/systemd/user/spice-{suffix}.service")),
            home,
        ),
        (
            home.join(format!("Library/LaunchAgents/dev.spice.{suffix}.plist")),
            home,
        ),
    ]
    .into_iter()
    .map(|(path, permitted_root)| destination(path, permitted_root))
    .collect()
}

fn artifact(destination: Destination, desired: Vec<u8>, mode: u32) -> Result<Artifact, Error> {
    let mode = effective_mode(mode);
    let prior = snapshot(&destination.path)?;
    let observed_digest = snapshot_digest(&prior);
    let desired_digest = Digest::compute(Domain::InstallArtifact, &desired);
    let change = match (&prior, observed_digest.as_ref()) {
        (Snapshot::Absent, _) => Change::Create,
        (Snapshot::File { mode: found, .. }, Some(digest))
            if digest == &desired_digest && *found == mode =>
        {
            Change::Retain
        }
        (Snapshot::File { .. }, _) => Change::Replace,
    };
    Ok(Artifact {
        operation: Operation {
            path: destination.path.clone(),
            parent: destination.parent.clone(),
            change,
            observed: observation(&prior),
            desired: desired_digest,
            mode,
        },
        destination,
        desired,
        prior,
    })
}

fn destination(logical: PathBuf, root: &Path) -> Result<Destination, Error> {
    if !logical.is_absolute() || !logical.starts_with(root) {
        return Err(destination_root_refusal(&logical, root));
    }
    let file_name = logical.file_name().ok_or_else(|| {
        refusal(
            "install::destination-shape",
            Some("one absolute file destination".to_owned()),
            Some(logical.display().to_string()),
            "repair the compiled installation destination",
        )
    })?;
    let mut candidate = logical
        .parent()
        .expect("a destination with a file name has a parent")
        .to_path_buf();
    let mut missing = Vec::<OsString>::new();
    let (ancestor, generation) = loop {
        match fs::symlink_metadata(&candidate) {
            Ok(_) => {
                let canonical = fs::canonicalize(&candidate).map_err(|source| {
                    io_fault("resolve install destination parent", &candidate, source)
                })?;
                let metadata = fs::metadata(&canonical).map_err(|source| {
                    io_fault("inspect install destination parent", &canonical, source)
                })?;
                if !metadata.is_dir() {
                    return Err(refusal(
                        "install::destination-shape",
                        Some("directory destination parent".to_owned()),
                        Some(canonical.display().to_string()),
                        "move the non-directory parent aside before installing",
                    ));
                }
                break (canonical, generation(&metadata));
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                let name = candidate.file_name().ok_or_else(|| {
                    io_fault("resolve install destination parent", &candidate, source)
                })?;
                missing.push(name.to_os_string());
                candidate = candidate
                    .parent()
                    .expect("an absolute missing path has an ancestor")
                    .to_path_buf();
            }
            Err(source) => {
                return Err(io_fault(
                    "inspect install destination parent",
                    &candidate,
                    source,
                ));
            }
        }
    };
    let mut missing_path = PathBuf::new();
    for component in missing.into_iter().rev() {
        missing_path.push(component);
    }
    let path = ancestor.join(&missing_path).join(file_name);
    if !path.starts_with(root) {
        return Err(destination_root_refusal(&path, root));
    }
    Ok(Destination {
        logical,
        path,
        root: root.to_path_buf(),
        parent: ParentObservation {
            ancestor,
            generation,
            missing: missing_path,
        },
    })
}

fn validate_destination(expected: &Destination) -> Result<(), Error> {
    let observed = destination(expected.logical.clone(), &expected.root)?;
    if observed.path != expected.path || observed.parent != expected.parent {
        return Err(refusal(
            "install::destination-drift",
            Some(format!(
                "{} at {:?}",
                expected.path.display(),
                expected.parent
            )),
            Some(format!(
                "{} at {:?}",
                observed.path.display(),
                observed.parent
            )),
            "preview the installation again after destination parents settle",
        ));
    }
    Ok(())
}

fn snapshot(path: &Path) -> Result<Snapshot, Error> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Snapshot::Absent);
        }
        Err(source) => return Err(io_fault("inspect install destination", path, source)),
    };
    if !metadata.file_type().is_file() {
        return Err(refusal(
            "install::destination-shape",
            Some("regular file or absence".to_owned()),
            Some(path.display().to_string()),
            "move the non-file destination aside before installing",
        ));
    }
    let mut file = fs::File::open(path)
        .map_err(|source| io_fault("open install destination", path, source))?;
    let opened = file
        .metadata()
        .map_err(|source| io_fault("inspect open install destination", path, source))?;
    if generation(&metadata) != generation(&opened) {
        return Err(destination_drift_refusal(path));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|source| io_fault("read install destination", path, source))?;
    let read = file
        .metadata()
        .map_err(|source| io_fault("reinspect open install destination", path, source))?;
    let after = fs::symlink_metadata(path)
        .map_err(|source| io_fault("reinspect install destination", path, source))?;
    if snapshot_signature(&metadata) != snapshot_signature(&opened)
        || snapshot_signature(&opened) != snapshot_signature(&read)
        || snapshot_signature(&read) != snapshot_signature(&after)
    {
        return Err(destination_drift_refusal(path));
    }
    Ok(Snapshot::File {
        bytes,
        mode: file_mode(&metadata),
        generation: generation(&metadata),
    })
}

fn snapshot_digest(snapshot: &Snapshot) -> Option<Digest> {
    match snapshot {
        Snapshot::Absent => None,
        Snapshot::File { bytes, .. } => Some(Digest::compute(Domain::InstallArtifact, bytes)),
    }
}

fn observation(snapshot: &Snapshot) -> Observation {
    match snapshot {
        Snapshot::Absent => Observation::Absent,
        Snapshot::File {
            bytes,
            mode,
            generation,
        } => Observation::File {
            digest: Digest::compute(Domain::InstallArtifact, bytes),
            mode: *mode,
            generation: *generation,
        },
    }
}

fn record_from(prepared: &Prepared) -> Result<Record, Error> {
    let writes = prepared
        .artifacts
        .iter()
        .filter(|artifact| artifact.operation.change != Change::Retain)
        .map(|artifact| {
            validate_installed_state(
                &artifact.operation.path,
                &artifact.operation.desired,
                artifact.operation.mode,
            )?;
            Ok(RecordedWrite {
                path: artifact.operation.path.clone(),
                prior: artifact.prior.clone(),
                installed: artifact.operation.desired.clone(),
                mode: artifact.operation.mode,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let installed_hook = hook_bytes(prepared, prepared.plan.hook.selected.as_deref())?;
    let installed_hook_digest = Digest::compute(Domain::InstallArtifact, &installed_hook);
    validate_installed_state(
        &prepared.plan.hook.config,
        &installed_hook_digest,
        prepared.config_mode,
    )?;
    Ok(Record {
        version: RECORD_VERSION,
        repository: prepared.repository.worktree_root().to_path_buf(),
        home: prepared.home.clone(),
        plan: prepared.plan.digest.clone(),
        operations: prepared.plan.operations.clone(),
        selection: prepared.plan.hook.clone(),
        hook: RecordedHook {
            config: prepared.plan.hook.config.clone(),
            previous: prepared.plan.hook.previous.clone().into_option(),
            selected: prepared
                .plan
                .hook
                .selected
                .as_deref()
                .expect("installation always selects one hook path")
                .to_owned(),
            prior: prepared.config_prior.clone(),
            installed: installed_hook_digest,
            mode: prepared.config_mode,
            changed: prepared.plan.hook.change != Change::Retain,
        },
        writes,
    })
}

fn apply_prepared(prepared: &Prepared) -> Result<(), Error> {
    preflight_apply(prepared)?;
    let mut written = Vec::new();
    for artifact in &prepared.artifacts {
        if artifact.operation.change == Change::Retain {
            continue;
        }
        if let Err(error) = publish(
            &artifact.destination,
            &artifact.desired,
            artifact.operation.mode,
        ) {
            let _ = restore_snapshot(&artifact.destination, &artifact.prior);
            rollback_writes(&written);
            return Err(error);
        }
        written.push(artifact);
    }
    let hook_changed = prepared.plan.hook.change != Change::Retain;
    if hook_changed && let Err(error) = write_hook(prepared, prepared.plan.hook.selected.as_deref())
    {
        let _ = restore_snapshot(&prepared.hook_destination, &prepared.config_prior);
        rollback_writes(&written);
        return Err(error);
    }
    let record = match record_from(prepared) {
        Ok(record) => record,
        Err(error) => {
            if hook_changed {
                let _ = restore_snapshot(&prepared.hook_destination, &prepared.config_prior);
            }
            rollback_writes(&written);
            return Err(error);
        }
    };
    let record_bytes =
        wire::encode(&record).map_err(|source| encode_fault("install record", source))?;
    if let Err(error) = publish(&prepared.record, &record_bytes, RECORD_MODE) {
        let _ = fs::remove_file(&prepared.record.path);
        if hook_changed {
            let _ = restore_snapshot(&prepared.hook_destination, &prepared.config_prior);
        }
        rollback_writes(&written);
        return Err(error);
    }
    Ok(())
}

fn write_hook(prepared: &Prepared, value: Option<&str>) -> Result<(), Error> {
    let updated = hook_bytes(prepared, value)?;
    publish(&prepared.hook_destination, &updated, prepared.config_mode)
}

fn hook_bytes(prepared: &Prepared, value: Option<&str>) -> Result<Vec<u8>, Error> {
    let text = String::from_utf8(prepared.config_bytes.clone()).map_err(|_| {
        fault(
            "install::git-config-encoding",
            "Git configuration stopped being UTF-8",
        )
    })?;
    let updated = set_hook(&text, value)?;
    Ok(updated.into_bytes())
}

fn rollback_writes(written: &[&Artifact]) {
    for artifact in written.iter().rev() {
        let _ = restore_snapshot(&artifact.destination, &artifact.prior);
    }
}

fn preflight_apply(prepared: &Prepared) -> Result<(), Error> {
    for artifact in &prepared.artifacts {
        validate_destination(&artifact.destination)?;
    }
    validate_destination(&prepared.hook_destination)?;
    validate_destination(&prepared.record)?;
    for artifact in &prepared.artifacts {
        if snapshot(&artifact.operation.path)? != artifact.prior {
            return Err(destination_drift_refusal(&artifact.operation.path));
        }
    }
    if snapshot(&prepared.plan.hook.config)? != prepared.config_prior {
        return Err(destination_drift_refusal(&prepared.plan.hook.config));
    }
    if snapshot(&prepared.record.path)? != Snapshot::Absent {
        return Err(destination_drift_refusal(&prepared.record.path));
    }
    Ok(())
}

fn publish(destination: &Destination, bytes: &[u8], mode: u32) -> Result<(), Error> {
    let path = &destination.path;
    let parent = path.parent().ok_or_else(|| {
        fault(
            "install::destination",
            format!("{} has no parent", path.display()),
        )
    })?;
    fs::create_dir_all(parent)
        .map_err(|source| io_fault("create install directory", parent, source))?;
    let canonical_parent = fs::canonicalize(parent)
        .map_err(|source| io_fault("resolve install publication parent", parent, source))?;
    if canonical_parent != parent || !canonical_parent.starts_with(&destination.root) {
        return Err(destination_drift_refusal(path));
    }
    let destination = atomic::Destination::new(path)
        .map_err(|source| fault("install::publication", source.to_string()))?;
    destination
        .publish(|staged| {
            staged.write_all(bytes)?;
            set_mode(staged.temporary_path(), mode)
        })
        .map_err(|source| fault("install::publication", source.to_string()))
}

fn read_record(path: &Path) -> Result<Record, Error> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(refusal(
                "install::record-missing",
                Some(path.display().to_string()),
                None,
                "run spice install and apply its digest before unapplying",
            ));
        }
        Err(source) => return Err(io_fault("read install record", path, source)),
    };
    let header: RecordHeader = wire::decode(&bytes).map_err(|source| {
        refusal(
            "install::record-invalid",
            Some("one current canonical install record".to_owned()),
            Some(source.to_string()),
            "restore the exact install record or reconcile the installation manually",
        )
    })?;
    if header.version != RECORD_VERSION {
        return Err(record_version_refusal(header.version));
    }
    let record: Record = wire::decode(&bytes).map_err(|source| {
        refusal(
            "install::record-invalid",
            Some("one current canonical install record".to_owned()),
            Some(source.to_string()),
            "restore the exact install record or reconcile the installation manually",
        )
    })?;
    let canonical =
        wire::encode(&record).map_err(|source| encode_fault("install record", source))?;
    if canonical != bytes {
        return Err(record_shape_refusal(
            "install record bytes are not the one canonical encoding",
        ));
    }
    Ok(record)
}
