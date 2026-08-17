//! Digest-bound, repository-local Spice installation.
//!
//! Plans are side-effect free; apply re-plans and reverses only exact images.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wire::{Digest, Domain};

use crate::repository::Repository;

mod audit;
mod binding;
mod error;
mod files;
mod fold;
mod git_hooks;
mod handle;
mod hook_activation;
mod maintenance;
mod model;
mod path;
mod progress;
mod record;
mod rollback;
mod validation;

pub use audit::{ApplianceAudit, ApplianceState, audit_appliance};
use binding::{BoundDestination, bind_destination, validate_operation_binding};
pub use binding::{Generation, ParentObservation};
pub use error::InstallError;
use files::{
    cleanup_operation, matches_generated, materialize_generated, observe, observe_bound,
    publish_bound, publish_operation, restore_operation, restore_retirement, retire_path,
};
use path::validate_relative;
use progress::{
    apply_with_interruption, unapply_with_interruption, verify_progress, verify_refresh_progress,
};
#[cfg(test)]
use record::record_path;
use record::{RefreshRecord, read_record, read_record_for_refresh};
use rollback::{RollbackArchive, RollbackState, rollback_image_matches};
use validation::{asset_contract, fixed_content_differs_from_current_build};

pub use fold::{
    HookActivationSummary, InstallOperationSummary, InstallPlanSummary, InstallPreview,
    InstallReceipt, InstallRetirementSummary, InstallStageKind, InstallStatus,
    RollbackArchiveSummary,
};
pub use hook_activation::{
    HookActivationPlan, HookConfigurationMutation, HookConfigurationScope, HookWorktreeObservation,
};
pub use maintenance::{
    InstallTempInspection, inspect_temporary_records, reclaim_temporary_records,
};
pub use model::{Asset, FileImage, GeneratedImage, Location, Operation, Retirement};
use model::{ObservedFile, ReceiptGeneration, RepositoryIdentity};

const PLAN_VERSION: u16 = 9;
const RECORD_VERSION: u16 = 9;
const RETIREMENT_WITNESS_MISMATCH: &str =
    "migration rollback archive does not match its retirement witnesses";
const MIGRATABLE_RECORD_VERSION: u16 = 7;
const FORWARD_MIGRATABLE_RECORD_VERSION: u16 = 8;
/// Environment marker exported only by the installed pre-commit launcher.
pub const INSTALLED_PRE_COMMIT_ENVIRONMENT: &str = "SPICE_SHELL_HOOK_INSTALLED_APPLIANCE"; // env-policy: allow
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
const RETIRED_XTASK_PATH: &str = ".spice-install/bin/xtask";
const RETIRED_XTASK_GUARD_PATH: &str = ".spice-install/bin/xtask.guard";

const SKILL: &str = include_str!("../agent/SKILL.md");

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

/// Configures the process-test clock used by temporary-record maintenance.
#[cfg(debug_assertions)]
#[doc(hidden)]
pub fn configure_test_maintenance_now(now: Option<std::ffi::OsString>) {
    maintenance::configure_test_now(now);
}

/// Rewrites one complete debug fixture receipt into the immediately previous
/// schema so process tests can exercise the production migration boundary.
#[cfg(debug_assertions)]
#[doc(hidden)]
pub fn publish_test_version_7_receipt(repository: &Repository) -> Result<(), InstallError> {
    record::publish_test_version_7_receipt(repository)
}

/// Side-effect-free canonical installation plan.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallPlan {
    version: u16,
    repository: RepositoryIdentity,
    record: BoundDestination,
    operations: Vec<Operation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    retirements: Vec<Retirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rollback_archive: Option<RollbackArchive>,
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

    /// Legacy paths this plan removes only after all live assets publish.
    #[must_use]
    pub fn retirements(&self) -> &[Retirement] {
        &self.retirements
    }

    /// Whether reversal uses the compact, digest-bound version-7 snapshot.
    #[must_use]
    pub const fn has_rollback_archive(&self) -> bool {
        self.rollback_archive.is_some()
    }

    /// Repository-wide Git hook activation appended after every file operation.
    #[must_use]
    pub const fn hook_activation(&self) -> Option<&HookActivationPlan> {
        self.hook_activation.as_ref()
    }

    /// Number of durable publication stages in this plan.
    #[must_use]
    pub fn operation_count(&self) -> usize {
        self.operations.len() + self.retirements.len() + usize::from(self.hook_activation.is_some())
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
        validation::validate_plan(self, content)
    }

    fn rollback_state(&self) -> Result<Option<RollbackState>, InstallError> {
        self.rollback_archive
            .as_ref()
            .map(RollbackArchive::decode)
            .transpose()
    }

    fn validate_rollback_state(&self, rollback: &RollbackState) -> Result<(), InstallError> {
        let prior = record::decode_rollback_record(&rollback.record, &self.repository)?;
        if rollback.operations.len() != self.operations.len()
            || rollback.retirements.len() != self.retirements.len()
        {
            return Err(InstallError::InvalidRecord(
                "migration rollback archive has the wrong image inventory length".to_owned(),
            ));
        }
        for ((image, operation), prior_operation) in rollback
            .operations
            .iter()
            .zip(&self.operations)
            .zip(&prior.plan.operations)
        {
            if operation.asset != prior_operation.asset
                || !image.as_ref().is_some_and(|image| {
                    image.validate() && image.generated() == prior_operation.generated
                })
            {
                return Err(InstallError::InvalidRecord(
                    "migration rollback archive does not reproduce the version-7 appliance"
                        .to_owned(),
                ));
            }
        }
        if rollback
            .retirements
            .iter()
            .zip(&self.retirements)
            .any(|(image, retirement)| {
                !rollback_image_matches(image.as_ref(), retirement.rollback_witness())
            })
        {
            return Err(InstallError::InvalidRecord(
                RETIREMENT_WITNESS_MISMATCH.to_owned(),
            ));
        }
        if self
            .operations
            .iter()
            .any(|operation| operation.previous.is_some())
            || self
                .retirements
                .iter()
                .any(|retirement| retirement.previous.is_some() || retirement.observed.is_some())
        {
            return Err(InstallError::InvalidRecord(
                "compact migration plan duplicates expanded rollback images".to_owned(),
            ));
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
        for retirement in &self.retirements {
            validate_operation_binding(
                repository,
                Location::CommonGit,
                &retirement.path,
                &retirement.destination,
                &retirement.parent,
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
        for retirement in &self.retirements {
            let current = bind_destination(repository.common_dir(), &retirement.path)?;
            if current.destination != retirement.destination || current.parent != retirement.parent
            {
                return Err(InstallError::ParentChanged(
                    retirement.destination(repository),
                ));
            }
            let (observed, _) = observe_bound(
                repository.common_dir(),
                &retirement.destination,
                &retirement.parent,
            )?;
            if !retirement.expects_in_place(observed.as_ref()) {
                return Err(retirement_drift(retirement, observed));
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
    if let Some(refresh) = read_record_for_refresh(repository)? {
        return plan_from_refresh(repository, spice_binary, refresh);
    }
    build_plan(repository, spice_binary)
}

fn plan_from_refresh(
    repository: &Repository,
    spice_binary: Option<&Path>,
    refresh: RefreshRecord,
) -> Result<InstallPlan, InstallError> {
    let repairable = refresh.is_repairable();
    let loaded = refresh.into_loaded();
    let record = loaded.record;
    let rollback_record = if record.plan.version == MIGRATABLE_RECORD_VERSION {
        Some(
            loaded
                .image
                .as_ref()
                .expect("only a version-7 receipt retains its source image"),
        )
    } else {
        None
    };
    let rollback_archive = (record.plan.version != MIGRATABLE_RECORD_VERSION)
        .then_some(record.plan.rollback_archive.as_ref())
        .flatten();
    if record.status == InstallStatus::Uninstalled {
        verify_progress(repository, &record)?;
        return build_plan(repository, spice_binary);
    }
    if record.status == InstallStatus::Complete
        && (repairable
            || current_build_differs(&record.plan, spice_binary)?
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
            rollback_record,
            rollback_archive,
        );
    }
    verify_binary_context(&record.plan, spice_binary)?;
    verify_progress(repository, &record)?;
    Ok(record.plan)
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
    build_plan_over(repository, spice_binary, None, None, None, None)
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
    rollback_record: Option<&FileImage>,
    rollback_archive: Option<&RollbackArchive>,
) -> Result<InstallPlan, InstallError> {
    let mut virtual_directories = BTreeSet::new();
    let mut operations = Vec::new();
    let mut rollback_operations = Vec::new();
    let compacting = rollback_record.is_some();

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
        if compacting {
            rollback_operations.push(rollback_operation_image(
                installed,
                asset,
                observed.as_ref(),
            ));
        }
        let (previous, superseded) = reversal_context(installed, asset, observed, None, compacting);
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
        if compacting {
            rollback_operations.push(rollback_operation_image(
                installed,
                Asset::Binary,
                observed.as_ref(),
            ));
        }
        let (previous, superseded) = reversal_context(
            installed,
            Asset::Binary,
            observed,
            binary_superseded,
            compacting,
        );
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
    let planned_retirements = [RETIRED_XTASK_PATH, RETIRED_XTASK_GUARD_PATH]
        .into_iter()
        .map(|path| retirement(repository, installed, path, compacting))
        .collect::<Result<Vec<_>, _>>()?;
    let (retirements, rollback_retirements): (Vec<_>, Vec<_>) =
        planned_retirements.into_iter().unzip();
    let hook_activation = spice_binary
        .is_some()
        .then(|| {
            hook_activation::plan(
                repository,
                installed,
                installed.is_some_and(|plan| plan.version == MIGRATABLE_RECORD_VERSION),
            )
        })
        .transpose()?;
    let rollback_archive = match rollback_record {
        Some(record) => Some(RollbackArchive::new(&RollbackState {
            record: record.clone(),
            operations: rollback_operations,
            retirements: rollback_retirements,
        })?),
        None => rollback_archive.cloned(),
    };
    let plan = InstallPlan {
        version: PLAN_VERSION,
        repository: RepositoryIdentity::capture(repository),
        record,
        operations,
        retirements,
        rollback_archive,
        hook_activation,
    };
    plan.validate_pristine(repository)?;
    Ok(plan)
}

fn retirement(
    repository: &Repository,
    installed: Option<&InstallPlan>,
    path: &str,
    compacting: bool,
) -> Result<(Retirement, Option<FileImage>), InstallError> {
    let (bound, observed) = destination_state(repository, Location::CommonGit, path)?;
    let previous = installed
        .and_then(|plan| {
            plan.retirements
                .iter()
                .find(|retirement| retirement.path == path)
        })
        .map_or_else(
            || observed.clone(),
            |retirement| retirement.previous.clone(),
        );
    let rollback_image = compacting
        .then(|| observed.as_ref().map(|observed| observed.image.clone()))
        .flatten();
    let superseded = compacting
        .then(|| observed.as_ref().map(ObservedFile::generated))
        .flatten();
    let rollback_witness = if compacting {
        superseded.clone()
    } else {
        installed
            .and_then(|plan| {
                plan.retirements
                    .iter()
                    .find(|retirement| retirement.path == path)
            })
            .and_then(Retirement::rollback_witness)
            .cloned()
    };
    Ok((
        Retirement {
            path: path.to_owned(),
            destination: bound.destination,
            parent: bound.parent,
            previous: (!compacting).then_some(previous).flatten(),
            observed: (!compacting).then_some(observed).flatten(),
            superseded,
            rollback: rollback_witness,
        },
        rollback_image,
    ))
}

fn rollback_operation_image(
    installed: Option<&InstallPlan>,
    asset: Asset,
    observed: Option<&ObservedFile>,
) -> Option<FileImage> {
    let installed = installed.expect("rollback compaction supersedes an installed plan");
    debug_assert_eq!(installed.version, MIGRATABLE_RECORD_VERSION);
    debug_assert!(
        installed
            .operations
            .iter()
            .any(|operation| operation.asset == asset)
    );
    observed.map(|observed| observed.image.clone())
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
    compacting: bool,
) -> (Option<ObservedFile>, Option<GeneratedImage>) {
    match installed.and_then(|plan| {
        plan.operations
            .iter()
            .find(|operation| operation.asset == asset)
    }) {
        Some(superseded) => (
            if compacting {
                None
            } else {
                superseded.previous.clone()
            },
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
    if plan.version != PLAN_VERSION {
        return Ok(true);
    }
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

fn retirement_drift(retirement: &Retirement, observed: Option<ObservedFile>) -> InstallError {
    InstallError::Drift {
        path: retirement.path.clone(),
        observed: observed.map(|observed| observed.image.digest),
    }
}

#[cfg(test)]
mod tests;
