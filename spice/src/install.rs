//! Digest-bound, repository-local Spice installation.
//!
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
mod git_hooks;
mod handle;
mod hook_activation;
mod path;
mod progress;
mod record;
mod validation;

use binding::{BoundDestination, bind_destination, validate_operation_binding};
pub use binding::{Generation, ParentObservation};
pub use error::InstallError;
use files::{
    cleanup_operation, matches_generated, materialize_generated, observe, observe_bound,
    publish_operation, restore_operation,
};
use path::validate_relative;
use progress::{
    apply_with_interruption, unapply_with_interruption, verify_progress, verify_refresh_progress,
};
use record::read_record;
#[cfg(test)]
use record::record_path;
use validation::{asset_contract, fixed_content_differs_from_current_build, validate_operation};

pub use hook_activation::{
    HookActivationPlan, HookConfigurationMutation, HookConfigurationScope, HookWorktreeObservation,
};

const PLAN_VERSION: u16 = 7;
const RECORD_VERSION: u16 = 7;
const LEGACY_RECORD_VERSIONS: [(u16, &str); 5] = [
    (2, "SURFACE-1kJRGpf2"),
    (3, "HOOKS-1kJQnmG8"),
    (4, "INSTALL-1kKQTfBp"),
    (5, "INSTALL-1kLW2GVN"),
    (6, "HOOKS-1kMFzkkc"),
];
const RECORD_NAME: &str = "install.json";
const RECORD_PATH: &str = ".spice-rs/install.json";
const RECORD_MODE: u32 = 0o600;
const DIRECTORY_MODE: u32 = 0o700;
const DATA_MODE: u32 = 0o644;
const EXECUTABLE_MODE: u32 = 0o755;

// The receipt materializes `.spice-rs` before applying operations. Appliance
// assets use a sibling namespace so that publication cannot invalidate the
// side-effect-free preview's descriptor-bound parent observations.
const EXCLUDE_PATH: &str = "info/exclude";
const GIT_PRE_COMMIT_PATH: &str = ".spice-install/git-hooks/pre-commit";
const GIT_COMMIT_MESSAGE_PATH: &str = ".spice-install/git-hooks/commit-msg";
const GIT_REFERENCE_TRANSACTION_PATH: &str = ".spice-install/git-hooks/reference-transaction";
const ENTRY_PATH: &str = ".spice-install/hooks/entry.sh";
const STATIC_PATH: &str = ".spice-install/hooks/static/stage.sh";
const SKILL_PATH: &str = ".spice-install/agents/skills/spice/SKILL.md";
const BINARY_PATH: &str = ".spice-install/bin/spice";

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
#[cfg(debug_assertions)]
#[doc(hidden)]
pub const TEST_EXECUTABLE: &str = "SPICE_INSTALL_TEST_EXECUTABLE"; // env-policy: allow

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

/// One Spice-owned asset in canonical installation order.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Asset {
    /// Retired version-5 asset retained only so its record can name its owner.
    Exclusions,
    GitPreCommit,
    GitCommitMessage,
    GitReferenceTransaction,
    HookEntry,
    HookStatic,
    Skill,
    Binary,
}

/// Root against which an operation's relative path resolves.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Location {
    /// Retired: no operation resolves here, and none can.
    ///
    /// An install identifies its repository by common directory alone, which
    /// `RepositoryIdentity::capture` enforces by capturing no worktree at
    /// all. A worktree-relative destination is therefore unresolvable from the
    /// record that owns it: one record serves every linked worktree, so the
    /// root this variant names is whichever worktree happened to run install
    /// rather than a place the record can rediscover later. Current appliance
    /// assets therefore live exclusively below [`Self::CommonGit`].
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    worktree: Option<PathBuf>,
    common_git: PathBuf,
}

impl RepositoryIdentity {
    fn capture(repository: &Repository) -> Self {
        Self {
            worktree: None,
            common_git: repository.common_dir().to_path_buf(),
        }
    }

    fn validate(&self, repository: &Repository) -> Result<(), InstallError> {
        if self.worktree.is_none() && self.common_git == repository.common_dir() {
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

fn images_match(observed: Option<&ObservedFile>, expected: Option<&ObservedFile>) -> bool {
    // Durable drift belongs to the complete file image. A generation is only
    // an immediate compare-and-swap guard for the mutation that follows.
    match (observed, expected) {
        (None, None) => true,
        (Some(observed), Some(expected)) => observed.image == expected.image,
        _ => false,
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
    /// Installed image this operation replaces, when it refreshes a completed
    /// install rather than introducing one.
    ///
    /// `previous` never moves: it stays the pre-install state a reversal
    /// restores, so refreshing repeatedly still uninstalls to the tree the
    /// first apply found. What a refresh needs beyond that is the image it
    /// expects to *find* in place, which is the superseded build's output
    /// rather than `previous`. Absent for a first install, and skipped from the
    /// canonical bytes then, so plans that introduce an install keep the digest
    /// they had before refreshing existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    superseded: Option<GeneratedImage>,
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

    /// Installed image a refresh replaces, absent when this install introduces it.
    #[must_use]
    pub const fn superseded(&self) -> Option<&GeneratedImage> {
        self.superseded.as_ref()
    }

    /// Whether `observed` is the exact state this operation may publish over.
    ///
    /// A first install may publish only over the image it planned against. A
    /// refresh may publish only over the build it supersedes, because the
    /// destination already holds an installed image rather than `previous`.
    fn expects_in_place(&self, observed: Option<&ObservedFile>) -> bool {
        match &self.superseded {
            Some(superseded) => {
                observed.is_some_and(|observed| &observed.generated() == superseded)
            }
            None => images_match(observed, self.previous.as_ref()),
        }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hook_activation: Option<HookActivationPlan>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum GeneratedContentAuthority {
    CurrentBuild,
    HistoricalReceipt,
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

    /// Repository-wide Git hook activation appended after every file operation.
    #[must_use]
    pub const fn hook_activation(&self) -> Option<&HookActivationPlan> {
        self.hook_activation.as_ref()
    }

    /// Number of durable publication stages in this plan.
    #[must_use]
    pub fn operation_count(&self) -> usize {
        self.operations.len() + usize::from(self.hook_activation.is_some())
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
        self.validate_with_content(GeneratedContentAuthority::CurrentBuild)
    }

    /// Validate receipt history whose generated bytes came from another build.
    ///
    /// An uninstalled record can authorize only proof of the image its old plan
    /// restored. A completed record carrying its installed executable can also
    /// authorize one refresh input, after every destination is proved against
    /// that receipt. In both cases the fixed digests are history, not bytes this
    /// build may apply; every new plan is built and validated from the current
    /// executable's embedded assets.
    fn validate_historical_receipt(&self) -> Result<(), InstallError> {
        self.validate_with_content(GeneratedContentAuthority::HistoricalReceipt)
    }

    fn validate_with_content(
        &self,
        content: GeneratedContentAuthority,
    ) -> Result<(), InstallError> {
        if self.version != PLAN_VERSION {
            return Err(InstallError::InvalidRecord(format!(
                "install plan version {} is not {PLAN_VERSION}",
                self.version
            )));
        }
        let expected = [
            Asset::GitPreCommit,
            Asset::GitCommitMessage,
            Asset::GitReferenceTransaction,
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
        if self.hook_activation.is_some()
            != self
                .operations
                .last()
                .is_some_and(|operation| operation.asset == Asset::Binary)
        {
            return Err(InstallError::InvalidRecord(
                "hook activation and installed executable must be planned together".to_owned(),
            ));
        }
        if let Some(activation) = &self.hook_activation {
            activation.validate(content == GeneratedContentAuthority::HistoricalReceipt)?;
        }

        if self.repository.worktree.is_some() || !self.repository.common_git.is_absolute() {
            return Err(InstallError::InvalidRecord(
                "install repository identity is not common-directory scoped".to_owned(),
            ));
        }
        validate_relative(&self.record.destination)?;
        let mut introduced = BTreeSet::new();
        let mut destinations = BTreeSet::new();
        for operation in &self.operations {
            validate_operation(
                operation,
                &mut introduced,
                content,
                &self.repository.common_git,
            )?;
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
        self.validate_repository_binding(repository)
    }

    fn validate_repository_binding(&self, repository: &Repository) -> Result<(), InstallError> {
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
            if !operation.expects_in_place(observed.as_ref()) {
                return Err(drift(operation, observed));
            }
        }
        if let Some(activation) = &self.hook_activation {
            hook_activation::verify_pristine(activation)?;
        }
        Ok(())
    }

    /// Whether an embedded hook or skill identity belongs to another build.
    fn fixed_content_differs_from_current_build(&self) -> Result<bool, InstallError> {
        for operation in &self.operations {
            if fixed_content_differs_from_current_build(operation, &self.repository.common_git)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// One side-effect-free install preview and the exact identity admitted by apply.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallPreview {
    digest: Digest,
    plan: InstallPlan,
}

impl InstallPreview {
    /// Bind a canonical plan to its dedicated-domain identity.
    pub fn from_plan(plan: InstallPlan) -> Result<Self, InstallError> {
        let digest = plan.digest()?;
        Ok(Self { digest, plan })
    }

    /// Exact canonical plan identity accepted by `install --apply`.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.digest
    }

    /// Side-effect-free canonical installation plan.
    #[must_use]
    pub const fn plan(&self) -> &InstallPlan {
        &self.plan
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

/// Inspect the current strict installation receipt without deriving a plan or
/// acquiring mutation authority.
pub fn inspect(repository: &Repository) -> Result<Option<InstallReceipt>, InstallError> {
    let Some(loaded) = read_record(repository)? else {
        return Ok(None);
    };
    verify_progress(repository, &loaded.record)?;
    Ok(Some(loaded.record.receipt()))
}

/// Inspect one repository and derive the exact Spice installation plan.
pub fn plan(
    repository: &Repository,
    spice_binary: Option<&Path>,
) -> Result<InstallPlan, InstallError> {
    if let Some(loaded) = read_record(repository)? {
        let record = loaded.record;
        if record.status == InstallStatus::Uninstalled {
            verify_progress(repository, &record)?;
            return build_plan(repository, spice_binary);
        }
        if record.status == InstallStatus::Complete
            && (current_build_differs(&record.plan, spice_binary)?
                || hook_activation::topology_changed(repository, &record.plan)?)
        {
            // A rebuilt executable over a complete install is a refresh, not a
            // conflict. The same transaction also carries fixed hooks and the
            // generated skill across build boundaries. Every receipt-owned
            // destination must still be intact; only binary drift has the
            // explicit reconciliation authority described below.
            let binary_superseded = verify_refresh_progress(repository, &record)?;
            return build_plan_over(
                repository,
                spice_binary,
                Some(&record.plan),
                binary_superseded.as_ref(),
            );
        }
        verify_binary_context(&record.plan, spice_binary)?;
        verify_progress(repository, &record)?;
        return Ok(record.plan);
    }
    build_plan(repository, spice_binary)
}

/// Apply only the exact current plan named by `expected`.
pub fn apply(
    repository: &Repository,
    spice_binary: Option<&Path>,
    expected: &Digest,
) -> Result<InstallReceipt, InstallError> {
    apply_with_interruption(repository, spice_binary, expected, None)
}

/// Reverse the complete current install without touching drifted targets.
pub fn unapply(repository: &Repository) -> Result<InstallReceipt, InstallError> {
    unapply_with_interruption(repository, None)
}

fn build_plan(
    repository: &Repository,
    spice_binary: Option<&Path>,
) -> Result<InstallPlan, InstallError> {
    build_plan_over(repository, spice_binary, None, None)
}

/// Derive the plan that installs this build over `installed`, or from nothing.
///
/// Passing a completed plan makes this a refresh: every operation keeps that
/// plan's reversal image and names its generated image as superseded, so apply
/// replaces each destination in place instead of reversing the install first.
fn build_plan_over(
    repository: &Repository,
    spice_binary: Option<&Path>,
    installed: Option<&InstallPlan>,
    binary_superseded: Option<&GeneratedImage>,
) -> Result<InstallPlan, InstallError> {
    let mut virtual_directories = BTreeSet::new();
    let mut operations = Vec::new();

    for asset in [
        Asset::GitPreCommit,
        Asset::GitCommitMessage,
        Asset::GitReferenceTransaction,
        Asset::HookEntry,
        Asset::HookStatic,
        Asset::Skill,
    ] {
        let (_, path, mode) = asset_contract(asset);
        let bytes = fixed_asset_bytes(asset, repository.common_dir())?
            .expect("the fixed asset inventory contains no variable asset");
        let (bound, observed) = destination_state(repository, Location::CommonGit, path)?;
        let (previous, superseded) = reversal_context(installed, asset, observed, None);
        operations.push(operation(
            &mut virtual_directories,
            asset,
            bound,
            previous,
            superseded,
            GeneratedImage::new(mode, &bytes),
        )?);
    }

    if let Some(binary) = spice_binary {
        let bytes = read_binary(binary)?;
        let (bound, observed) = destination_state(repository, Location::CommonGit, BINARY_PATH)?;
        let (previous, superseded) =
            reversal_context(installed, Asset::Binary, observed, binary_superseded);
        operations.push(operation(
            &mut virtual_directories,
            Asset::Binary,
            bound,
            previous,
            superseded,
            GeneratedImage::new(EXECUTABLE_MODE, &bytes),
        )?);
    }

    let record = bind_destination(repository.common_dir(), RECORD_PATH)?;
    let hook_activation = spice_binary
        .is_some()
        .then(|| hook_activation::plan(repository, installed))
        .transpose()?;
    let plan = InstallPlan {
        version: PLAN_VERSION,
        repository: RepositoryIdentity::capture(repository),
        record,
        operations,
        hook_activation,
    };
    plan.validate_pristine(repository)?;
    Ok(plan)
}

/// Reversal image and superseded build for one asset of `installed`.
///
/// A first install reverses to what it observed. A refresh reverses to whatever
/// the install it supersedes reversed to, because the destination it observes
/// now is that install's own output rather than operator state.
fn reversal_context(
    installed: Option<&InstallPlan>,
    asset: Asset,
    observed: Option<ObservedFile>,
    reconciled: Option<&GeneratedImage>,
) -> (Option<ObservedFile>, Option<GeneratedImage>) {
    match installed.and_then(|plan| {
        plan.operations
            .iter()
            .find(|operation| operation.asset == asset)
    }) {
        Some(superseded) => (
            superseded.previous.clone(),
            Some(
                reconciled
                    .cloned()
                    .unwrap_or_else(|| superseded.generated.clone()),
            ),
        ),
        None => (observed, None),
    }
}

fn operation(
    virtual_directories: &mut BTreeSet<(Location, String)>,
    asset: Asset,
    bound: BoundDestination,
    previous: Option<ObservedFile>,
    superseded: Option<GeneratedImage>,
    generated: GeneratedImage,
) -> Result<Operation, InstallError> {
    // The asset fixes where it lands; `validate_operation_contract` refuses any
    // operation whose location and path disagree with this same contract.
    let (location, path, _) = asset_contract(asset);
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
        superseded,
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

fn read_binary(path: &Path) -> Result<Vec<u8>, InstallError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| InstallError::io("inspect Spice executable", path, source))?;
    if !metadata.file_type().is_file() {
        return Err(InstallError::BinaryNotRegular(path.to_path_buf()));
    }
    let bytes =
        fs::read(path).map_err(|source| InstallError::io("read Spice executable", path, source))?;
    if bytes.is_empty() {
        return Err(InstallError::BinaryEmpty(path.to_path_buf()));
    }
    Ok(bytes)
}

/// Whether `spice_binary` rebuilds the executable `plan` already published.
///
/// Only a differing image of an executable the plan owns is a refresh. A caller
/// that supplies no binary for a plan that published one — or one for a plan
/// that did not — disagrees about the install's shape rather than its build,
/// and that stays [`InstallError::BinaryContextChanged`]. Refreshing it instead
/// would derive a plan missing the very operation that owns the published
/// executable, leaving a file on disk with no reversal to withdraw it.
fn rebuilt_binary(plan: &InstallPlan, spice_binary: Option<&Path>) -> Result<bool, InstallError> {
    match (binary_operation(plan), spice_binary) {
        (Some(planned), Some(path)) => {
            let bytes = read_binary(path)?;
            Ok(GeneratedImage::new(EXECUTABLE_MODE, &bytes) != planned.generated)
        }
        (None, None) => Ok(false),
        _ => Err(InstallError::BinaryContextChanged),
    }
}

/// Whether a completed installed appliance belongs to another build.
fn current_build_differs(
    plan: &InstallPlan,
    spice_binary: Option<&Path>,
) -> Result<bool, InstallError> {
    // Evaluate the binary first so adding or dropping the executable remains a
    // shape refusal even when an embedded asset also changed.
    let binary_differs = rebuilt_binary(plan, spice_binary)?;
    Ok(binary_differs || plan.fixed_content_differs_from_current_build()?)
}

fn fixed_asset_bytes(asset: Asset, common_git: &Path) -> Result<Option<Vec<u8>>, InstallError> {
    let bytes = match asset {
        Asset::GitPreCommit => git_hooks::launcher(common_git, git_hooks::GitHook::PreCommit)?,
        Asset::GitCommitMessage => {
            git_hooks::launcher(common_git, git_hooks::GitHook::CommitMessage)?
        }
        Asset::GitReferenceTransaction => {
            git_hooks::launcher(common_git, git_hooks::GitHook::ReferenceTransaction)?
        }
        Asset::HookEntry => steer::hook::STAGES[0].bytes().as_bytes().to_vec(),
        Asset::HookStatic => steer::hook::STAGES[1].bytes().as_bytes().to_vec(),
        Asset::Skill => SKILL.as_bytes().to_vec(),
        Asset::Exclusions | Asset::Binary => return Ok(None),
    };
    Ok(Some(bytes))
}

fn binary_operation(plan: &InstallPlan) -> Option<&Operation> {
    plan.operations
        .iter()
        .find(|operation| operation.asset == Asset::Binary)
}

fn verify_binary_context(
    plan: &InstallPlan,
    spice_binary: Option<&Path>,
) -> Result<(), InstallError> {
    let planned = binary_operation(plan);
    match (planned, spice_binary) {
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

/// Installed Spice executable shared by every linked worktree.
///
/// Which build that is depends on which lane installed last;
/// [`Repository::discover`] records what the sharing costs and why the
/// predecessor's re-exec is not the correction.
#[must_use]
pub fn installed_binary(repository: &Repository) -> PathBuf {
    repository.common_dir().join(BINARY_PATH)
}

/// Installed operator skill shared by every linked worktree.
#[must_use]
pub fn installed_skill(repository: &Repository) -> PathBuf {
    repository.common_dir().join(SKILL_PATH)
}

/// Installed Git hook directory shared by every linked worktree.
#[must_use]
pub fn installed_git_hooks(repository: &Repository) -> PathBuf {
    repository.common_dir().join(".spice-install/git-hooks")
}

/// Installed outer shell used for every supervised top-level command.
#[must_use]
pub fn installed_shell(repository: &Repository) -> PathBuf {
    repository.common_dir().join(ENTRY_PATH)
}

/// Installed directory containing both structural shell stages.
#[must_use]
pub fn installed_shell_hooks(repository: &Repository) -> PathBuf {
    repository.common_dir().join(".spice-install/hooks")
}

fn drift(operation: &Operation, observed: Option<ObservedFile>) -> InstallError {
    InstallError::Drift {
        path: operation.path.clone(),
        observed: observed.map(|observed| observed.image.digest),
    }
}

#[cfg(test)]
mod tests;
