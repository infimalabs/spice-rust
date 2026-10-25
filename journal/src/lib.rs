//! The sole SQLite authority store.

/// The exact journal schema this build reads and writes.
///
/// Existing authority must already be current, or one of the admitted
/// predecessors, which writable open crosses exactly once into the current
/// shape. Retired schemas refuse before persistent storage configuration and
/// name the release that last migrated them; newer schemas refuse because this
/// build cannot know what their owning release added. Schema 61 admits the
/// `human:<principal>` actor spelling and principal capability digests, which
/// an older release cannot read. Schema 62 bound every retained Design notice
/// to its statement and selection provenance; schema 63 retires that
/// provenance while preserving exact statement evidence and adds the bounded
/// per-Design maturity and notice projections. Schema 64 crosses retained
/// settings answers from their single operator identity to a principal
/// directory; schema 65 gives each of that directory's entries its principal's
/// code. Schema 66 makes the activity stance explicit in retained working-group
/// facts and their partition head before strict fact readers admit them.
/// Schema 67 lifts task priority and taste-review standing out of statement
/// tags into typed statement fields, which an older release would drop.
/// Schema 68 closes task statement provenance and resolves every retained
/// spelling into it.
/// Schema 69 lifts document slugs and taste-proposal evidence out of statement
/// tags into typed statement fields, which an older release would drop. Schema
/// 70 removes the unused generic statement-tag field and its retired metadata.
/// Schema 71 rebuilds Design maturity so it counts asked lanes that declined
/// by leaving the room, which an older release's projection cannot decode.
/// Schema 72 crosses retained terminal review fields into the typed workflow
/// state without changing their positive or negative standing.
/// Schema 73 makes each retained Design pass proposal's justification explicit.
/// Schema 74 lifts each taste hold's evidence out of its statement's audit
/// notes into the typed hold. Schema 75 crosses retained OOPS filing rows into
/// ordinary Plans controlled by one repository-owned opportunistic Design.
/// Schema 76 indexes each retained task note that is not a taste-review audit
/// into its task's room, which an older release's room reads cannot present.
/// Schema 77 gives the repository-owned OOPS Design generation one typed
/// singleton authority. Schema 78 replaces derived preparation authorization
/// with ordinary dependency edges and closes retained work whose controller
/// can no longer release it. Schema 79 records every distinct Design decision
/// that authorizes the same output. Schema 80 lets a task completion carry a
/// Plan's no-construction disposition, which an older release would drop.
/// Schema 81 lets a lane's room catch-up cursor pass a delivered Design request.
/// Schema 82 targets Design invitations at continuing vendor sessions, separates
/// their triggering opportunity from retained evidence, and preserves an exact
/// task-claim witness when a predecessor recorded no successful completion.
/// Schema 83 binds each invitation to its exact compaction-delimited context
/// generation. Schema 84 records each invitation's supporting-work
/// classification and bounded authored excerpt. Schema 85 stores every task
/// creation context on the registered transcript stream or a current
/// unavailable reason, retires the gather route, and gates every retained
/// pending direct Design output on its Design. Schema 86 indexes each steering
/// head's room as a typed column for newest-route lookup. Schema 87 retires the
/// question-result protocol and required question-result evidence while preserving
/// their receipts and facts as inert authority. Schema 88 materializes task Git
/// boundaries and review corrections as typed release-note authority. Schema 89
/// records on every task review the workflow status its verdict left. Schema 90
/// retires contribution labels into charges: reason-for is +1, reason-against
/// is -1, evidence is 0, on each fact and its command receipt.
/// Schema 91 records on every item-bearing contribution its question item's
/// text identity, which keys that item's charge series. Schema 92 lets a
/// Design invitation name the one question item it asks, which an older
/// release would refuse. Schema 93 retires the automated room-resume rule
/// from every retained coordination record, direction head, and receipt.
/// Schema 94 lets one Design invitation list every question item it asks.
/// Schema 95 drops the repeat-leave flag from every retained leave answer.
/// Schema 96 fills a pitch on every retained task statement once, derived
/// from its title and description. Schema 97 retires room coordination: its
/// facts become inert evidence, its receipts move to the historical table,
/// and its direction and shelving heads drop. Schema 98 retires hidden
/// projects: a retained row naming a leading-dot project refuses the crossing
/// with the last release that admitted it.
pub const SCHEMA_VERSION: u32 = 98;

/// How long a connection to the on-disk authority waits on a lock another
/// process holds before giving up, instead of refusing on the first try.
///
/// A sibling resident releasing this same file (its own shutdown, a commit,
/// a read-only inspection) can hold SQLite's lock for a moment after every
/// higher-level handoff this crate coordinates has already completed, since
/// that handoff has no visibility into SQLite's own file lock. Two seconds
/// outlasts that moment without masking a genuinely stuck holder, and it is
/// the ceiling for any wait on the resident's event loop: that loop drains a
/// lossy notice queue, so every second it spends inside a commit is a second
/// in which the kernel may refuse the notices the loop exists to read.
#[cfg(not(target_arch = "wasm32"))]
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);

#[cfg(not(target_arch = "wasm32"))]
mod guard;
#[cfg(not(target_arch = "wasm32"))]
mod historical_receipts;
#[cfg(not(target_arch = "wasm32"))]
mod migrate;
#[cfg(not(target_arch = "wasm32"))]
mod read;
#[cfg(not(target_arch = "wasm32"))]
mod release_notes;
#[cfg(not(target_arch = "wasm32"))]
mod replay;
#[cfg(not(target_arch = "wasm32"))]
mod restore;
#[cfg(not(target_arch = "wasm32"))]
mod rooms;
#[cfg(not(target_arch = "wasm32"))]
mod taste_index;

#[cfg(not(target_arch = "wasm32"))]
pub use guard::{
    Applied, BuildError, CommandReplay, CommitError, CommitFault, CurrentHeadWitness, FactCause,
    FactPageStart, FactReadBudget, GuardError, GuardRefusal, JournalGuard, JournalHead,
    JournalRead, JournalWrite, MAX_DURABLE_FACT_ENVELOPE_BYTES, MAX_RETAINED_FACT_PAGE_BYTES,
    MAX_RETAINED_FACT_PAGE_ROWS, MAX_STEERING_PAGE_ROWS, OpenRoomItemPage, OperatorDirection,
    OperatorDirections, PendingFact, ProposalAuthority, ROOM_AUTHORITY_REFUSED, ReadError,
    RetiredSteering, RoomRelevanceMetrics, RoomRelevancePage, RoomRelevanceRecord,
    SelectedFactPage, SteeringPage, TaskGeneration, TaskHeadPage, TaskHeads, TasteEvidenceFact,
    TasteEvidencePage, TasteEvidenceSummary, ThreadError, TrustHeadPage,
};
#[cfg(not(target_arch = "wasm32"))]
pub use migrate::{Migration, MigrationError, OpenError};
#[cfg(not(target_arch = "wasm32"))]
pub use read::{FactSnapshot, SnapshotError, SteeringSnapshot};
#[cfg(not(target_arch = "wasm32"))]
pub use release_notes::{ReleaseCorrection, ReleaseLanding, ReleaseNoteAuthority};
#[cfg(not(target_arch = "wasm32"))]
pub use replay::{
    ReindexError, ReindexReport, Verification, VerifyError, Violation, ViolationClass,
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
    pub use wire::principal;
    pub use wire::rooms;
    pub use wire::{
        Actor, AnyFact, CanonicalUuid, ClaimCell, ClaimDisposition, ClaimHead, Command,
        CommandKind, Digest, Envelope, Environment, EnvironmentRequirement,
        EnvironmentRequirements, Fact, FactKind, FindingHead, Instant, JournalCursor,
        LaneResidency, LaneSession, LeaseHead, OccupantAddress, OopsGenerationHead, Origin, Packed,
        Phase, Producer, ResidencyHead, ResidentState, SeatAddress, Seq, SteeringHead, TapeLocus,
        TaskFiling, TaskHandle, TaskHead, TaskProvenance, TaskStamp, TranscriptSourceHead,
        TranscriptStreamKey, TrustHead, TrustState, WireVersion, Witness, WitnessHead,
        WorkflowCompletionReview, WorkflowConclusion, WorkflowHead, WorkflowNegative,
        WorkflowReview, WorkflowStatus,
    };
}

#[cfg(not(target_arch = "wasm32"))]
use std::fs::{self, File};
#[cfg(not(target_arch = "wasm32"))]
use std::io::{self, Seek, Write};
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use rusqlite::{Connection, OpenFlags};

/// One open handle to the authority database.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct Journal {
    connection: Connection,
    migration: Option<Migration>,
    residency_cache: guard::ResidencyCache,
}

/// A capability-distinct, read-only view of finalized authority.
///
/// Opening this handle never creates, migrates, or repairs a database:
/// `query_only` refuses every write on the connection, so callers that only
/// need schema and cursor identity cannot accidentally cross the resident's
/// sole-writer boundary. The file itself is admitted read-write, without
/// creation, for one reason: a write-ahead-log authority keeps its log and
/// index beside itself, and only a connection SQLite considers writable may
/// fold an empty log back in and retire both when it closes last. A handle
/// admitted read-only would leave that pair behind after every probe, which
/// restore would then have to refuse as an unresolved sidecar.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct JournalReader {
    connection: Connection,
    residency_cache: guard::ResidencyCache,
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
    sill::authority::create_private_file(path).map_err(|source| SnapshotFileError::Io {
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

    /// Opens an isolated in-memory authority database.
    pub fn in_memory() -> Result<Self, OpenError> {
        Self::from_connection(Connection::open_in_memory().map_err(OpenError::Sqlite)?)
    }

    /// Returns the finalized schema version.
    pub fn schema_version(&self) -> Result<u32, OpenError> {
        migrate::finalized_version(&self.connection)
    }

    /// Initialization or predecessor crossing this open completed, absent when
    /// the authority was already current.
    #[must_use]
    pub const fn migration(&self) -> Option<Migration> {
        self.migration
    }

    /// Inspects one existing authority database without creating or rewriting it.
    ///
    /// This is the daemonless read used by identity-reporting surfaces. The
    /// read-only SQLite handle makes absence and write attempts refusals at the
    /// storage boundary; unlike [`Self::open`], this path never baselines an
    /// empty file and never enters a schema-admission transaction.
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
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|source| OpenError::Open {
                path: path.to_path_buf(),
                source,
            })?;
        connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(OpenError::Sqlite)?;
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
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(OpenError::Sqlite)?;
        connection
            .execute_batch("PRAGMA trusted_schema = OFF; PRAGMA foreign_keys = ON;")
            .map_err(OpenError::Sqlite)?;
        let migration = migrate::admit(&mut connection)?;
        admit_write_ahead_log(&connection)?;
        Ok(Self {
            connection,
            migration,
            residency_cache: Default::default(),
        })
    }
}

/// Spell `path` as the `file:` URI that marks it immutable to SQLite.
///
/// Only the three bytes SQLite's URI parser reads as structure are escaped;
/// everything else in a path passes through unchanged.
#[cfg(not(target_arch = "wasm32"))]
fn immutable_uri(path: &Path) -> Option<String> {
    let mut uri = String::from("file:");
    for byte in path.to_str()?.bytes() {
        match byte {
            b'%' => uri.push_str("%25"),
            b'?' => uri.push_str("%3F"),
            b'#' => uri.push_str("%23"),
            other => uri.push(char::from(other)),
        }
    }
    uri.push_str("?immutable=1");
    Some(uri)
}

/// Keep a file-backed authority in write-ahead-log mode.
///
/// Under SQLite's default rollback journal a writer's exclusive lock stalls
/// every reader in every other process for the whole write, and a reader that
/// then needs to write is refused outright rather than waiting, since SQLite
/// cannot let it upgrade behind the holder. The resident holds its one writer
/// open for its whole life while every `spice` command in every lane opens a
/// reader against the same file, so that shape is the ordinary case here, not
/// the exception. Write-ahead logging lets readers proceed against their own
/// snapshot while the writer commits, leaving only writer-against-writer
/// contention for [`BUSY_TIMEOUT`] to absorb.
///
/// The mode is persistent in the file, so this is idempotent after the first
/// writer sets it. An in-memory or temporary database has no file to log
/// against and stays on its own journal.
#[cfg(not(target_arch = "wasm32"))]
fn admit_write_ahead_log(connection: &Connection) -> Result<(), OpenError> {
    let file_backed = connection.path().is_some_and(|path| !path.is_empty());
    if !file_backed {
        return Ok(());
    }
    let found = connection
        .query_row("PRAGMA journal_mode = WAL", [], |row| {
            row.get::<_, String>(0)
        })
        .map_err(OpenError::Sqlite)?;
    if found != "wal" {
        return Err(OpenError::JournalMode { found });
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
impl JournalReader {
    /// Opens an existing, finalized authority without mutating it.
    ///
    /// # Errors
    ///
    /// Returns the SQLite open failure when the file is absent or unreadable,
    /// or the existing schema-admission refusal when authority is unfinalized
    /// or belongs to another release.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, OpenError> {
        let path = path.as_ref();
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|source| OpenError::Open {
                path: path.to_path_buf(),
                source,
            })?;
        Self::from_connection(connection)
    }

    /// Opens one captured generation that nothing can be writing to.
    ///
    /// A captured authority is a private regular file sealed against every
    /// writer, so SQLite is told it is immutable: it takes no locks and asks
    /// for none of the write-ahead-log siblings a live authority keeps beside
    /// itself, which is what lets a sealed, read-only capture directory be
    /// verified at all. That same promise is why this must never open a live
    /// authority -- an immutable open reads only the main file, so commits
    /// still waiting in a live log would be invisible to it. Live reads go
    /// through [`Self::open`].
    ///
    /// # Errors
    ///
    /// Returns the SQLite open failure when the file is absent, unreadable, or
    /// not spellable as a URI, or the existing schema-admission refusal.
    pub fn open_immutable(path: impl AsRef<Path>) -> Result<Self, OpenError> {
        let path = path.as_ref();
        let uri = immutable_uri(path).ok_or_else(|| OpenError::Open {
            path: path.to_path_buf(),
            source: rusqlite::Error::InvalidPath(path.to_path_buf()),
        })?;
        let connection = Connection::open_with_flags(
            uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|source| OpenError::Open {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_connection(connection)
    }

    fn from_connection(connection: Connection) -> Result<Self, OpenError> {
        connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(OpenError::Sqlite)?;
        connection
            .pragma_update(None, "query_only", true)
            .map_err(OpenError::Sqlite)?;
        connection
            .pragma_update(None, "trusted_schema", false)
            .map_err(OpenError::Sqlite)?;
        migrate::inspect_version(&connection)?;
        Ok(Self {
            connection,
            residency_cache: Default::default(),
        })
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
