use serde::{Deserialize, Serialize};
use wire::{Digest, Domain};

use super::super::{
    Asset, BoundDestination, FileImage, GeneratedImage, Generation, HookActivationPlan,
    InstallError, InstallPlan, InstallStatus, Location, ObservedFile, Operation, ParentObservation,
    RepositoryIdentity, Retirement, RollbackArchive, SymbolProvenance, app,
};
use super::InstallRecord;

/// Version-15 asset vocabulary, decoded only long enough to prove and migrate
/// the authored receipt. No installation path executes these variants.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum PreviousAsset {
    Exclusions,
    GitPreCommit,
    GitCommitMessage,
    GitReferenceTransaction,
    HookEntry,
    HookStatic,
    HookZsh,
    HookBash,
    HookWrapperSpice,
    Skill,
    #[serde(rename = "canvas-app-module")]
    AppModule,
    #[serde(rename = "canvas-wasm")]
    AppWasm,
    #[serde(rename = "canvas-html")]
    AppHtml,
    #[serde(rename = "canvas-chrome")]
    AppChrome,
    #[serde(rename = "canvas-composer")]
    AppComposer,
    #[serde(rename = "canvas-mosaic")]
    AppMosaic,
    #[serde(rename = "canvas-responsive")]
    AppResponsive,
    #[serde(rename = "canvas-style")]
    AppStyle,
    #[serde(rename = "canvas-work")]
    AppWork,
    #[serde(rename = "canvas-renderer")]
    AppRenderer(wire::manifest::renderer::File),
    #[serde(rename = "canvas-current")]
    AppCurrent,
    Binary,
    SymbolsInfo,
    SymbolsDwarf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PreviousOperation {
    asset: PreviousAsset,
    location: Location,
    path: String,
    destination: String,
    parent: ParentObservation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous: Option<ObservedFile>,
    generated: GeneratedImage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    superseded: Option<GeneratedImage>,
    introduced_parents: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PreviousPlan {
    version: u16,
    repository: RepositoryIdentity,
    record: BoundDestination,
    operations: Vec<PreviousOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    app: Option<Digest>,
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PreviousRecord {
    version: u16,
    repository: RepositoryIdentity,
    plan: PreviousPlan,
    plan_digest: Digest,
    reversals: Vec<Option<ObservedFile>>,
    destinations: Vec<(Location, String)>,
    status: InstallStatus,
    completed: usize,
    installed: Vec<Option<Generation>>,
    installed_parents: Vec<Option<Generation>>,
    restored: Vec<Option<Generation>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    retired_parents: Vec<Option<Generation>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    restored_retirements: Vec<Option<Generation>>,
}

pub(super) fn decode(bytes: &[u8]) -> Result<InstallRecord, InstallError> {
    let previous: PreviousRecord = wire::decode(bytes).map_err(InstallError::Decode)?;
    let canonical = wire::encode(&previous).map_err(InstallError::Encode)?;
    if canonical != bytes {
        return Err(InstallError::InvalidRecord(
            "install record is not canonical".to_owned(),
        ));
    }
    if previous.version != super::super::PREVIOUS_RECORD_VERSION
        || previous.plan.version != super::super::PREVIOUS_RECORD_VERSION
    {
        return Err(InstallError::InvalidRecord(
            "previous install record has the wrong schema version".to_owned(),
        ));
    }
    let plan_bytes = wire::encode(&previous.plan).map_err(InstallError::Encode)?;
    if previous.plan_digest != Digest::compute(Domain::InstallPlan, &plan_bytes) {
        return Err(InstallError::InvalidRecord(
            "install record plan digest does not match its canonical plan".to_owned(),
        ));
    }
    Ok(previous.into())
}

const RETIRED_SHELL: &str = "canvas-app.html";

pub(super) struct ShellRefresh {
    pub(super) plan: InstallPlan,
    pub(super) retirements: Vec<String>,
    pub(super) introductions: Vec<String>,
}

pub(super) fn required(plan: &InstallPlan) -> bool {
    plan.operations.iter().any(|operation| {
        operation.asset == Asset::AppHtml && retired_shell_generation(&operation.path).is_some()
    })
}

/// Normalize one admitted pre-rename shell receipt before current planning.
///
/// Installed predecessor files still use the retired name while progress is
/// verified. The derived current plan publishes only `app.html` and withdraws
/// those old files through ordinary retirement execution.
pub(super) fn refresh(plan: &InstallPlan) -> Result<ShellRefresh, InstallError> {
    let mut plan = plan.clone();
    let mut retirements = Vec::new();
    let mut introductions = Vec::new();
    for operation in &mut plan.operations {
        let Some((generation, current_path)) =
            migrated_shell_path(operation.asset, &operation.path)?
        else {
            continue;
        };
        if !plan
            .app
            .iter()
            .chain(&plan.app_predecessors)
            .any(|installed| installed == &generation)
        {
            return Err(InstallError::InvalidRecord(
                "retired app shell names an unrecorded generation".to_owned(),
            ));
        }
        if plan.version != super::super::PREVIOUS_RECORD_VERSION
            && plan.app.as_ref() == Some(&generation)
        {
            return Err(InstallError::InvalidRecord(
                "current receipt publishes a retired app shell".to_owned(),
            ));
        }
        let current_destination = respell_destination(&operation.destination)?;
        retirements.push(std::mem::replace(&mut operation.path, current_path.clone()));
        introductions.push(current_path);
        operation.destination = current_destination;
    }
    for retirement in &mut plan.retirements {
        let Some((_, current_path)) = migrated_shell_path(Asset::AppHtml, &retirement.path)? else {
            continue;
        };
        retirement.destination = respell_destination(&retirement.destination)?;
        retirement.path = current_path;
    }
    retirements.sort();
    introductions.sort();
    Ok(ShellRefresh {
        plan,
        retirements,
        introductions,
    })
}

/// Validate a receipt requiring old-shell migration through its normalized
/// in-memory shape without changing the receipt or its on-disk destinations.
pub(super) fn validate(record: &InstallRecord) -> Result<(), InstallError> {
    if record.version != super::super::PREVIOUS_RECORD_VERSION
        && record.plan_digest != record.plan.digest()?
    {
        return Err(InstallError::InvalidRecord(
            "install record plan digest does not match its canonical plan".to_owned(),
        ));
    }
    let mut normalized = record.clone();
    normalized.plan = refresh(&record.plan)?.plan;
    normalized.plan_digest = normalized.plan.digest()?;
    for ((_, destination), operation) in normalized
        .destinations
        .iter_mut()
        .zip(&normalized.plan.operations)
    {
        destination.clone_from(&operation.destination);
    }
    let historical = normalized.status == InstallStatus::Uninstalled
        || (normalized.status == InstallStatus::Complete
            && normalized.version == super::super::PREVIOUS_RECORD_VERSION)
        || (normalized.status == InstallStatus::Complete
            && super::super::binary_operation(&normalized.plan).is_some());
    normalized.validate_with_content(historical)
}

/// Validate the one-time old-shell withdrawals carried by a current plan.
pub(super) fn validate_retirements(
    plan: &InstallPlan,
    retirements: &[Retirement],
) -> Result<(), InstallError> {
    let mut prior = None;
    for retirement in retirements {
        let Some(generation) = retired_shell_generation(&retirement.path) else {
            return Err(InstallError::InvalidRecord(
                "install retirement inventory is not exact and ordered".to_owned(),
            ));
        };
        let generation: wire::Digest = generation.parse().map_err(|_| {
            InstallError::InvalidRecord(
                "previous app shell has an invalid generation identity".to_owned(),
            )
        })?;
        if let Some(prior) = &prior
            && prior >= &generation
        {
            return Err(InstallError::InvalidRecord(format!(
                "previous app shell retirements are not ordered: {prior} before {generation}"
            )));
        }
        let current = app::operation_path(Asset::AppHtml, Some(&generation))?
            .expect("an app shell has a generation path");
        let Some(operation) = plan
            .operations
            .iter()
            .find(|operation| operation.asset == Asset::AppHtml && operation.path == current)
        else {
            return Err(InstallError::InvalidRecord(
                "previous app shell retirement has no current replacement".to_owned(),
            ));
        };
        if retirement
            .observed
            .as_ref()
            .is_none_or(|observed| observed.image.generated() != operation.generated)
        {
            return Err(InstallError::InvalidRecord(
                "previous app shell retirement has no authenticated replacement image".to_owned(),
            ));
        }
        prior = Some(generation);
    }
    Ok(())
}

/// Authenticated retired bytes republished once under their current name.
pub(super) fn introduction_image<'a>(
    plan: &'a InstallPlan,
    operation: &Operation,
) -> Result<Option<&'a FileImage>, InstallError> {
    let Some(generation) = app::operation_generation(operation.asset, &operation.path) else {
        return Ok(None);
    };
    if operation.asset != Asset::AppHtml {
        return Ok(None);
    }
    let retired = format!("{}/{generation}/{RETIRED_SHELL}", app::GENERATIONS_PATH);
    let Some(retirement) = plan
        .retirements
        .iter()
        .find(|retirement| retirement.path == retired)
    else {
        return Ok(None);
    };
    let image = retirement
        .observed
        .as_ref()
        .map(|observed| &observed.image)
        .ok_or_else(|| {
            InstallError::InvalidRecord(
                "previous app shell retirement has no authenticated replacement image".to_owned(),
            )
        })?;
    if image.generated() != operation.generated {
        return Err(InstallError::InvalidRecord(
            "previous app shell retirement image does not match its current replacement".to_owned(),
        ));
    }
    Ok(Some(image))
}

fn migrated_shell_path(
    asset: Asset,
    path: &str,
) -> Result<Option<(wire::Digest, String)>, InstallError> {
    if asset != Asset::AppHtml {
        return Ok(None);
    }
    let Some(generation) = retired_shell_generation(path) else {
        return Ok(None);
    };
    let generation = generation.parse().map_err(|_| {
        InstallError::InvalidRecord(
            "previous app shell has an invalid generation identity".to_owned(),
        )
    })?;
    let current =
        app::operation_path(asset, Some(&generation))?.expect("an app shell has a generation path");
    Ok(Some((generation, current)))
}

fn retired_shell_generation(path: &str) -> Option<&str> {
    let relative = path
        .strip_prefix(app::GENERATIONS_PATH)?
        .strip_prefix('/')?;
    relative
        .strip_suffix(RETIRED_SHELL)?
        .strip_suffix('/')
        .filter(|generation| !generation.is_empty() && !generation.contains('/'))
}

fn respell_destination(path: &str) -> Result<String, InstallError> {
    path.strip_suffix(RETIRED_SHELL)
        .map(|parent| format!("{parent}{}", wire::manifest::APP_HTML))
        .ok_or_else(|| {
            InstallError::InvalidRecord(
                "previous app shell destination has no retired spelling".to_owned(),
            )
        })
}

#[cfg(debug_assertions)]
pub(super) fn encode(record: &InstallRecord) -> Result<Vec<u8>, InstallError> {
    let mut previous = PreviousRecord::from(record);
    let plan_bytes = wire::encode(&previous.plan).map_err(InstallError::Encode)?;
    previous.plan_digest = Digest::compute(Domain::InstallPlan, &plan_bytes);
    wire::encode(&previous).map_err(InstallError::Encode)
}

impl From<PreviousAsset> for Asset {
    fn from(asset: PreviousAsset) -> Self {
        match asset {
            PreviousAsset::Exclusions => Self::Exclusions,
            PreviousAsset::GitPreCommit => Self::GitPreCommit,
            PreviousAsset::GitCommitMessage => Self::GitCommitMessage,
            PreviousAsset::GitReferenceTransaction => Self::GitReferenceTransaction,
            PreviousAsset::HookEntry => Self::HookEntry,
            PreviousAsset::HookStatic => Self::HookStatic,
            PreviousAsset::HookZsh => Self::HookZsh,
            PreviousAsset::HookBash => Self::HookBash,
            PreviousAsset::HookWrapperSpice => Self::HookWrapperSpice,
            PreviousAsset::Skill => Self::Skill,
            PreviousAsset::AppModule => Self::AppModule,
            PreviousAsset::AppWasm => Self::AppWasm,
            PreviousAsset::AppHtml => Self::AppHtml,
            PreviousAsset::AppChrome => Self::AppChrome,
            PreviousAsset::AppComposer => Self::AppComposer,
            PreviousAsset::AppMosaic => Self::AppMosaic,
            PreviousAsset::AppResponsive => Self::AppResponsive,
            PreviousAsset::AppStyle => Self::AppStyle,
            PreviousAsset::AppWork => Self::AppWork,
            PreviousAsset::AppRenderer(file) => Self::AppRenderer(file),
            PreviousAsset::AppCurrent => Self::AppCurrent,
            PreviousAsset::Binary => Self::Binary,
            PreviousAsset::SymbolsInfo => Self::SymbolsInfo,
            PreviousAsset::SymbolsDwarf => Self::SymbolsDwarf,
        }
    }
}

impl From<Asset> for PreviousAsset {
    fn from(asset: Asset) -> Self {
        match asset {
            Asset::Exclusions => Self::Exclusions,
            Asset::GitPreCommit => Self::GitPreCommit,
            Asset::GitCommitMessage => Self::GitCommitMessage,
            Asset::GitReferenceTransaction => Self::GitReferenceTransaction,
            Asset::HookEntry => Self::HookEntry,
            Asset::HookStatic => Self::HookStatic,
            Asset::HookZsh => Self::HookZsh,
            Asset::HookBash => Self::HookBash,
            Asset::HookWrapperSpice => Self::HookWrapperSpice,
            Asset::Skill => Self::Skill,
            Asset::AppModule => Self::AppModule,
            Asset::AppWasm => Self::AppWasm,
            Asset::AppHtml => Self::AppHtml,
            Asset::AppChrome => Self::AppChrome,
            Asset::AppComposer => Self::AppComposer,
            Asset::AppMosaic => Self::AppMosaic,
            Asset::AppResponsive => Self::AppResponsive,
            Asset::AppStyle => Self::AppStyle,
            Asset::AppWork => Self::AppWork,
            Asset::AppRenderer(file) => Self::AppRenderer(file),
            Asset::AppCurrent => Self::AppCurrent,
            Asset::Binary => Self::Binary,
            Asset::SymbolsInfo => Self::SymbolsInfo,
            Asset::SymbolsDwarf => Self::SymbolsDwarf,
        }
    }
}

impl From<PreviousOperation> for Operation {
    fn from(operation: PreviousOperation) -> Self {
        Self {
            asset: operation.asset.into(),
            location: operation.location,
            path: operation.path,
            destination: operation.destination,
            parent: operation.parent,
            previous: operation.previous,
            generated: operation.generated,
            superseded: operation.superseded,
            introduced_parents: operation.introduced_parents,
        }
    }
}

impl From<&Operation> for PreviousOperation {
    fn from(operation: &Operation) -> Self {
        Self {
            asset: operation.asset.into(),
            location: operation.location,
            path: operation.path.clone(),
            destination: operation.destination.clone(),
            parent: operation.parent.clone(),
            previous: operation.previous.clone(),
            generated: operation.generated.clone(),
            superseded: operation.superseded.clone(),
            introduced_parents: operation.introduced_parents.clone(),
        }
    }
}

impl From<PreviousPlan> for InstallPlan {
    fn from(plan: PreviousPlan) -> Self {
        Self {
            version: plan.version,
            repository: plan.repository,
            record: plan.record,
            operations: plan.operations.into_iter().map(Into::into).collect(),
            app: plan.app,
            app_predecessors: plan.app_predecessors,
            retirements: plan.retirements,
            rollback_archive: plan.rollback_archive,
            hook_activation: plan.hook_activation,
            symbols: plan.symbols,
        }
    }
}

impl From<&InstallPlan> for PreviousPlan {
    fn from(plan: &InstallPlan) -> Self {
        Self {
            version: plan.version,
            repository: plan.repository.clone(),
            record: plan.record.clone(),
            operations: plan.operations.iter().map(Into::into).collect(),
            app: plan.app.clone(),
            app_predecessors: plan.app_predecessors.clone(),
            retirements: plan.retirements.clone(),
            rollback_archive: plan.rollback_archive.clone(),
            hook_activation: plan.hook_activation.clone(),
            symbols: plan.symbols.clone(),
        }
    }
}

impl From<PreviousRecord> for InstallRecord {
    fn from(record: PreviousRecord) -> Self {
        Self {
            version: record.version,
            repository: record.repository,
            plan: record.plan.into(),
            plan_digest: record.plan_digest,
            reversals: record.reversals,
            destinations: record.destinations,
            status: record.status,
            completed: record.completed,
            installed: record.installed,
            installed_parents: record.installed_parents,
            restored: record.restored,
            retired_parents: record.retired_parents,
            restored_retirements: record.restored_retirements,
        }
    }
}

impl From<&InstallRecord> for PreviousRecord {
    fn from(record: &InstallRecord) -> Self {
        Self {
            version: record.version,
            repository: record.repository.clone(),
            plan: PreviousPlan::from(&record.plan),
            plan_digest: record.plan_digest.clone(),
            reversals: record.reversals.clone(),
            destinations: record.destinations.clone(),
            status: record.status,
            completed: record.completed,
            installed: record.installed.clone(),
            installed_parents: record.installed_parents.clone(),
            restored: record.restored.clone(),
            retired_parents: record.retired_parents.clone(),
            restored_retirements: record.restored_retirements.clone(),
        }
    }
}
