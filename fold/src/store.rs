use std::error::Error;
use std::fmt;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::{Cursor, FoldId, Horizon};

const STORE_SCHEMA_VERSION: u32 = 1;

/// Pure fold logic used to rebuild one complete generation.
pub trait Reducer {
    /// Fold this reducer owns.
    fn fold(&self) -> FoldId;

    /// Version of the reducer semantics stored beside its output.
    fn version(&self) -> u32;

    /// Derives one replacement from an immutable authority snapshot.
    fn rebuild(&self, facts: &journal::FactSnapshot) -> Result<Reduction, String>;
}

/// Complete bytes and boundaries prepared before publication begins.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reduction {
    cursor: Cursor,
    horizon: Horizon,
    payload: serde_json::Value,
}

impl Reduction {
    /// A complete reduction over every fact in the snapshot.
    #[must_use]
    pub fn complete(facts: &journal::FactSnapshot, payload: serde_json::Value) -> Self {
        let cursor = facts.horizon().map_or_else(Cursor::empty, Cursor::at);
        let horizon = facts.horizon().map_or_else(Horizon::empty, Horizon::at);
        Self {
            cursor,
            horizon,
            payload,
        }
    }

    /// A reduction whose completeness may trail its applied cursor.
    pub fn bounded(
        cursor: Cursor,
        horizon: Horizon,
        payload: serde_json::Value,
    ) -> Result<Self, Refusal> {
        if horizon.position() > cursor.position() {
            return Err(Refusal::HorizonPastCursor { cursor, horizon });
        }
        Ok(Self {
            cursor,
            horizon,
            payload,
        })
    }
}

/// One complete, immutable fold generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Materialization {
    pub fold: FoldId,
    pub generation: u64,
    pub reducer_version: u32,
    pub cursor: Cursor,
    pub horizon: Horizon,
    payload: serde_json::Value,
    payload_bytes: Vec<u8>,
}

impl Materialization {
    /// Decoded fold answer.
    #[must_use]
    pub const fn payload(&self) -> &serde_json::Value {
        &self.payload
    }

    /// Canonical bytes written by the reducer.
    #[must_use]
    pub fn payload_bytes(&self) -> &[u8] {
        &self.payload_bytes
    }
}

/// Why the store had to derive a replacement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RebuildCause {
    Missing,
    Corrupt,
    Stale,
}

/// A fold read and whether it healed storage first.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadOutcome {
    pub materialization: Materialization,
    pub rebuilt: Option<RebuildCause>,
}

/// Disposable SQLite materialization store.
#[derive(Debug)]
pub struct Store {
    connection: Connection,
}

impl Store {
    /// Opens or creates the disposable folds store.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::from_connection(Connection::open(path).map_err(StoreError::Sqlite)?)
    }

    /// Opens an isolated folds store.
    pub fn in_memory() -> Result<Self, StoreError> {
        Self::from_connection(Connection::open_in_memory().map_err(StoreError::Sqlite)?)
    }

    /// Reads the current complete generation, if one exists.
    pub fn read(&self, fold: FoldId) -> Result<Option<Materialization>, StoreError> {
        match inspect(&self.connection, fold)? {
            Inspection::Missing => Ok(None),
            Inspection::Corrupt(detail) => Err(StoreError::Corrupt { fold, detail }),
            Inspection::Present(materialization) => Ok(Some(materialization)),
        }
    }

    /// Reads or heals one fold from a read-only journal snapshot.
    pub fn read_or_rebuild<R: Reducer>(
        &mut self,
        journal: &journal::Journal,
        reducer: &R,
    ) -> Result<ReadOutcome, StoreError> {
        let facts = journal.fact_snapshot().map_err(StoreError::Journal)?;
        self.read_or_rebuild_snapshot(&facts, reducer)
    }

    /// Reads or heals one fold from an already validated snapshot.
    pub fn read_or_rebuild_snapshot<R: Reducer>(
        &mut self,
        facts: &journal::FactSnapshot,
        reducer: &R,
    ) -> Result<ReadOutcome, StoreError> {
        let fold = reducer.fold();
        if reducer.version() == 0 {
            return Err(StoreError::Refused(Refusal::ZeroReducerVersion { fold }));
        }
        let inspection = inspect(&self.connection, fold)?;
        if let Inspection::Present(current) = &inspection
            && is_fresh(current, facts, reducer.version())
        {
            return Ok(ReadOutcome {
                materialization: current.clone(),
                rebuilt: None,
            });
        }

        let cause = inspection.cause();
        let reduction = reducer
            .rebuild(facts)
            .map_err(|detail| StoreError::Reducer { fold, detail })?;
        admit_reduction(facts, &reduction).map_err(StoreError::Refused)?;
        let materialization = self.publish(reducer, reduction, cause)?;
        Ok(ReadOutcome {
            materialization,
            rebuilt: Some(cause),
        })
    }

    fn from_connection(connection: Connection) -> Result<Self, StoreError> {
        connection
            .execute_batch("PRAGMA foreign_keys = ON; PRAGMA trusted_schema = OFF;")
            .map_err(StoreError::Sqlite)?;
        baseline(&connection)?;
        Ok(Self { connection })
    }

    fn publish<R: Reducer>(
        &mut self,
        reducer: &R,
        reduction: Reduction,
        cause: RebuildCause,
    ) -> Result<Materialization, StoreError> {
        let fold = reducer.fold();
        let payload = wire::encode(&reduction.payload).map_err(|error| StoreError::Reducer {
            fold,
            detail: format!("reduction payload does not encode: {error}"),
        })?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StoreError::Sqlite)?;
        let generation = next_generation(&transaction, fold)?;
        enforce_monotone(&transaction, fold, reduction.cursor, cause)?;
        insert_generation(
            &transaction,
            fold,
            generation,
            reducer.version(),
            &reduction,
            &payload,
        )?;
        point_head(&transaction, fold, generation)?;
        delete_predecessors(&transaction, fold, generation)?;
        transaction.commit().map_err(StoreError::Sqlite)?;
        Ok(Materialization {
            fold,
            generation,
            reducer_version: reducer.version(),
            cursor: reduction.cursor,
            horizon: reduction.horizon,
            payload: reduction.payload,
            payload_bytes: payload,
        })
    }
}

fn is_fresh(current: &Materialization, facts: &journal::FactSnapshot, version: u32) -> bool {
    let required = facts.horizon().map_or_else(Horizon::empty, Horizon::at);
    current.reducer_version == version && current.horizon >= required
}

fn admit_reduction(facts: &journal::FactSnapshot, reduction: &Reduction) -> Result<(), Refusal> {
    if reduction.horizon.position() > reduction.cursor.position() {
        return Err(Refusal::HorizonPastCursor {
            cursor: reduction.cursor,
            horizon: reduction.horizon,
        });
    }
    let required = facts.horizon().map_or_else(Horizon::empty, Horizon::at);
    if reduction.horizon < required {
        return Err(Refusal::BehindSnapshot {
            produced: reduction.horizon,
            required,
        });
    }
    Ok(())
}

enum Inspection {
    Missing,
    Corrupt(String),
    Present(Materialization),
}

impl Inspection {
    const fn cause(&self) -> RebuildCause {
        match self {
            Self::Missing => RebuildCause::Missing,
            Self::Corrupt(_) => RebuildCause::Corrupt,
            Self::Present(_) => RebuildCause::Stale,
        }
    }
}

fn inspect(connection: &Connection, fold: FoldId) -> Result<Inspection, StoreError> {
    let row = connection
        .query_row(
            "SELECT g.generation, g.reducer_version, g.cursor, g.horizon, g.payload
             FROM fold_heads AS h
             JOIN fold_generations AS g
               ON g.fold = h.fold AND g.generation = h.generation
             WHERE h.fold = ?1",
            params![fold.as_str()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                ))
            },
        )
        .optional()
        .map_err(StoreError::Sqlite)?;
    let Some((generation, version, cursor, horizon, payload)) = row else {
        return Ok(Inspection::Missing);
    };
    Ok(decode_materialization(
        fold, generation, version, cursor, horizon, payload,
    ))
}

fn decode_materialization(
    fold: FoldId,
    generation: i64,
    version: i64,
    cursor: Option<i64>,
    horizon: Option<i64>,
    payload_bytes: Vec<u8>,
) -> Inspection {
    let Some(generation) = u64::try_from(generation).ok().filter(|value| *value > 0) else {
        return Inspection::Corrupt("generation is not a positive integer".to_owned());
    };
    let Some(reducer_version) = u32::try_from(version).ok().filter(|value| *value > 0) else {
        return Inspection::Corrupt("reducer version is not a positive integer".to_owned());
    };
    let Some(cursor) = decode_cursor(cursor) else {
        return Inspection::Corrupt("cursor is outside the wire position range".to_owned());
    };
    let Some(horizon) = decode_horizon(horizon) else {
        return Inspection::Corrupt("horizon is outside the wire position range".to_owned());
    };
    if horizon.position() > cursor.position() {
        return Inspection::Corrupt("horizon advances past the applied cursor".to_owned());
    }
    let Ok(payload) = wire::decode::<serde_json::Value>(&payload_bytes) else {
        return Inspection::Corrupt("payload does not decode".to_owned());
    };
    if wire::encode(&payload).ok().as_deref() != Some(payload_bytes.as_slice()) {
        return Inspection::Corrupt("payload is not canonical wire bytes".to_owned());
    }
    Inspection::Present(Materialization {
        fold,
        generation,
        reducer_version,
        cursor,
        horizon,
        payload,
        payload_bytes,
    })
}

fn decode_cursor(value: Option<i64>) -> Option<Cursor> {
    decode_position(value).map(|position| position.map_or_else(Cursor::empty, Cursor::at))
}

fn decode_horizon(value: Option<i64>) -> Option<Horizon> {
    decode_position(value).map(|position| position.map_or_else(Horizon::empty, Horizon::at))
}

fn decode_position(value: Option<i64>) -> Option<Option<wire::Seq>> {
    match value {
        None => Some(None),
        Some(value) => u64::try_from(value).ok().and_then(wire::Seq::new).map(Some),
    }
}

fn next_generation(connection: &Connection, fold: FoldId) -> Result<u64, StoreError> {
    let current = connection
        .query_row(
            "SELECT MAX(generation) FROM fold_generations WHERE fold = ?1",
            params![fold.as_str()],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(StoreError::Sqlite)?;
    let current = current.map_or(0, |value| value);
    let next = current
        .checked_add(1)
        .ok_or(StoreError::Refused(Refusal::GenerationExhausted { fold }))?;
    u64::try_from(next).map_err(|_| StoreError::Refused(Refusal::GenerationExhausted { fold }))
}

fn enforce_monotone(
    connection: &Connection,
    fold: FoldId,
    next: Cursor,
    cause: RebuildCause,
) -> Result<(), StoreError> {
    if cause == RebuildCause::Corrupt {
        return Ok(());
    }
    let current = connection
        .query_row(
            "SELECT g.cursor FROM fold_heads AS h JOIN fold_generations AS g
             ON g.fold = h.fold AND g.generation = h.generation WHERE h.fold = ?1",
            params![fold.as_str()],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()
        .map_err(StoreError::Sqlite)?
        .flatten();
    let current = decode_cursor(current).unwrap_or_else(Cursor::empty);
    if next < current {
        return Err(StoreError::Refused(Refusal::CursorMovedBackward {
            fold,
            current,
            next,
        }));
    }
    Ok(())
}

fn insert_generation(
    connection: &Connection,
    fold: FoldId,
    generation: u64,
    version: u32,
    reduction: &Reduction,
    payload: &[u8],
) -> Result<(), StoreError> {
    connection
        .execute(
            "INSERT INTO fold_generations(fold, generation, reducer_version, cursor, horizon, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                fold.as_str(),
                to_sql(generation, fold)?,
                i64::from(version),
                position_to_sql(reduction.cursor.position(), fold)?,
                position_to_sql(reduction.horizon.position(), fold)?,
                payload,
            ],
        )
        .map_err(StoreError::Sqlite)?;
    Ok(())
}

fn point_head(connection: &Connection, fold: FoldId, generation: u64) -> Result<(), StoreError> {
    connection
        .execute(
            "INSERT INTO fold_heads(fold, generation) VALUES (?1, ?2)
             ON CONFLICT(fold) DO UPDATE SET generation = excluded.generation",
            params![fold.as_str(), to_sql(generation, fold)?],
        )
        .map_err(StoreError::Sqlite)?;
    Ok(())
}

fn delete_predecessors(
    connection: &Connection,
    fold: FoldId,
    generation: u64,
) -> Result<(), StoreError> {
    connection
        .execute(
            "DELETE FROM fold_generations WHERE fold = ?1 AND generation <> ?2",
            params![fold.as_str(), to_sql(generation, fold)?],
        )
        .map_err(StoreError::Sqlite)?;
    Ok(())
}

fn position_to_sql(position: Option<wire::Seq>, fold: FoldId) -> Result<Option<i64>, StoreError> {
    position
        .map(|position| to_sql(position.get(), fold))
        .transpose()
}

fn to_sql(value: u64, fold: FoldId) -> Result<i64, StoreError> {
    i64::try_from(value)
        .map_err(|_| StoreError::Refused(Refusal::PositionOutOfRange { fold, value }))
}

fn baseline(connection: &Connection) -> Result<(), StoreError> {
    connection
        .execute_batch(BASELINE_SQL)
        .map_err(StoreError::Sqlite)?;
    let version: u32 = connection
        .query_row(
            "SELECT version FROM fold_schema WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(StoreError::Sqlite)?;
    if version != STORE_SCHEMA_VERSION {
        return Err(StoreError::SchemaVersion {
            found: version,
            current: STORE_SCHEMA_VERSION,
        });
    }
    Ok(())
}

/// A generation publication refusal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Refusal {
    HorizonPastCursor {
        cursor: Cursor,
        horizon: Horizon,
    },
    BehindSnapshot {
        produced: Horizon,
        required: Horizon,
    },
    CursorMovedBackward {
        fold: FoldId,
        current: Cursor,
        next: Cursor,
    },
    GenerationExhausted {
        fold: FoldId,
    },
    PositionOutOfRange {
        fold: FoldId,
        value: u64,
    },
    ZeroReducerVersion {
        fold: FoldId,
    },
}

/// A folds-store operation failure.
#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    Journal(journal::SnapshotError),
    Reducer { fold: FoldId, detail: String },
    Corrupt { fold: FoldId, detail: String },
    Refused(Refusal),
    SchemaVersion { found: u32, current: u32 },
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => {
                write!(formatter, "folds store SQLite operation failed: {error}")
            }
            Self::Journal(error) => {
                write!(formatter, "fold rebuild cannot read authority: {error}")
            }
            Self::Reducer { fold, detail } => {
                write!(formatter, "fold {fold} rebuild failed: {detail}")
            }
            Self::Corrupt { fold, detail } => write!(formatter, "fold {fold} is corrupt: {detail}"),
            Self::Refused(refusal) => write!(formatter, "fold publication refused: {refusal:?}"),
            Self::SchemaVersion { found, current } => write!(
                formatter,
                "folds store schema {found} is not this build's schema {current}"
            ),
        }
    }
}

impl Error for StoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::Journal(error) => Some(error),
            Self::Reducer { .. }
            | Self::Corrupt { .. }
            | Self::Refused(_)
            | Self::SchemaVersion { .. } => None,
        }
    }
}

const BASELINE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS fold_schema (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    version INTEGER NOT NULL CHECK (version > 0)
) STRICT;
INSERT OR IGNORE INTO fold_schema(singleton, version) VALUES (1, 1);

CREATE TABLE IF NOT EXISTS fold_generations (
    fold TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK (generation > 0),
    reducer_version INTEGER NOT NULL CHECK (reducer_version > 0),
    cursor INTEGER CHECK (cursor > 0),
    horizon INTEGER CHECK (horizon > 0),
    payload BLOB NOT NULL,
    PRIMARY KEY (fold, generation),
    CHECK (horizon IS NULL OR (cursor IS NOT NULL AND horizon <= cursor))
) STRICT;

CREATE TABLE IF NOT EXISTS fold_heads (
    fold TEXT PRIMARY KEY,
    generation INTEGER NOT NULL,
    FOREIGN KEY (fold, generation)
        REFERENCES fold_generations(fold, generation) ON DELETE RESTRICT
) STRICT;
"#;
