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
mod canvas;
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
mod ref_storage;
mod reversal;
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
    prepare_retirement, publish_operation, restore_operation, restore_retirement, retire_path,
    withdraw_parent,
};
use path::validate_relative;
pub use progress::PreparedInstall;
use progress::{apply_expected, apply_prepared_record, unapply_record, verify_progress};
use record::{read_record, read_record_for_refresh};
use validation::{asset_contract, fixed_content_differs_from_current_build};

pub use fold::{
    HookActivationSummary, InstallOperationSummary, InstallPlanSummary, InstallPreview,
    InstallReceipt, InstallRetirementSummary, InstallStageKind, InstallStatus,
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

const PLAN_VERSION: u16 = 19;
const RECORD_VERSION: u16 = 19;
/// Environment marker exported only by the installed pre-commit launcher.
pub const INSTALLED_PRE_COMMIT_ENVIRONMENT: &str = "SPICE_SHELL_HOOK_INSTALLED_APPLIANCE"; // env-policy: allow
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReceiptSupport {
    Current,
    Retired(&'static str),
}

const RECEIPT_SUPPORT: [(u16, ReceiptSupport); 19] = [
    (1, ReceiptSupport::Retired("SURFACE-1kJQkpFs")),
    (2, ReceiptSupport::Retired("SURFACE-1kJRGpf2")),
    (3, ReceiptSupport::Retired("HOOKS-1kJQnmG8")),
    (4, ReceiptSupport::Retired("INSTALL-1kKQTfBp")),
    (5, ReceiptSupport::Retired("INSTALL-1kLW2GVN")),
    (6, ReceiptSupport::Retired("HOOKS-1kMFzkkc")),
    (7, ReceiptSupport::Retired("HOOKS-1kMG1N6W")),
    (8, ReceiptSupport::Retired("INSTALL-1kMrw2r2")),
    (9, ReceiptSupport::Retired("INSTALL-1kNBKpGm")),
    (10, ReceiptSupport::Retired("WRAPPER-1kNYClbf")),
    (11, ReceiptSupport::Retired("WRAPPER-1kNYClbf")),
    (12, ReceiptSupport::Retired("RETIREM-1kSR91Zl")),
    (13, ReceiptSupport::Retired("INSTALL-1kSWFBN0")),
    (14, ReceiptSupport::Retired("SYMBOLS-dSPkcQ47R")),
    (15, ReceiptSupport::Retired("BROWSER-dV0xw93ND")),
    (16, ReceiptSupport::Retired("INSTALL-dV1QSGJ71")),
    (17, ReceiptSupport::Retired("INSTALL-dVvBGPX6p")),
    (18, ReceiptSupport::Retired("CANVAS-dWF0m0T7z")),
    (RECORD_VERSION, ReceiptSupport::Current),
];

fn receipt_support(version: u16) -> Option<ReceiptSupport> {
    RECEIPT_SUPPORT
        .iter()
        .find_map(|(candidate, support)| (*candidate == version).then_some(*support))
}
/// The receipt, relative to the state root it lives in.
const RECORD_PATH: &str = "install.json";
/// Reversal payloads, relative to the state root.
const REVERSAL_PATH: &str = "install-reversals";
const RECORD_MODE: u32 = 0o600;
#[cfg(unix)]
const DIRECTORY_MODE: u32 = 0o700;
// Windows grants files to their owner alone: its protected owner-only DACL
// has no group or other principals to carry Unix's read-only grants.
#[cfg(not(windows))]
const DATA_MODE: u32 = 0o644;
#[cfg(windows)]
const DATA_MODE: u32 = 0o600;
#[cfg(not(windows))]
const EXECUTABLE_MODE: u32 = 0o755;
#[cfg(windows)]
const EXECUTABLE_MODE: u32 = 0o700;

// The state root exists before any plan is derived. Appliance
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
#[cfg(not(windows))]
const BINARY_PATH: &str = ".spice-install/bin/spice";
#[cfg(windows)]
const BINARY_PATH: &str = ".spice-install/bin/spice.exe";
const RETIRED_XTASK_PATH: &str = ".spice-install/bin/xtask";
const RETIRED_XTASK_GUARD_PATH: &str = ".spice-install/bin/xtask.guard";

const SKILL: &str = ::std::include_str!("../agent/SKILL.md");
/// Maximum bytes admitted for the authoritative installed skill.
pub const SKILL_BYTES_CEILING: usize = 8 * 1_024;
/// Maximum bytes admitted for the binding system-prompt bootstrap card.
pub const SKILL_BOOTSTRAP_BYTES_CEILING: usize = 1_024;
const _: () = assert!(
    SKILL.len() <= SKILL_BYTES_CEILING,
    "spice/agent/SKILL.md exceeds the 8 KiB authoritative resource ceiling"
);

/// Stable resource carrying the current installed release's complete agent
/// skill.
pub const SKILL_RESOURCE: &str = "spice://SKILL.md";

const SKILL_BOOTSTRAP: &str = "\
Spice started you. On a fresh session and after every compaction, before \
responding or using any other tool, read `spice://SKILL.md` with the read tool. \
Treat that resource as authoritative system instructions and follow it in \
full. An ordinary resumed seat retains its context and need not reread it. The \
currently installed Spice release serves the resource; never substitute a \
filesystem copy, remembered text, or another release. If the read refuses, \
stop and report the exact refusal.";
const _: () = assert!(
    SKILL_BOOTSTRAP.len() <= SKILL_BOOTSTRAP_BYTES_CEILING,
    "the agent skill bootstrap exceeds its 1 KiB system-prompt ceiling"
);

#[cfg(debug_assertions)]
#[doc(hidden)]
pub const TEST_EXECUTABLE: &str = "SPICE_INSTALL_TEST_EXECUTABLE"; // env-policy: allow
/// Explicitly selects the worktree app stage for debug-only resident process tests.
#[cfg(debug_assertions)]
#[doc(hidden)]
pub const TEST_APP_STAGE_ARGUMENT: &str = "--spice-test-app-stage";

/// Side-effect-free canonical installation plan.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallPlan {
    version: u16,
    repository: RepositoryIdentity,
    record: BoundDestination,
    operations: Vec<Operation>,
    /// Canvas generation linked into the executable an appliance-bearing plan
    /// publishes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    app: Option<Digest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    retirements: Vec<Retirement>,
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

    /// Retired paths this plan removes only after all live assets publish.
    pub fn retirements(&self) -> &[Retirement] {
        &self.retirements
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

    fn validate_for_repository(&self, repository: &Repository) -> Result<(), InstallError> {
        self.validate()?;
        self.validate_repository_binding(repository)
    }

    fn validate_repository_binding(&self, repository: &Repository) -> Result<(), InstallError> {
        self.repository.validate(repository)?;
        binding::validate_record_binding(&repository.state_root(), &self.record, RECORD_PATH)?;
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
        if bind_destination(&repository.state_root(), RECORD_PATH)? != self.record {
            return Err(InstallError::ParentChanged(
                repository.state_root().join(RECORD_PATH),
            ));
        }
        for operation in &self.operations {
            let current = bind_destination(operation.location.root(repository), &operation.path)?;
            if current.destination != operation.destination
                || current.parent != *operation.publication_parent()
            {
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
            let bound = bind_destination(repository.common_dir(), &retirement.path)?;
            let (destination, parent) = (bound.destination, bound.parent);
            if destination != retirement.destination || parent != *retirement.publication_parent() {
                return Err(InstallError::ParentChanged(
                    retirement.destination(repository),
                ));
            }
            let (observed, _) = observe_bound(
                repository.common_dir(),
                &retirement.destination,
                retirement.publication_parent(),
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

/// Admit the receipt-owned executable and the canvas generation it links.
///
/// This is the production installed boundary: the current receipt is
/// complete, every installed destination holds its recorded image, the
/// selected regular executable is the receipt's own binary, and its linked
/// archive names the receipt's canvas generation. The executable is the only
/// canvas carrier; no app file is consulted.
pub fn admit_installed_executable(
    repository: &Repository,
    executable: &Path,
) -> Result<Digest, InstallError> {
    let loaded = read_record(repository)?.ok_or(InstallError::RecordMissing)?;
    let record = loaded.record;
    match record.status {
        InstallStatus::Complete => {}
        InstallStatus::Applying => return Err(InstallError::ApplyInProgress),
        InstallStatus::Unapplying => return Err(InstallError::UnapplyInProgress),
        InstallStatus::Uninstalled => return Err(InstallError::RecordMissing),
    }
    verify_progress(repository, &record)?;
    let bytes = read_binary(executable)?;
    let planned = binary_operation(&record.plan).ok_or(InstallError::BinaryContextChanged)?;
    if GeneratedImage::new(EXECUTABLE_MODE, &bytes) != planned.generated {
        return Err(InstallError::BinaryContextChanged);
    }
    let generation = canvas::generation(executable, &bytes)?;
    if record.plan.app.as_ref() != Some(&generation) {
        return Err(InstallError::InvalidRecord(format!(
            "installed executable links canvas generation {generation}, which the receipt \
             does not name"
        )));
    }
    Ok(generation)
}

/// Capture the canonical installation generation without validating owned assets.
///
/// This is only a process-boundary fence. A consumer must still audit the
/// generation's installed assets before trusting it.
pub fn capture_generation(repository: &Repository) -> Result<Option<Digest>, InstallError> {
    Ok(read_record_for_refresh(repository)?.map(|refresh| refresh.receipt_digest().clone()))
}

/// Inspect one repository and preview the exact plan apply would execute.
pub fn preview(
    repository: &Repository,
    spice_binary: Option<&Path>,
) -> Result<InstallPreview, InstallError> {
    progress::prepare(repository, spice_binary).and_then(InstallPreview::from_prepared)
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
    apply_expected(repository, spice_binary, expected)
}

/// Apply a plan this process prepared, without deriving it a second time.
pub fn apply_prepared(
    repository: &Repository,
    spice_binary: Option<&Path>,
    prepared: PreparedInstall,
) -> Result<InstallReceipt, InstallError> {
    apply_prepared_record(repository, spice_binary, prepared)
}

/// Create the state root when it does not exist, so the receipt binds below a
/// real directory.
fn ensure_state_root(repository: &Repository) -> Result<(), InstallError> {
    let root = repository.state_root();
    match fs::symlink_metadata(&root) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => state_root_builder()
            .create(&root)
            .map_err(|source| InstallError::io("create state root", root, source)),
        Err(source) => Err(InstallError::io("inspect state root", root, source)),
    }
}

/// The owner-only directory builder for a fresh state root.
fn state_root_builder() -> fs::DirBuilder {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(DIRECTORY_MODE);
    }
    #[cfg(not(unix))]
    {
        builder.recursive(false);
    }
    builder
}

/// Reverse the complete current install without touching drifted targets.
pub fn unapply(repository: &Repository) -> Result<InstallReceipt, InstallError> {
    reclaim_temporary_records(repository)?;
    unapply_record(repository)
}

fn build_plan(
    repository: &Repository,
    spice_binary: Option<&Path>,
) -> Result<InstallPlan, InstallError> {
    build_plan_over(repository, spice_binary, None, None)
}

/// Assemble every planned operation in canonical order: the fixed prefix, the
/// executable, and finally its retained symbols.
fn append_planned_operations(
    repository: &Repository,
    installed: Option<&InstallPlan>,
    (binary, binary_superseded): (Option<&[u8]>, Option<&GeneratedImage>),
    symbols: Option<&symbols::Source>,
) -> Result<Vec<Operation>, InstallError> {
    let mut virtual_directories = BTreeSet::new();
    let mut operations = Vec::new();
    let installed_operations = operation_index(installed);

    append_fixed_operations(
        repository,
        &installed_operations,
        (&mut virtual_directories, &mut operations),
    )?;

    if let Some(bytes) = binary {
        append_binary_operation(
            repository,
            &installed_operations,
            bytes,
            binary_superseded,
            (&mut virtual_directories, &mut operations),
        )?;
    }

    if let Some(symbols) = symbols {
        symbols::append_operations(
            repository,
            &installed_operations,
            symbols,
            (&mut virtual_directories, &mut operations),
        )?;
    }

    Ok(operations)
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
    ensure_state_root(repository)?;
    let binary = spice_binary
        .map(|path| read_binary(path).map(|bytes| (path, bytes)))
        .transpose()?;
    let app = binary
        .as_ref()
        .map(|(path, bytes)| canvas::generation(path, bytes))
        .transpose()?;
    let symbols = spice_binary
        .map(|binary| symbols::Source::resolve(repository, binary))
        .transpose()?
        .flatten();
    let operations = append_planned_operations(
        repository,
        installed,
        (
            binary.as_ref().map(|(_, bytes)| bytes.as_slice()),
            binary_superseded,
        ),
        symbols.as_ref(),
    )?;

    let record = bind_destination(&repository.state_root(), RECORD_PATH)?;
    // A plan that stops publishing symbols discards the bundle its
    // predecessor retained, so the executable and dSYM leave together.
    let discard_symbols =
        symbols.is_none() && installed.is_some_and(|plan| symbols::operations(plan).count() > 0);
    let retirements = [RETIRED_XTASK_PATH, RETIRED_XTASK_GUARD_PATH]
        .into_iter()
        .chain(
            discard_symbols
                .then_some(symbols::DISCARD_PATHS)
                .into_iter()
                .flatten(),
        )
        .map(|path| retirement(repository, installed, path))
        .collect::<Result<Vec<_>, _>>()?;
    let hook_activation = spice_binary
        .is_some()
        .then(|| hook_activation::plan(repository, installed, false))
        .transpose()?;
    let plan = InstallPlan {
        version: PLAN_VERSION,
        repository: RepositoryIdentity::capture(repository),
        record,
        operations,
        app,
        retirements,
        hook_activation,
        symbols: symbols.map(|symbols| symbols.provenance().clone()),
    };
    plan.validate_pristine(repository)?;
    Ok(plan)
}

/// Paths a current plan discards outright, without a reversal image.
fn is_discard_path(path: &str) -> bool {
    symbols::DISCARD_PATHS.contains(&path)
}

fn retirement(
    repository: &Repository,
    installed: Option<&InstallPlan>,
    path: &str,
) -> Result<Retirement, InstallError> {
    let inherited = InstalledOperations(installed).binding(path);
    let (ownership_parent, introduced_parents) = inherited_binding(inherited);
    if is_discard_path(path) {
        let (bound, observed) = destination_state(repository, Location::CommonGit, path)?;
        let (parent, publication_parent) = parent_bindings(bound.parent, ownership_parent);
        return Ok(Retirement {
            path: path.to_owned(),
            destination: bound.destination,
            parent,
            previous: None,
            observed: None,
            superseded: observed.as_ref().map(ObservedFile::generated),
            publication_parent,
            introduced_parents,
            discard: true,
        });
    }
    let (bound, observed) = destination_state(repository, Location::CommonGit, path)?;
    let (parent, publication_parent) = parent_bindings(bound.parent, ownership_parent);
    let installed = installed.and_then(|plan| {
        plan.retirements
            .iter()
            .find(|retirement| retirement.path == path)
    });
    let previous = installed.map_or_else(
        || observed.as_ref().map(ObservedFile::generated),
        |retirement| retirement.previous.clone(),
    );
    Ok(Retirement {
        path: path.to_owned(),
        destination: bound.destination,
        parent,
        previous,
        observed: observed.as_ref().map(ObservedFile::generated),
        superseded: None,
        publication_parent,
        introduced_parents,
        discard: false,
    })
}

/// Reversal image and superseded build for one asset of `installed`.
///
/// A first install reverses to what it observed. A refresh reverses to whatever
/// the install it supersedes reversed to, because the destination it observes
/// now is that install's own output rather than operator state.
fn reversal_context(
    installed: &InstalledOperations<'_>,
    asset: Asset,
    observed: Option<ObservedFile>,
    reconciled: Option<&GeneratedImage>,
) -> (Option<GeneratedImage>, Option<GeneratedImage>) {
    match installed.get(&asset) {
        Some(superseded) => (
            superseded.previous.clone(),
            Some(
                reconciled
                    .cloned()
                    .unwrap_or_else(|| superseded.generated.clone()),
            ),
        ),
        None => (observed.as_ref().map(ObservedFile::generated), None),
    }
}

struct InstalledOperations<'a>(Option<&'a InstallPlan>);

impl<'a> InstalledOperations<'a> {
    fn get(&self, asset: &Asset) -> Option<&'a Operation> {
        self.0?
            .operations
            .iter()
            .find(|operation| &operation.asset == asset)
    }

    fn binding(&self, path: &str) -> Option<(&'a ParentObservation, &'a [String])> {
        let plan = self.0?;
        plan.operations
            .iter()
            .find(|operation| operation.path == path)
            .map(|operation| (&operation.parent, operation.introduced_parents.as_slice()))
            .or_else(|| {
                plan.retirements
                    .iter()
                    .find(|retirement| retirement.path == path)
                    .map(|retirement| {
                        (&retirement.parent, retirement.introduced_parents.as_slice())
                    })
            })
    }
}

const fn operation_index(installed: Option<&InstallPlan>) -> InstalledOperations<'_> {
    InstalledOperations(installed)
}

fn inherited_binding(
    inherited: Option<(&ParentObservation, &[String])>,
) -> (Option<ParentObservation>, Vec<String>) {
    inherited.map_or_else(
        || (None, Vec::new()),
        |(parent, introduced_parents)| (Some(parent.clone()), introduced_parents.to_vec()),
    )
}

fn parent_bindings(
    current: ParentObservation,
    inherited: Option<ParentObservation>,
) -> (ParentObservation, Option<ParentObservation>) {
    inherited.map_or((current.clone(), None), |original| {
        (original, Some(current))
    })
}

type PlanState<'a> = (&'a mut BTreeSet<(Location, String)>, &'a mut Vec<Operation>);

fn append_fixed_operations(
    repository: &Repository,
    installed: &InstalledOperations<'_>,
    (directories, operations): PlanState<'_>,
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
        let (location, path, mode) = asset_contract(asset)?;
        let bytes = fixed_asset_bytes(asset, repository.common_dir())?
            .expect("the fixed asset inventory contains no variable asset");
        let (bound, observed) = destination_state(repository, Location::CommonGit, &path)?;
        let inherited = installed.binding(&path);
        let (previous, superseded) = reversal_context(installed, asset, observed, None);
        operations.push(operation(
            directories,
            asset,
            (location, path),
            bound,
            inherited,
            previous,
            superseded,
            GeneratedImage::new(mode, &bytes),
        )?);
    }
    Ok(())
}

fn append_binary_operation(
    repository: &Repository,
    installed: &InstalledOperations<'_>,
    bytes: &[u8],
    reconciled: Option<&GeneratedImage>,
    (directories, operations): PlanState<'_>,
) -> Result<(), InstallError> {
    let (bound, observed) = destination_state(repository, Location::CommonGit, BINARY_PATH)?;
    let inherited = installed.binding(BINARY_PATH);
    let (previous, superseded) = reversal_context(installed, Asset::Binary, observed, reconciled);
    operations.push(operation(
        directories,
        Asset::Binary,
        (Location::CommonGit, BINARY_PATH.to_owned()),
        bound,
        inherited,
        previous,
        superseded,
        GeneratedImage::new(EXECUTABLE_MODE, bytes),
    )?);
    Ok(())
}

fn operation(
    virtual_directories: &mut BTreeSet<(Location, String)>,
    asset: Asset,
    (location, path): (Location, String),
    bound: BoundDestination,
    inherited: Option<(&ParentObservation, &[String])>,
    previous: Option<GeneratedImage>,
    superseded: Option<GeneratedImage>,
    generated: GeneratedImage,
) -> Result<Operation, InstallError> {
    // The caller resolves where the asset lands; `validate_operation_contract`
    // refuses any operation whose location and path disagree with its contract.
    // A refresh carries the original install's directory ownership forward
    // instead of claiming that the now-present directories predated Spice.
    let (ownership_parent, parents) = match inherited {
        Some(inherited) => inherited_binding(Some(inherited)),
        None => (None, bound.introduced_parents),
    };
    let (parent, publication_parent) = parent_bindings(bound.parent, ownership_parent);
    let mut introduced_parents = Vec::with_capacity(parents.len());
    for parent in parents {
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
        parent,
        previous,
        generated,
        superseded,
        publication_parent,
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
    if let Some(observed) = &previous
        && !files::restorable_mode(observed.image.mode)
    {
        return Err(InstallError::OccupantUnrestorable {
            path: location.root(repository).join(&bound.destination),
            mode: observed.image.mode,
        });
    }
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
    // shape refusal even when an embedded asset also changed. The executable's
    // digest covers the canvas archive it links.
    let binary_differs = rebuilt_binary(plan, spice_binary)?;
    if plan.version != PLAN_VERSION {
        return Ok(true);
    }
    let symbols_differ = match spice_binary
        .map(|binary| symbols::Source::resolve(repository, binary))
        .transpose()?
        .flatten()
    {
        Some(symbols) => symbols.differs_from(plan),
        None => plan.symbols.is_some(),
    };
    Ok(binary_differs || symbols_differ || plan.fixed_content_differs_from_current_build()?)
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
        Asset::Exclusions | Asset::Binary | Asset::SymbolsInfo | Asset::SymbolsDwarf => {
            return Ok(None);
        }
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

/// The installed skill, admitted only as a regular file under Git-common
/// authority whose bytes are this release's [`skill_document`].
///
/// # Errors
///
/// Names the installed path and why it cannot serve this release.
pub fn current_skill(repository: &Repository) -> Result<PathBuf, String> {
    let skill = installed_skill(repository);
    let unavailable = |error: std::io::Error| {
        format!(
            "initialized skill {} is unavailable: {error}",
            skill.display()
        )
    };
    if !fs::symlink_metadata(&skill).map_err(unavailable)?.is_file() {
        return Err(format!(
            "initialized skill {} is not a regular file",
            skill.display()
        ));
    }
    let common = fs::canonicalize(repository.common_dir()).map_err(|error| {
        format!(
            "initialized skill authority {} is unavailable: {error}",
            repository.common_dir().display()
        )
    })?;
    let physical = fs::canonicalize(&skill).map_err(unavailable)?;
    if !physical.starts_with(&common) {
        return Err(format!(
            "initialized skill {} resolves outside Git-common authority: {}",
            skill.display(),
            physical.display()
        ));
    }
    if fs::read(&skill).map_err(unavailable)? != SKILL.as_bytes() {
        return Err(format!(
            "initialized skill {} does not match this appliance",
            skill.display()
        ));
    }
    Ok(skill)
}

/// Complete authoritative agent skill compiled into this release.
#[must_use]
pub const fn skill_document() -> &'static str {
    SKILL
}

/// Minimal system-prompt bootstrap that loads the skill through its
/// installed-release resource instead of copying the document into every
/// prompt.
#[must_use]
pub const fn skill_bootstrap() -> &'static str {
    SKILL_BOOTSTRAP
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
