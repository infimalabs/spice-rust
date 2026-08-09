use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use wire::Digest;

use super::binding::bind_destination;
use super::files::{observe_bound, publish_bound};
use super::{
    FileImage, Generation, InstallError, InstallPlan, InstallReceipt, InstallStatus,
    LEGACY_RECORD_VERSIONS, RECORD_MODE, RECORD_NAME, RECORD_PATH, RECORD_VERSION, Repository,
    RepositoryIdentity,
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
        match self.status {
            InstallStatus::Uninstalled => self.plan.validate_historical_receipt()?,
            InstallStatus::Applying | InstallStatus::Complete | InstallStatus::Unapplying => {
                self.plan.validate()?;
            }
        }
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
        self.plan.validate_repository_binding(repository)
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
    let generation = observed.generation;
    let bytes = observed.image.bytes;
    let record: InstallRecord = wire::decode(&bytes).map_err(InstallError::Decode)?;
    if let Some((_, owner)) = LEGACY_RECORD_VERSIONS
        .iter()
        .find(|(version, _)| *version == record.version)
    {
        return Err(InstallError::LegacyRecord {
            path: record_path(repository)?,
            owner,
        });
    }
    record.validate_for_repository(repository)?;
    let canonical = wire::encode(&record).map_err(InstallError::Encode)?;
    if canonical != bytes {
        return Err(InstallError::InvalidRecord(
            "install record is not canonical".to_owned(),
        ));
    }
    Ok(Some(LoadedRecord { record, generation }))
}

pub(super) fn write_initial_record(
    repository: &Repository,
    record: &InstallRecord,
) -> Result<Generation, InstallError> {
    record.validate_for_repository(repository)?;
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
    expected: Generation,
) -> Result<Generation, InstallError> {
    record.validate_for_repository(repository)?;
    write_record_bytes(repository, record, Some(expected))
}

fn write_record_bytes(
    repository: &Repository,
    record: &InstallRecord,
    expected: Option<Generation>,
) -> Result<Generation, InstallError> {
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
