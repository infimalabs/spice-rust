//! Read-only journal-schema observation for installed-appliance orchestration.

use std::fmt;
use std::path::Path;

/// A finalized journal schema could not be inspected without mutation.
#[derive(Debug)]
pub struct InspectionError(journal::OpenError);

impl fmt::Display for InspectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for InspectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// Inspect one existing finalized journal without creating or migrating it.
///
/// # Errors
///
/// Returns the journal's exact read-only schema-admission failure.
pub fn inspect(path: impl AsRef<Path>) -> Result<u32, InspectionError> {
    journal::Journal::inspect_schema(path).map_err(InspectionError)
}
