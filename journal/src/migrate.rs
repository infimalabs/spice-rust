use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::SCHEMA_VERSION;

mod fact_bounds;

#[cfg(test)]
pub(crate) use fact_bounds::bounds_sql;
pub(crate) use fact_bounds::{
    FACT_ENVELOPE_INSERT_TRIGGER, FACT_ENVELOPE_UPDATE_TRIGGER, insert_sql, update_sql,
};
const FINALIZED: &str = "finalized";
const PREPARED: &str = "prepared";

/// Schemas whose retained authority decodes as the current shape unchanged.
///
/// Writable admission creates the room-authority projection, which no
/// predecessor could have populated, and advances the schema identity;
/// retained facts and heads remain byte-for-byte intact. Older writers refuse
/// the new boundary.
const PREDECESSOR_SCHEMAS: [u32; 3] = [33, 34, 35];

/// Admit exact-current authority, cross the predecessor once, or initialize
/// one truly empty database.
///
/// Existing authority is classified without a transaction so a retired file
/// refuses before SQLite can create a rollback journal or change its persistent
/// journal mode. Empty initialization and the predecessor crossing take the
/// exclusive transaction, then recheck what they observed to close the race
/// with another opener.
pub(crate) fn admit(connection: &mut Connection) -> Result<Option<Migration>, OpenError> {
    let trusted_schema: bool = connection
        .query_row("PRAGMA trusted_schema", [], |row| row.get(0))
        .map_err(OpenError::Sqlite)?;
    if trusted_schema {
        return Err(OpenError::TrustedSchemaEnabled);
    }

    if table_exists(connection, "journal_schema")? {
        let version = admit_readable_state(connection, stored_state(connection)?)?;
        if !PREDECESSOR_SCHEMAS.contains(&version) {
            admit_version(version)?;
            return Ok(None);
        }
        return cross_predecessor(connection);
    }
    let object_count = authority_object_count(connection)?;
    if object_count != 0 {
        return Err(MigrationError::UnversionedAuthority { object_count }.into());
    }

    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Exclusive)
        .map_err(OpenError::Sqlite)?;
    if table_exists(&transaction, "journal_schema")? {
        admit_state(&transaction, stored_state(&transaction)?)?;
        transaction.commit().map_err(OpenError::Sqlite)?;
        return Ok(None);
    }
    let object_count = authority_object_count(&transaction)?;
    if object_count != 0 {
        return Err(MigrationError::UnversionedAuthority { object_count }.into());
    }

    transaction
        .execute_batch(BASELINE_SQL)
        .map_err(OpenError::Sqlite)?;
    transaction
        .execute_batch(crate::rooms::SCHEMA_SQL)
        .map_err(OpenError::Sqlite)?;
    crate::historical_receipts::create_schema(&transaction)?;
    crate::historical_receipts::reserve_identities(&transaction)?;
    transaction
        .execute_batch(&fact_bounds::bounds_sql())
        .map_err(OpenError::Sqlite)?;
    transaction.commit().map_err(OpenError::Sqlite)?;
    Ok(Some(Migration {
        from: 0,
        to: SCHEMA_VERSION,
    }))
}

/// Current-schema initialization or predecessor crossing completed by this open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Migration {
    from: u32,
    to: u32,
}

impl Migration {
    /// Schema identity before this open: `0` for initialization, otherwise
    /// the crossed predecessor.
    #[must_use]
    pub const fn from(self) -> u32 {
        self.from
    }

    /// Current schema this open reached.
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

/// Admit a finalized authority at the sole executable schema.
fn admit_state(connection: &Connection, state: SchemaState) -> Result<u32, OpenError> {
    admit_version(admit_readable_state(connection, state)?)
}

fn admit_version(version: u32) -> Result<u32, OpenError> {
    if PREDECESSOR_SCHEMAS.contains(&version) {
        return Err(MigrationError::PendingCrossing { found: version }.into());
    }
    if version != SCHEMA_VERSION {
        return Err(MigrationError::UnknownVersion { found: version }.into());
    }
    Ok(version)
}

/// Move a finalized predecessor authority to the current schema exactly once.
fn cross_predecessor(connection: &mut Connection) -> Result<Option<Migration>, OpenError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Exclusive)
        .map_err(OpenError::Sqlite)?;
    let version = admit_readable_state(&transaction, stored_state(&transaction)?)?;
    if !PREDECESSOR_SCHEMAS.contains(&version) {
        admit_version(version)?;
        transaction.commit().map_err(OpenError::Sqlite)?;
        return Ok(None);
    }
    transaction
        .execute_batch(crate::rooms::SCHEMA_SQL)
        .map_err(OpenError::Sqlite)?;
    transaction
        .execute(
            "UPDATE journal_schema SET version = ?1 WHERE singleton = 1",
            params![SCHEMA_VERSION],
        )
        .map_err(OpenError::Sqlite)?;
    transaction
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(OpenError::Sqlite)?;
    transaction.commit().map_err(OpenError::Sqlite)?;
    Ok(Some(Migration {
        from: version,
        to: SCHEMA_VERSION,
    }))
}

/// Validate a finalized schema identity before applying the current cutoff.
///
/// Version disagreement and forward authority are corruption/compatibility
/// failures distinct from a retired predecessor. They are classified here
/// before [`admit_state`] applies the exact-current requirement.
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

const RETIRED_RELEASES: [&str; 32] = [
    "122d2a842",
    "IDENTIT-1kLGl2S5",
    "IDENTIT-1kLGl2S5",
    "b6acb226d",
    "d0c14dcc4",
    "SESSION-1kLLCyqw",
    "MINING-1kLNTy1C",
    "MINING-1kLNTy1C",
    "87636d7af",
    "1kRNj6X7",
    "257200978",
    "6ad57c713",
    "d811711a3",
    "ENVIRON-dRyzKgHJw",
    "SCHEMAR-dRyzNKsdp",
    "b9a45a6ec",
    "37cc1297f",
    "ALLOCAT-dS8cvNplX",
    "BOUNDS-dSHsMK0kz",
    "eab8365a6",
    "d431530eb",
    "c64c48cac",
    "TASKS-dSPJyjNSH",
    "TASKS-dSPW61MXd",
    "fb96c12a7",
    "966d33bef",
    "916081f8a",
    "02715f9e1",
    "SPEECH-dTQBpHHJq",
    "a4abe071f",
    "55ad00080",
    "a15ed13f4",
];

fn retired_release(version: u32) -> Option<&'static str> {
    let index = usize::try_from(version.checked_sub(1)?).ok()?;
    RETIRED_RELEASES.get(index).copied()
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
    /// Schema admission was attempted before disabling trusted schema objects.
    TrustedSchemaEnabled,
    InvalidCursor {
        found: i64,
    },
    /// The on-disk authority would not enter write-ahead-log mode.
    JournalMode {
        found: String,
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
            Self::TrustedSchemaEnabled => {
                formatter.write_str("journal schema admission requires PRAGMA trusted_schema = OFF")
            }
            Self::InvalidCursor { found } => {
                write!(
                    formatter,
                    "journal cursor must be a positive position, found {found}"
                )
            }
            Self::JournalMode { found } => {
                write!(
                    formatter,
                    "journal authority must run in write-ahead-log mode, found {found:?}"
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
            Self::InvalidCursor { .. } | Self::JournalMode { .. } | Self::TrustedSchemaEnabled => {
                None
            }
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
    UnknownVersion {
        found: u32,
    },
    /// Predecessor authority that only a writable open crosses forward.
    PendingCrossing {
        found: u32,
    },
    VersionDisagreement {
        row: u32,
        pragma: u32,
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
            Self::PendingCrossing { found } => write!(
                formatter,
                "journal schema {found} crosses once to schema {SCHEMA_VERSION} at the next \
                 writable open; start the resident with this build before reading it"
            ),
            Self::UnknownVersion { found } => {
                if let Some(release) = retired_release(*found) {
                    write!(
                        formatter,
                        "journal schema {found} was last migrated by release {release}; \
                         use that release to reach schema {SCHEMA_VERSION} before opening it \
                         with this build"
                    )
                } else {
                    write!(
                        formatter,
                        "journal schema {found} has no migration path to schema \
                         {SCHEMA_VERSION}; use the release that owns schema {found} to reach \
                         schema {SCHEMA_VERSION} first"
                    )
                }
            }
            Self::VersionDisagreement { row, pragma } => write!(
                formatter,
                "journal schema row names version {row}, but SQLite user_version names {pragma}"
            ),
        }
    }
}

impl Error for MigrationError {}

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
CREATE INDEX fact_envelopes_kind_seq ON fact_envelopes(kind, seq);
CREATE INDEX fact_envelopes_origin_kind_seq ON fact_envelopes(origin, kind, seq);

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
CREATE INDEX task_heads_project_stamp
ON task_heads(json_extract(CAST(body AS TEXT), '$.project'), stamp);
CREATE TABLE workflow_heads (
    stamp TEXT PRIMARY KEY REFERENCES task_heads(stamp),
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL,
    revised_seq INTEGER REFERENCES fact_envelopes(seq)
) STRICT;
CREATE INDEX task_heads_head_seq ON task_heads(head_seq);
CREATE INDEX workflow_heads_head_seq ON workflow_heads(head_seq);
CREATE INDEX workflow_heads_revised_seq ON workflow_heads(revised_seq)
    WHERE revised_seq IS NOT NULL;
CREATE TABLE task_generations (
    stamp TEXT NOT NULL REFERENCES task_heads(stamp),
    generation INTEGER NOT NULL CHECK (generation > 0),
    seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    baseline INTEGER NOT NULL DEFAULT 0 CHECK (baseline IN (0, 1)),
    PRIMARY KEY (stamp, generation)
) STRICT, WITHOUT ROWID;
CREATE TABLE claim_heads (
    stamp TEXT PRIMARY KEY REFERENCES task_heads(stamp),
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE claim_index (
    stamp TEXT PRIMARY KEY REFERENCES claim_heads(stamp),
    actor TEXT NOT NULL,
    lane TEXT NOT NULL,
    disposition TEXT NOT NULL CHECK (disposition IN ('active', 'freed', 'lapsed')),
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq)
) STRICT;
CREATE INDEX claim_index_actor_disposition_stamp
    ON claim_index(actor, disposition, stamp, lane, head_seq);
CREATE INDEX claim_index_disposition_actor
    ON claim_index(disposition, actor);
CREATE INDEX claim_index_disposition_lane_stamp
    ON claim_index(disposition, lane, stamp, head_seq);
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
    lane TEXT NOT NULL,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE INDEX transcript_source_heads_lane_stream
    ON transcript_source_heads(lane, stream, head_seq);
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

CREATE TABLE taste_fires (
    seq INTEGER PRIMARY KEY REFERENCES fact_envelopes(seq),
    taste TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal > 0),
    UNIQUE(taste, ordinal)
) STRICT;
CREATE INDEX taste_fires_taste_seq ON taste_fires(taste, seq DESC);
CREATE TABLE taste_judgments (
    seq INTEGER PRIMARY KEY REFERENCES fact_envelopes(seq),
    taste TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal > 0),
    confirmed INTEGER NOT NULL CHECK (confirmed >= 0),
    cleared INTEGER NOT NULL CHECK (cleared >= 0),
    UNIQUE(taste, ordinal)
) STRICT;
CREATE INDEX taste_judgments_taste_seq ON taste_judgments(taste, seq DESC);
CREATE TABLE lane_seats (
    lane TEXT PRIMARY KEY,
    vendor TEXT NOT NULL,
    seq INTEGER NOT NULL REFERENCES fact_envelopes(seq)
) STRICT;

CREATE TRIGGER cursor_head_monotone
BEFORE UPDATE ON cursor_head
WHEN (OLD.seq IS NOT NULL AND (NEW.seq IS NULL OR NEW.seq < OLD.seq))
  OR (OLD.wake IS NOT NULL AND (NEW.wake IS NULL OR NEW.wake < OLD.wake))
BEGIN
    SELECT RAISE(ABORT, 'cursor head cannot move backward');
END;

INSERT INTO journal_schema(singleton, version, state) VALUES (1, 36, 'finalized');
INSERT INTO cursor_head(singleton, seq, wake) VALUES (1, NULL, NULL);
INSERT INTO steering_index(singleton, indexed_seq) VALUES (1, NULL);
PRAGMA user_version = 36;
"#;
