use serde::{Deserialize, Serialize};
use wire::{Digest, Domain};

use super::super::{
    Asset, BoundDestination, GeneratedImage, Generation, HookActivationPlan, InstallError,
    InstallPlan, InstallStatus, Location, ObservedFile, Operation, ParentObservation,
    RepositoryIdentity, Retirement, RollbackArchive, SymbolProvenance,
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
