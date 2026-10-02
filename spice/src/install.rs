//! Digest-bound, repository-local Spice installation.
//!
//! Plans are side-effect free; apply re-plans and reverses only exact images.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wire::{Digest, Domain};

use crate::repository::Repository;

mod app;
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
mod retention;
mod rollback;
mod symbol_sample;
mod symbol_verification;
mod symbols;
mod validation;

pub use audit::{ApplianceAudit, ApplianceState, audit_appliance};
use binding::{BoundDestination, bind_destination, validate_operation_binding};
pub use binding::{Generation, ParentObservation};
pub use error::InstallError;
use files::{
    cleanup_operation, matches_generated, materialize_generated, observe, observe_bound,
    prepare_retirement, publish_bound, publish_operation, restore_operation, restore_retirement,
    retire_path, withdraw_parent,
};
use path::validate_relative;
pub use progress::PreparedInstall;
use progress::{
    apply_prepared_with_interruption, apply_with_interruption, unapply_with_interruption,
    verify_progress,
};
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
pub use symbol_sample::{
    DwarfLookup, DwarfdumpLookup, LookupError, MAX_SAMPLE_ADDRESSES, MAX_SAMPLE_BYTES, SampleError,
    SampleSymbolization, SampleTarget, SampledFrame, SourceLocation, SymbolizedFrame,
    symbolize_sample,
};
pub use symbol_verification::{
    SymbolImage, SymbolVerification, SymbolVerificationError, verify_symbols,
};
pub use symbols::SymbolProvenance;

const PLAN_VERSION: u16 = 17;
const RECORD_VERSION: u16 = 17;
const RETIREMENT_WITNESS_MISMATCH: &str =
    "migration rollback archive does not match its retirement witnesses";
const MIGRATABLE_RECORD_VERSION: u16 = 7;
const FORWARD_MIGRATABLE_RECORD_VERSION: u16 = 8;
const INLINE_ROLLBACK_RECORD_VERSION: u16 = 9;
const PREVIOUS_RECORD_VERSION: u16 = 16;
/// App generations a plan retains immediately before its current one.
///
/// A page loaded from a generation keeps fetching that generation's lazily
/// loaded renderers and styles until it notices the new pointer and reloads.
/// Two predecessors let such a page survive back-to-back refreshes; anything
/// older retires with the refresh that pushes it past this bound, so installed
/// bytes and receipt size stay constant however often the appliance refreshes.
const RETAINED_APP_PREDECESSORS: usize = 2;
/// Environment marker exported only by the installed pre-commit launcher.
pub const INSTALLED_PRE_COMMIT_ENVIRONMENT: &str = "SPICE_SHELL_HOOK_INSTALLED_APPLIANCE"; // env-policy: allow
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReceiptSupport {
    Current,
    Direct,
    Retired(&'static str),
}

const RECEIPT_SUPPORT: [(u16, ReceiptSupport); 17] = [
    (1, ReceiptSupport::Retired("SURFACE-1kJQkpFs")),
    (2, ReceiptSupport::Retired("SURFACE-1kJRGpf2")),
    (3, ReceiptSupport::Retired("HOOKS-1kJQnmG8")),
    (4, ReceiptSupport::Retired("INSTALL-1kKQTfBp")),
    (5, ReceiptSupport::Retired("INSTALL-1kLW2GVN")),
    (6, ReceiptSupport::Retired("HOOKS-1kMFzkkc")),
    (MIGRATABLE_RECORD_VERSION, ReceiptSupport::Direct),
    (FORWARD_MIGRATABLE_RECORD_VERSION, ReceiptSupport::Direct),
    (INLINE_ROLLBACK_RECORD_VERSION, ReceiptSupport::Direct),
    (10, ReceiptSupport::Retired("WRAPPER-1kNYClbf")),
    (11, ReceiptSupport::Retired("WRAPPER-1kNYClbf")),
    (12, ReceiptSupport::Retired("RETIREM-1kSR91Zl")),
    (13, ReceiptSupport::Retired("INSTALL-1kSWFBN0")),
    (14, ReceiptSupport::Retired("SYMBOLS-dSPkcQ47R")),
    (15, ReceiptSupport::Retired("BROWSER-dV0xw93ND")),
    (PREVIOUS_RECORD_VERSION, ReceiptSupport::Direct),
    (RECORD_VERSION, ReceiptSupport::Current),
];

fn receipt_support(version: u16) -> Option<ReceiptSupport> {
    RECEIPT_SUPPORT
        .iter()
        .find_map(|(candidate, support)| (*candidate == version).then_some(*support))
}
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
const ZSH_HOOK_PATH: &str = ".spice-install/hooks/shell/.zshenv";
const BASH_HOOK_PATH: &str = ".spice-install/hooks/shell/bash_env";
const WRAPPER_SPICE_PATH: &str = ".spice-install/hooks/wrappers/spice";
const SKILL_PATH: &str = ".spice-install/agents/skills/spice/SKILL.md";
const BINARY_PATH: &str = ".spice-install/bin/spice";
const APP_CURRENT_PATH: &str = ".spice-install/app/current";
const RETIRED_XTASK_PATH: &str = ".spice-install/bin/xtask";
const RETIRED_XTASK_GUARD_PATH: &str = ".spice-install/bin/xtask.guard";
const RETIRED_FOLD_STORE_PATH: &str = ".spice-rs/data/spicefolds.sqlite3";

const SKILL: &str = ::std::include_str!("../agent/SKILL.md");

#[cfg(debug_assertions)]
#[doc(hidden)]
pub const TEST_EXECUTABLE: &str = "SPICE_INSTALL_TEST_EXECUTABLE"; // env-policy: allow
/// Explicitly selects the worktree app stage for debug-only resident process tests.
#[cfg(debug_assertions)]
#[doc(hidden)]
pub const TEST_APP_STAGE_ARGUMENT: &str = "--spice-test-app-stage";

/// Rewrites one complete debug fixture receipt into a directly upgradable schema.
#[cfg(debug_assertions)]
#[doc(hidden)]
pub fn publish_test_historical_receipt(
    repository: &Repository,
    version: u16,
) -> Result<(), InstallError> {
    record::publish_test_historical_receipt(repository, version)
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
    app: Option<Digest>,
    /// Retained app generations before `app`, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    app_predecessors: Vec<Digest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    retirements: Vec<Retirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rollback_archive: Option<RollbackArchive>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hook_activation: Option<HookActivationPlan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    symbols: Option<SymbolProvenance>,
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

    /// App generation published by an appliance-bearing plan.
    #[must_use]
    pub const fn app_generation(&self) -> Option<&Digest> {
        self.app.as_ref()
    }

    /// Legacy paths and out-of-bound app generations this plan removes only
    /// after all live assets publish.
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

    /// Provenance of the executable and dSYM a symbol-bearing plan retains.
    #[must_use]
    pub const fn symbol_provenance(&self) -> Option<&SymbolProvenance> {
        self.symbols.as_ref()
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

    fn rollback_state(
        &self,
        repository: &Repository,
    ) -> Result<Option<RollbackState>, InstallError> {
        self.rollback_archive
            .as_ref()
            .map(|archive| archive.decode(repository))
            .transpose()
    }

    fn validated_rollback_state(
        &self,
        repository: &Repository,
    ) -> Result<Option<RollbackState>, InstallError> {
        let rollback = self.rollback_state(repository)?;
        if let Some(rollback) = &rollback {
            self.validate_rollback_state(rollback)?;
        }
        Ok(rollback)
    }

    fn staged_rollback_state(&self) -> Result<Option<RollbackState>, InstallError> {
        self.rollback_archive
            .as_ref()
            .map(RollbackArchive::staged_state)
            .transpose()
            .map(Option::flatten)
    }

    fn validate_rollback_storage(&self, repository: &Repository) -> Result<(), InstallError> {
        self.rollback_archive
            .as_ref()
            .map_or(Ok(()), |archive| archive.validate_stored(repository))
    }

    fn publish_staged_rollback(&self, repository: &Repository) -> Result<(), InstallError> {
        self.rollback_archive
            .as_ref()
            .map_or(Ok(()), |archive| archive.publish_staged(repository))
    }

    fn lock_rollback(
        &self,
        repository: &Repository,
    ) -> Result<Option<rollback::RollbackLock>, InstallError> {
        self.has_rollback_archive()
            .then(|| rollback::acquire_lock(repository))
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
        let prior_operations = operation_index(Some(&prior.plan));
        for (image, operation) in rollback.operations.iter().zip(&self.operations) {
            let valid = prior_operations
                .get(&operation_key(operation.asset, &operation.path))
                .map_or_else(
                    || image.as_ref().is_none_or(FileImage::validate),
                    |prior| {
                        image.as_ref().is_some_and(|image| {
                            image.validate() && image.generated() == prior.generated
                        })
                    },
                );
            if !valid {
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
            let (destination, parent) = retention::retirement_binding(repository, retirement)?;
            if destination != retirement.destination || parent != retirement.parent {
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
            if fixed_content_differs_from_current_build(
                operation,
                &self.repository.common_git,
                self.app.as_ref(),
            )? {
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

/// Admit the receipt-owned Canvas publication for the selected Spice executable.
///
/// This is the production resident boundary: the current receipt, every
/// installed destination, the executable bytes, and the Git-common Canvas
/// generation must all agree before an app source is returned. It never reads
/// the managed worktree's development stage.
pub fn installed_app_source(
    repository: &Repository,
    executable: &Path,
) -> Result<canvas::AppSource, InstallError> {
    let loaded = read_record(repository)?.ok_or(InstallError::RecordMissing)?;
    let record = loaded.record;
    match record.status {
        InstallStatus::Complete => {}
        InstallStatus::Applying => return Err(InstallError::ApplyInProgress),
        InstallStatus::Unapplying => return Err(InstallError::UnapplyInProgress),
        InstallStatus::Uninstalled => return Err(InstallError::RecordMissing),
    }
    verify_binary_context(&record.plan, Some(executable))?;
    verify_progress(repository, &record)?;
    let expected = record.plan.app.as_ref().ok_or_else(|| {
        InstallError::InvalidRecord("installed appliance owns no app generation".to_owned())
    })?;
    let source =
        canvas::AppSource::resolve_installed(repository.common_dir()).map_err(|error| {
            InstallError::InvalidRecord(format!("installed app publication: {error}"))
        })?;
    let observed = source.current_generation().map_err(|error| {
        InstallError::InvalidRecord(format!("installed app publication: {error}"))
    })?;
    if observed.identity() != expected {
        return Err(InstallError::InvalidRecord(format!(
            "installed app pointer names {}, receipt names {expected}",
            observed.identity()
        )));
    }
    Ok(source)
}

/// Capture the canonical installation generation without validating owned assets.
///
/// This is only a process-boundary fence. A consumer must still audit the
/// generation's installed assets before trusting it.
pub fn capture_generation(repository: &Repository) -> Result<Option<Digest>, InstallError> {
    let Some(refresh) = read_record_for_refresh(repository)? else {
        return Ok(None);
    };
    let generation = match refresh {
        RefreshRecord::Current(loaded) => loaded.record.plan_digest,
        RefreshRecord::Repairable {
            original_digest, ..
        } => original_digest,
    };
    Ok(Some(generation))
}

/// Inspect one repository and derive the exact Spice installation plan.
pub fn plan(
    repository: &Repository,
    spice_binary: Option<&Path>,
) -> Result<InstallPlan, InstallError> {
    progress::prepare(repository, spice_binary).map(PreparedInstall::into_plan)
}

/// Derive the exact plan [`apply_prepared`] would execute, keeping it for this
/// process instead of reducing it to a digest.
pub fn prepare(
    repository: &Repository,
    spice_binary: Option<&Path>,
) -> Result<PreparedInstall, InstallError> {
    progress::prepare(repository, spice_binary)
}

/// Apply only the exact current plan named by `expected`.
///
/// This route trusts nothing from any earlier preview: it derives the plan
/// from the receipt and sources again and refuses unless that plan has the
/// named digest.
pub fn apply(
    repository: &Repository,
    spice_binary: Option<&Path>,
    expected: &Digest,
) -> Result<InstallReceipt, InstallError> {
    apply_with_interruption(repository, spice_binary, expected, None)
}

/// Apply a plan this process prepared, without deriving it a second time.
pub fn apply_prepared(
    repository: &Repository,
    spice_binary: Option<&Path>,
    prepared: PreparedInstall,
) -> Result<InstallReceipt, InstallError> {
    apply_prepared_with_interruption(repository, spice_binary, prepared, None)
}

/// Reverse the complete current install without touching drifted targets.
pub fn unapply(repository: &Repository) -> Result<InstallReceipt, InstallError> {
    reclaim_temporary_records(repository)?;
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
/// Assemble every planned operation in canonical order: the fixed prefix,
/// the app bundle, the executable, and finally its retained symbols.
fn append_planned_operations(
    repository: &Repository,
    installed: Option<&InstallPlan>,
    (spice_binary, binary_superseded): (Option<&Path>, Option<&GeneratedImage>),
    (app, predecessors, symbols): (Option<&app::Source>, &[Digest], Option<&symbols::Source>),
    archiving: bool,
) -> Result<(Vec<Operation>, Vec<Option<FileImage>>), InstallError> {
    let mut virtual_directories = BTreeSet::new();
    let mut operations = Vec::new();
    let mut observed_operations = Vec::new();
    let installed_operations = operation_index(installed);

    append_fixed_operations(
        repository,
        &installed_operations,
        archiving,
        (
            &mut virtual_directories,
            &mut operations,
            &mut observed_operations,
        ),
    )?;

    if let Some(app) = app {
        append_app_operations(
            repository,
            installed,
            &installed_operations,
            (app, predecessors),
            archiving,
            (
                &mut virtual_directories,
                &mut operations,
                &mut observed_operations,
            ),
        )?;
    }

    append_binary_operation(
        repository,
        &installed_operations,
        spice_binary,
        binary_superseded,
        archiving,
        (
            &mut virtual_directories,
            &mut operations,
            &mut observed_operations,
        ),
    )?;

    if let Some(symbols) = symbols {
        symbols::append_operations(
            repository,
            &installed_operations,
            symbols,
            archiving,
            (
                &mut virtual_directories,
                &mut operations,
                &mut observed_operations,
            ),
        )?;
    }

    Ok((operations, observed_operations))
}

fn build_plan_over(
    repository: &Repository,
    spice_binary: Option<&Path>,
    installed: Option<&InstallPlan>,
    binary_superseded: Option<&GeneratedImage>,
    rollback_record: Option<&FileImage>,
    rollback_archive: Option<&RollbackArchive>,
) -> Result<InstallPlan, InstallError> {
    let app = spice_binary
        .map(|binary| app::Source::resolve(repository, binary))
        .transpose()?;
    let symbols = spice_binary
        .map(|binary| symbols::Source::resolve(repository, binary))
        .transpose()?
        .flatten();
    let predecessors = app.as_ref().map_or_else(Vec::new, |app| {
        retention::predecessors(installed, app.generation())
    });
    let compacting = rollback_record.is_some();
    let archiving = compacting || rollback_archive.is_some();
    let (operations, observed_operations) = append_planned_operations(
        repository,
        installed,
        (spice_binary, binary_superseded),
        (app.as_ref(), &predecessors, symbols.as_ref()),
        archiving,
    )?;

    let record = bind_destination(repository.common_dir(), RECORD_PATH)?;
    // A plan that stops publishing symbols discards the bundle its
    // predecessor retained, so the executable and dSYM leave together.
    let discard_symbols =
        symbols.is_none() && installed.is_some_and(|plan| symbols::operations(plan).count() > 0);
    let planned_retirements = [
        RETIRED_XTASK_PATH,
        RETIRED_XTASK_GUARD_PATH,
        RETIRED_FOLD_STORE_PATH,
    ]
    .into_iter()
    .chain(
        discard_symbols
            .then_some(symbols::DISCARD_PATHS)
            .into_iter()
            .flatten(),
    )
    .map(|path| retirement(repository, installed, path, archiving))
    .collect::<Result<Vec<_>, _>>()?;
    let (mut retirements, mut rollback_retirements): (Vec<_>, Vec<_>) =
        planned_retirements.into_iter().unzip();
    if let Some(app) = &app {
        let retired =
            retention::retirements(repository, installed, app.generation(), &predecessors)?;
        rollback_retirements.resize(rollback_retirements.len() + retired.len(), None);
        retirements.extend(retired);
    }
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
            operations: observed_operations,
            retirements: rollback_retirements,
        })?),
        None => current_rollback_archive(
            rollback_archive,
            repository,
            installed,
            &operations,
            &observed_operations,
            &retirements,
            &rollback_retirements,
        )?,
    };
    let plan = InstallPlan {
        version: PLAN_VERSION,
        repository: RepositoryIdentity::capture(repository),
        record,
        operations,
        app: app.as_ref().map(|app| app.generation().clone()),
        app_predecessors: predecessors,
        retirements,
        rollback_archive,
        hook_activation,
        symbols: symbols.map(|symbols| symbols.provenance().clone()),
    };
    plan.validate_pristine(repository)?;
    Ok(plan)
}

/// Paths a current plan discards outright, without a reversal image.
fn is_discard_path(path: &str) -> bool {
    path == RETIRED_FOLD_STORE_PATH || symbols::DISCARD_PATHS.contains(&path)
}

fn current_rollback_archive(
    archive: Option<&RollbackArchive>,
    repository: &Repository,
    installed: Option<&InstallPlan>,
    operations: &[Operation],
    observed_operations: &[Option<FileImage>],
    retirements: &[Retirement],
    observed_retirements: &[Option<FileImage>],
) -> Result<Option<RollbackArchive>, InstallError> {
    let Some(archive) = archive else {
        return Ok(None);
    };
    let installed = installed.expect("a carried rollback archive belongs to an installed plan");
    let operations_unchanged = installed
        .operations
        .iter()
        .map(|operation| operation.asset)
        .eq(operations.iter().map(|operation| operation.asset));
    let retirements_unchanged = installed
        .retirements
        .iter()
        .map(|retirement| (retirement.path.as_str(), retirement.discard))
        .eq(retirements
            .iter()
            .map(|retirement| (retirement.path.as_str(), retirement.discard)));
    if operations_unchanged && retirements_unchanged {
        return archive.to_current().map(Some);
    }

    let mut rollback = archive.decode(repository)?;
    let installed_operations = installed
        .operations
        .iter()
        .enumerate()
        .map(|(index, operation)| (operation_key(operation.asset, &operation.path), index))
        .collect::<BTreeMap<_, _>>();
    rollback.operations = operations
        .iter()
        .zip(observed_operations)
        .map(|(operation, observed)| {
            installed_operations
                .get(&operation_key(operation.asset, &operation.path))
                .map_or_else(
                    || observed.clone(),
                    |index| rollback.operations[*index].clone(),
                )
        })
        .collect();
    let previous_retirements = std::mem::take(&mut rollback.retirements);
    let installed_retirements = installed
        .retirements
        .iter()
        .enumerate()
        .map(|(index, retirement)| (retirement.path.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    rollback.retirements = retirements
        .iter()
        .zip(observed_retirements)
        .map(|(retirement, observed)| {
            if retirement.discards_payload() {
                return None;
            }
            installed_retirements
                .get(retirement.path.as_str())
                .map_or_else(
                    || observed.clone(),
                    |index| previous_retirements[*index].clone(),
                )
        })
        .collect();
    RollbackArchive::new(&rollback).map(Some)
}

fn retirement(
    repository: &Repository,
    installed: Option<&InstallPlan>,
    path: &str,
    archiving: bool,
) -> Result<(Retirement, Option<FileImage>), InstallError> {
    if is_discard_path(path) {
        let (bound, observed) = destination_state(repository, Location::CommonGit, path)?;
        return Ok((
            Retirement {
                path: path.to_owned(),
                destination: bound.destination,
                parent: bound.parent,
                previous: None,
                observed: None,
                superseded: observed.as_ref().map(ObservedFile::generated),
                rollback: None,
                discard: true,
            },
            None,
        ));
    }
    let (bound, observed) = destination_state(repository, Location::CommonGit, path)?;
    let installed = installed.and_then(|plan| {
        plan.retirements
            .iter()
            .find(|retirement| retirement.path == path)
    });
    let previous = installed.map_or_else(
        || observed.clone(),
        |retirement| retirement.previous.clone(),
    );
    let compact_new = archiving && installed.is_none();
    let rollback_image = compact_new
        .then(|| observed.as_ref().map(|observed| observed.image.clone()))
        .flatten();
    let superseded = compact_new
        .then(|| observed.as_ref().map(ObservedFile::generated))
        .flatten();
    let rollback_witness = if compact_new {
        superseded.clone()
    } else {
        installed.and_then(Retirement::rollback_witness).cloned()
    };
    Ok((
        Retirement {
            path: path.to_owned(),
            destination: bound.destination,
            parent: bound.parent,
            previous: (!compact_new).then_some(previous).flatten(),
            observed: (!compact_new).then_some(observed).flatten(),
            superseded,
            rollback: rollback_witness,
            discard: false,
        },
        rollback_image,
    ))
}

/// Reversal image and superseded build for one asset of `installed`.
///
/// A first install reverses to what it observed. A refresh reverses to whatever
/// the install it supersedes reversed to, because the destination it observes
/// now is that install's own output rather than operator state.
fn reversal_context(
    installed: &InstalledOperations<'_>,
    asset: Asset,
    path: &str,
    observed: Option<ObservedFile>,
    reconciled: Option<&GeneratedImage>,
    compacting: bool,
) -> (Option<ObservedFile>, Option<GeneratedImage>) {
    match installed.get(&operation_key(asset, path)) {
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
        None if compacting => (None, None),
        None => (observed, None),
    }
}

type InstalledOperations<'a> = BTreeMap<(Asset, &'a str), &'a Operation>;

fn operation_index(installed: Option<&InstallPlan>) -> InstalledOperations<'_> {
    installed
        .into_iter()
        .flat_map(|plan| &plan.operations)
        .map(|operation| (operation_key(operation.asset, &operation.path), operation))
        .collect()
}

fn operation_key(asset: Asset, path: &str) -> (Asset, &str) {
    // Fixed assets have one identity across receipt versions. Immutable app
    // assets additionally belong to the generation embedded in their path.
    let path = if app::generation_spelling(asset, path).is_some() {
        path
    } else {
        ""
    };
    (asset, path)
}

struct PlannedAppAsset {
    asset: Asset,
    generation: Digest,
    /// Recorded path of a retained generation's file, or the current spelling
    /// of the source generation's.
    path: String,
    generated: GeneratedImage,
}

fn planned_app_assets(
    installed: Option<&InstallPlan>,
    source: &app::Source,
    predecessors: &[Digest],
) -> Result<Vec<PlannedAppAsset>, InstallError> {
    let mut planned = installed
        .into_iter()
        .flat_map(|plan| &plan.operations)
        .filter_map(|operation| {
            let generation = app::operation_generation(operation.asset, &operation.path)?;
            predecessors.contains(&generation).then(|| PlannedAppAsset {
                asset: operation.asset,
                generation,
                path: operation.path.clone(),
                generated: operation.generated.clone(),
            })
        })
        .collect::<Vec<_>>();
    planned.sort_by(|left, right| {
        left.generation
            .cmp(&right.generation)
            .then(left.asset.cmp(&right.asset))
    });
    for asset in source.assets().chain([Asset::AppCurrent]) {
        let generated = source
            .image(asset)
            .expect("the complete app source carries every asset");
        let (_, path, _) = asset_contract(asset, Some(source.generation()))?;
        planned.push(PlannedAppAsset {
            asset,
            generation: source.generation().clone(),
            path,
            generated,
        });
    }
    Ok(planned)
}

type AppPlanState<'a> = (
    &'a mut BTreeSet<(Location, String)>,
    &'a mut Vec<Operation>,
    &'a mut Vec<Option<FileImage>>,
);

fn append_fixed_operations(
    repository: &Repository,
    installed: &InstalledOperations<'_>,
    archiving: bool,
    (directories, operations, observed_operations): AppPlanState<'_>,
) -> Result<(), InstallError> {
    for asset in [
        Asset::GitPreCommit,
        Asset::GitCommitMessage,
        Asset::GitReferenceTransaction,
        Asset::HookEntry,
        Asset::HookStatic,
        Asset::HookZsh,
        Asset::HookBash,
        Asset::HookWrapperSpice,
        Asset::Skill,
    ] {
        let (location, path, mode) = asset_contract(asset, None)?;
        let bytes = fixed_asset_bytes(asset, repository.common_dir())?
            .expect("the fixed asset inventory contains no variable asset");
        let (bound, observed) = destination_state(repository, Location::CommonGit, &path)?;
        if archiving {
            observed_operations.push(observed.as_ref().map(|observed| observed.image.clone()));
        }
        let (previous, superseded) =
            reversal_context(installed, asset, &path, observed, None, archiving);
        operations.push(operation(
            directories,
            asset,
            (location, path),
            bound,
            previous,
            superseded,
            GeneratedImage::new(mode, &bytes),
        )?);
    }
    Ok(())
}

fn append_app_operations(
    repository: &Repository,
    installed: Option<&InstallPlan>,
    installed_operations: &InstalledOperations<'_>,
    (source, predecessors): (&app::Source, &[Digest]),
    archiving: bool,
    (directories, operations, observed_operations): AppPlanState<'_>,
) -> Result<(), InstallError> {
    for planned in planned_app_assets(installed, source, predecessors)? {
        let (location, _, _) = asset_contract(planned.asset, Some(&planned.generation))?;
        let path = planned.path;
        let (bound, observed) = destination_state(repository, Location::CommonGit, &path)?;
        if archiving {
            observed_operations.push(observed.as_ref().map(|observed| observed.image.clone()));
        }
        let (previous, superseded) = reversal_context(
            installed_operations,
            planned.asset,
            &path,
            observed,
            None,
            archiving,
        );
        operations.push(operation(
            directories,
            planned.asset,
            (location, path),
            bound,
            previous,
            superseded,
            planned.generated,
        )?);
    }
    Ok(())
}

fn append_binary_operation(
    repository: &Repository,
    installed: &InstalledOperations<'_>,
    binary: Option<&Path>,
    reconciled: Option<&GeneratedImage>,
    archiving: bool,
    (directories, operations, observed_operations): AppPlanState<'_>,
) -> Result<(), InstallError> {
    let Some(binary) = binary else {
        return Ok(());
    };
    let bytes = read_binary(binary)?;
    let (bound, observed) = destination_state(repository, Location::CommonGit, BINARY_PATH)?;
    if archiving {
        observed_operations.push(observed.as_ref().map(|observed| observed.image.clone()));
    }
    let (previous, superseded) = reversal_context(
        installed,
        Asset::Binary,
        BINARY_PATH,
        observed,
        reconciled,
        archiving,
    );
    operations.push(operation(
        directories,
        Asset::Binary,
        (Location::CommonGit, BINARY_PATH.to_owned()),
        bound,
        previous,
        superseded,
        GeneratedImage::new(EXECUTABLE_MODE, &bytes),
    )?);
    Ok(())
}

fn operation(
    virtual_directories: &mut BTreeSet<(Location, String)>,
    asset: Asset,
    (location, path): (Location, String),
    bound: BoundDestination,
    previous: Option<ObservedFile>,
    superseded: Option<GeneratedImage>,
    generated: GeneratedImage,
) -> Result<Operation, InstallError> {
    // The caller resolves where the asset lands; `validate_operation_contract`
    // refuses any operation whose location and path disagree with its contract.
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
        path,
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
    repository: &Repository,
    plan: &InstallPlan,
    spice_binary: Option<&Path>,
) -> Result<bool, InstallError> {
    // Evaluate the binary first so adding or dropping the executable remains a
    // shape refusal even when an embedded asset also changed.
    let binary_differs = rebuilt_binary(plan, spice_binary)?;
    if plan.version != PLAN_VERSION {
        return Ok(true);
    }
    let app_differs = spice_binary
        .map(|binary| app::Source::resolve(repository, binary))
        .transpose()?
        .is_some_and(|app| app.differs_from(plan));
    let symbols_differ = match spice_binary
        .map(|binary| symbols::Source::resolve(repository, binary))
        .transpose()?
        .flatten()
    {
        Some(symbols) => symbols.differs_from(plan),
        None => plan.symbols.is_some(),
    };
    Ok(binary_differs
        || app_differs
        || symbols_differ
        || plan.fixed_content_differs_from_current_build()?)
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
        Asset::HookZsh => steer::hook::STAGES[2].bytes().as_bytes().to_vec(),
        Asset::HookBash => steer::hook::STAGES[3].bytes().as_bytes().to_vec(),
        Asset::HookWrapperSpice => steer::hook::STAGES[4].bytes().as_bytes().to_vec(),
        Asset::Skill => SKILL.as_bytes().to_vec(),
        Asset::Exclusions
        | Asset::AppModule
        | Asset::AppWasm
        | Asset::AppHtml
        | Asset::AppChrome
        | Asset::AppComposer
        | Asset::AppMosaic
        | Asset::AppResponsive
        | Asset::AppStyle
        | Asset::AppWork
        | Asset::AppRenderer(_)
        | Asset::AppCurrent
        | Asset::Binary
        | Asset::SymbolsInfo
        | Asset::SymbolsDwarf => return Ok(None),
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

/// Hard ceiling on the skill omp carries whole in every system prompt.
///
/// The skill is paid for on every request of every lane, so a skill that
/// outgrows this fails the build rather than silently taxing each turn. The
/// working target is half of it.
const SKILL_BYTES_CEILING: usize = 16 * 1024;
const _: () = assert!(
    SKILL.len() <= SKILL_BYTES_CEILING,
    "spice/agent/SKILL.md exceeds the 16 KiB it may occupy in every agent's system prompt"
);

/// The installed skill's instructions: its body without the frontmatter.
///
/// The frontmatter is catalogue metadata for skill loaders; an agent handed the
/// skill inline needs only the instructions after it.
#[must_use]
pub fn skill_instructions() -> &'static str {
    SKILL
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map_or(SKILL, |(_, body)| body)
        .trim()
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

/// Installed zsh startup directory for direct agent command shells.
#[must_use]
pub fn installed_zdotdir(repository: &Repository) -> PathBuf {
    repository.common_dir().join(".spice-install/hooks/shell")
}

/// Installed Bash startup file for direct agent command shells.
#[must_use]
pub fn installed_bash_env(repository: &Repository) -> PathBuf {
    repository.common_dir().join(BASH_HOOK_PATH)
}

/// Installed shell wrapper selected by tracked `wrappers.spice` policy.
#[must_use]
pub fn installed_spice_wrapper(repository: &Repository) -> PathBuf {
    repository.common_dir().join(WRAPPER_SPICE_PATH)
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
