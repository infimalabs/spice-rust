//! The sole SQLite authority store.

/// The one journal schema this build reads and writes.
pub const SCHEMA_VERSION: u32 = 1;

#[cfg(not(target_arch = "wasm32"))]
mod guard;
#[cfg(not(target_arch = "wasm32"))]
mod migrate;
#[cfg(not(target_arch = "wasm32"))]
mod read;
#[cfg(not(target_arch = "wasm32"))]
mod replay;

#[cfg(not(target_arch = "wasm32"))]
pub use guard::{
    Applied, BuildError, CommitError, CommitFault, FactCause, GuardError, GuardRefusal,
    JournalGuard, JournalHead, JournalRead, JournalWrite, PendingFact,
};
#[cfg(not(target_arch = "wasm32"))]
pub use migrate::{MigrationError, OpenError};
#[cfg(not(target_arch = "wasm32"))]
pub use read::{FactSnapshot, SnapshotError};
#[cfg(not(target_arch = "wasm32"))]
pub use replay::{Verification, VerifyError, Violation, ViolationClass};

/// Current crossing types used to prepare journal work without depending on
/// the journal's storage implementation.
pub mod crossing {
    pub use wire::body;
    pub use wire::intent;
    pub use wire::{
        Actor, CanonicalUuid, ClaimHead, Command, CommandKind, Digest, Fact, FactKind, FindingHead,
        Instant, LaneResidency, LeaseHead, Origin, Phase, Producer, ResidencyHead, ResidentState,
        Seq, TaskHandle, TaskHead, TrustHead, WireVersion, Witness, WitnessHead, WorkflowHead,
    };
}

#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;

#[cfg(not(target_arch = "wasm32"))]
use rusqlite::Connection;

/// One open handle to the authority database.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct Journal {
    connection: Connection,
}

#[cfg(not(target_arch = "wasm32"))]
impl Journal {
    /// Opens or creates an authority database and admits only schema v1.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, OpenError> {
        Self::from_connection(Connection::open(path).map_err(OpenError::Sqlite)?)
    }

    /// Opens an isolated in-memory authority database.
    pub fn in_memory() -> Result<Self, OpenError> {
        Self::from_connection(Connection::open_in_memory().map_err(OpenError::Sqlite)?)
    }

    /// Returns the finalized schema version.
    pub fn schema_version(&self) -> Result<u32, OpenError> {
        migrate::finalized_version(&self.connection)
    }

    fn from_connection(mut connection: Connection) -> Result<Self, OpenError> {
        connection
            .execute_batch("PRAGMA foreign_keys = ON; PRAGMA trusted_schema = OFF;")
            .map_err(OpenError::Sqlite)?;
        migrate::baseline(&mut connection)?;
        Ok(Self { connection })
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
