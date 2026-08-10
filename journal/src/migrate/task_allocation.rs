//! Schema 3 to 4 backfill of durable allocator evidence on task and claim heads.
//!
//! Queue origin is the one `task::added` fact position. Claim origin is the
//! most recent `claim::won` position, while the legacy cell is reconstructed
//! from the task's current project and phase. Every claim minted after this
//! migration records the exact claim-time cell in both its fact and head.

use std::collections::BTreeMap;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{MigrationError, OpenError};

const FROM: u32 = 3;
const TO: u32 = 4;

type QueueOrigins = BTreeMap<wire::TaskHandle, wire::Seq>;
type ClaimOrigins = BTreeMap<wire::TaskHandle, (wire::Seq, wire::Actor, Option<wire::ClaimCell>)>;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AllocatedTaskHead {
    handle: wire::TaskHandle,
    uuid: wire::CanonicalUuid,
    queue_origin: wire::Seq,
    title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    acceptance: Vec<String>,
    project: String,
    tags: Vec<String>,
    dependencies: Vec<wire::TaskHandle>,
    provenance: wire::Origin,
    revision: wire::Seq,
}

pub(super) fn migrate(connection: &Connection) -> Result<(), OpenError> {
    let (created, claimed) = allocation_facts(connection)?;
    migrate_task_heads(connection, &created)?;
    migrate_claim_heads(connection, &claimed)
}

fn allocation_facts(connection: &Connection) -> Result<(QueueOrigins, ClaimOrigins), OpenError> {
    let rows = {
        let mut statement = connection
            .prepare(
                "SELECT seq, kind, envelope FROM fact_envelopes \
                 WHERE kind IN ('task::added', 'claim::won') ORDER BY seq",
            )
            .map_err(OpenError::Sqlite)?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(OpenError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };
    let mut created = BTreeMap::new();
    let mut claimed = BTreeMap::new();
    for (position, indexed_kind, bytes) in rows {
        let row = position.to_string();
        let document = decode("fact_envelopes", &row, &bytes)?;
        let fact = document
            .get("fact")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("fact_envelopes", &row, "envelope.fact is not an object"))?;
        let kind = fact
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("fact_envelopes", &row, "allocator fact has no kind"))?;
        if kind != indexed_kind {
            return Err(invalid(
                "fact_envelopes",
                &row,
                "indexed kind disagrees with envelope.fact.kind",
            ));
        }
        let task = fact
            .get("task")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("fact_envelopes", &row, "allocator fact has no task"))?
            .parse::<wire::TaskHandle>()
            .map_err(|error| {
                invalid(
                    "fact_envelopes",
                    &row,
                    &format!("allocator fact task is invalid: {error}"),
                )
            })?;
        let position = u64::try_from(position)
            .ok()
            .and_then(wire::Seq::new)
            .ok_or_else(|| invalid("fact_envelopes", &row, "fact sequence is not positive"))?;
        match kind {
            "task::added" => {
                if created.insert(task, position).is_some() {
                    return Err(invalid(
                        "fact_envelopes",
                        &row,
                        "task has more than one task::added fact",
                    ));
                }
            }
            "claim::won" => {
                let actor = value_field::<wire::Actor>(fact, "actor", &row)?;
                let cell = optional_value_field::<wire::ClaimCell>(fact, "cell", &row)?;
                claimed.insert(task, (position, actor, cell));
            }
            _ => unreachable!("SQL restricts allocator fact kinds"),
        }
    }
    Ok((created, claimed))
}

fn migrate_task_heads(connection: &Connection, created: &QueueOrigins) -> Result<(), OpenError> {
    let rows = text_rows(connection, "task_heads")?;
    for (handle, bytes) in rows {
        let task = handle.parse::<wire::TaskHandle>().map_err(|error| {
            invalid(
                "task_heads",
                &handle,
                &format!("head key is invalid: {error}"),
            )
        })?;
        let queue_origin = created.get(&task).copied().ok_or_else(|| {
            invalid(
                "task_heads",
                &handle,
                "head has no task::added fact from which to derive queue_origin",
            )
        })?;
        let mut document = decode("task_heads", &handle, &bytes)?;
        let object = object("task_heads", &handle, &mut document)?;
        insert_or_match(
            "task_heads",
            &handle,
            object,
            "queue_origin",
            serde_json::to_value(queue_origin).expect("Seq serializes"),
        )?;
        let encoded = current_head::<AllocatedTaskHead>("task_heads", &handle, &document)?;
        connection
            .execute(
                "UPDATE task_heads SET body = ?1 WHERE handle = ?2",
                params![encoded, handle],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn migrate_claim_heads(connection: &Connection, claimed: &ClaimOrigins) -> Result<(), OpenError> {
    let current_cells = current_cells(connection)?;
    let rows = text_rows(connection, "claim_heads")?;
    for (handle, bytes) in rows {
        let task = handle.parse::<wire::TaskHandle>().map_err(|error| {
            invalid(
                "claim_heads",
                &handle,
                &format!("head key is invalid: {error}"),
            )
        })?;
        let (claim_origin, actor, recorded_cell) = claimed.get(&task).ok_or_else(|| {
            invalid(
                "claim_heads",
                &handle,
                "head has no claim::won fact from which to derive claim origin",
            )
        })?;
        let mut document = decode("claim_heads", &handle, &bytes)?;
        let object = object("claim_heads", &handle, &mut document)?;
        let head_actor = object
            .get("actor")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("claim_heads", &handle, "head has no actor"))?;
        if head_actor != actor.to_string() {
            return Err(invalid(
                "claim_heads",
                &handle,
                "most recent claim::won actor disagrees with claim head",
            ));
        }
        let cell = recorded_cell
            .clone()
            .or_else(|| current_cells.get(&task).cloned())
            .ok_or_else(|| {
                invalid(
                    "claim_heads",
                    &handle,
                    "head has no current task cell for legacy claim migration",
                )
            })?;
        insert_or_match(
            "claim_heads",
            &handle,
            object,
            "claimed",
            serde_json::to_value(claim_origin).expect("Seq serializes"),
        )?;
        insert_or_match(
            "claim_heads",
            &handle,
            object,
            "cell",
            serde_json::to_value(cell).expect("ClaimCell serializes"),
        )?;
        let encoded = current_head::<wire::ClaimHead>("claim_heads", &handle, &document)?;
        connection
            .execute(
                "UPDATE claim_heads SET body = ?1 WHERE handle = ?2",
                params![encoded, handle],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn current_cells(
    connection: &Connection,
) -> Result<BTreeMap<wire::TaskHandle, wire::ClaimCell>, OpenError> {
    let rows = {
        let mut statement = connection
            .prepare(
                "SELECT task_heads.handle, task_heads.body, workflow_heads.body \
                 FROM task_heads JOIN workflow_heads USING(handle) ORDER BY task_heads.handle",
            )
            .map_err(OpenError::Sqlite)?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(OpenError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };
    rows.into_iter()
        .map(|(handle, statement, workflow)| {
            let statement = wire::decode::<AllocatedTaskHead>(&statement).map_err(|error| {
                invalid(
                    "task_heads",
                    &handle,
                    &format!("migrated task head is invalid: {error}"),
                )
            })?;
            let workflow = wire::decode::<wire::WorkflowHead>(&workflow).map_err(|error| {
                invalid(
                    "workflow_heads",
                    &handle,
                    &format!("workflow head is invalid: {error}"),
                )
            })?;
            Ok((
                statement.handle,
                wire::ClaimCell {
                    project: statement.project,
                    phase: workflow.phase,
                },
            ))
        })
        .collect()
}

fn value_field<T>(object: &Map<String, Value>, field: &str, row: &str) -> Result<T, OpenError>
where
    T: serde::de::DeserializeOwned,
{
    let value = object
        .get(field)
        .ok_or_else(|| invalid("fact_envelopes", row, &format!("claim fact has no {field}")))?;
    serde_json::from_value(value.clone()).map_err(|error| {
        invalid(
            "fact_envelopes",
            row,
            &format!("claim fact {field} is invalid: {error}"),
        )
    })
}

fn optional_value_field<T>(
    object: &Map<String, Value>,
    field: &str,
    row: &str,
) -> Result<Option<T>, OpenError>
where
    T: serde::de::DeserializeOwned,
{
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|error| {
                invalid(
                    "fact_envelopes",
                    row,
                    &format!("claim fact {field} is invalid: {error}"),
                )
            }),
    }
}

fn text_rows(
    connection: &Connection,
    table: &'static str,
) -> Result<Vec<(String, Vec<u8>)>, OpenError> {
    let sql = format!("SELECT handle, body FROM {table} ORDER BY handle");
    let mut statement = connection.prepare(&sql).map_err(OpenError::Sqlite)?;
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(OpenError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)
}

fn insert_or_match(
    table: &'static str,
    row: &str,
    object: &mut Map<String, Value>,
    field: &str,
    value: Value,
) -> Result<(), OpenError> {
    if let Some(existing) = object.get(field) {
        if existing == &value {
            return Ok(());
        }
        return Err(invalid(
            table,
            row,
            &format!("stored {field} disagrees with durable history"),
        ));
    }
    object.insert(field.to_owned(), value);
    Ok(())
}

fn current_head<T>(table: &'static str, row: &str, value: &Value) -> Result<Vec<u8>, OpenError>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let bytes = serde_json::to_vec(value)
        .map_err(|error| invalid(table, row, &format!("migrated JSON cannot encode: {error}")))?;
    let head: T = wire::decode(&bytes).map_err(|error| {
        invalid(
            table,
            row,
            &format!("migrated body is not current wire: {error}"),
        )
    })?;
    wire::encode(&head).map_err(|error| {
        invalid(
            table,
            row,
            &format!("migrated body cannot encode canonically: {error}"),
        )
    })
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
        .ok_or_else(|| invalid(table, row, "head body is not an object"))
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
