//! Schema 30 to 31 source qualification of retained lane sessions.

use std::collections::BTreeMap;

use rusqlite::{Connection, params};
use serde_json::{Value, json};
use wire::{AnyFact, CanonicalUuid, Envelope, body};

use super::{MigrationError, OpenError};

const FROM: u32 = 30;
const TO: u32 = 31;
type Position = (u64, u64);
type Stream = (CanonicalUuid, String);
type Pending = BTreeMap<Position, Vec<usize>>;

#[derive(Clone, Copy)]
struct Batch {
    source: CanonicalUuid,
    from: Position,
    to: Position,
}

impl Batch {
    fn spans(self, position: Position) -> bool {
        self.from <= position && position < self.to
    }
}

struct Declaration {
    seq: i64,
    stream: Stream,
    candidate: Option<CanonicalUuid>,
    source: Option<CanonicalUuid>,
    bytes: Vec<u8>,
}

pub(super) fn migrate(connection: &Connection) -> Result<(), OpenError> {
    let rows = rows(connection)?;
    let mut declarations = Vec::<Declaration>::new();
    let mut pending = BTreeMap::<Stream, Pending>::new();
    let mut current = BTreeMap::<Stream, Batch>::new();

    for (seq, indexed_kind, bytes) in rows {
        let row = seq.to_string();
        let envelope: Envelope<AnyFact> = wire::decode(&bytes).map_err(|error| {
            invalid(
                &row,
                &format!("schema-30 envelope is not current-compatible wire: {error}"),
            )
        })?;
        if envelope.seq.get() != u64::try_from(seq).unwrap_or_default()
            || envelope.fact.kind().as_str() != indexed_kind
        {
            return Err(invalid(
                &row,
                "stored sequence or kind disagrees with its envelope",
            ));
        }

        if let Some(session) = envelope.fact.read::<body::LaneSessionDeclared>() {
            if session.source.is_some() {
                continue;
            }
            let stream = (session.lane, session.vendor.to_string());
            let position = (session.member, session.offset);
            let candidate = current
                .get(&stream)
                .copied()
                .filter(|batch| batch.spans(position))
                .map(|batch| batch.source);
            let index = declarations.len();
            declarations.push(Declaration {
                seq,
                stream: stream.clone(),
                candidate,
                source: None,
                bytes,
            });
            pending
                .entry(stream)
                .or_default()
                .entry(position)
                .or_default()
                .push(index);
        } else if let Some(batch) = envelope.fact.read::<body::JournalIngested>() {
            let stream = (batch.lane, batch.vendor.to_string());
            let observed = Batch {
                source: batch.source,
                from: (batch.from.member, batch.from.offset),
                to: (batch.to.member, batch.to.offset),
            };
            if observed.from >= observed.to {
                return Err(invalid(
                    &row,
                    "transcript ingestion span is empty or reversed",
                ));
            }
            let continued = current
                .get(&stream)
                .is_some_and(|prior| prior.source == observed.source);
            bind(
                &stream,
                observed,
                continued,
                &mut pending,
                &mut declarations,
            );
            current.insert(stream, observed);
        }
    }

    for declarations_at in pending.values().flat_map(BTreeMap::values) {
        for &index in declarations_at {
            let declaration = &mut declarations[index];
            declaration.source = declaration.candidate.filter(|candidate| {
                current
                    .get(&declaration.stream)
                    .is_some_and(|batch| batch.source == *candidate)
            });
            if declaration.source.is_none() {
                return Err(invalid(
                    &declaration.seq.to_string(),
                    "lane session has no uniquely recoverable tape generation; use the release that owns schema 30 to advance this transcript before upgrading",
                ));
            }
        }
    }

    for declaration in declarations {
        let source = declaration
            .source
            .expect("every retained declaration was qualified before rewriting");
        let canonical = qualify(&declaration, source)?;
        connection
            .execute(
                "UPDATE fact_envelopes SET envelope = ?1 WHERE seq = ?2",
                params![canonical, declaration.seq],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn bind(
    stream: &Stream,
    batch: Batch,
    continued: bool,
    pending: &mut BTreeMap<Stream, Pending>,
    declarations: &mut [Declaration],
) {
    let Some(sessions) = pending.get_mut(stream) else {
        return;
    };
    while let Some((&position, _)) = if continued {
        sessions.range(..batch.to).next()
    } else {
        sessions.range(batch.from..batch.to).next()
    } {
        let indexes = sessions
            .remove(&position)
            .expect("observed pending position");
        for index in indexes {
            declarations[index].source = Some(batch.source);
        }
    }
    if sessions.is_empty() {
        pending.remove(stream);
    }
}

fn qualify(declaration: &Declaration, source: CanonicalUuid) -> Result<Vec<u8>, OpenError> {
    let row = declaration.seq.to_string();
    let mut document: Value = serde_json::from_slice(&declaration.bytes)
        .map_err(|error| invalid(&row, &format!("stored envelope JSON is invalid: {error}")))?;
    let fact = document
        .get_mut("fact")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| invalid(&row, "stored envelope fact is not an object"))?;
    if fact
        .insert("source".to_owned(), json!(source.to_string()))
        .is_some()
    {
        return Err(invalid(
            &row,
            "schema-30 lane session already carries a source field",
        ));
    }
    let intermediate = super::team::current(&document)
        .map_err(|error| invalid(&row, &format!("migrated JSON cannot encode: {error}")))?;
    let envelope: Envelope<AnyFact> = wire::decode(&intermediate).map_err(|error| {
        invalid(
            &row,
            &format!("migrated envelope is not current fact wire: {error}"),
        )
    })?;
    let session = envelope
        .fact
        .read::<body::LaneSessionDeclared>()
        .ok_or_else(|| invalid(&row, "migrated envelope is not a lane session declaration"))?;
    if envelope.seq.get() != u64::try_from(declaration.seq).unwrap_or_default()
        || session.source != Some(source)
    {
        return Err(invalid(
            &row,
            "migrated declaration lost its sequence or tape generation",
        ));
    }
    wire::encode(&envelope).map_err(|error| {
        invalid(
            &row,
            &format!("migrated envelope cannot encode canonically: {error}"),
        )
    })
}

fn rows(connection: &Connection) -> Result<Vec<(i64, String, Vec<u8>)>, OpenError> {
    let mut statement = connection
        .prepare(
            "SELECT seq, kind, envelope FROM fact_envelopes \
             WHERE kind IN ('lane::declared', 'journal::ingested') ORDER BY seq",
        )
        .map_err(OpenError::Sqlite)?;
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(OpenError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)
}

fn invalid(row: &str, detail: &str) -> OpenError {
    MigrationError::StoredContent {
        from: FROM,
        to: TO,
        table: "fact_envelopes",
        row: row.to_owned(),
        detail: detail.to_owned(),
    }
    .into()
}

#[cfg(test)]
mod tests;
