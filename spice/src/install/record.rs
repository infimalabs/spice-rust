use std::path::PathBuf;

#[cfg(debug_assertions)]
use std::fs;

use serde::{Deserialize, Serialize};
use wire::Digest;

use super::binding::bind_destination;
use super::files::{observe_bound, publish_bound};
use super::{
    FileImage, Generation, InstallError, InstallPlan, InstallPlanSummary, InstallReceipt,
    InstallStatus, RECORD_MODE, RECORD_NAME, RECORD_PATH, RECORD_VERSION, ReceiptGeneration,
    ReceiptSupport, Repository, RepositoryIdentity, RollbackArchive,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InstallRecord {
    pub(super) version: u16,
    repository: RepositoryIdentity,
    pub(super) plan: InstallPlan,
    pub(super) plan_digest: Digest,
    reversals: Vec<Option<super::ObservedFile>>,
    destinations: Vec<(super::Location, String)>,
    pub(super) status: InstallStatus,
    pub(super) completed: usize,
    pub(super) installed: Vec<Option<Generation>>,
    pub(super) installed_parents: Vec<Option<Generation>>,
    pub(super) restored: Vec<Option<Generation>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) retired_parents: Vec<Option<Generation>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) restored_retirements: Vec<Option<Generation>>,
}

#[derive(Deserialize)]
struct InstallRecordHeader {
    version: u16,
}

pub(super) struct LoadedRecord {
    pub(super) record: InstallRecord,
    pub(super) generation: ReceiptGeneration,
    pub(super) image: Option<FileImage>,
}

pub(super) enum RefreshRecord {
    Current(LoadedRecord),
    Repairable {
        loaded: LoadedRecord,
        original_digest: Digest,
    },
}

impl RefreshRecord {
    pub(super) const fn is_repairable(&self) -> bool {
        matches!(self, Self::Repairable { .. })
    }

    pub(super) const fn loaded(&self) -> &LoadedRecord {
        match self {
            Self::Current(loaded) | Self::Repairable { loaded, .. } => loaded,
        }
    }

    pub(super) fn into_loaded(self) -> LoadedRecord {
        match self {
            Self::Current(loaded) | Self::Repairable { loaded, .. } => loaded,
        }
    }
}

impl InstallRecord {
    pub(super) fn new(plan: InstallPlan) -> Result<Self, InstallError> {
        let plan_digest = plan.digest()?;
        let repository = plan.repository.clone();
        let reversals = plan
            .operations
            .iter()
            .map(|operation| operation.previous.clone())
            .collect();
        let destinations = plan
            .operations
            .iter()
            .map(|operation| (operation.location, operation.destination.clone()))
            .collect();
        let slots = plan.operations.len();
        let retirements = plan.retirements.len();
        Ok(Self {
            version: RECORD_VERSION,
            repository,
            plan,
            plan_digest,
            reversals,
            destinations,
            status: InstallStatus::Applying,
            completed: 0,
            installed: vec![None; slots],
            installed_parents: vec![None; slots],
            restored: vec![None; slots],
            retired_parents: vec![None; retirements],
            restored_retirements: vec![None; retirements],
        })
    }

    /// Strip the version-15 symbol operations, provenance, and published files.
    #[cfg(debug_assertions)]
    pub(super) fn remove_test_symbol_assets(
        &mut self,
        repository: &Repository,
    ) -> Result<(), InstallError> {
        if self.plan.symbols.take().is_none() {
            return Ok(());
        }
        self.remove_test_operations(repository, &super::symbols::ASSETS, None)?;
        let bundle = repository.common_dir().join(super::symbols::SYMBOLS_PATH);
        fs::remove_dir_all(&bundle)
            .map_err(|source| InstallError::io("remove future symbol test bundle", bundle, source))
    }

    /// Remove `assets` from the plan, its bookkeeping, and the filesystem.
    #[cfg(debug_assertions)]
    fn remove_test_operations(
        &mut self,
        repository: &Repository,
        assets: &[super::Asset],
        app: Option<&Digest>,
    ) -> Result<(), InstallError> {
        let mut indices = assets
            .iter()
            .map(|asset| {
                self.plan
                    .operations
                    .iter()
                    .position(|operation| operation.asset == *asset)
                    .ok_or_else(|| {
                        InstallError::InvalidRecord(format!(
                            "test receipt has no {:?} asset",
                            asset
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        indices.sort_unstable_by(|left, right| right.cmp(left));
        if let Some(archive) = &self.plan.rollback_archive {
            let inline = archive.is_inline();
            let mut rollback = archive.decode(repository)?;
            for index in &indices {
                rollback.operations.remove(*index);
            }
            let rebuilt = super::RollbackArchive::new(&rollback)?;
            self.plan.rollback_archive = Some(if inline {
                rebuilt.to_inline(repository)?
            } else {
                rebuilt
            });
        }
        for index in indices {
            self.plan.operations.remove(index);
            self.reversals.remove(index);
            self.destinations.remove(index);
            self.installed.remove(index);
            self.installed_parents.remove(index);
            self.restored.remove(index);
        }
        self.completed = self.plan.operation_count();
        self.plan_digest = self.plan.digest()?;
        for asset in assets {
            let (_, relative, _) = super::asset_contract(*asset, app)?;
            let path = repository.common_dir().join(relative);
            fs::remove_file(&path)
                .map_err(|source| InstallError::io("remove future test asset", path, source))?;
        }
        Ok(())
    }

    /// Remove hook assets introduced after the version-7 receipt shape.
    #[cfg(debug_assertions)]
    pub(super) fn remove_test_newer_hook_assets(
        &mut self,
        repository: &Repository,
    ) -> Result<(), InstallError> {
        let mut assets = vec![
            super::Asset::HookZsh,
            super::Asset::HookBash,
            super::Asset::HookWrapperSpice,
        ];
        assets.extend(super::app::ASSETS.into_iter().map(|(asset, _)| asset));
        assets.push(super::Asset::CanvasCurrent);
        let app = self.plan.app.take();
        self.remove_test_operations(repository, &assets, app.as_ref())?;
        let app_root = repository.common_dir().join(".spice-install/app");
        fs::remove_dir_all(&app_root).map_err(|source| {
            InstallError::io("remove future Canvas test assets", app_root, source)
        })?;
        for relative in [
            ".spice-install/hooks/shell",
            ".spice-install/hooks/wrappers",
        ] {
            let path = repository.common_dir().join(relative);
            fs::remove_dir(&path).map_err(|source| {
                InstallError::io("remove future hook test directory", path, source)
            })?;
        }
        Ok(())
    }

    pub(super) fn validate(&self) -> Result<(), InstallError> {
        self.validate_with_content(self.status == InstallStatus::Uninstalled)
    }

    /// Validate state read from disk before deciding whether to refresh it.
    ///
    /// A complete appliance carrying a binary may be the intact output of a
    /// previous build. Its fixed digests are admitted as history only until
    /// progress verification proves every destination still matches. The
    /// immediately previous schema is also admitted long enough to add its new
    /// fixed asset, including for a binary-less install. Other binary-less
    /// records have no installed build identity to anchor that exception,
    /// while interrupted directions remain owned by the build that started
    /// them.
    fn validate_loaded(&self) -> Result<(), InstallError> {
        let historical = self.status == InstallStatus::Uninstalled
            || (self.status == InstallStatus::Complete
                && self.version == super::PREVIOUS_RECORD_VERSION)
            || (self.status == InstallStatus::Complete
                && super::binary_operation(&self.plan).is_some());
        self.validate_with_content(historical)
    }

    fn rebase_repository(&mut self, repository: &Repository) -> Result<(), InstallError> {
        if !matches!(
            self.status,
            InstallStatus::Complete | InstallStatus::Uninstalled
        ) {
            return Err(InstallError::InvalidRecord(
                "an interrupted install cannot be relocated".to_owned(),
            ));
        }
        let prior = self.repository.common_git.clone();
        if prior == repository.common_dir() {
            return Ok(());
        }
        if prior.exists() {
            return Err(InstallError::InvalidRecord(
                "install record was copied while its owning repository still exists".to_owned(),
            ));
        }
        self.repository = RepositoryIdentity::capture(repository);
        self.plan.repository = self.repository.clone();
        if let Some(activation) = &mut self.plan.hook_activation {
            activation.rebase_main_worktree(repository)?;
        }
        if let Some(archive) = &mut self.plan.rollback_archive {
            let mut rollback = archive.decode(repository)?;
            rollback.record = rebase_rollback_record(&rollback.record, repository)?;
            *archive = super::RollbackArchive::new(&rollback)?;
        }
        self.plan_digest = self.plan.digest()?;
        Ok(())
    }

    /// Rebind one internally coherent receipt after macOS renumbers its mount.
    ///
    /// The persisted device number is an immediate race witness, not a stable
    /// APFS volume identity. A reboot may change it while every inode and path
    /// remains intact. Only a single old device shared by every recorded parent
    /// is eligible; strict repository validation below still proves every inode
    /// before the repaired receipt can drive a refresh.
    fn repair_mount_device(
        &mut self,
        repository: &Repository,
    ) -> Result<Option<Digest>, InstallError> {
        #[cfg(not(target_vendor = "apple"))]
        {
            let _ = repository;
            Ok(None)
        }
        #[cfg(target_vendor = "apple")]
        {
            let metadata =
                std::fs::symlink_metadata(repository.common_dir()).map_err(|source| {
                    InstallError::io(
                        "inspect repository mount generation",
                        repository.common_dir(),
                        source,
                    )
                })?;
            let current = Generation::observe(&metadata).device;
            let prior = self.plan.record.parent.generation.device;
            if current == prior {
                return Ok(None);
            }
            let coherent = self
                .plan
                .operations
                .iter()
                .map(|operation| operation.parent.generation.device)
                .chain(
                    self.plan
                        .retirements
                        .iter()
                        .map(|retirement| retirement.parent.generation.device),
                )
                .chain(
                    self.installed_parents
                        .iter()
                        .flatten()
                        .map(|parent| parent.device),
                )
                .chain(
                    self.retired_parents
                        .iter()
                        .flatten()
                        .map(|parent| parent.device),
                )
                .all(|device| device == prior);
            if !coherent {
                return Ok(None);
            }

            let original_digest = self.plan_digest.clone();
            self.plan.record.parent.generation.device = current;
            for operation in &mut self.plan.operations {
                operation.parent.generation.device = current;
            }
            for retirement in &mut self.plan.retirements {
                retirement.parent.generation.device = current;
            }
            for parent in self.installed_parents.iter_mut().flatten() {
                parent.device = current;
            }
            for parent in self.retired_parents.iter_mut().flatten() {
                parent.device = current;
            }
            self.plan_digest = self.plan.digest()?;
            Ok(Some(original_digest))
        }
    }

    fn validate_with_content(&self, historical: bool) -> Result<(), InstallError> {
        if self.version != self.plan.version {
            return Err(InstallError::InvalidRecord(
                "install record and plan versions do not match".to_owned(),
            ));
        }
        let rollback_migratable = historical
            && self.version == super::MIGRATABLE_RECORD_VERSION
            && self.plan.version == super::MIGRATABLE_RECORD_VERSION;
        let forward_migratable = historical
            && self.version == super::FORWARD_MIGRATABLE_RECORD_VERSION
            && self.plan.version == super::FORWARD_MIGRATABLE_RECORD_VERSION
            && self.status == InstallStatus::Complete
            && super::binary_operation(&self.plan).is_some();
        let inline_migratable = historical
            && self.version == super::INLINE_ROLLBACK_RECORD_VERSION
            && self.plan.version == super::INLINE_ROLLBACK_RECORD_VERSION
            && self.status == InstallStatus::Complete
            && super::binary_operation(&self.plan).is_some();
        let previous_migratable = historical
            && self.version == super::PREVIOUS_RECORD_VERSION
            && self.plan.version == super::PREVIOUS_RECORD_VERSION
            && self.status == InstallStatus::Complete;
        let supported = matches!(
            super::receipt_support(self.version),
            Some(ReceiptSupport::Current | ReceiptSupport::Direct)
        );
        if !supported
            || (self.version != RECORD_VERSION
                && !rollback_migratable
                && !forward_migratable
                && !inline_migratable
                && !previous_migratable)
        {
            return Err(InstallError::InvalidRecord(format!(
                "install record version {} is not {RECORD_VERSION}",
                self.version
            )));
        }
        self.validate_plan_content(historical)?;
        self.validate_progress_content()
    }

    fn validate_plan_content(&self, historical: bool) -> Result<(), InstallError> {
        if historical {
            self.plan.validate_historical_receipt()?;
        } else {
            self.plan.validate()?;
        }
        self.validate_plan_closure()
    }

    fn validate_plan_closure(&self) -> Result<(), InstallError> {
        if self.repository != self.plan.repository {
            return Err(InstallError::InvalidRecord(
                "install record repository does not close over its plan".to_owned(),
            ));
        }
        if self.plan_digest != self.plan.digest()? {
            return Err(InstallError::InvalidRecord(
                "install record plan digest does not match its canonical plan".to_owned(),
            ));
        }
        let reversals = self
            .plan
            .operations
            .iter()
            .map(|operation| operation.previous.clone())
            .collect::<Vec<_>>();
        if self.reversals != reversals {
            return Err(InstallError::InvalidRecord(
                "install reversal images do not close over the plan".to_owned(),
            ));
        }
        let destinations = self
            .plan
            .operations
            .iter()
            .map(|operation| (operation.location, operation.destination.clone()))
            .collect::<Vec<_>>();
        if self.destinations != destinations {
            return Err(InstallError::InvalidRecord(
                "install destination inventory is not exact and ordered".to_owned(),
            ));
        }
        Ok(())
    }

    /// Reconstruct only the exact witness-free v8 refresh shape emitted before
    /// retirement archives became witness-bound. The original authored digest
    /// remains the audit identity; the reconstructed plan must be previewed and
    /// separately authorized before any durable receipt is replaced.
    fn repair_missing_retirement_witnesses(
        &mut self,
        repository: &Repository,
    ) -> Result<Digest, InstallError> {
        let original_digest = self.plan_digest.clone();
        if self.version != super::FORWARD_MIGRATABLE_RECORD_VERSION
            || self.plan.version != super::FORWARD_MIGRATABLE_RECORD_VERSION
            || self.status != InstallStatus::Complete
            || super::binary_operation(&self.plan).is_none()
            || self.plan.rollback_archive.is_none()
            || self.plan.retirements.is_empty()
        {
            return Err(retirement_witness_mismatch());
        }
        self.validate_plan_closure()?;
        self.validate_progress_content()?;
        if self.plan.retirements.iter().any(|retirement| {
            retirement.previous.is_some()
                || retirement.observed.is_some()
                || retirement.superseded.is_some()
                || retirement.rollback.is_some()
        }) {
            return Err(retirement_witness_mismatch());
        }
        let rollback = self
            .plan
            .rollback_state(repository)?
            .ok_or_else(retirement_witness_mismatch)?;
        if rollback.retirements.len() != self.plan.retirements.len() {
            return Err(InstallError::InvalidRecord(
                "migration rollback archive has the wrong image inventory length".to_owned(),
            ));
        }
        for (retirement, image) in self.plan.retirements.iter_mut().zip(&rollback.retirements) {
            retirement.rollback = image.as_ref().map(FileImage::generated);
        }
        self.plan_digest = self.plan.digest()?;
        self.validate_loaded()?;
        Ok(original_digest)
    }

    fn validate_progress_content(&self) -> Result<(), InstallError> {
        let slots = self.plan.operations.len();
        if self.installed.len() != slots
            || self.installed_parents.len() != slots
            || self.restored.len() != slots
        {
            return Err(InstallError::InvalidRecord(
                "install generation inventory has the wrong length".to_owned(),
            ));
        }
        let retirements = self.plan.retirements.len();
        if self.retired_parents.len() != retirements
            || self.restored_retirements.len() != retirements
        {
            return Err(InstallError::InvalidRecord(
                "install retirement progress has the wrong length".to_owned(),
            ));
        }
        let operation_count = self.plan.operation_count();
        if self.completed > operation_count {
            return Err(InstallError::InvalidRecord(
                "install progress is past the operation inventory".to_owned(),
            ));
        }
        if self
            .installed
            .iter()
            .zip(&self.installed_parents)
            .any(|(file, parent)| file.is_some() != parent.is_some())
        {
            return Err(InstallError::InvalidRecord(
                "installed file and parent generations are not paired".to_owned(),
            ));
        }
        if matches!(
            self.status,
            InstallStatus::Complete | InstallStatus::Uninstalled
        ) && self.completed != operation_count
        {
            return Err(InstallError::InvalidRecord(
                "terminal install record has incomplete progress".to_owned(),
            ));
        }
        if self.status == InstallStatus::Applying && self.restored.iter().any(Option::is_some) {
            return Err(InstallError::InvalidRecord(
                "applying record carries reversal generations".to_owned(),
            ));
        }
        if self.status == InstallStatus::Applying
            && self.restored_retirements.iter().any(Option::is_some)
        {
            return Err(InstallError::InvalidRecord(
                "applying record carries retirement reversals".to_owned(),
            ));
        }
        if matches!(
            self.status,
            InstallStatus::Complete | InstallStatus::Unapplying
        ) && (self.installed.iter().any(Option::is_none)
            || self.installed_parents.iter().any(Option::is_none))
        {
            return Err(InstallError::InvalidRecord(
                "installed record omits a destination generation".to_owned(),
            ));
        }
        Ok(())
    }

    pub(super) fn validate_for_repository(
        &self,
        repository: &Repository,
    ) -> Result<(), InstallError> {
        self.validate()?;
        self.repository.validate(repository)?;
        self.plan.validate_repository_binding(repository)
    }

    /// Validate only mutable progress and repository bindings before a trusted
    /// in-process checkpoint. The immutable plan was deeply validated when it
    /// was built or loaded; re-inflating a large rollback archive for every
    /// counter write adds no new evidence.
    fn validate_for_write(&self, repository: &Repository) -> Result<(), InstallError> {
        self.validate_progress_content()?;
        if self.repository != self.plan.repository {
            return Err(InstallError::InvalidRecord(
                "install record repository does not close over its plan".to_owned(),
            ));
        }
        self.repository.validate(repository)?;
        self.plan.validate_repository_binding(repository)
    }

    pub(super) fn validate_loaded_for_repository(
        &self,
        repository: &Repository,
    ) -> Result<(), InstallError> {
        self.validate_loaded()?;
        self.repository.validate(repository)?;
        self.plan.validate_repository_binding(repository)
    }

    pub(super) fn receipt(&self) -> InstallReceipt {
        InstallReceipt {
            plan: InstallPlanSummary::from_plan(&self.plan),
            digest: self.plan_digest.clone(),
            status: self.status,
            completed: self.completed,
        }
    }
}

pub(super) fn read_record(repository: &Repository) -> Result<Option<LoadedRecord>, InstallError> {
    let Some(mut loaded) = decode_record(repository)? else {
        return Ok(None);
    };
    let _ = finish_loaded_record(repository, &mut loaded)?;
    Ok(Some(loaded))
}

pub(super) fn read_record_for_refresh(
    repository: &Repository,
) -> Result<Option<RefreshRecord>, InstallError> {
    let Some(mut loaded) = decode_record(repository)? else {
        return Ok(None);
    };
    match finish_loaded_record(repository, &mut loaded) {
        Ok(Some(original_digest)) => Ok(Some(RefreshRecord::Repairable {
            loaded,
            original_digest,
        })),
        Ok(None) => Ok(Some(RefreshRecord::Current(loaded))),
        Err(error) if is_retirement_witness_mismatch(&error) => {
            let original_digest = loaded
                .record
                .repair_missing_retirement_witnesses(repository)?;
            let _ = finish_loaded_record(repository, &mut loaded)?;
            Ok(Some(RefreshRecord::Repairable {
                loaded,
                original_digest,
            }))
        }
        Err(error) => Err(error),
    }
}

pub(super) fn stored_rollback_digest(
    repository: &Repository,
) -> Result<Option<Digest>, InstallError> {
    let Some(loaded) = decode_record(repository)? else {
        return Ok(None);
    };
    let record = loaded.record;
    record.validate_loaded()?;
    if record.repository.common_git == repository.common_dir() {
        record.repository.validate(repository)?;
        record.plan.validate_repository_binding(repository)?;
    } else {
        if !matches!(
            record.status,
            InstallStatus::Complete | InstallStatus::Uninstalled
        ) {
            return Err(InstallError::InvalidRecord(
                "an interrupted install cannot be relocated".to_owned(),
            ));
        }
        if record.repository.common_git.exists() {
            return Err(InstallError::InvalidRecord(
                "install record was copied while its owning repository still exists".to_owned(),
            ));
        }
    }
    record.plan.validate_rollback_storage(repository)?;
    Ok(record
        .plan
        .rollback_archive
        .as_ref()
        .and_then(RollbackArchive::stored_digest)
        .cloned())
}

fn decode_record(repository: &Repository) -> Result<Option<LoadedRecord>, InstallError> {
    record_path(repository)?;
    let binding = bind_destination(repository.common_dir(), RECORD_PATH)?;
    let (observed, _) = observe_bound(
        repository.common_dir(),
        &binding.destination,
        &binding.parent,
    )?;
    let Some(observed) = observed else {
        return Ok(None);
    };
    let generation = ReceiptGeneration::observed(&observed);
    let image = observed.image;
    let header: InstallRecordHeader = wire::decode(&image.bytes).map_err(InstallError::Decode)?;
    if let Some(ReceiptSupport::Retired(owner)) = super::receipt_support(header.version) {
        return Err(InstallError::LegacyRecord {
            path: record_path(repository)?,
            owner,
        });
    }
    let record: InstallRecord = wire::decode(&image.bytes).map_err(InstallError::Decode)?;
    let canonical = wire::encode(&record).map_err(InstallError::Encode)?;
    if canonical != image.bytes {
        return Err(InstallError::InvalidRecord(
            "install record is not canonical".to_owned(),
        ));
    }
    Ok(Some(LoadedRecord {
        record,
        generation,
        image: Some(image),
    }))
}

fn finish_loaded_record(
    repository: &Repository,
    loaded: &mut LoadedRecord,
) -> Result<Option<Digest>, InstallError> {
    loaded.record.validate_loaded()?;
    let record = &mut loaded.record;
    if record.repository.common_git != repository.common_dir() {
        record.rebase_repository(repository)?;
    }
    let original_digest = record.repair_mount_device(repository)?;
    record.validate_loaded_for_repository(repository)?;
    record.plan.validate_rollback_storage(repository)?;
    if record.version != super::MIGRATABLE_RECORD_VERSION {
        loaded.image = None;
    }
    Ok(original_digest)
}

fn is_retirement_witness_mismatch(error: &InstallError) -> bool {
    matches!(error, InstallError::InvalidRecord(detail)
        if detail == super::RETIREMENT_WITNESS_MISMATCH)
}

fn retirement_witness_mismatch() -> InstallError {
    InstallError::InvalidRecord(super::RETIREMENT_WITNESS_MISMATCH.to_owned())
}

pub(super) fn decode_rollback_record(
    image: &FileImage,
    repository: &RepositoryIdentity,
) -> Result<InstallRecord, InstallError> {
    if image.mode != RECORD_MODE || !image.validate() {
        return Err(InstallError::InvalidRecord(
            "migration rollback receipt has an invalid image".to_owned(),
        ));
    }
    let record: InstallRecord = wire::decode(&image.bytes).map_err(InstallError::Decode)?;
    if record.version != super::MIGRATABLE_RECORD_VERSION
        || record.plan.version != super::MIGRATABLE_RECORD_VERSION
        || record.repository != *repository
        || record.status != InstallStatus::Complete
        || record.plan.rollback_archive.is_some()
    {
        return Err(InstallError::InvalidRecord(
            "migration rollback receipt is not one complete version-7 installation".to_owned(),
        ));
    }
    record.validate_with_content(true)?;
    let canonical = wire::encode(&record).map_err(InstallError::Encode)?;
    if canonical != image.bytes {
        return Err(InstallError::InvalidRecord(
            "migration rollback receipt is not canonical".to_owned(),
        ));
    }
    Ok(record)
}

pub(super) fn rebase_rollback_record(
    image: &FileImage,
    repository: &Repository,
) -> Result<FileImage, InstallError> {
    let mut record: InstallRecord = wire::decode(&image.bytes).map_err(InstallError::Decode)?;
    record.rebase_repository(repository)?;
    let bytes = wire::encode(&record).map_err(InstallError::Encode)?;
    Ok(FileImage::new(RECORD_MODE, bytes))
}

pub(super) fn write_initial_record(
    repository: &Repository,
    record: &InstallRecord,
) -> Result<ReceiptGeneration, InstallError> {
    record.validate_for_write(repository)?;
    let (observed, _) = observe_bound(
        repository.common_dir(),
        &record.plan.record.destination,
        &record.plan.record.parent,
    )?;
    if observed.is_some() {
        return Err(InstallError::ParentChanged(record_path(repository)?));
    }
    write_record_bytes(repository, record, None)
}

pub(super) fn write_record(
    repository: &Repository,
    record: &InstallRecord,
    expected: ReceiptGeneration,
) -> Result<ReceiptGeneration, InstallError> {
    record.validate_for_write(repository)?;
    write_record_bytes(repository, record, Some(expected))
}

fn write_record_bytes(
    repository: &Repository,
    record: &InstallRecord,
    expected: Option<ReceiptGeneration>,
) -> Result<ReceiptGeneration, InstallError> {
    let bytes = wire::encode(record).map_err(InstallError::Encode)?;
    publish_bound(
        repository.common_dir(),
        &record.plan.record.destination,
        &record.plan.record.parent,
        record.plan.record.introduced_parents.len(),
        &FileImage::new(RECORD_MODE, bytes),
        expected,
    )
}

pub(super) fn record_path(repository: &Repository) -> Result<PathBuf, InstallError> {
    Ok(repository
        .admit_state()
        .map_err(InstallError::State)?
        .runtime()
        .join(RECORD_NAME))
}

#[cfg(debug_assertions)]
pub(super) fn publish_test_historical_receipt(
    repository: &Repository,
    version: u16,
) -> Result<(), InstallError> {
    let mut record = read_record(repository)?
        .ok_or_else(|| InstallError::InvalidRecord("test fixture receipt is absent".to_owned()))?
        .record;
    if record.status != InstallStatus::Complete {
        return Err(InstallError::InvalidRecord(
            "test fixture receipt is not complete".to_owned(),
        ));
    }
    record.remove_test_symbol_assets(repository)?;
    match version {
        super::MIGRATABLE_RECORD_VERSION
        | super::FORWARD_MIGRATABLE_RECORD_VERSION
        | super::INLINE_ROLLBACK_RECORD_VERSION => {
            record.remove_test_newer_hook_assets(repository)?;
            if version == super::MIGRATABLE_RECORD_VERSION {
                record.plan.retirements.clear();
                record.plan.rollback_archive = None;
                record.retired_parents.clear();
                record.restored_retirements.clear();
            } else {
                let index = record
                    .plan
                    .retirements
                    .iter()
                    .position(|retirement| retirement.path == super::RETIRED_FOLD_STORE_PATH)
                    .ok_or_else(|| {
                        InstallError::InvalidRecord(
                            "test receipt has no fold-store retirement".to_owned(),
                        )
                    })?;
                record.plan.retirements.remove(index);
                record.retired_parents.remove(index);
                record.restored_retirements.remove(index);
                if let Some(archive) = &record.plan.rollback_archive {
                    record.plan.rollback_archive = Some(archive.to_inline(repository)?);
                }
            }
        }
        super::PREVIOUS_RECORD_VERSION => {}
        _ => {
            return Err(InstallError::InvalidRecord(format!(
                "test receipt schema {version} is not directly upgradable"
            )));
        }
    }
    record.version = version;
    record.plan.version = version;
    record.completed = record.plan.operation_count();
    record.plan_digest = record.plan.digest()?;
    record.validate_with_content(true)?;
    record.plan.publish_staged_rollback(repository)?;
    let bytes = wire::encode(&record).map_err(InstallError::Encode)?;
    let path = record_path(repository)?;
    fs::write(&path, bytes)
        .map_err(|source| InstallError::io("publish historical test receipt", path, source))
}
