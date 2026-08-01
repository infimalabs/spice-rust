use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use wire::Digest;

use super::files::{create_directory, publish_image};
use super::{
    FileImage, Generation, InstallError, InstallPlan, InstallReceipt, InstallStatus,
    LEGACY_RECORDS, RECORD_MODE, RECORD_NAME, RECORD_VERSION, Repository, RepositoryIdentity,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InstallRecord {
    version: u16,
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
}

pub(super) struct LoadedRecord {
    pub(super) record: InstallRecord,
    pub(super) generation: Generation,
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
        })
    }

    pub(super) fn validate(&self) -> Result<(), InstallError> {
        if self.version != RECORD_VERSION {
            return Err(InstallError::InvalidRecord(format!(
                "install record version {} is not {RECORD_VERSION}",
                self.version
            )));
        }
        self.plan.validate()?;
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
        let slots = self.plan.operations.len();
        if self.installed.len() != slots
            || self.installed_parents.len() != slots
            || self.restored.len() != slots
        {
            return Err(InstallError::InvalidRecord(
                "install generation inventory has the wrong length".to_owned(),
            ));
        }
        if self.completed > slots {
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
        ) && self.completed != slots
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
        self.plan.validate_for_repository(repository)
    }

    pub(super) fn receipt(&self) -> InstallReceipt {
        InstallReceipt {
            plan: self.plan.clone(),
            digest: self.plan_digest.clone(),
            status: self.status,
            completed: self.completed,
        }
    }
}

pub(super) fn read_record(repository: &Repository) -> Result<Option<LoadedRecord>, InstallError> {
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
    record.validate_for_repository(repository)?;
    let canonical = wire::encode(&record).map_err(InstallError::Encode)?;
    if canonical != bytes {
        return Err(InstallError::InvalidRecord(
            "install record is not canonical".to_owned(),
        ));
    }
    Ok(Some(LoadedRecord {
        record,
        generation: Generation::observe(&metadata),
    }))
}

pub(super) fn write_initial_record(
    repository: &Repository,
    record: &InstallRecord,
) -> Result<Generation, InstallError> {
    record.validate_for_repository(repository)?;
    let logical = record_path(repository);
    match fs::symlink_metadata(&logical) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Ok(_) => return Err(InstallError::ParentChanged(logical)),
        Err(source) => {
            return Err(InstallError::io(
                "inspect absent install record",
                logical,
                source,
            ));
        }
    }
    for relative in &record.plan.record.introduced_parents {
        create_directory(repository.common_dir(), relative)?;
    }
    write_record_bytes(repository, record)
}

pub(super) fn write_record(
    repository: &Repository,
    record: &InstallRecord,
    expected: Generation,
) -> Result<Generation, InstallError> {
    record.validate_for_repository(repository)?;
    let path = record_path(repository);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|source| InstallError::io("inspect install record", &path, source))?;
    if !metadata.file_type().is_file() || Generation::observe(&metadata) != expected {
        return Err(InstallError::ParentChanged(path));
    }
    write_record_bytes(repository, record)
}

fn write_record_bytes(
    repository: &Repository,
    record: &InstallRecord,
) -> Result<Generation, InstallError> {
    let path = record_path(repository);
    let bytes = wire::encode(record).map_err(InstallError::Encode)?;
    publish_image(&path, &FileImage::new(RECORD_MODE, bytes), true)
}

pub(super) fn record_path(repository: &Repository) -> PathBuf {
    repository.state_root().join(RECORD_NAME)
}

fn refuse_legacy_record(repository: &Repository) -> Result<(), InstallError> {
    for (relative, owner) in LEGACY_RECORDS {
        let path = repository.common_dir().join(relative);
        match fs::symlink_metadata(&path) {
            Ok(_) => return Err(InstallError::LegacyRecord { path, owner }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(InstallError::io(
                    "inspect legacy install record",
                    path,
                    source,
                ));
            }
        }
    }
    Ok(())
}
