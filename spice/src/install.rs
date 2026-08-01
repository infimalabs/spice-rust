//! Digest-bound repository installation and exact recorded reversal.

#[cfg(not(target_arch = "wasm32"))]
pub mod coexistence;

use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wire::{Digest, Domain, Fault, FaultClass, Refusal};

use crate::repository::Repository;

const PLAN_VERSION: u16 = 1;
const RECORD_VERSION: u16 = 1;
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

/// The state observed at a filesystem destination before the plan was built.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Observation {
    Absent,
    File { digest: Digest, mode: u32 },
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
    plan: Digest,
    hook: RecordedHook,
    writes: Vec<RecordedWrite>,
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
    File { bytes: Vec<u8>, mode: u32 },
}

#[derive(Clone, Debug)]
struct Artifact {
    operation: Operation,
    desired: Vec<u8>,
    prior: Snapshot,
}

#[derive(Clone, Debug)]
struct Prepared {
    plan: Plan,
    artifacts: Vec<Artifact>,
    config_bytes: Vec<u8>,
    config_mode: u32,
    config_prior: Snapshot,
    record: PathBuf,
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
    if prepared.record.exists() {
        validate_existing_record(&prepared)?;
        prepared.plan.action = Action::Applied;
        return Ok(prepared.plan);
    }
    let record = record_from(&prepared)?;
    apply_prepared(&prepared, &record)?;
    prepared.plan.action = Action::Applied;
    Ok(prepared.plan)
}

/// Reverse only the exact files and hook selection named by the install record.
pub fn unapply(start: &Path) -> Result<Plan, Error> {
    let repository = Repository::discover(start).map_err(repository_error)?;
    let record_path = record_path(&repository);
    let record = read_record(&record_path)?;
    let (installed, hook_observed) = preflight_unapply(&record)?;
    let mut restored = Vec::new();
    for index in (0..record.writes.len()).rev() {
        let write = &record.writes[index];
        if let Err(error) = restore_snapshot(&write.path, &write.prior) {
            rollback_unapply(&record, &installed, &restored);
            return Err(error);
        }
        restored.push(index);
    }
    if record.hook.changed
        && let Err(error) = restore_snapshot(&record.hook.config, &record.hook.prior)
    {
        rollback_unapply(&record, &installed, &restored);
        return Err(error);
    }
    if let Err(source) = fs::remove_file(&record_path) {
        if record.hook.changed {
            let _ = restore_snapshot(&record.hook.config, &hook_observed);
        }
        rollback_unapply(&record, &installed, &restored);
        return Err(io_fault("remove install record", &record_path, source));
    }
    let prepared = prepare_unapplied(&repository, record, &hook_observed)?;
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
    Ok(Prepared {
        plan,
        artifacts,
        config_bytes: hook.bytes,
        config_mode: hook.mode,
        config_prior: hook.prior,
        record: record_path(&repository),
    })
}

struct PreparedHook {
    selection: HookSelection,
    bytes: Vec<u8>,
    mode: u32,
    prior: Snapshot,
}

fn hook_selection(repository: &Repository) -> Result<PreparedHook, Error> {
    let config = repository.common_dir().join("config");
    let snapshot = snapshot(&config)?;
    let observed = observation(&snapshot);
    let (bytes, mode) = match &snapshot {
        Snapshot::Absent => (Vec::new(), DATA_MODE),
        Snapshot::File { bytes, mode } => (bytes.clone(), *mode),
    };
    let text = String::from_utf8(bytes.clone()).map_err(|_| {
        refusal(
            "install::git-config-encoding",
            Some("UTF-8 Git configuration".to_owned()),
            Some(config.display().to_string()),
            "repair the Git configuration before installing",
        )
    })?;
    let parsed = parse_hook(&text)?;
    let selected = repository
        .common_dir()
        .join(".spice/hooks")
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
            config,
            observed,
            previous: HookValue::from_option(parsed.value),
            selected: HookValue::Value { value: selected },
            mode,
            change,
        },
        bytes,
        mode,
        prior: snapshot,
    })
}

fn artifacts(
    repository: &Repository,
    executable: &Path,
    home: &Path,
    repository_id: &Digest,
) -> Result<Vec<Artifact>, Error> {
    let root = repository.worktree_root();
    let state = repository.common_dir().join(".spice");
    let hooks = state.join("hooks");
    let suffix = &repository_id.as_str()[..12];
    let policy = seeded_policy(root.join("spice.toml"))?;
    let installed_executable = home.join(".local/bin/spice-rust");
    let executable_bytes = fs::read(executable)
        .map_err(|source| io_fault("read Rust executable", executable, source))?;
    let pre_commit = pre_commit(&installed_executable);
    let systemd = systemd_unit(&installed_executable, root);
    let launchd = launchd_unit(&installed_executable, root, suffix);
    let specifications = vec![
        (installed_executable, executable_bytes, EXECUTABLE_MODE),
        (hooks.join("entry.sh"), HOOK_ENTRY.to_vec(), EXECUTABLE_MODE),
        (
            hooks.join("static/stage.sh"),
            HOOK_STAGE.to_vec(),
            EXECUTABLE_MODE,
        ),
        (
            hooks.join("pre-commit"),
            pre_commit.into_bytes(),
            EXECUTABLE_MODE,
        ),
        (
            root.join(".agents/skills/spice/SKILL.md"),
            SKILL.to_vec(),
            DATA_MODE,
        ),
        (root.join("spice.toml"), policy, DATA_MODE),
        (
            root.join("baseline.toml"),
            seeded_baseline(root.join("baseline.toml"))?,
            DATA_MODE,
        ),
        (
            home.join(format!(".config/systemd/user/spice-{suffix}.service")),
            systemd.into_bytes(),
            DATA_MODE,
        ),
        (
            home.join(format!("Library/LaunchAgents/dev.spice.{suffix}.plist")),
            launchd.into_bytes(),
            DATA_MODE,
        ),
    ];
    specifications
        .into_iter()
        .map(|(path, bytes, mode)| artifact(path, bytes, mode))
        .collect()
}

fn artifact(path: PathBuf, desired: Vec<u8>, mode: u32) -> Result<Artifact, Error> {
    let mode = effective_mode(mode);
    let prior = snapshot(&path)?;
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
            path,
            change,
            observed: observation(&prior),
            desired: desired_digest,
            mode,
        },
        desired,
        prior,
    })
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
    let bytes =
        fs::read(path).map_err(|source| io_fault("read install destination", path, source))?;
    Ok(Snapshot::File {
        bytes,
        mode: file_mode(&metadata),
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
        Snapshot::File { bytes, mode } => Observation::File {
            digest: Digest::compute(Domain::InstallArtifact, bytes),
            mode: *mode,
        },
    }
}

fn record_from(prepared: &Prepared) -> Result<Record, Error> {
    let writes = prepared
        .artifacts
        .iter()
        .filter(|artifact| artifact.operation.change != Change::Retain)
        .map(|artifact| RecordedWrite {
            path: artifact.operation.path.clone(),
            prior: artifact.prior.clone(),
            installed: artifact.operation.desired.clone(),
            mode: artifact.operation.mode,
        })
        .collect();
    let installed_hook = hook_bytes(prepared, prepared.plan.hook.selected.as_deref())?;
    Ok(Record {
        version: RECORD_VERSION,
        plan: prepared.plan.digest.clone(),
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
            installed: Digest::compute(Domain::InstallArtifact, &installed_hook),
            mode: prepared.config_mode,
            changed: prepared.plan.hook.change != Change::Retain,
        },
        writes,
    })
}

fn apply_prepared(prepared: &Prepared, record: &Record) -> Result<(), Error> {
    let mut written = Vec::new();
    for artifact in &prepared.artifacts {
        if artifact.operation.change == Change::Retain {
            continue;
        }
        if let Err(error) = publish(
            &artifact.operation.path,
            &artifact.desired,
            artifact.operation.mode,
        ) {
            let _ = restore_snapshot(&artifact.operation.path, &artifact.prior);
            rollback_writes(&written);
            return Err(error);
        }
        written.push(artifact);
    }
    let hook_changed = prepared.plan.hook.change != Change::Retain;
    if hook_changed && let Err(error) = write_hook(prepared, prepared.plan.hook.selected.as_deref())
    {
        let _ = restore_snapshot(&prepared.plan.hook.config, &prepared.config_prior);
        rollback_writes(&written);
        return Err(error);
    }
    let record_bytes =
        wire::encode(record).map_err(|source| encode_fault("install record", source))?;
    if let Err(error) = publish(&prepared.record, &record_bytes, RECORD_MODE) {
        let _ = fs::remove_file(&prepared.record);
        if hook_changed {
            let _ = restore_snapshot(&prepared.plan.hook.config, &prepared.config_prior);
        }
        rollback_writes(&written);
        return Err(error);
    }
    Ok(())
}

fn write_hook(prepared: &Prepared, value: Option<&str>) -> Result<(), Error> {
    let updated = hook_bytes(prepared, value)?;
    publish(&prepared.plan.hook.config, &updated, prepared.config_mode)
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
        let _ = restore_snapshot(&artifact.operation.path, &artifact.prior);
    }
}

fn validate_existing_record(prepared: &Prepared) -> Result<(), Error> {
    let record = read_record(&prepared.record)?;
    if prepared
        .artifacts
        .iter()
        .any(|artifact| artifact.operation.change != Change::Retain)
    {
        return Err(refusal(
            "install::already-applied",
            Some("the recorded installed artifact set".to_owned()),
            Some(prepared.plan.digest.to_string()),
            "unapply the recorded installation before applying changed artifacts",
        ));
    }
    for write in &record.writes {
        validate_installed(write)?;
    }
    validate_installed_hook(&record.hook)?;
    Ok(())
}

fn preflight_unapply(record: &Record) -> Result<(Vec<Snapshot>, Snapshot), Error> {
    if record.version != RECORD_VERSION {
        return Err(refusal(
            "install::record-version",
            Some(RECORD_VERSION.to_string()),
            Some(record.version.to_string()),
            "use the Spice release that wrote this install record",
        ));
    }
    let installed = record
        .writes
        .iter()
        .map(validate_installed)
        .collect::<Result<Vec<_>, _>>()?;
    let hook_observed = validate_installed_hook(&record.hook)?;
    Ok((installed, hook_observed))
}

fn validate_installed_hook(hook: &RecordedHook) -> Result<Snapshot, Error> {
    let snapshot = snapshot(&hook.config)?;
    if hook.changed
        && (snapshot_digest(&snapshot).as_ref() != Some(&hook.installed)
            || snapshot_mode(&snapshot) != Some(hook.mode))
    {
        return Err(drift_refusal(&hook.config));
    }
    Ok(snapshot)
}

fn validate_installed(write: &RecordedWrite) -> Result<Snapshot, Error> {
    let snapshot = snapshot(&write.path)?;
    let digest = snapshot_digest(&snapshot);
    let mode = match &snapshot {
        Snapshot::File { mode, .. } => Some(mode),
        Snapshot::Absent => None,
    };
    if digest.as_ref() != Some(&write.installed) || mode.copied() != Some(write.mode) {
        Err(drift_refusal(&write.path))
    } else {
        Ok(snapshot)
    }
}

fn rollback_unapply(record: &Record, installed: &[Snapshot], restored: &[usize]) {
    for index in restored.iter().rev() {
        let write = &record.writes[*index];
        let _ = restore_snapshot(&write.path, &installed[*index]);
    }
}

fn restore_snapshot(path: &Path, snapshot: &Snapshot) -> Result<(), Error> {
    match snapshot {
        Snapshot::Absent => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(io_fault("remove installed file", path, source)),
        },
        Snapshot::File { bytes, mode } => publish(path, bytes, *mode),
    }
}

fn prepare_unapplied(
    repository: &Repository,
    record: Record,
    hook_observed: &Snapshot,
) -> Result<Plan, Error> {
    let operations = record
        .writes
        .iter()
        .map(|write| {
            let (change, desired, mode) = match &write.prior {
                Snapshot::Absent => (
                    Change::Remove,
                    Digest::compute(Domain::InstallArtifact, &[]),
                    0,
                ),
                Snapshot::File { bytes, mode } => (
                    Change::Replace,
                    Digest::compute(Domain::InstallArtifact, bytes),
                    *mode,
                ),
            };
            Operation {
                path: write.path.clone(),
                change,
                observed: Observation::File {
                    digest: write.installed.clone(),
                    mode: write.mode,
                },
                desired,
                mode,
            }
        })
        .collect();
    Ok(Plan {
        version: PLAN_VERSION,
        action: Action::Unapplied,
        repository: repository.worktree_root().to_path_buf(),
        operations,
        hook: HookSelection {
            config: record.hook.config,
            observed: observation(hook_observed),
            previous: HookValue::Value {
                value: record.hook.selected,
            },
            selected: HookValue::from_option(record.hook.previous),
            mode: snapshot_mode(&record.hook.prior).unwrap_or(0),
            change: if record.hook.changed {
                Change::Replace
            } else {
                Change::Retain
            },
        },
        digest: record.plan,
    })
}

fn publish(path: &Path, bytes: &[u8], mode: u32) -> Result<(), Error> {
    let parent = path.parent().ok_or_else(|| {
        fault(
            "install::destination",
            format!("{} has no parent", path.display()),
        )
    })?;
    fs::create_dir_all(parent)
        .map_err(|source| io_fault("create install directory", parent, source))?;
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
    wire::decode(&bytes).map_err(|source| fault("install::record-invalid", source.to_string()))
}

fn seeded_policy(path: PathBuf) -> Result<Vec<u8>, Error> {
    let existing = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(source) => return Err(io_fault("read tracked configuration", &path, source)),
    };
    let text = String::from_utf8(existing).map_err(|_| {
        refusal(
            "install::tracked-config-encoding",
            Some("UTF-8 TOML".to_owned()),
            Some(path.display().to_string()),
            "repair spice.toml before installing",
        )
    })?;
    if !text.is_empty() {
        toml::from_str::<toml::Value>(&text).map_err(|source| {
            refusal(
                "install::tracked-config-invalid",
                Some("valid TOML".to_owned()),
                Some(source.to_string()),
                "repair spice.toml before installing",
            )
        })?;
    }
    seed_tastes(text)
}

fn seed_tastes(mut text: String) -> Result<Vec<u8>, Error> {
    let parsed = if text.is_empty() {
        toml::Value::Table(toml::Table::new())
    } else {
        toml::from_str::<toml::Value>(&text)
            .map_err(|source| fault("install::tracked-config-invalid", source.to_string()))?
    };
    let present = parsed.get("taste").and_then(toml::Value::as_table);
    for name in TASTE_NAMES {
        if present.is_some_and(|table| table.contains_key(name)) {
            continue;
        }
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(taste_block(name)?);
    }
    toml::from_str::<toml::Value>(&text).map_err(|source| {
        refusal(
            "install::tracked-config-shape",
            Some("taste must be a table that accepts generated seed rows".to_owned()),
            Some(source.to_string()),
            "repair the taste configuration before installing",
        )
    })?;
    Ok(text.into_bytes())
}

fn taste_block(name: &str) -> Result<&'static str, Error> {
    let heading = format!("[taste.{name}]");
    let start = TRACKED_POLICY.find(&heading).ok_or_else(|| {
        fault(
            "install::seed-missing",
            format!("compiled policy omitted {heading}"),
        )
    })?;
    let rest = &TRACKED_POLICY[start..];
    let end = rest[heading.len()..]
        .find("\n[taste.")
        .map_or(rest.len(), |offset| heading.len() + offset + 1);
    Ok(rest[..end].trim_end())
}

fn seeded_baseline(path: PathBuf) -> Result<Vec<u8>, Error> {
    match fs::read(&path) {
        Ok(bytes) => {
            let text = String::from_utf8(bytes.clone()).map_err(|_| {
                refusal(
                    "install::baseline-encoding",
                    Some("UTF-8 TOML".to_owned()),
                    Some(path.display().to_string()),
                    "repair baseline.toml before installing",
                )
            })?;
            toml::from_str::<toml::Value>(&text).map_err(|source| {
                refusal(
                    "install::baseline-invalid",
                    Some("valid TOML".to_owned()),
                    Some(source.to_string()),
                    "repair baseline.toml before installing",
                )
            })?;
            Ok(bytes)
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(BASELINE.to_vec()),
        Err(source) => Err(io_fault("read baseline", &path, source)),
    }
}

fn pre_commit(executable: &Path) -> String {
    format!(
        "#!/bin/sh\nset -eu\nexec {} gate\n",
        shell_word(&executable.display().to_string())
    )
}

fn systemd_unit(executable: &Path, root: &Path) -> String {
    format!(
        concat!(
            "[Unit]\nDescription=Spice repository resident\n\n",
            "[Service]\nType=simple\nExecStart={} up\nWorkingDirectory={}\n",
            "Restart=on-failure\n\n[Install]\nWantedBy=default.target\n",
        ),
        systemd_word(&executable.display().to_string()),
        systemd_word(&root.display().to_string()),
    )
}

fn launchd_unit(executable: &Path, root: &Path, suffix: &str) -> String {
    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
            "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" ",
            "\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
            "<plist version=\"1.0\"><dict>\n",
            "<key>Label</key><string>dev.spice.{}</string>\n",
            "<key>ProgramArguments</key><array><string>{}</string><string>up</string></array>\n",
            "<key>WorkingDirectory</key><string>{}</string>\n",
            "<key>KeepAlive</key><true/>\n</dict></plist>\n",
        ),
        suffix,
        xml(&executable.display().to_string()),
        xml(&root.display().to_string()),
    )
}

fn shell_word(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn systemd_word(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[derive(Clone, Debug)]
struct ParsedHook {
    value: Option<String>,
    line: Option<usize>,
    core_end: Option<usize>,
}

fn parse_hook(text: &str) -> Result<ParsedHook, Error> {
    let mut section = String::new();
    let mut value = None;
    let mut line_index = None;
    let mut core_end = None;
    for (index, line) in text.split('\n').enumerate() {
        let trimmed = line.trim();
        if let Some(name) = section_name(trimmed) {
            if section == "core" && core_end.is_none() {
                core_end = Some(index);
            }
            section = name;
            continue;
        }
        if section != "core" || trimmed.is_empty() || trimmed.starts_with(['#', ';']) {
            continue;
        }
        let Some(found) = config_value(trimmed, "hookspath")? else {
            continue;
        };
        if value.is_some() {
            return Err(refusal(
                "install::hook-setting-ambiguous",
                Some("at most one core.hooksPath".to_owned()),
                Some("multiple values".to_owned()),
                "reduce core.hooksPath to one value before installing",
            ));
        }
        value = Some(found);
        line_index = Some(index);
    }
    if section == "core" && core_end.is_none() {
        core_end = Some(text.split('\n').count());
    }
    Ok(ParsedHook {
        value,
        line: line_index,
        core_end,
    })
}

fn section_name(line: &str) -> Option<String> {
    let body = line.strip_prefix('[')?.strip_suffix(']')?;
    Some(body.split_whitespace().next()?.to_ascii_lowercase())
}

fn config_value(line: &str, expected: &str) -> Result<Option<String>, Error> {
    let (key, raw) = match line.split_once('=') {
        Some((key, raw)) => (key.trim(), raw.trim()),
        None => {
            let mut parts = line.splitn(2, char::is_whitespace);
            (
                parts.next().unwrap_or_default(),
                parts.next().unwrap_or_default().trim(),
            )
        }
    };
    if !key.eq_ignore_ascii_case(expected) {
        return Ok(None);
    }
    decode_config_value(raw).map(Some)
}

fn decode_config_value(raw: &str) -> Result<String, Error> {
    if !raw.starts_with('"') {
        return Ok(raw.to_owned());
    }
    if !raw.ends_with('"') || raw.len() < 2 {
        return Err(config_shape_refusal(raw));
    }
    let mut decoded = String::new();
    let mut escaped = false;
    for character in raw[1..raw.len() - 1].chars() {
        if escaped {
            match character {
                '\\' | '"' => decoded.push(character),
                'n' => decoded.push('\n'),
                't' => decoded.push('\t'),
                'b' => decoded.push('\u{0008}'),
                _ => return Err(config_shape_refusal(raw)),
            }
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            decoded.push(character);
        }
    }
    if escaped {
        return Err(config_shape_refusal(raw));
    }
    Ok(decoded)
}

fn set_hook(text: &str, value: Option<&str>) -> Result<String, Error> {
    let parsed = parse_hook(text)?;
    let mut lines = text.split('\n').map(str::to_owned).collect::<Vec<_>>();
    let replacement = value.map(|value| format!("\thooksPath = {}", encode_config_value(value)));
    match (parsed.line, replacement) {
        (Some(index), Some(line)) => lines[index] = line,
        (Some(index), None) => {
            lines.remove(index);
        }
        (None, Some(line)) => insert_hook(&mut lines, parsed.core_end, line),
        (None, None) => {}
    }
    Ok(lines.join("\n"))
}

fn insert_hook(lines: &mut Vec<String>, core_end: Option<usize>, line: String) {
    match core_end {
        Some(index) => lines.insert(index.min(lines.len()), line),
        None => {
            if lines.last().is_some_and(|line| !line.is_empty()) {
                lines.push(String::new());
            }
            lines.push("[core]".to_owned());
            lines.push(line);
        }
    }
}

fn encode_config_value(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\t', "\\t")
    )
}

fn snapshot_mode(snapshot: &Snapshot) -> Option<u32> {
    match snapshot {
        Snapshot::Absent => None,
        Snapshot::File { mode, .. } => Some(*mode),
    }
}

fn canonical_file(path: &Path, reason: &str) -> Result<PathBuf, Error> {
    let canonical =
        fs::canonicalize(path).map_err(|source| io_fault("resolve executable", path, source))?;
    let metadata = fs::metadata(&canonical)
        .map_err(|source| io_fault("inspect executable", &canonical, source))?;
    if !metadata.is_file() {
        return Err(refusal(
            reason,
            Some("regular file".to_owned()),
            Some(canonical.display().to_string()),
            "run the installed Rust spice executable directly",
        ));
    }
    Ok(canonical)
}

fn canonical_directory(path: &Path, reason: &str) -> Result<PathBuf, Error> {
    let canonical =
        fs::canonicalize(path).map_err(|source| io_fault("resolve home", path, source))?;
    let metadata =
        fs::metadata(&canonical).map_err(|source| io_fault("inspect home", &canonical, source))?;
    if !metadata.is_dir() {
        return Err(refusal(
            reason,
            Some("directory".to_owned()),
            Some(canonical.display().to_string()),
            "select an existing home directory",
        ));
    }
    Ok(canonical)
}

fn record_path(repository: &Repository) -> PathBuf {
    repository.common_dir().join(".spice/install")
}

fn repository_error(error: crate::repository::ResolveError) -> Error {
    refusal(
        "install::repository",
        Some("one containing Git repository".to_owned()),
        Some(error.to_string()),
        "run spice install from inside the repository to configure",
    )
}

fn drift_refusal(path: &Path) -> Error {
    refusal(
        "install::drift",
        Some("the bytes and mode recorded at apply".to_owned()),
        Some(path.display().to_string()),
        "restore the installed bytes or reconcile the path before unapplying",
    )
}

fn config_shape_refusal(observed: &str) -> Error {
    refusal(
        "install::git-config-shape",
        Some("one ordinary core.hooksPath value".to_owned()),
        Some(observed.to_owned()),
        "normalize core.hooksPath before installing",
    )
}

fn refusal(
    reason: &str,
    expected: Option<String>,
    observed: Option<String>,
    remedy: &str,
) -> Error {
    Error::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: None,
        expected,
        observed,
        remedy: remedy.to_owned(),
    })
}

fn fault(reason: &str, detail: impl Into<String>) -> Error {
    Error::Fault(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.into(),
    })
}

fn io_fault(operation: &str, path: &Path, source: std::io::Error) -> Error {
    fault(
        "install::filesystem",
        format!("{operation} at {}: {source}", path.display()),
    )
}

fn encode_fault(subject: &str, source: impl fmt::Display) -> Error {
    fault("install::encoding", format!("encode {subject}: {source}"))
}

#[cfg(unix)]
fn file_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode() & 0o777
}

#[cfg(not(unix))]
fn file_mode(_metadata: &fs::Metadata) -> u32 {
    DATA_MODE
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn effective_mode(mode: u32) -> u32 {
    mode
}

#[cfg(not(unix))]
fn effective_mode(_mode: u32) -> u32 {
    DATA_MODE
}
