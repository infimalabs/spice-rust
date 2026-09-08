use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::SCHEMA_VERSION;

mod claim_index;
mod creation_context;
mod identity_stamp;
mod release_notes;
mod renewal_organization;
mod residency_lineage;
mod steering_identity;
mod steering_microseconds;
mod task_allocation;
mod topology_revision;
mod vocabulary;

const FINALIZED: &str = "finalized";
const PREPARED: &str = "prepared";

pub(crate) fn baseline(connection: &mut Connection) -> Result<Option<Migration>, OpenError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Exclusive)
        .map_err(OpenError::Sqlite)?;
    if !table_exists(&transaction, "journal_schema")? {
        let object_count = authority_object_count(&transaction)?;
        if object_count != 0 {
            return Err(MigrationError::UnversionedAuthority { object_count }.into());
        }

        transaction
            .execute_batch(BASELINE_SQL)
            .map_err(OpenError::Sqlite)?;
        transaction.commit().map_err(OpenError::Sqlite)?;
        return Ok(Some(Migration {
            from: 0,
            to: SCHEMA_VERSION,
        }));
    }

    let migration = advance(&transaction)?;
    transaction.commit().map_err(OpenError::Sqlite)?;
    Ok(migration)
}

/// One schema transition completed by this journal open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Migration {
    from: u32,
    to: u32,
}

impl Migration {
    /// Schema version the open left.
    #[must_use]
    pub const fn from(self) -> u32 {
        self.from
    }

    /// Schema version the open finalized.
    #[must_use]
    pub const fn to(self) -> u32 {
        self.to
    }
}

pub(crate) fn finalized_version(connection: &Connection) -> Result<u32, OpenError> {
    admit_state(connection, stored_state(connection)?)
}

fn stored_state(connection: &Connection) -> Result<SchemaState, OpenError> {
    let count: u32 = connection
        .query_row("SELECT count(*) FROM journal_schema", [], |row| row.get(0))
        .map_err(OpenError::Sqlite)?;
    if count != 1 {
        return Err(MigrationError::SchemaRowCount { count }.into());
    }

    let state = connection
        .query_row(
            "SELECT version, state, from_version, to_version FROM journal_schema WHERE singleton = 1",
            [],
            |row| {
                Ok(SchemaState {
                    version: row.get(0)?,
                    state: row.get(1)?,
                    from_version: row.get(2)?,
                    to_version: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(OpenError::Sqlite)?
        .ok_or(MigrationError::MissingSingleton)?;
    Ok(state)
}

/// Reads an existing schema identity without performing baseline or migration.
pub(crate) fn inspect_version(connection: &Connection) -> Result<u32, OpenError> {
    if !table_exists(connection, "journal_schema")? {
        let object_count = authority_object_count(connection)?;
        return Err(if object_count == 0 {
            MigrationError::MissingSchema
        } else {
            MigrationError::UnversionedAuthority { object_count }
        }
        .into());
    }
    finalized_version(connection)
}

/// Observe a finalized stored schema identity without admitting its version.
///
/// Upgrade orchestration needs the exact before/after version even when this
/// build would refuse to read that version's authority. The identity still has
/// to be finalized and agree with SQLite's independent version record.
pub(crate) fn inspect_stored_version(connection: &Connection) -> Result<u32, OpenError> {
    if !table_exists(connection, "journal_schema")? {
        let object_count = authority_object_count(connection)?;
        return Err(if object_count == 0 {
            MigrationError::MissingSchema
        } else {
            MigrationError::UnversionedAuthority { object_count }
        }
        .into());
    }
    finalized_stored_state(connection, stored_state(connection)?)
}

/// Admit a schema identity for a caller that will not migrate it.
///
/// Read-only opens and post-open verification land here, so a version behind the
/// current one is refused two different ways on purpose. One this build could
/// step forward names the write open that would do it; one it could not names
/// the release that owns it. Answering both with the same text would tell a
/// reader holding a migratable authority to go find another release.
fn admit_state(connection: &Connection, state: SchemaState) -> Result<u32, OpenError> {
    let version = admit_readable_state(connection, state)?;
    if version != SCHEMA_VERSION {
        return Err(if migration_path(version).is_some() {
            MigrationError::UnmigratedRead {
                found: version,
                current: SCHEMA_VERSION,
            }
        } else {
            MigrationError::UnknownVersion { found: version }
        }
        .into());
    }
    Ok(version)
}

/// Admit a schema identity this build can read, migrated or not.
///
/// The version equality check is deliberately not here: a version behind the
/// current one is a candidate for migration rather than a refusal, and it can
/// only be judged one after the state and the two version records agree. A
/// disagreement between the row and the pragma is corruption at any version, so
/// it is answered before anything decides to write to the database.
fn admit_readable_state(connection: &Connection, state: SchemaState) -> Result<u32, OpenError> {
    let version = finalized_stored_state(connection, state)?;
    if version > SCHEMA_VERSION {
        return Err(MigrationError::ForwardSchema {
            found: version,
            current: SCHEMA_VERSION,
        }
        .into());
    }

    Ok(version)
}

fn finalized_stored_state(connection: &Connection, state: SchemaState) -> Result<u32, OpenError> {
    if state.state == PREPARED {
        return Err(MigrationError::Unfinalized {
            from: state.from_version,
            to: state.to_version,
        }
        .into());
    }
    if state.state != FINALIZED {
        return Err(MigrationError::UnknownState { state: state.state }.into());
    }

    let pragma: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(OpenError::Sqlite)?;
    if pragma != state.version {
        return Err(MigrationError::VersionDisagreement {
            row: state.version,
            pragma,
        }
        .into());
    }
    Ok(state.version)
}

/// Step an admitted authority forward to the schema this build reads.
///
/// Every step runs inside the caller's exclusive transaction together with the
/// two version records it advances, so a failure part-way leaves the authority
/// at the version it started from rather than between two. Additive DDL and
/// validated content rewrites therefore share one rollback boundary: a rewrite
/// may change canonical bytes, but it cannot strand only some rows or version
/// records in the new schema.
///
/// It takes the caller's transaction rather than opening its own, and is visible
/// to the crate so a test can be that caller: the promise above is a property of
/// this seam, and it cannot be observed through [`baseline`], which always
/// commits. A caller that fails after this returns is the case worth pinning,
/// because both version records are already written by then.
pub(crate) fn advance(connection: &Connection) -> Result<Option<Migration>, OpenError> {
    let from = admit_readable_state(connection, stored_state(connection)?)?;
    if from == SCHEMA_VERSION {
        return Ok(None);
    }

    let path = migration_path(from).ok_or(MigrationError::UnknownVersion { found: from })?;
    for step in path {
        step.apply(connection)?;
    }
    connection
        .execute(
            "UPDATE journal_schema SET version = ?1 WHERE singleton = 1",
            params![SCHEMA_VERSION],
        )
        .map_err(OpenError::Sqlite)?;
    connection
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(OpenError::Sqlite)?;
    Ok(Some(Migration {
        from,
        to: SCHEMA_VERSION,
    }))
}

/// The ordered steps from `from` up to the current schema, if all of them exist.
///
/// An unbroken chain is the whole question, so this answers it once and both
/// callers ask it the same way. A version whose first step exists but whose
/// chain breaks later is not migratable, and admitting it would refuse a
/// read-only open with an offer to migrate that the write open cannot keep.
fn migration_path(from: u32) -> Option<Vec<MigrationStep>> {
    (from..SCHEMA_VERSION)
        .map(|version| {
            MIGRATIONS
                .iter()
                .find(|(leaves, _)| *leaves == version)
                .map(|(_, step)| *step)
        })
        .collect()
}

/// Versions this build can step forward from, rendered for a refusal.
///
/// Answers the reader's actual next question -- which older authority this build
/// does accept -- so the refusal points at a release to route through instead of
/// at discarding the authority. A build that carries no steps at all says so
/// rather than rendering an empty list into the sentence.
fn migratable_versions() -> String {
    let versions: Vec<String> = (1..SCHEMA_VERSION)
        .filter(|version| migration_path(*version).is_some())
        .map(|version| version.to_string())
        .collect();
    if versions.is_empty() {
        "no earlier schema".to_owned()
    } else {
        format!("schema {}", versions.join(", "))
    }
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool, OpenError> {
    connection
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            params![table],
            |_| Ok(()),
        )
        .optional()
        .map(|row| row.is_some())
        .map_err(OpenError::Sqlite)
}

fn authority_object_count(connection: &Connection) -> Result<u32, OpenError> {
    connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .map_err(OpenError::Sqlite)
}

struct SchemaState {
    version: u32,
    state: String,
    from_version: Option<u32>,
    to_version: Option<u32>,
}

/// A database open, schema admission, or SQLite operation failure.
#[derive(Debug)]
pub enum OpenError {
    Open {
        path: PathBuf,
        source: rusqlite::Error,
    },
    Sqlite(rusqlite::Error),
    Migration(MigrationError),
    InvalidCursor {
        found: i64,
    },
}

impl fmt::Display for OpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open { path, source } => {
                write!(
                    formatter,
                    "journal could not open {}: {source}",
                    path.display()
                )
            }
            Self::Sqlite(error) => write!(formatter, "journal SQLite operation failed: {error}"),
            Self::Migration(error) => error.fmt(formatter),
            Self::InvalidCursor { found } => {
                write!(
                    formatter,
                    "journal cursor must be a positive position, found {found}"
                )
            }
        }
    }
}

impl Error for OpenError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Open { source, .. } => Some(source),
            Self::Sqlite(error) => Some(error),
            Self::Migration(error) => Some(error),
            Self::InvalidCursor { .. } => None,
        }
    }
}

impl From<MigrationError> for OpenError {
    fn from(error: MigrationError) -> Self {
        Self::Migration(error)
    }
}

/// A named schema-state refusal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MigrationError {
    MissingSchema,
    UnversionedAuthority {
        object_count: u32,
    },
    SchemaRowCount {
        count: u32,
    },
    MissingSingleton,
    Unfinalized {
        from: Option<u32>,
        to: Option<u32>,
    },
    UnknownState {
        state: String,
    },
    ForwardSchema {
        found: u32,
        current: u32,
    },
    UnmigratedRead {
        found: u32,
        current: u32,
    },
    UnknownVersion {
        found: u32,
    },
    VersionDisagreement {
        row: u32,
        pragma: u32,
    },
    StoredContent {
        from: u32,
        to: u32,
        table: &'static str,
        row: String,
        detail: String,
    },
}

impl fmt::Display for MigrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSchema => formatter.write_str(
                "journal has no schema identity; release 1 requires a finalized schema before read-only inspection",
            ),
            Self::UnversionedAuthority { object_count } => write!(
                formatter,
                "journal refuses an unversioned nonempty authority containing {object_count} schema objects; release 1 requires backup and an empty authority"
            ),
            Self::SchemaRowCount { count } => write!(
                formatter,
                "journal schema identity requires exactly one row, found {count}"
            ),
            Self::MissingSingleton => {
                formatter.write_str("journal schema singleton row 1 is missing")
            }
            Self::Unfinalized { from, to } => write!(
                formatter,
                "journal migration remains prepared and unfinalized from {from:?} to {to:?}"
            ),
            Self::UnknownState { state } => write!(
                formatter,
                "journal schema state {state:?} is unknown to release 1"
            ),
            Self::ForwardSchema { found, current } => write!(
                formatter,
                "journal schema {found} is newer than this build's schema {current}; use the release that owns schema {found}"
            ),
            Self::UnmigratedRead { found, current } => write!(
                formatter,
                "journal schema {found} is behind this build's schema {current} and a \
                 read-only open never migrates; open this authority for writing once \
                 to step it forward"
            ),
            Self::UnknownVersion { found } => write!(
                formatter,
                "journal schema {found} has no migration path to schema \
                 {SCHEMA_VERSION}; this build steps forward from {}, so use the \
                 release that owns schema {found} to reach one of those first",
                migratable_versions()
            ),
            Self::VersionDisagreement { row, pragma } => write!(
                formatter,
                "journal schema row names version {row}, but SQLite user_version names {pragma}"
            ),
            Self::StoredContent {
                from,
                to,
                table,
                row,
                detail,
            } => write!(
                formatter,
                "journal schema {from} to {to} migration refuses {table} row {row}: {detail}"
            ),
        }
    }
}

impl Error for MigrationError {}

/// Every schema version this build steps forward from, and the step it runs.
///
/// Indexed by the version the step *leaves*, so `advance` can walk one entry at
/// a time up to `SCHEMA_VERSION` instead of needing a pair for every reachable
/// combination. A version absent from this table is refused by name.
#[derive(Clone, Copy)]
enum MigrationStep {
    Sql(&'static str),
    Rust(fn(&Connection) -> Result<(), OpenError>),
}

impl MigrationStep {
    fn apply(self, connection: &Connection) -> Result<(), OpenError> {
        match self {
            Self::Sql(sql) => connection.execute_batch(sql).map_err(OpenError::Sqlite),
            Self::Rust(migrate) => migrate(connection),
        }
    }
}

fn migrate_schema_3(connection: &Connection) -> Result<(), OpenError> {
    steering_identity::migrate(connection)?;
    task_allocation::migrate(connection)
}

fn migrate_schema_5(connection: &Connection) -> Result<(), OpenError> {
    identity_stamp::migrate(connection)?;
    topology_revision::migrate(connection)
}

const MIGRATIONS: &[(u32, MigrationStep)] = &[
    (1, MigrationStep::Sql(MIGRATE_1_TO_2_SQL)),
    (2, MigrationStep::Rust(residency_lineage::migrate)),
    (3, MigrationStep::Rust(migrate_schema_3)),
    (4, MigrationStep::Rust(release_notes::migrate)),
    (5, MigrationStep::Rust(migrate_schema_5)),
    (6, MigrationStep::Sql(MIGRATE_6_TO_7_SQL)),
    (7, MigrationStep::Rust(claim_index::migrate)),
    (8, MigrationStep::Rust(vocabulary::migrate)),
    (9, MigrationStep::Sql(MIGRATE_9_TO_10_SQL)),
    (10, MigrationStep::Sql(MIGRATE_10_TO_11_SQL)),
    (11, MigrationStep::Sql(MIGRATE_11_TO_12_SQL)),
    (12, MigrationStep::Rust(creation_context::migrate)),
    (13, MigrationStep::Rust(steering_microseconds::migrate)),
    (14, MigrationStep::Sql(MIGRATE_14_TO_15_SQL)),
    (15, MigrationStep::Sql(MIGRATE_15_TO_16_SQL)),
    (16, MigrationStep::Rust(renewal_organization::migrate)),
];

/// Schema 15 to 16: environmental-requirement rollout boundary.
///
/// Schema 15 already stores the typed requirement wire. This no-data step is
/// the environmental fact a task may require before a successor is installed:
/// the task stays byte-identical while the admitted runtime observation moves.
const MIGRATE_15_TO_16_SQL: &str = "SELECT 1;";

/// Schema 14 to 15: typed environmental requirements on task authority.
///
/// Existing task facts and heads omit the collection and decode it as empty;
/// their immutable bytes remain untouched. The explicit no-data step makes the
/// expanded task wire visible at the journal boundary, so a schema-14 build
/// refuses the authority before it can decode a requirement-bearing row as if
/// the field did not exist.
const MIGRATE_14_TO_15_SQL: &str = "SELECT 1;";

/// Schema 11 to 12: producing-worktree identity on durable trust authority.
///
/// Existing trust rows omit the optional identity and decode as explicitly
/// unbound current authority. New trust transitions always write it. The
/// schema step prevents older releases from decoding the expanded wire shape;
/// current rulings refuse an unbound row rather than guessing its producer.
const MIGRATE_11_TO_12_SQL: &str = "SELECT 1;";

/// Schema 10 to 11: operator fanout identity in steering facts and heads.
///
/// The SQL tables do not change. Existing steering facts and heads omit the
/// optional identity; every current operator send writes it. The explicit step
/// makes the durable wire expansion visible to schema-10 releases before they
/// try to decode current authority.
const MIGRATE_10_TO_11_SQL: &str = "SELECT 1;";

/// Schema 9 to 10: source-aware transcript loci in immutable fact envelopes.
///
/// The SQL tables do not change. Legacy envelopes omit the optional source and
/// remain byte-canonical; new envelopes may name their tape generation. The
/// explicit schema step makes that wire expansion visible to older releases,
/// which refuse schema 10 before trying to decode a sourced fact.
const MIGRATE_9_TO_10_SQL: &str = "SELECT 1;";

/// Schema 6 to 7: bounded current steering authority and missed-evidence index.
///
/// Existing facts are deliberately not interpreted in this storage-only step.
/// The resident's one canonical steering reducer rebuilds these empty tables
/// once after open, validates the complete immutable stream, and advances the
/// nullable index cursor atomically. A current schema with a lagging cursor is
/// refused by every indexed steering read and write until that rebuild lands.
const MIGRATE_6_TO_7_SQL: &str = r#"
CREATE TABLE steering_heads (
    directive TEXT PRIMARY KEY,
    steer_key TEXT NOT NULL UNIQUE,
    lane TEXT NOT NULL,
    disposition TEXT NOT NULL,
    created_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    minted_millis INTEGER,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE INDEX steering_heads_lane_disposition_created
    ON steering_heads(lane, disposition, created_seq);
CREATE INDEX steering_heads_disposition_created
    ON steering_heads(disposition, created_seq);
CREATE INDEX steering_heads_minted_millis
    ON steering_heads(minted_millis) WHERE minted_millis IS NOT NULL;
CREATE TABLE steering_misses (
    identity TEXT PRIMARY KEY,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE steering_index (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    indexed_seq INTEGER REFERENCES fact_envelopes(seq)
) STRICT;
INSERT INTO steering_index(singleton, indexed_seq) VALUES (1, NULL);
"#;

/// Schema 1 to 2: the table transcript accounting reads its heads from.
///
/// Schema 2 added exactly this one table to the baseline and nothing else --
/// no column changed and no row moved -- so stepping forward is a create rather
/// than a copy, and a schema-1 authority keeps every fact it already holds. A
/// journal that has never accounted a transcript has no head rows to backfill,
/// and one that has will re-derive them as it ingests.
const MIGRATE_1_TO_2_SQL: &str = r#"
CREATE TABLE transcript_source_heads (
    stream TEXT PRIMARY KEY,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
"#;

const BASELINE_SQL: &str = r#"
CREATE TABLE journal_schema (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    version INTEGER NOT NULL CHECK (version > 0),
    state TEXT NOT NULL CHECK (state IN ('prepared', 'finalized')),
    from_version INTEGER,
    to_version INTEGER,
    plan_digest TEXT,
    CHECK (
        (state = 'finalized' AND from_version IS NULL AND to_version IS NULL AND plan_digest IS NULL)
        OR
        (state = 'prepared' AND to_version IS NOT NULL AND plan_digest IS NOT NULL)
    )
) STRICT;

CREATE TABLE fact_envelopes (
    seq INTEGER PRIMARY KEY CHECK (seq > 0),
    wire_version INTEGER NOT NULL CHECK (wire_version > 0),
    instant TEXT NOT NULL,
    actor TEXT NOT NULL,
    origin TEXT NOT NULL,
    cause INTEGER REFERENCES fact_envelopes(seq) DEFERRABLE INITIALLY DEFERRED,
    kind TEXT NOT NULL,
    envelope BLOB NOT NULL,
    CHECK (cause IS NULL OR cause < seq)
) STRICT;
CREATE INDEX fact_envelopes_origin_seq ON fact_envelopes(origin, seq);
CREATE INDEX fact_envelopes_cause ON fact_envelopes(cause) WHERE cause IS NOT NULL;

CREATE TABLE thread_index (
    seq INTEGER PRIMARY KEY REFERENCES fact_envelopes(seq) ON DELETE RESTRICT,
    origin TEXT NOT NULL,
    cause INTEGER REFERENCES fact_envelopes(seq) ON DELETE RESTRICT,
    root INTEGER NOT NULL REFERENCES fact_envelopes(seq) ON DELETE RESTRICT,
    depth INTEGER NOT NULL CHECK (depth >= 0),
    CHECK ((cause IS NULL AND root = seq AND depth = 0) OR cause IS NOT NULL)
) STRICT;
CREATE INDEX thread_index_origin_seq ON thread_index(origin, seq);
CREATE INDEX thread_index_root_seq ON thread_index(root, seq);

CREATE TABLE command_receipts (
    command_id TEXT PRIMARY KEY,
    actor TEXT NOT NULL,
    command_kind TEXT NOT NULL,
    request BLOB NOT NULL,
    evaluated_witnesses BLOB NOT NULL,
    response BLOB NOT NULL,
    first_seq INTEGER REFERENCES fact_envelopes(seq),
    last_seq INTEGER REFERENCES fact_envelopes(seq),
    snapshot_seq INTEGER REFERENCES fact_envelopes(seq),
    epoch TEXT NOT NULL,
    CHECK (
        (first_seq IS NOT NULL AND last_seq IS NOT NULL AND snapshot_seq IS NULL AND first_seq <= last_seq)
        OR
        (first_seq IS NULL AND last_seq IS NULL AND snapshot_seq IS NOT NULL)
    )
) STRICT;

CREATE TABLE task_heads (
    stamp TEXT PRIMARY KEY,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE workflow_heads (
    stamp TEXT PRIMARY KEY REFERENCES task_heads(stamp),
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE claim_heads (
    stamp TEXT PRIMARY KEY REFERENCES task_heads(stamp),
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE claim_index (
    stamp TEXT PRIMARY KEY REFERENCES claim_heads(stamp),
    actor TEXT NOT NULL,
    disposition TEXT NOT NULL CHECK (disposition IN ('active', 'freed', 'lapsed')),
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq)
) STRICT;
CREATE INDEX claim_index_actor_disposition_stamp
    ON claim_index(actor, disposition, stamp);
CREATE INDEX claim_index_disposition_actor
    ON claim_index(disposition, actor);
CREATE TABLE lease_heads (
    stamp TEXT PRIMARY KEY REFERENCES task_heads(stamp),
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE witness_heads (
    name TEXT PRIMARY KEY,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE trust_heads (
    digest TEXT PRIMARY KEY,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE finding_heads (
    identity TEXT PRIMARY KEY,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE transcript_source_heads (
    stream TEXT PRIMARY KEY,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE steering_heads (
    directive TEXT PRIMARY KEY,
    steer_key TEXT NOT NULL UNIQUE,
    lane TEXT NOT NULL,
    disposition TEXT NOT NULL,
    created_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    minted_micros INTEGER,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE INDEX steering_heads_lane_disposition_created
    ON steering_heads(lane, disposition, created_seq);
CREATE INDEX steering_heads_disposition_created
    ON steering_heads(disposition, created_seq);
CREATE INDEX steering_heads_minted_micros
    ON steering_heads(minted_micros) WHERE minted_micros IS NOT NULL;
CREATE TABLE steering_misses (
    identity TEXT PRIMARY KEY,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE steering_index (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    indexed_seq INTEGER REFERENCES fact_envelopes(seq)
) STRICT;
CREATE TABLE residency_head (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE cursor_head (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    seq INTEGER REFERENCES fact_envelopes(seq),
    wake INTEGER REFERENCES fact_envelopes(seq),
    CHECK (wake IS NULL OR (seq IS NOT NULL AND wake <= seq))
) STRICT;

CREATE TRIGGER cursor_head_monotone
BEFORE UPDATE ON cursor_head
WHEN (OLD.seq IS NOT NULL AND (NEW.seq IS NULL OR NEW.seq < OLD.seq))
  OR (OLD.wake IS NOT NULL AND (NEW.wake IS NULL OR NEW.wake < OLD.wake))
BEGIN
    SELECT RAISE(ABORT, 'cursor head cannot move backward');
END;

INSERT INTO journal_schema(singleton, version, state) VALUES (1, 17, 'finalized');
INSERT INTO cursor_head(singleton, seq, wake) VALUES (1, NULL, NULL);
INSERT INTO steering_index(singleton, indexed_seq) VALUES (1, NULL);
PRAGMA user_version = 17;
"#;
