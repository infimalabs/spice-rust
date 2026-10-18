//! Windows boundary for Unix handle-bound install maintenance.

use super::InstallError;
use crate::repository::Repository;

/// Read-only evidence about crashed install-record publication files.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InstallTempInspection {
    runtime_present: bool,
}

impl InstallTempInspection {
    #[must_use]
    pub const fn runtime_present(&self) -> bool {
        self.runtime_present
    }

    #[must_use]
    pub const fn examined_entries(&self) -> usize {
        0
    }

    #[must_use]
    pub const fn omitted_entries(&self) -> usize {
        0
    }

    #[must_use]
    pub const fn omitted_candidates(&self) -> usize {
        0
    }

    #[must_use]
    pub const fn reclaimable_count(&self) -> usize {
        0
    }

    #[must_use]
    pub const fn reclaimable_bytes(&self) -> u64 {
        0
    }

    #[must_use]
    pub fn unsafe_candidates(&self) -> &[String] {
        &[]
    }

    #[must_use]
    pub const fn incomplete(&self) -> bool {
        false
    }
}

/// Refuse install maintenance where handle-bound admission is unavailable.
pub fn inspect_temporary_records(
    _repository: &Repository,
) -> Result<InstallTempInspection, InstallError> {
    Err(unsupported())
}

/// Refuse install maintenance where handle-bound admission is unavailable.
pub fn reclaim_temporary_records(
    _repository: &Repository,
) -> Result<InstallTempInspection, InstallError> {
    Err(unsupported())
}

fn unsupported() -> InstallError {
    InstallError::InvalidRecord(
        "handle-bound installation maintenance requires a Unix filesystem".to_owned(),
    )
}
