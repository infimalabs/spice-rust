//! Schema 28 to 29: the task generation index.
//!
//! Every later transaction that advances a task's generation records the
//! position of its last fact mutating that task. Schema 28 already advanced
//! generations without this record, so a task it left above zero receives one
//! row flagged `baseline`, naming its current generation and the last fact
//! that mutated it; the generations below that row predate the index, and the
//! flag is what lets verification tell it from a lost row. An authority that
//! crossed from schema 27 directly holds every task at generation zero and
//! gains an empty index. No head or fact is rewritten.

use std::collections::BTreeMap;

use rusqlite::{Connection, params};

use super::{MigrationError, OpenError};

const FROM: u32 = 28;
const TO: u32 = 29;

pub(super) fn migrate(connection: &Connection) -> Result<(), OpenError> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS task_generations (
                 stamp TEXT NOT NULL REFERENCES task_heads(stamp),
                 generation INTEGER NOT NULL CHECK (generation > 0),
                 seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
                 baseline INTEGER NOT NULL DEFAULT 0 CHECK (baseline IN (0, 1)),
                 PRIMARY KEY (stamp, generation)
             ) STRICT, WITHOUT ROWID;",
        )
        .map_err(OpenError::Sqlite)?;
    let mut advanced = advanced_tasks(connection)?;
    if advanced.is_empty() {
        return Ok(());
    }
    let mut facts = connection
        .prepare("SELECT seq, envelope FROM fact_envelopes ORDER BY seq DESC")
        .map_err(OpenError::Sqlite)?;
    let mut rows = facts.query([]).map_err(OpenError::Sqlite)?;
    while let Some(row) = rows.next().map_err(OpenError::Sqlite)? {
        let seq: i64 = row.get(0).map_err(OpenError::Sqlite)?;
        let bytes: Vec<u8> = row.get(1).map_err(OpenError::Sqlite)?;
        let envelope = wire::decode::<wire::Envelope<wire::AnyFact>>(&bytes).map_err(|error| {
            invalid(
                "fact_envelopes",
                &seq.to_string(),
                &format!("fact does not decode: {error}"),
            )
        })?;
        let Some(task) = envelope.fact.mutated_task() else {
            continue;
        };
        let Some(generation) = advanced.remove(task.as_str()) else {
            continue;
        };
        connection
            .execute(
                "INSERT INTO task_generations(stamp, generation, seq, baseline) \
                 VALUES (?1, ?2, ?3, 1)",
                params![task.as_str(), generation, seq],
            )
            .map_err(OpenError::Sqlite)?;
        if advanced.is_empty() {
            return Ok(());
        }
    }
    let (stamp, _) = advanced.pop_first().expect("an unmatched task remains");
    Err(invalid(
        "workflow_heads",
        &stamp,
        "an advanced generation has no fact that mutated its task",
    ))
}

/// Every task schema 28 left above generation zero without an index row,
/// with that generation.
fn advanced_tasks(connection: &Connection) -> Result<BTreeMap<String, i64>, OpenError> {
    let mut statement = connection
        .prepare(
            "SELECT stamp, body FROM workflow_heads AS workflow WHERE NOT EXISTS \
             (SELECT 1 FROM task_generations AS indexed WHERE indexed.stamp = workflow.stamp)",
        )
        .map_err(OpenError::Sqlite)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(OpenError::Sqlite)?;
    let mut advanced = BTreeMap::new();
    for row in rows {
        let (stamp, body) = row.map_err(OpenError::Sqlite)?;
        let head = wire::decode::<wire::WorkflowHead>(&body).map_err(|error| {
            invalid(
                "workflow_heads",
                &stamp,
                &format!("workflow head is invalid: {error}"),
            )
        })?;
        if head.generation > 0 {
            let generation = i64::try_from(head.generation).map_err(|_| {
                invalid(
                    "workflow_heads",
                    &stamp,
                    "generation exceeds SQLite integer range",
                )
            })?;
            advanced.insert(stamp, generation);
        }
    }
    Ok(advanced)
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
