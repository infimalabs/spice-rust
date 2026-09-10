//! Schema 12 to 13 task creation transcript provenance.
//!
//! Available context is reconstructed only from immutable facts preceding the
//! task creation fact. Current heads, timestamps, filesystem metadata, and
//! configured windows are deliberately absent from this migration.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{Connection, params};
use serde_json::{Map, Value};
use wire::body::{
    JournalIngested, LaneCancelled, LaneDrained, LaneExited, LaneKilled, LaneRemoved, LaneRenewed,
    LaneSessionDeclared, LaneSignalled, LaneStarted, LaneTimedOut, TaskAdded, TaskCreationContext,
    TaskCreationUnavailable, TranscriptPosition,
};
use wire::{Actor, AnyFact, CanonicalUuid, Envelope, FactKind, TaskStamp, TranscriptSourceHead};

use super::{MigrationError, OpenError};

const FROM: u32 = 12;
const TO: u32 = 13;
const ORIGIN: TranscriptPosition = TranscriptPosition {
    member: 1,
    offset: 0,
};

pub(super) fn migrate(connection: &Connection) -> Result<(), OpenError> {
    let contexts = migrate_facts(connection)?;
    migrate_task_heads(connection, &contexts)
}

#[derive(Default)]
struct History {
    occupants: BTreeMap<CanonicalUuid, CanonicalUuid>,
    sessions: BTreeMap<CanonicalUuid, LaneSessionDeclared>,
    sources: BTreeMap<String, TranscriptSourceHead>,
    invalid_sources: BTreeSet<String>,
}

fn migrate_facts(
    connection: &Connection,
) -> Result<BTreeMap<TaskStamp, TaskCreationContext>, OpenError> {
    let rows = fact_rows(connection)?;
    let mut history = History::default();
    let mut contexts = BTreeMap::new();
    for (seq, indexed_kind, bytes) in rows {
        let row = seq.to_string();
        let envelope: Envelope<AnyFact> = wire::decode(&bytes).map_err(|error| {
            invalid(
                "fact_envelopes",
                &row,
                &format!("legacy envelope is not current-compatible wire: {error}"),
            )
        })?;
        if envelope.seq.get() != u64::try_from(seq).unwrap_or_default()
            || envelope.fact.kind().as_str() != indexed_kind
        {
            return Err(invalid(
                "fact_envelopes",
                &row,
                "indexed fact identity disagrees with its envelope",
            ));
        }
        match envelope.fact.kind() {
            FactKind::TaskAdded => {
                let added = envelope
                    .fact
                    .read::<TaskAdded>()
                    .expect("registered task-added fact decodes");
                let context = history.creation_context(&envelope.actor);
                if contexts.insert(added.task, context.clone()).is_some() {
                    return Err(invalid(
                        "fact_envelopes",
                        &row,
                        "task has more than one creation fact",
                    ));
                }
                rewrite_task_added(connection, seq, &row, &bytes, &context)?;
            }
            FactKind::LaneSessionDeclared => {
                let declared = envelope
                    .fact
                    .read::<LaneSessionDeclared>()
                    .expect("registered lane session fact decodes");
                history.sessions.insert(declared.lane, declared);
            }
            FactKind::LaneStarted => {
                let started = envelope
                    .fact
                    .read::<LaneStarted>()
                    .expect("registered lane-started fact decodes");
                history.start(started.lane, started.occupant);
            }
            FactKind::LaneRenewed => {
                let renewed = envelope
                    .fact
                    .read::<LaneRenewed>()
                    .expect("registered lane-renewed fact decodes");
                history.start(renewed.lane, renewed.occupant);
            }
            FactKind::LaneExited
            | FactKind::LaneTimedOut
            | FactKind::LaneCancelled
            | FactKind::LaneSignalled
            | FactKind::LaneDrained
            | FactKind::LaneKilled => history.stop_fact(&envelope.fact),
            FactKind::LaneRemoved => {
                let removed = envelope
                    .fact
                    .read::<LaneRemoved>()
                    .expect("registered lane-removed fact decodes");
                history.occupants.remove(&removed.lane);
                history.sessions.remove(&removed.lane);
            }
            FactKind::JournalIngested => {
                let ingested = envelope
                    .fact
                    .read::<JournalIngested>()
                    .expect("registered journal-ingested fact decodes");
                history.ingest(ingested);
            }
            _ => {}
        }
    }
    Ok(contexts)
}

impl History {
    fn stop_fact(&mut self, fact: &AnyFact) {
        let (lane, occupant) = match fact.kind() {
            FactKind::LaneExited => {
                let body = fact
                    .read::<LaneExited>()
                    .expect("registered lane-exited fact decodes");
                (body.lane, body.occupant)
            }
            FactKind::LaneTimedOut => {
                let body = fact
                    .read::<LaneTimedOut>()
                    .expect("registered lane-timedout fact decodes");
                (body.lane, body.occupant)
            }
            FactKind::LaneCancelled => {
                let body = fact
                    .read::<LaneCancelled>()
                    .expect("registered lane-cancelled fact decodes");
                (body.lane, body.occupant)
            }
            FactKind::LaneSignalled => {
                let body = fact
                    .read::<LaneSignalled>()
                    .expect("registered lane-signalled fact decodes");
                (body.lane, body.occupant)
            }
            FactKind::LaneDrained => {
                let body = fact
                    .read::<LaneDrained>()
                    .expect("registered lane-drained fact decodes");
                (body.lane, body.occupant)
            }
            FactKind::LaneKilled => {
                let body = fact
                    .read::<LaneKilled>()
                    .expect("registered lane-killed fact decodes");
                (body.lane, body.occupant)
            }
            _ => unreachable!("stop_fact is called only for lane terminal facts"),
        };
        self.stop(lane, occupant);
    }

    fn start(&mut self, lane: CanonicalUuid, occupant: CanonicalUuid) {
        self.occupants.insert(lane, occupant);
        if self
            .sessions
            .get(&lane)
            .is_some_and(|session| session.occupant != occupant)
        {
            self.sessions.remove(&lane);
        }
    }

    fn stop(&mut self, lane: CanonicalUuid, occupant: CanonicalUuid) {
        if self.occupants.get(&lane) == Some(&occupant) {
            self.occupants.remove(&lane);
            self.sessions.remove(&lane);
        }
    }

    fn ingest(&mut self, batch: JournalIngested) {
        let head = TranscriptSourceHead {
            lane: batch.lane,
            vendor: batch.vendor,
            stream: batch.stream,
            source: batch.source,
            next: batch.to,
        };
        let key = head.key().into_string();
        let valid = self.sources.get(&key).map_or_else(
            || batch.from == ORIGIN && batch.supersedes.is_none(),
            |prior| {
                if prior.source == batch.source {
                    batch.supersedes.is_none() && batch.from == prior.next
                } else {
                    batch.supersedes == Some(prior.source) && batch.from == ORIGIN
                }
            },
        ) && position(&batch.to) > position(&batch.from);
        if valid && !self.invalid_sources.contains(&key) {
            self.sources.insert(key, head);
        } else {
            self.sources.remove(&key);
            self.invalid_sources.insert(key);
        }
    }

    fn creation_context(&self, actor: &Actor) -> TaskCreationContext {
        let lane = match actor {
            Actor::Operator => {
                return TaskCreationContext::unavailable(TaskCreationUnavailable::OperatorActor);
            }
            Actor::Spice => {
                return TaskCreationContext::unavailable(TaskCreationUnavailable::SpiceActor);
            }
            Actor::Lane(lane) => lane,
        };
        let Some(occupant) = self.occupants.get(lane) else {
            return unavailable();
        };
        let Some(session) = self
            .sessions
            .get(lane)
            .filter(|session| &session.occupant == occupant && session.lane == *lane)
        else {
            return unavailable();
        };
        let matching = self
            .sources
            .values()
            .filter(|head| head.lane == *lane && head.vendor == session.vendor)
            .collect::<Vec<_>>();
        let registered = matching
            .iter()
            .filter(|head| head.stream == wire::body::TASK_CREATION_CONTEXT_STREAM)
            .copied()
            .collect::<Vec<_>>();
        if registered.is_empty() && !matching.is_empty() {
            return TaskCreationContext::unavailable(
                TaskCreationUnavailable::LegacyStreamUnregistered,
            );
        }
        let [boundary] = registered.as_slice() else {
            return unavailable();
        };
        TaskCreationContext::Transcript {
            session: session.identity.clone(),
            boundary: (**boundary).clone(),
            creator_lineage: None,
        }
    }
}

fn unavailable() -> TaskCreationContext {
    TaskCreationContext::unavailable(TaskCreationUnavailable::LegacyEvidenceMissing)
}

fn position(position: &TranscriptPosition) -> (u64, u64) {
    (position.member, position.offset)
}

fn rewrite_task_added(
    connection: &Connection,
    seq: i64,
    row: &str,
    bytes: &[u8],
    context: &TaskCreationContext,
) -> Result<(), OpenError> {
    let mut document = decode("fact_envelopes", row, bytes)?;
    let envelope = object("fact_envelopes", row, &mut document)?;
    let fact = envelope
        .get_mut("fact")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| invalid("fact_envelopes", row, "envelope fact is not an object"))?;
    insert_or_match("fact_envelopes", row, fact, "creation", context)?;
    let canonical = canonical_envelope(row, &document)?;
    connection
        .execute(
            "UPDATE fact_envelopes SET envelope = ?1 WHERE seq = ?2",
            params![canonical, seq],
        )
        .map_err(OpenError::Sqlite)?;
    Ok(())
}

fn migrate_task_heads(
    connection: &Connection,
    contexts: &BTreeMap<TaskStamp, TaskCreationContext>,
) -> Result<(), OpenError> {
    let rows = task_head_rows(connection)?;
    for (stamp, bytes) in rows {
        let task = stamp.parse::<TaskStamp>().map_err(|error| {
            invalid(
                "task_heads",
                &stamp,
                &format!("task head key is invalid: {error}"),
            )
        })?;
        let context = contexts.get(&task).ok_or_else(|| {
            invalid(
                "task_heads",
                &stamp,
                "task head has no immutable creation fact",
            )
        })?;
        let mut document = decode("task_heads", &stamp, &bytes)?;
        let body = object("task_heads", &stamp, &mut document)?;
        insert_or_match("task_heads", &stamp, body, "creation", context)?;
        let intermediate = serde_json::to_vec(&document).map_err(|error| {
            invalid(
                "task_heads",
                &stamp,
                &format!("migrated task head cannot encode: {error}"),
            )
        })?;
        let head: wire::TaskHead = wire::decode(&intermediate).map_err(|error| {
            invalid(
                "task_heads",
                &stamp,
                &format!("migrated task head is not current wire: {error}"),
            )
        })?;
        let canonical = wire::encode(&head).map_err(|error| {
            invalid(
                "task_heads",
                &stamp,
                &format!("migrated task head cannot encode canonically: {error}"),
            )
        })?;
        connection
            .execute(
                "UPDATE task_heads SET body = ?1 WHERE stamp = ?2",
                params![canonical, stamp],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn canonical_envelope(row: &str, document: &Value) -> Result<Vec<u8>, OpenError> {
    let intermediate = serde_json::to_vec(document).map_err(|error| {
        invalid(
            "fact_envelopes",
            row,
            &format!("migrated envelope cannot encode: {error}"),
        )
    })?;
    let envelope: Envelope<AnyFact> = wire::decode(&intermediate).map_err(|error| {
        invalid(
            "fact_envelopes",
            row,
            &format!("migrated envelope is not current wire: {error}"),
        )
    })?;
    wire::encode(&envelope).map_err(|error| {
        invalid(
            "fact_envelopes",
            row,
            &format!("migrated envelope cannot encode canonically: {error}"),
        )
    })
}

fn insert_or_match(
    table: &'static str,
    row: &str,
    object: &mut Map<String, Value>,
    field: &str,
    context: &TaskCreationContext,
) -> Result<(), OpenError> {
    let value = serde_json::to_value(context).expect("task creation context serializes");
    if let Some(existing) = object.get(field) {
        if existing == &value {
            return Ok(());
        }
        return Err(invalid(
            table,
            row,
            &format!("stored {field} disagrees with immutable creation history"),
        ));
    }
    object.insert(field.to_owned(), value);
    Ok(())
}

fn fact_rows(connection: &Connection) -> Result<Vec<(i64, String, Vec<u8>)>, OpenError> {
    let mut statement = connection
        .prepare("SELECT seq, kind, envelope FROM fact_envelopes ORDER BY seq")
        .map_err(OpenError::Sqlite)?;
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(OpenError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)
}

fn task_head_rows(connection: &Connection) -> Result<Vec<(String, Vec<u8>)>, OpenError> {
    let mut statement = connection
        .prepare("SELECT stamp, body FROM task_heads ORDER BY stamp")
        .map_err(OpenError::Sqlite)?;
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(OpenError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)
}

fn decode(table: &'static str, row: &str, bytes: &[u8]) -> Result<Value, OpenError> {
    serde_json::from_slice(bytes)
        .map_err(|error| invalid(table, row, &format!("stored JSON is invalid: {error}")))
}

fn object<'a>(
    table: &'static str,
    row: &str,
    value: &'a mut Value,
) -> Result<&'a mut Map<String, Value>, OpenError> {
    value
        .as_object_mut()
        .ok_or_else(|| invalid(table, row, "stored body is not an object"))
}

fn invalid(table: &'static str, row: &str, detail: &str) -> OpenError {
    MigrationError::StoredContent {
        from: FROM,
        to: TO,
        table,
        row: row.to_owned(),
        detail: detail.to_owned(),
    }
    .into()
}
