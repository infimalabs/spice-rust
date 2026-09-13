//! The sole SQLite authority store.

/// The one journal schema this build reads and writes.
///
/// An authority written at an older version is stepped forward to this one on
/// open when `migrate::MIGRATIONS` carries the step, and refused by name when it
/// does not. A newer version is always refused: this build cannot know what the
/// release that wrote it added.
pub const SCHEMA_VERSION: u32 = 25;

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
    BackupVerifyError, MAX_BACKUP_BYTES, RestoreCapture, RestoreMember, capture_restore_bundle,
    verify_restore_bundle,
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
use std::fs::{self, File, OpenOptions};
#[cfg(not(target_arch = "wasm32"))]
use std::io::{self, Seek, Write};
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};

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

/// One consistent SQLite generation held in a private regular file.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct JournalSnapshot {
    path: PathBuf,
    file: File,
    bytes: u64,
    digest: wire::Digest,
}

#[cfg(not(target_arch = "wasm32"))]
struct SnapshotCleanup(Option<PathBuf>);

#[cfg(not(target_arch = "wasm32"))]
impl Drop for SnapshotCleanup {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_file(path);
        }
    }
}

/// Why a file-backed SQLite snapshot could not be captured.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub enum SnapshotFileError {
    Io { path: PathBuf, source: io::Error },
    Sqlite(rusqlite::Error),
}

#[cfg(not(target_arch = "wasm32"))]
impl std::fmt::Display for SnapshotFileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(
                    formatter,
                    "journal snapshot failed at {}: {source}",
                    path.display()
                )
            }
            Self::Sqlite(error) => write!(formatter, "journal snapshot failed: {error}"),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl std::error::Error for SnapshotFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Sqlite(error) => Some(error),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl JournalSnapshot {
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }

    #[must_use]
    pub const fn digest(&self) -> &wire::Digest {
        &self.digest
    }

    pub fn copy_to(&mut self, target: &mut impl Write) -> io::Result<u64> {
        self.file.rewind()?;
        io::copy(&mut self.file, target)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for JournalSnapshot {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn create_snapshot(path: &Path) -> Result<File, SnapshotFileError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(|source| SnapshotFileError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn finish_snapshot(
    mut file: File,
    path: &Path,
) -> Result<(File, u64, wire::Digest), SnapshotFileError> {
    file.sync_all().map_err(|source| SnapshotFileError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let limit = file
        .metadata()
        .map_err(|source| SnapshotFileError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    let (bytes, digest) =
        wire::Digest::compute_reader(wire::Domain::JournalBackupMember, &mut file, limit).map_err(
            |source| SnapshotFileError::Io {
                path: path.to_path_buf(),
                source,
            },
        )?;
    Ok((file, bytes, digest))
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

    /// Captures one consistent SQLite generation into a new private file.
    pub fn snapshot_file(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<JournalSnapshot, SnapshotFileError> {
        let path = path.as_ref();
        let mut cleanup = SnapshotCleanup(None);
        let file = create_snapshot(path)?;
        cleanup.0 = Some(path.to_path_buf());
        self.connection
            .backup(rusqlite::MAIN_DB, path, None)
            .map_err(SnapshotFileError::Sqlite)?;
        let (file, bytes, digest) = finish_snapshot(file, path)?;
        cleanup.0.take();
        Ok(JournalSnapshot {
            path: path.to_path_buf(),
            file,
            bytes,
            digest,
        })
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
