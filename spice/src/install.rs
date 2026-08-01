//! Digest-bound, repository-local Rust coexistence installation.
//!
//! It keeps the ambient `spice` entrypoint and records every namespaced asset.
//! Plans are side-effect free; apply re-plans and reverses only exact images.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wire::{Digest, Domain};

use crate::repository::Repository;

mod error;
mod files;
mod path;

pub use error::InstallError;
use files::{
    cleanup_parents, ensure_directory, matches_generated, materialize_generated, observe,
    publish_image, publish_operation, restore_operation,
};
use path::{relative_parents, validate_relative};

const PLAN_VERSION: u16 = 1;
const RECORD_VERSION: u16 = 1;
const LEGACY_RECORD_PATH: &str = ".spice/install";
const RECORD_PATH: &str = "install.json";
const RECORD_MODE: u32 = 0o600;
const DIRECTORY_MODE: u32 = 0o700;
const DATA_MODE: u32 = 0o644;
const EXECUTABLE_MODE: u32 = 0o755;

const EXCLUDE_PATH: &str = "info/exclude";
const ENTRY_PATH: &str = ".spice/rust/hooks/entry.sh";
const STATIC_PATH: &str = ".spice/rust/hooks/static/stage.sh";
const SKILL_PATH: &str = ".agents/skills/spice-rust/SKILL.md";
const BINARY_PATH: &str = ".spice/rust/bin/spice-rust";

const EXCLUDE_BEGIN: &str = "# BEGIN spice-rust coexistence";
const EXCLUDE_END: &str = "# END spice-rust coexistence";
const EXCLUDE_BLOCK: &str = concat!(
    "# BEGIN spice-rust coexistence\n",
    "/.spice/rust/\n",
    "/.agents/skills/spice-rust/\n",
    "# END spice-rust coexistence\n",
);
const SKILL: &str = include_str!("install/skill.md");

/// One Rust-owned asset in canonical installation order.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Asset {
    Exclusions,
    HookEntry,
    HookStatic,
    Skill,
    Binary,
}

/// Root against which an operation's relative path resolves.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Location {
    Worktree,
    CommonGit,
}

impl Location {
    fn root(self, repository: &Repository) -> &Path {
        match self {
            Self::Worktree => repository.worktree_root(),
            Self::CommonGit => repository.common_dir(),
        }
    }
}

/// Complete bytes, mode, and content identity of one regular file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileImage {
    mode: u32,
    bytes: Vec<u8>,
    digest: Digest,
}

impl FileImage {
    fn new(mode: u32, bytes: Vec<u8>) -> Self {
        let digest = Digest::compute(Domain::InstallArtifact, &bytes);
        Self {
            mode,
            bytes,
            digest,
        }
    }

    /// Installed permission bits.
    #[must_use]
    pub const fn mode(&self) -> u32 {
        self.mode
    }

    /// Exact file bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Content identity of the file bytes.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.digest
    }

    fn validate(&self) -> bool {
        self.mode <= 0o777 && self.digest == Digest::compute(Domain::InstallArtifact, &self.bytes)
    }

    fn generated(&self) -> GeneratedImage {
        GeneratedImage {
            mode: self.mode,
            digest: self.digest.clone(),
        }
    }
}

/// Mode and content identity of bytes an install operation will publish.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedImage {
    mode: u32,
    digest: Digest,
}

impl GeneratedImage {
    fn new(mode: u32, bytes: &[u8]) -> Self {
        Self {
            mode,
            digest: Digest::compute(Domain::InstallArtifact, bytes),
        }
    }

    /// Installed permission bits.
    #[must_use]
    pub const fn mode(&self) -> u32 {
        self.mode
    }

    /// Identity of the exact bytes selected by the plan.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.digest
    }

    fn validate(&self) -> bool {
        self.mode <= 0o777
    }
}

/// One exact file replacement and the state it observed before installation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    asset: Asset,
    location: Location,
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous: Option<FileImage>,
    generated: GeneratedImage,
    introduced_parents: Vec<String>,
}

impl Operation {
    /// Kind of asset this operation owns.
    #[must_use]
    pub const fn asset(&self) -> Asset {
        self.asset
    }

    /// Root against which [`Self::path`] resolves.
    #[must_use]
    pub const fn location(&self) -> Location {
        self.location
    }

    /// Repository-relative destination spelling.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Prior regular-file image, or absence when this install introduces it.
    #[must_use]
    pub const fn previous(&self) -> Option<&FileImage> {
        self.previous.as_ref()
    }

    /// Exact image this install places.
    #[must_use]
    pub const fn generated(&self) -> &GeneratedImage {
        &self.generated
    }

    /// Parent directories absent when the plan was derived, outermost first.
    #[must_use]
    pub fn introduced_parents(&self) -> &[String] {
        &self.introduced_parents
    }

    fn destination(&self, repository: &Repository) -> PathBuf {
        self.location.root(repository).join(&self.path)
    }
}

/// Side-effect-free canonical installation plan.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallPlan {
    version: u16,
    operations: Vec<Operation>,
}

impl InstallPlan {
    /// Install schema version carried by canonical plan bytes.
    #[must_use]
    pub const fn version(&self) -> u16 {
        self.version
    }

    /// Operations in their sole application order.
    #[must_use]
    pub fn operations(&self) -> &[Operation] {
        &self.operations
    }

    /// Canonical plan bytes consumed by preview and digest admission.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, InstallError> {
        wire::encode(self).map_err(InstallError::Encode)
    }

    /// Dedicated identity of the canonical plan bytes.
    pub fn digest(&self) -> Result<Digest, InstallError> {
        Ok(Digest::compute(
            Domain::InstallPlan,
            &self.canonical_bytes()?,
        ))
    }

    fn validate(&self) -> Result<(), InstallError> {
        if self.version != PLAN_VERSION {
            return Err(InstallError::InvalidRecord(format!(
                "install plan version {} is not {PLAN_VERSION}",
                self.version
            )));
        }
        let expected = [
            Asset::Exclusions,
            Asset::HookEntry,
            Asset::HookStatic,
            Asset::Skill,
        ];
        if self.operations.len() != expected.len() && self.operations.len() != expected.len() + 1 {
            return Err(InstallError::InvalidRecord(
                "install plan has an incomplete or extra operation inventory".to_owned(),
            ));
        }
        for (operation, asset) in self.operations.iter().zip(expected) {
            if operation.asset != asset {
                return Err(InstallError::InvalidRecord(
                    "install plan operation order is not canonical".to_owned(),
                ));
            }
        }
        if self.operations.len() > expected.len()
            && self.operations.last().map(|operation| operation.asset) != Some(Asset::Binary)
        {
            return Err(InstallError::InvalidRecord(
                "optional binary is not the final operation".to_owned(),
            ));
        }

        let mut introduced = BTreeSet::new();
        for operation in &self.operations {
            validate_operation(operation, &mut introduced)?;
        }
        Ok(())
    }
}

/// Durable progress state for apply and reversal.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallStatus {
    Applying,
    Complete,
    Unapplying,
    Uninstalled,
}

/// Current strict installation receipt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallReceipt {
    plan: InstallPlan,
    digest: Digest,
    status: InstallStatus,
    completed: usize,
}

impl InstallReceipt {
    /// Exact plan retained for reversal and repeat application.
    #[must_use]
    pub const fn plan(&self) -> &InstallPlan {
        &self.plan
    }

    /// Plan identity admitted by apply.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.digest
    }

    /// Durable progress state.
    #[must_use]
    pub const fn status(&self) -> InstallStatus {
        self.status
    }

    /// Operations durably acknowledged in the current direction.
    #[must_use]
    pub const fn completed(&self) -> usize {
        self.completed
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct InstallRecord {
    version: u16,
    plan: InstallPlan,
    plan_digest: Digest,
    status: InstallStatus,
    completed: usize,
}

impl InstallRecord {
    fn new(plan: InstallPlan) -> Result<Self, InstallError> {
        let plan_digest = plan.digest()?;
        Ok(Self {
            version: RECORD_VERSION,
            plan,
            plan_digest,
            status: InstallStatus::Applying,
            completed: 0,
        })
    }

    fn validate(&self) -> Result<(), InstallError> {
        if self.version != RECORD_VERSION {
            return Err(InstallError::InvalidRecord(format!(
                "install record version {} is not {RECORD_VERSION}",
                self.version
            )));
        }
        self.plan.validate()?;
        if self.plan_digest != self.plan.digest()? {
            return Err(InstallError::InvalidRecord(
                "install record plan digest does not match its canonical plan".to_owned(),
            ));
        }
        if self.completed > self.plan.operations.len() {
            return Err(InstallError::InvalidRecord(
                "install progress is past the operation inventory".to_owned(),
            ));
        }
        if matches!(
            self.status,
            InstallStatus::Complete | InstallStatus::Uninstalled
        ) && self.completed != self.plan.operations.len()
        {
            return Err(InstallError::InvalidRecord(
                "terminal install record has incomplete progress".to_owned(),
            ));
        }
        Ok(())
    }

    fn receipt(&self) -> InstallReceipt {
        InstallReceipt {
            plan: self.plan.clone(),
            digest: self.plan_digest.clone(),
            status: self.status,
            completed: self.completed,
        }
    }
}

/// Inspect one repository and derive the exact coexistence installation plan.
pub fn plan(
    repository: &Repository,
    rust_binary: Option<&Path>,
) -> Result<InstallPlan, InstallError> {
    if let Some(record) = read_record(repository)? {
        record.validate()?;
        verify_binary_context(&record.plan, rust_binary)?;
        verify_progress(repository, &record)?;
        return Ok(record.plan);
    }
    build_plan(repository, rust_binary)
}

/// Apply only the exact current plan named by `expected`.
pub fn apply(
    repository: &Repository,
    rust_binary: Option<&Path>,
    expected: &Digest,
) -> Result<InstallReceipt, InstallError> {
    apply_with_interruption(repository, rust_binary, expected, None)
}

/// Reverse the complete current install without touching drifted targets.
pub fn unapply(repository: &Repository) -> Result<InstallReceipt, InstallError> {
    unapply_with_interruption(repository, None)
}

fn build_plan(
    repository: &Repository,
    rust_binary: Option<&Path>,
) -> Result<InstallPlan, InstallError> {
    let mut virtual_directories = BTreeSet::new();
    let mut operations = Vec::new();

    let exclude_previous = observe(&repository.common_dir().join(EXCLUDE_PATH))?;
    let exclude_bytes = exclusions(exclude_previous.as_ref().map(FileImage::bytes))?;
    let exclude_mode = exclude_previous.as_ref().map_or(DATA_MODE, FileImage::mode);
    operations.push(operation(
        repository,
        &mut virtual_directories,
        Asset::Exclusions,
        Location::CommonGit,
        EXCLUDE_PATH,
        exclude_previous,
        GeneratedImage::new(exclude_mode, &exclude_bytes),
    )?);

    for (asset, path, bytes, mode) in [
        (
            Asset::HookEntry,
            ENTRY_PATH,
            steer::hook::STAGES[0].bytes().as_bytes(),
            EXECUTABLE_MODE,
        ),
        (
            Asset::HookStatic,
            STATIC_PATH,
            steer::hook::STAGES[1].bytes().as_bytes(),
            EXECUTABLE_MODE,
        ),
        (Asset::Skill, SKILL_PATH, SKILL.as_bytes(), DATA_MODE),
    ] {
        operations.push(operation(
            repository,
            &mut virtual_directories,
            asset,
            Location::Worktree,
            path,
            observe(&repository.worktree_root().join(path))?,
            GeneratedImage::new(mode, bytes),
        )?);
    }

    if let Some(binary) = rust_binary {
        let bytes = read_binary(binary)?;
        operations.push(operation(
            repository,
            &mut virtual_directories,
            Asset::Binary,
            Location::Worktree,
            BINARY_PATH,
            observe(&repository.worktree_root().join(BINARY_PATH))?,
            GeneratedImage::new(EXECUTABLE_MODE, &bytes),
        )?);
    }

    let plan = InstallPlan {
        version: PLAN_VERSION,
        operations,
    };
    plan.validate()?;
    Ok(plan)
}

fn operation(
    repository: &Repository,
    virtual_directories: &mut BTreeSet<(Location, String)>,
    asset: Asset,
    location: Location,
    path: &str,
    previous: Option<FileImage>,
    generated: GeneratedImage,
) -> Result<Operation, InstallError> {
    let mut introduced_parents = Vec::new();
    for parent in relative_parents(path)? {
        let key = (location, parent.clone());
        if virtual_directories.insert(key) {
            let absolute = location.root(repository).join(&parent);
            match fs::symlink_metadata(&absolute) {
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_) => return Err(InstallError::PathShape(absolute)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    introduced_parents.push(parent);
                }
                Err(source) => return Err(InstallError::io("inspect directory", absolute, source)),
            }
        }
    }
    Ok(Operation {
        asset,
        location,
        path: path.to_owned(),
        previous,
        generated,
        introduced_parents,
    })
}

fn exclusions(previous: Option<&[u8]>) -> Result<Vec<u8>, InstallError> {
    let previous = previous.unwrap_or_default();
    let text = std::str::from_utf8(previous).map_err(|_| InstallError::ExclusionsNotUtf8)?;
    let begin_count = text.matches(EXCLUDE_BEGIN).count();
    let end_count = text.matches(EXCLUDE_END).count();
    if begin_count == 1 && end_count == 1 && text.matches(EXCLUDE_BLOCK).count() == 1 {
        return Ok(previous.to_vec());
    }
    if begin_count != 0 || end_count != 0 {
        return Err(InstallError::DivergentExclusions);
    }
    let mut generated = previous.to_vec();
    if !generated.is_empty() && !generated.ends_with(b"\n") {
        generated.push(b'\n');
    }
    generated.extend_from_slice(EXCLUDE_BLOCK.as_bytes());
    Ok(generated)
}

fn read_binary(path: &Path) -> Result<Vec<u8>, InstallError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| InstallError::io("inspect Rust binary", path, source))?;
    if !metadata.file_type().is_file() {
        return Err(InstallError::BinaryNotRegular(path.to_path_buf()));
    }
    let bytes =
        fs::read(path).map_err(|source| InstallError::io("read Rust binary", path, source))?;
    if bytes.is_empty() {
        return Err(InstallError::BinaryEmpty(path.to_path_buf()));
    }
    Ok(bytes)
}

fn verify_binary_context(
    plan: &InstallPlan,
    rust_binary: Option<&Path>,
) -> Result<(), InstallError> {
    let planned = plan
        .operations
        .iter()
        .find(|operation| operation.asset == Asset::Binary);
    match (planned, rust_binary) {
        (None, None) => Ok(()),
        (Some(operation), Some(path)) => {
            let bytes = read_binary(path)?;
            let observed = GeneratedImage::new(EXECUTABLE_MODE, &bytes);
            if observed == operation.generated {
                Ok(())
            } else {
                Err(InstallError::BinaryContextChanged)
            }
        }
        _ => Err(InstallError::BinaryContextChanged),
    }
}

fn apply_with_interruption(
    repository: &Repository,
    rust_binary: Option<&Path>,
    expected: &Digest,
    interrupt_after_write: Option<usize>,
) -> Result<InstallReceipt, InstallError> {
    let existing = read_record(repository)?;
    let mut record = match existing {
        Some(record) => {
            record.validate()?;
            verify_binary_context(&record.plan, rust_binary)?;
            if &record.plan_digest != expected {
                return Err(InstallError::DigestMismatch {
                    expected: expected.clone(),
                    observed: record.plan_digest,
                });
            }
            match record.status {
                InstallStatus::Complete => {
                    verify_progress(repository, &record)?;
                    return Ok(record.receipt());
                }
                InstallStatus::Unapplying => return Err(InstallError::UnapplyInProgress),
                InstallStatus::Uninstalled => {
                    verify_progress(repository, &record)?;
                    InstallRecord {
                        status: InstallStatus::Applying,
                        completed: 0,
                        ..record
                    }
                }
                InstallStatus::Applying => {
                    verify_progress(repository, &record)?;
                    record
                }
            }
        }
        None => {
            let plan = build_plan(repository, rust_binary)?;
            let observed = plan.digest()?;
            if &observed != expected {
                return Err(InstallError::DigestMismatch {
                    expected: expected.clone(),
                    observed,
                });
            }
            InstallRecord::new(plan)?
        }
    };

    let generated = record
        .plan
        .operations
        .iter()
        .map(|operation| materialize_generated(operation, rust_binary))
        .collect::<Result<Vec<_>, _>>()?;
    write_record(repository, &record)?;
    while record.completed < record.plan.operations.len() {
        let index = record.completed;
        let operation = &record.plan.operations[index];
        let current = observe(&operation.destination(repository))?;
        if !matches_generated(current.as_ref(), operation) {
            if current != operation.previous {
                return Err(drift(operation, current));
            }
            publish_operation(repository, operation, &generated[index])?;
        }
        if interrupt_after_write == Some(index) {
            return Err(InstallError::Interrupted { operation: index });
        }
        record.completed += 1;
        write_record(repository, &record)?;
    }
    record.status = InstallStatus::Complete;
    write_record(repository, &record)?;
    Ok(record.receipt())
}

fn unapply_with_interruption(
    repository: &Repository,
    interrupt_after_write: Option<usize>,
) -> Result<InstallReceipt, InstallError> {
    let mut record = read_record(repository)?.ok_or(InstallError::RecordMissing)?;
    record.validate()?;
    match record.status {
        InstallStatus::Applying => return Err(InstallError::ApplyInProgress),
        InstallStatus::Uninstalled => {
            verify_progress(repository, &record)?;
            return Ok(record.receipt());
        }
        InstallStatus::Complete => {
            verify_progress(repository, &record)?;
            record.status = InstallStatus::Unapplying;
            record.completed = 0;
            write_record(repository, &record)?;
        }
        InstallStatus::Unapplying => verify_progress(repository, &record)?,
    }

    while record.completed < record.plan.operations.len() {
        let reverse = record.plan.operations.len() - 1 - record.completed;
        let operation = &record.plan.operations[reverse];
        let current = observe(&operation.destination(repository))?;
        if current != operation.previous {
            if !matches_generated(current.as_ref(), operation) {
                return Err(drift(operation, current));
            }
            restore_operation(repository, operation)?;
        }
        cleanup_parents(repository, operation)?;
        if interrupt_after_write == Some(reverse) {
            return Err(InstallError::Interrupted { operation: reverse });
        }
        record.completed += 1;
        write_record(repository, &record)?;
    }
    record.status = InstallStatus::Uninstalled;
    write_record(repository, &record)?;
    Ok(record.receipt())
}

fn verify_progress(repository: &Repository, record: &InstallRecord) -> Result<(), InstallError> {
    for (index, operation) in record.plan.operations.iter().enumerate() {
        let observed = observe(&operation.destination(repository))?;
        let valid = match record.status {
            InstallStatus::Applying => {
                if index < record.completed {
                    matches_generated(observed.as_ref(), operation)
                } else if index == record.completed {
                    matches_generated(observed.as_ref(), operation)
                        || observed == operation.previous
                } else {
                    observed == operation.previous
                }
            }
            InstallStatus::Complete => matches_generated(observed.as_ref(), operation),
            InstallStatus::Unapplying => {
                let reversed = record.plan.operations.len() - 1 - index;
                if reversed < record.completed {
                    observed == operation.previous
                } else if reversed == record.completed {
                    observed == operation.previous
                        || matches_generated(observed.as_ref(), operation)
                } else {
                    matches_generated(observed.as_ref(), operation)
                }
            }
            InstallStatus::Uninstalled => observed == operation.previous,
        };
        if !valid {
            return Err(drift(operation, observed));
        }
    }
    Ok(())
}

fn read_record(repository: &Repository) -> Result<Option<InstallRecord>, InstallError> {
    refuse_legacy_record(repository)?;
    let state_root = repository.state_root();
    match fs::symlink_metadata(&state_root) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(InstallError::PathShape(state_root)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(InstallError::io(
                "inspect install record directory",
                state_root,
                source,
            ));
        }
    }
    let path = record_path(repository);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(InstallError::io("inspect install record", path, source)),
    };
    if !metadata.file_type().is_file() {
        return Err(InstallError::PathShape(path));
    }
    let bytes =
        fs::read(&path).map_err(|source| InstallError::io("read install record", &path, source))?;
    let record: InstallRecord = wire::decode(&bytes).map_err(InstallError::Decode)?;
    record.validate()?;
    Ok(Some(record))
}

fn refuse_legacy_record(repository: &Repository) -> Result<(), InstallError> {
    let path = repository.common_dir().join(LEGACY_RECORD_PATH);
    match fs::symlink_metadata(&path) {
        Ok(_) => Err(InstallError::LegacyRecord(path)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(InstallError::io(
            "inspect legacy install record",
            path,
            source,
        )),
    }
}

fn write_record(repository: &Repository, record: &InstallRecord) -> Result<(), InstallError> {
    record.validate()?;
    let path = record_path(repository);
    ensure_private_record_parent(repository.common_dir())?;
    let bytes = wire::encode(record).map_err(InstallError::Encode)?;
    publish_image(&path, &FileImage::new(RECORD_MODE, bytes), true)
}

fn record_path(repository: &Repository) -> PathBuf {
    repository.state_root().join(RECORD_PATH)
}

fn ensure_private_record_parent(common: &Path) -> Result<(), InstallError> {
    ensure_directory(common, crate::repository::STATE_DIRECTORY)?;
    Ok(())
}

fn validate_operation(
    operation: &Operation,
    introduced: &mut BTreeSet<(Location, String)>,
) -> Result<(), InstallError> {
    let (location, path, fixed_mode) = asset_contract(operation.asset);
    let expected_mode = match operation.asset {
        Asset::Exclusions => operation
            .previous
            .as_ref()
            .map_or(DATA_MODE, FileImage::mode),
        _ => fixed_mode,
    };
    if operation.location != location
        || operation.path != path
        || operation.generated.mode != expected_mode
    {
        return Err(InstallError::InvalidRecord(format!(
            "install operation {:?} does not match its fixed contract",
            operation.asset
        )));
    }
    validate_relative(&operation.path)?;
    if !operation.generated.validate()
        || operation
            .previous
            .as_ref()
            .is_some_and(|image| !image.validate())
    {
        return Err(InstallError::InvalidRecord(
            "install operation carries an invalid file image".to_owned(),
        ));
    }
    match operation.asset {
        Asset::Exclusions => {
            let bytes = exclusions(operation.previous.as_ref().map(FileImage::bytes))?;
            let expected = GeneratedImage::new(expected_mode, &bytes);
            if operation.generated != expected {
                return Err(InstallError::InvalidRecord(
                    "exclusion operation is not the exact managed block edit".to_owned(),
                ));
            }
        }
        Asset::HookEntry
            if operation.generated
                != GeneratedImage::new(
                    EXECUTABLE_MODE,
                    steer::hook::STAGES[0].bytes().as_bytes(),
                ) =>
        {
            return Err(InstallError::InvalidRecord(
                "entry hook differs from this build".to_owned(),
            ));
        }
        Asset::HookStatic
            if operation.generated
                != GeneratedImage::new(
                    EXECUTABLE_MODE,
                    steer::hook::STAGES[1].bytes().as_bytes(),
                ) =>
        {
            return Err(InstallError::InvalidRecord(
                "static hook differs from this build".to_owned(),
            ));
        }
        Asset::Skill if operation.generated != GeneratedImage::new(DATA_MODE, SKILL.as_bytes()) => {
            return Err(InstallError::InvalidRecord(
                "generated skill differs from this build".to_owned(),
            ));
        }
        Asset::Binary
            if operation.generated.digest == Digest::compute(Domain::InstallArtifact, &[]) =>
        {
            return Err(InstallError::InvalidRecord(
                "explicit Rust binary is empty".to_owned(),
            ));
        }
        _ => {}
    }
    let parents = relative_parents(&operation.path)?;
    if operation.previous.is_some() && !operation.introduced_parents.is_empty() {
        return Err(InstallError::InvalidRecord(
            "an existing target cannot have introduced parent directories".to_owned(),
        ));
    }
    let mut minimum_parent = 0;
    for relative in &operation.introduced_parents {
        validate_relative(relative)?;
        let Some(offset) = parents[minimum_parent..]
            .iter()
            .position(|parent| parent == relative)
        else {
            return Err(InstallError::InvalidRecord(
                "introduced directories are not canonical target parents".to_owned(),
            ));
        };
        minimum_parent += offset + 1;
        if !introduced.insert((operation.location, relative.clone())) {
            return Err(InstallError::InvalidRecord(
                "introduced directory is duplicated across operations".to_owned(),
            ));
        }
    }
    Ok(())
}

fn asset_contract(asset: Asset) -> (Location, &'static str, u32) {
    match asset {
        Asset::Exclusions => (Location::CommonGit, EXCLUDE_PATH, DATA_MODE),
        Asset::HookEntry => (Location::Worktree, ENTRY_PATH, EXECUTABLE_MODE),
        Asset::HookStatic => (Location::Worktree, STATIC_PATH, EXECUTABLE_MODE),
        Asset::Skill => (Location::Worktree, SKILL_PATH, DATA_MODE),
        Asset::Binary => (Location::Worktree, BINARY_PATH, EXECUTABLE_MODE),
    }
}

fn drift(operation: &Operation, observed: Option<FileImage>) -> InstallError {
    InstallError::Drift {
        path: operation.path.clone(),
        observed: observed.map(|image| image.digest),
    }
}

#[cfg(test)]
mod tests;
