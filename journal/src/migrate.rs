use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::SCHEMA_VERSION;

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

    finalized_version(&transaction)?;
    transaction.commit().map_err(OpenError::Sqlite)?;
    Ok(None)
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
    admit_state(connection, state)
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

fn admit_state(connection: &Connection, state: SchemaState) -> Result<u32, OpenError> {
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
    if state.version > SCHEMA_VERSION {
        return Err(MigrationError::ForwardSchema {
            found: state.version,
            current: SCHEMA_VERSION,
        }
        .into());
    }
    if state.version != SCHEMA_VERSION {
        return Err(MigrationError::UnknownVersion {
            found: state.version,
        }
        .into());
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
    UnversionedAuthority { object_count: u32 },
    SchemaRowCount { count: u32 },
    MissingSingleton,
    Unfinalized { from: Option<u32>, to: Option<u32> },
    UnknownState { state: String },
    ForwardSchema { found: u32, current: u32 },
    UnknownVersion { found: u32 },
    VersionDisagreement { row: u32, pragma: u32 },
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
            Self::UnknownVersion { found } => write!(
                formatter,
                "journal schema {found} has no migration path in release 1; back up and start an empty authority"
            ),
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
    handle TEXT PRIMARY KEY,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE workflow_heads (
    handle TEXT PRIMARY KEY REFERENCES task_heads(handle),
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE claim_heads (
    handle TEXT PRIMARY KEY REFERENCES task_heads(handle),
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
    body BLOB NOT NULL
) STRICT;
CREATE TABLE lease_heads (
    handle TEXT PRIMARY KEY REFERENCES task_heads(handle),
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

INSERT INTO journal_schema(singleton, version, state) VALUES (1, 1, 'finalized');
INSERT INTO cursor_head(singleton, seq, wake) VALUES (1, NULL, NULL);
PRAGMA user_version = 1;
"#;
