use std::error::Error;
use std::fmt;

use rusqlite::Connection;

use crate::{Journal, JournalReader};

/// One immutable, contiguous view of the journal fact stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FactSnapshot {
    facts: Vec<wire::Envelope<wire::AnyFact>>,
    horizon: Option<wire::Seq>,
}

impl FactSnapshot {
    /// Validates a fact sequence for use by a disposable materialization.
    pub fn from_facts(facts: Vec<wire::Envelope<wire::AnyFact>>) -> Result<Self, SnapshotError> {
        let mut expected = 1_u64;
        for fact in &facts {
            if !fact.v.is_current() {
                return Err(SnapshotError::Invalid {
                    seq: Some(fact.seq.get()),
                    detail: "fact snapshot contains another wire version".to_owned(),
                });
            }
            if fact.seq.get() != expected {
                return Err(SnapshotError::Invalid {
                    seq: Some(fact.seq.get()),
                    detail: format!("expected contiguous seq {expected}"),
                });
            }
            expected = expected.saturating_add(1);
        }
        let horizon = facts.last().map(|fact| fact.seq);
        Ok(Self { facts, horizon })
    }

    /// Facts in ascending journal order.
    #[must_use]
    pub fn facts(&self) -> &[wire::Envelope<wire::AnyFact>] {
        &self.facts
    }

    /// Greatest fact position visible in this snapshot.
    #[must_use]
    pub const fn horizon(&self) -> Option<wire::Seq> {
        self.horizon
    }
}

impl Journal {
    /// Reads a canonical, contiguous fact snapshot without mutating authority.
    pub fn fact_snapshot(&self) -> Result<FactSnapshot, SnapshotError> {
        read_snapshot(&self.connection)
    }
}

impl JournalReader {
    /// Reads the canonical authority stream without writer capability,
    /// creation, or migration.
    pub fn fact_snapshot(&self) -> Result<FactSnapshot, SnapshotError> {
        read_snapshot(&self.connection)
    }
}

pub(crate) fn read_snapshot(connection: &Connection) -> Result<FactSnapshot, SnapshotError> {
    let mut statement = connection
        .prepare("SELECT seq, kind, envelope FROM fact_envelopes ORDER BY seq")
        .map_err(SnapshotError::Sqlite)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .map_err(SnapshotError::Sqlite)?;
    let mut facts = Vec::new();
    for row in rows {
        let (indexed_seq, indexed_kind, bytes) = row.map_err(SnapshotError::Sqlite)?;
        facts.push(decode_fact(indexed_seq, &indexed_kind, &bytes)?);
    }
    FactSnapshot::from_facts(facts)
}

pub(crate) fn decode_fact(
    indexed_seq: i64,
    indexed_kind: &str,
    bytes: &[u8],
) -> Result<wire::Envelope<wire::AnyFact>, SnapshotError> {
    let seq = u64::try_from(indexed_seq).ok();
    let fact = wire::decode::<wire::Envelope<wire::AnyFact>>(bytes).map_err(|error| {
        SnapshotError::Invalid {
            seq,
            detail: format!("fact envelope does not decode: {error}"),
        }
    })?;
    let encoded = wire::encode(&fact).map_err(|error| SnapshotError::Invalid {
        seq,
        detail: format!("fact envelope does not re-encode: {error}"),
    })?;
    if encoded != bytes
        || seq != Some(fact.seq.get())
        || indexed_kind != fact.fact.kind().as_str()
        || !fact.v.is_current()
    {
        return Err(SnapshotError::Invalid {
            seq,
            detail: "fact envelope is noncanonical or disagrees with indexed columns".to_owned(),
        });
    }
    Ok(fact)
}

/// A read-only fact snapshot failure.
#[derive(Debug)]
pub enum SnapshotError {
    /// SQLite could not produce the snapshot.
    Sqlite(rusqlite::Error),
    /// A stored fact was not a canonical member of one contiguous stream.
    Invalid { seq: Option<u64>, detail: String },
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(formatter, "journal fact snapshot failed: {error}"),
            Self::Invalid { seq, detail } => {
                write!(formatter, "journal fact {seq:?} is invalid: {detail}")
            }
        }
    }
}

impl Error for SnapshotError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::Invalid { .. } => None,
        }
    }
}
