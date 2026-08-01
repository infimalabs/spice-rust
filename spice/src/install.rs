//! Digest-bound, repository-local Rust coexistence installation.
//!
//! It keeps the ambient `spice` entrypoint and records every namespaced asset.
//! Plans are side-effect free; apply re-plans and reverses only exact images.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wire::{Digest, Domain};

use crate::repository::Repository;

mod binding;
mod error;
mod files;
mod handle;
mod path;
mod progress;
mod record;

use binding::{BoundDestination, bind_destination, validate_operation_binding};
pub use binding::{Generation, ParentObservation};
pub use error::InstallError;
use files::{
    cleanup_operation, matches_generated, materialize_generated, observe, observe_bound,
    publish_operation, restore_operation,
};
use path::{relative_parents, validate_relative};
use progress::{apply_with_interruption, unapply_with_interruption, verify_progress};
use record::read_record;
#[cfg(test)]
use record::record_path;

const PLAN_VERSION: u16 = 2;
const RECORD_VERSION: u16 = 2;
const LEGACY_RECORDS: [(&str, &str); 2] = [
    (".spice/install", "SURFACE-1kJQkpFs"),
    (".spice/rust/install.json", "SURFACE-1kJRGpf2"),
];
const RECORD_NAME: &str = "install.json";
const RECORD_PATH: &str = ".spice-rust/install.json";
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

#[cfg(debug_assertions)]
#[doc(hidden)]
pub const TEST_PAUSE_CONTROL: &str = "SPICE_INSTALL_TEST_PAUSE_CONTROL"; // env-policy: allow
#[cfg(debug_assertions)]
#[doc(hidden)]
pub const TEST_PAUSE_ACTION: &str = "SPICE_INSTALL_TEST_PAUSE_ACTION"; // env-policy: allow
#[cfg(debug_assertions)]
#[doc(hidden)]
pub const TEST_PAUSE_PATH: &str = "SPICE_INSTALL_TEST_PAUSE_PATH"; // env-policy: allow

/// Configures the process-test mutation rendezvous before installation starts.
#[cfg(debug_assertions)]
#[doc(hidden)]
pub fn configure_test_mutation_rendezvous(
    control: Option<std::ffi::OsString>,
    action: Option<std::ffi::OsString>,
    path: Option<std::ffi::OsString>,
) {
    handle::configure_mutation_rendezvous(control, action, path);
}

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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RepositoryIdentity {
    worktree: PathBuf,
    common_git: PathBuf,
}

impl RepositoryIdentity {
    fn capture(repository: &Repository) -> Self {
        Self {
            worktree: repository.worktree_root().to_path_buf(),
            common_git: repository.common_dir().to_path_buf(),
        }
    }

    fn validate(&self, repository: &Repository) -> Result<(), InstallError> {
        if self == &Self::capture(repository) {
            Ok(())
        } else {
            Err(InstallError::InvalidRecord(
                "install record belongs to another repository".to_owned(),
            ))
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ObservedFile {
    image: FileImage,
    generation: Generation,
}

impl ObservedFile {
    fn generated(&self) -> GeneratedImage {
        self.image.generated()
    }
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
    destination: String,
    parent: ParentObservation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous: Option<ObservedFile>,
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
        match self.previous.as_ref() {
            Some(previous) => Some(&previous.image),
            None => None,
        }
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
        self.location.root(repository).join(&self.destination)
    }
}

/// Side-effect-free canonical installation plan.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallPlan {
    version: u16,
    repository: RepositoryIdentity,
    record: BoundDestination,
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

        if !self.repository.worktree.is_absolute() || !self.repository.common_git.is_absolute() {
            return Err(InstallError::InvalidRecord(
                "install repository identity is not absolute".to_owned(),
            ));
        }
        validate_relative(&self.record.destination)?;
        let mut introduced = BTreeSet::new();
        let mut destinations = BTreeSet::new();
        for operation in &self.operations {
            validate_operation(operation, &mut introduced)?;
            if !destinations.insert((operation.location, operation.destination.clone())) {
                return Err(InstallError::InvalidRecord(
                    "install plan duplicates a physical destination".to_owned(),
                ));
            }
        }
        Ok(())
    }

    fn validate_for_repository(&self, repository: &Repository) -> Result<(), InstallError> {
        self.validate()?;
        self.repository.validate(repository)?;
        binding::validate_record_binding(repository.common_dir(), &self.record, RECORD_PATH)?;
        for operation in &self.operations {
            validate_operation_binding(
                repository,
                operation.location,
                &operation.path,
                &operation.destination,
                &operation.parent,
            )?;
        }
        Ok(())
    }

    fn validate_pristine(&self, repository: &Repository) -> Result<(), InstallError> {
        self.validate_for_repository(repository)?;
        if bind_destination(repository.common_dir(), RECORD_PATH)? != self.record {
            return Err(InstallError::ParentChanged(
                repository.common_dir().join(RECORD_PATH),
            ));
        }
        for operation in &self.operations {
            let current = bind_destination(operation.location.root(repository), &operation.path)?;
            if current.destination != operation.destination || current.parent != operation.parent {
                return Err(InstallError::ParentChanged(
                    operation.location.root(repository).join(&operation.path),
                ));
            }
            let observed = observe(&operation.destination(repository))?;
            if observed != operation.previous {
                return Err(drift(operation, observed));
            }
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

/// Inspect one repository and derive the exact coexistence installation plan.
pub fn plan(
    repository: &Repository,
    rust_binary: Option<&Path>,
) -> Result<InstallPlan, InstallError> {
    if let Some(loaded) = read_record(repository)? {
        let record = loaded.record;
        verify_binary_context(&record.plan, rust_binary)?;
        verify_progress(repository, &record)?;
        if record.status == InstallStatus::Uninstalled {
            return build_plan(repository, rust_binary);
        }
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

    let (exclude_bound, exclude_previous) =
        destination_state(repository, Location::CommonGit, EXCLUDE_PATH)?;
    let exclude_image = exclude_previous.as_ref().map(|previous| &previous.image);
    let exclude_bytes = exclusions(exclude_image.map(FileImage::bytes))?;
    let exclude_mode = exclude_image.map_or(DATA_MODE, FileImage::mode);
    operations.push(operation(
        &mut virtual_directories,
        Asset::Exclusions,
        Location::CommonGit,
        EXCLUDE_PATH,
        exclude_bound,
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
        let (bound, previous) = destination_state(repository, Location::Worktree, path)?;
        operations.push(operation(
            &mut virtual_directories,
            asset,
            Location::Worktree,
            path,
            bound,
            previous,
            GeneratedImage::new(mode, bytes),
        )?);
    }

    if let Some(binary) = rust_binary {
        let bytes = read_binary(binary)?;
        let (bound, previous) = destination_state(repository, Location::Worktree, BINARY_PATH)?;
        operations.push(operation(
            &mut virtual_directories,
            Asset::Binary,
            Location::Worktree,
            BINARY_PATH,
            bound,
            previous,
            GeneratedImage::new(EXECUTABLE_MODE, &bytes),
        )?);
    }

    let record = bind_destination(repository.common_dir(), RECORD_PATH)?;
    let plan = InstallPlan {
        version: PLAN_VERSION,
        repository: RepositoryIdentity::capture(repository),
        record,
        operations,
    };
    plan.validate_pristine(repository)?;
    Ok(plan)
}

fn operation(
    virtual_directories: &mut BTreeSet<(Location, String)>,
    asset: Asset,
    location: Location,
    path: &str,
    bound: BoundDestination,
    previous: Option<ObservedFile>,
    generated: GeneratedImage,
) -> Result<Operation, InstallError> {
    let mut introduced_parents = Vec::new();
    for parent in bound.introduced_parents {
        let key = (location, parent.clone());
        if virtual_directories.insert(key) {
            introduced_parents.push(parent);
        }
    }
    Ok(Operation {
        asset,
        location,
        path: path.to_owned(),
        destination: bound.destination,
        parent: bound.parent,
        previous,
        generated,
        introduced_parents,
    })
}

fn destination_state(
    repository: &Repository,
    location: Location,
    path: &str,
) -> Result<(BoundDestination, Option<ObservedFile>), InstallError> {
    let bound = bind_destination(location.root(repository), path)?;
    let (previous, _) =
        observe_bound(location.root(repository), &bound.destination, &bound.parent)?;
    Ok((bound, previous))
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

fn validate_operation(
    operation: &Operation,
    introduced: &mut BTreeSet<(Location, String)>,
) -> Result<(), InstallError> {
    let (location, path, fixed_mode) = asset_contract(operation.asset);
    let expected_mode = match operation.asset {
        Asset::Exclusions => operation
            .previous
            .as_ref()
            .map_or(DATA_MODE, |previous| previous.image.mode()),
        _ => fixed_mode,
    };
    validate_operation_contract(operation, location, path, expected_mode)?;
    validate_generated_content(operation, expected_mode)?;
    validate_introduced_parents(operation, introduced)
}

fn validate_operation_contract(
    operation: &Operation,
    location: Location,
    path: &str,
    expected_mode: u32,
) -> Result<(), InstallError> {
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
    validate_relative(&operation.destination)?;
    let expected_parent = Path::new(&operation.parent.ancestor).join(&operation.parent.missing);
    if Path::new(&operation.destination).parent() != Some(expected_parent.as_path()) {
        return Err(InstallError::InvalidRecord(
            "install destination does not match its bound parent".to_owned(),
        ));
    }
    if !operation.generated.validate()
        || operation
            .previous
            .as_ref()
            .is_some_and(|observed| !observed.image.validate())
    {
        return Err(InstallError::InvalidRecord(
            "install operation carries an invalid file image".to_owned(),
        ));
    }
    Ok(())
}

fn validate_generated_content(
    operation: &Operation,
    expected_mode: u32,
) -> Result<(), InstallError> {
    match operation.asset {
        Asset::Exclusions => {
            let bytes = exclusions(
                operation
                    .previous
                    .as_ref()
                    .map(|previous| previous.image.bytes()),
            )?;
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
    Ok(())
}

fn validate_introduced_parents(
    operation: &Operation,
    introduced: &mut BTreeSet<(Location, String)>,
) -> Result<(), InstallError> {
    let parents = relative_parents(&operation.destination)?;
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

fn drift(operation: &Operation, observed: Option<ObservedFile>) -> InstallError {
    InstallError::Drift {
        path: operation.path.clone(),
        observed: observed.map(|observed| observed.image.digest),
    }
}

#[cfg(test)]
mod tests;
