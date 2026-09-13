//! The sole SQLite authority store.

/// The one journal schema this build reads and writes.
///
/// An authority written at an older version is stepped forward to this one on
/// open when `migrate::MIGRATIONS` carries the step, and refused by name when it
/// does not. A newer version is always refused: this build cannot know what the
/// release that wrote it added.
pub const SCHEMA_VERSION: u32 = 24;

#[cfg(not(target_arch = "wasm32"))]
mod guard;
#[cfg(not(target_arch = "wasm32"))]
mod migrate;
#[cfg(not(target_arch = "wasm32"))]
mod read;
#[cfg(not(target_arch = "wasm32"))]
mod replay;
#[cfg(not(target_arch = "wasm32"))]
mod restore;

#[cfg(not(target_arch = "wasm32"))]
pub use guard::{
    Applied, BuildError, CommandReplay, CommitError, CommitFault, CurrentHeadWitness, FactCause,
    FactPageStart, FactReadBudget, GuardError, GuardRefusal, JournalGuard, JournalHead,
    JournalRead, JournalWrite, MAX_DURABLE_FACT_ENVELOPE_BYTES, MAX_RETAINED_FACT_PAGE_BYTES,
    MAX_RETAINED_FACT_PAGE_ROWS, MAX_STEERING_PAGE_ROWS, PendingFact, ReadError, SelectedFactPage,
    SteeringPage, TaskHeadPage, TaskHeads, TrustHeadPage,
};
#[cfg(not(target_arch = "wasm32"))]
pub use migrate::{Migration, MigrationError, OpenError};
#[cfg(not(target_arch = "wasm32"))]
pub use read::{FactSnapshot, SnapshotError, SteeringSnapshot};
#[cfg(not(target_arch = "wasm32"))]
pub use replay::{
    ReindexError, ReindexReport, ThreadError, ThreadSnapshot, Verification, VerifyError, Violation,
    ViolationClass,
};
#[cfg(not(target_arch = "wasm32"))]
pub use restore::{
    BackupVerifyError, RestoreCapture, RestoreMember, capture_restore_bundle, verify_restore_bundle,
};

/// Current crossing types used to prepare journal work without depending on
/// the journal's storage implementation.
pub mod crossing {
    pub use wire::body;
    pub use wire::intent;
    pub use wire::{
        Actor, AnyFact, CanonicalUuid, ClaimHead, Command, CommandKind, Digest, Envelope,
        Environment, EnvironmentRequirement, EnvironmentRequirements, Fact, FactKind, FindingHead,
        Instant, JournalCursor, LaneResidency, LaneSession, LeaseHead, Origin, Packed, Phase,
        Producer, ResidencyHead, ResidentState, Seq, SteeringHead, TapeLocus, TaskHandle, TaskHead,
        TaskStamp, TranscriptSourceHead, TranscriptStreamKey, TrustHead, TrustState, WireVersion,
        Witness, WitnessHead, WorkflowHead,
    };
}

#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;

#[cfg(not(target_arch = "wasm32"))]
use rusqlite::{Connection, OpenFlags};

/// One open handle to the authority database.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct Journal {
    connection: Connection,
    migration: Option<Migration>,
}

/// A capability-distinct, read-only view of finalized authority.
///
/// Opening this handle never creates, migrates, or repairs a database. SQLite
/// admits the file read-only, and `query_only` independently refuses writes on
/// the connection, so callers that only need schema and cursor identity cannot
/// accidentally cross the resident's sole-writer boundary.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct JournalReader {
    connection: Connection,
}

#[cfg(not(target_arch = "wasm32"))]
impl Journal {
    /// Opens or creates an authority database and admits the current schema.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, OpenError> {
        let path = path.as_ref();
        Self::from_connection(Connection::open(path).map_err(|source| OpenError::Open {
            path: path.to_path_buf(),
            source,
        })?)
    }

    /// Opens authority for the explicit offline journal maintenance operation.
    ///
    /// This is the sole unbounded schema certification and index activation
    /// path. Its caller must hold the repository's offline journal lock before
    /// opening.
    pub fn open_for_reindex(path: impl AsRef<Path>) -> Result<Self, OpenError> {
        let path = path.as_ref();
        let mut connection = Connection::open(path).map_err(|source| OpenError::Open {
            path: path.to_path_buf(),
            source,
        })?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON; PRAGMA trusted_schema = OFF;")
            .map_err(OpenError::Sqlite)?;
        let migration = migrate::baseline_for_reindex(&mut connection)?;
        Ok(Self {
            connection,
            migration,
        })
    }

    /// Opens an isolated in-memory authority database.
    pub fn in_memory() -> Result<Self, OpenError> {
        Self::from_connection(Connection::open_in_memory().map_err(OpenError::Sqlite)?)
    }

    /// Returns the finalized schema version.
    pub fn schema_version(&self) -> Result<u32, OpenError> {
        migrate::finalized_version(&self.connection)
    }

    /// Schema transition this open completed, absent when authority was current.
    #[must_use]
    pub const fn migration(&self) -> Option<Migration> {
        self.migration
    }

    /// Inspects one existing authority database without creating or migrating it.
    ///
    /// This is the daemonless read used by identity-reporting surfaces. The
    /// read-only SQLite handle makes absence and write attempts refusals at the
    /// storage boundary; unlike [`Self::open`], this path never baselines an
    /// empty file and never enters a migration transaction.
    pub fn inspect_schema(path: impl AsRef<Path>) -> Result<u32, OpenError> {
        JournalReader::open(path)?.schema_version()
    }

    /// Observes an existing finalized schema identity without admitting its version.
    ///
    /// This is reserved for upgrade orchestration that must compare a journal's
    /// before and after versions. It never creates, migrates, or grants authority
    /// reads to a version this build does not otherwise admit.
    pub fn inspect_stored_schema(path: impl AsRef<Path>) -> Result<u32, OpenError> {
        let path = path.as_ref();
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|source| OpenError::Open {
                path: path.to_path_buf(),
                source,
            })?;
        connection
            .pragma_update(None, "query_only", true)
            .map_err(OpenError::Sqlite)?;
        connection
            .pragma_update(None, "trusted_schema", false)
            .map_err(OpenError::Sqlite)?;
        migrate::inspect_stored_version(&connection)
    }

    fn from_connection(mut connection: Connection) -> Result<Self, OpenError> {
        connection
            .execute_batch("PRAGMA foreign_keys = ON; PRAGMA trusted_schema = OFF;")
            .map_err(OpenError::Sqlite)?;
        let migration = migrate::baseline(&mut connection)?;
        Ok(Self {
            connection,
            migration,
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl JournalReader {
    /// Opens an existing, finalized authority without mutating it.
    ///
    /// # Errors
    ///
    /// Returns the SQLite open failure when the file is absent or unreadable,
    /// or the existing schema-admission refusal when migration is unfinished
    /// or belongs to another release.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, OpenError> {
        let path = path.as_ref();
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|source| OpenError::Open {
                path: path.to_path_buf(),
                source,
            })?;
        Self::from_connection(connection)
    }

    /// Admits one exact captured SQLite generation without reopening its path.
    pub(crate) fn from_serialized(bytes: &[u8]) -> Result<Self, OpenError> {
        let mut connection = Connection::open_in_memory().map_err(OpenError::Sqlite)?;
        connection
            .deserialize_read_exact(
                rusqlite::MAIN_DB,
                std::io::Cursor::new(bytes),
                bytes.len(),
                true,
            )
            .map_err(OpenError::Sqlite)?;
        Self::from_connection(connection)
    }

    fn from_connection(connection: Connection) -> Result<Self, OpenError> {
        connection
            .pragma_update(None, "query_only", true)
            .map_err(OpenError::Sqlite)?;
        connection
            .pragma_update(None, "trusted_schema", false)
            .map_err(OpenError::Sqlite)?;
        migrate::inspect_version(&connection)?;
        Ok(Self { connection })
    }

    /// Returns the one finalized schema version this handle admitted.
    ///
    /// # Errors
    ///
    /// Returns a schema-admission failure if the authority changes into an
    /// unfinalized or unsupported state while the handle is open.
    pub fn schema_version(&self) -> Result<u32, OpenError> {
        migrate::finalized_version(&self.connection)
    }

    /// Returns the authority's current cursor, absent before its first fact.
    ///
    /// # Errors
    ///
    /// Returns the SQLite read failure or names an invalid stored cursor.
    pub fn cursor(&self) -> Result<Option<wire::Seq>, OpenError> {
        let value = self
            .connection
            .query_row(
                "SELECT seq FROM cursor_head WHERE singleton = 1",
                [],
                |row| row.get::<_, Option<i64>>(0),
            )
            .map_err(OpenError::Sqlite)?;
        match value {
            None => Ok(None),
            Some(found) => u64::try_from(found)
                .ok()
                .and_then(wire::Seq::new)
                .map(Some)
                .ok_or(OpenError::InvalidCursor { found }),
        }
    }

    /// Captures one consistent SQLite generation, including committed pager state.
    pub fn snapshot_bytes(&self) -> Result<Vec<u8>, OpenError> {
        self.connection
            .serialize(rusqlite::MAIN_DB)
            .map(|bytes| bytes.to_vec())
            .map_err(OpenError::Sqlite)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
