//! Schema 8 to 9 retirement of task and taste field spellings.
//!
//! Schema 8 stored task-head identity as `incepted` and taste reminder prose as
//! `driver`. Schema 9 rewrites those exact JSON keys once, then validates every
//! changed authority through the current wire DTO before the surrounding
//! migration transaction advances either version record.

use rusqlite::{Connection, params};
use serde_json::{Map, Value};

use super::{MigrationError, OpenError};

const FROM: u32 = 8;
const TO: u32 = 9;
const OLD_TASK_IDENTITY: &str = "incepted";
const TASK_IDENTITY: &str = "born";
const OLD_REMINDER_PROSE: &str = "driver";
const REMINDER_PROSE: &str = "vendor";

pub(super) fn migrate(connection: &Connection) -> Result<(), OpenError> {
    migrate_task_heads(connection)?;
    migrate_taste_facts(connection)?;
    migrate_receipts(connection)
}

fn migrate_task_heads(connection: &Connection) -> Result<(), OpenError> {
    let rows = {
        let mut statement = connection
            .prepare("SELECT stamp, body FROM task_heads ORDER BY stamp")
            .map_err(OpenError::Sqlite)?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(OpenError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };

    for (stamp, bytes) in rows {
        let mut document = decode("task_heads", &stamp, &bytes)?;
        let head = object("task_heads", &stamp, &mut document)?;
        let changed = rename_key("task_heads", &stamp, head, OLD_TASK_IDENTITY, TASK_IDENTITY)?;
        if !changed && !head.contains_key(TASK_IDENTITY) {
            return Err(invalid(
                "task_heads",
                &stamp,
                "task identity key is missing",
            ));
        }
        let bytes = intermediate("task_heads", &stamp, &document)?;
        let head: wire::TaskHead = wire::decode(&bytes).map_err(|error| {
            invalid(
                "task_heads",
                &stamp,
                &format!("migrated body is not current task-head wire: {error}"),
            )
        })?;
        let canonical = wire::encode(&head).map_err(|error| {
            invalid(
                "task_heads",
                &stamp,
                &format!("migrated body cannot encode canonically: {error}"),
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

fn migrate_taste_facts(connection: &Connection) -> Result<(), OpenError> {
    let rows = {
        let mut statement = connection
            .prepare(
                "SELECT seq, envelope FROM fact_envelopes \
                 WHERE kind = 'taste::judged' ORDER BY seq",
            )
            .map_err(OpenError::Sqlite)?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(OpenError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };

    for (seq, bytes) in rows {
        let row = seq.to_string();
        let mut document = decode("fact_envelopes", &row, &bytes)?;
        let fact = object("fact_envelopes", &row, &mut document)?
            .get_mut("fact")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| invalid("fact_envelopes", &row, "envelope.fact is not an object"))?;
        if !rename_key(
            "fact_envelopes",
            &row,
            fact,
            OLD_REMINDER_PROSE,
            REMINDER_PROSE,
        )? {
            return Err(invalid(
                "fact_envelopes",
                &row,
                "taste judgment does not carry the schema-8 reminder field",
            ));
        }
        let bytes = intermediate("fact_envelopes", &row, &document)?;
        let envelope: wire::Envelope<wire::AnyFact> = wire::decode(&bytes).map_err(|error| {
            invalid(
                "fact_envelopes",
                &row,
                &format!("migrated envelope is not current fact wire: {error}"),
            )
        })?;
        let canonical = wire::encode(&envelope).map_err(|error| {
            invalid(
                "fact_envelopes",
                &row,
                &format!("migrated envelope cannot encode canonically: {error}"),
            )
        })?;
        connection
            .execute(
                "UPDATE fact_envelopes SET envelope = ?1 WHERE seq = ?2",
                params![canonical, seq],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn migrate_receipts(connection: &Connection) -> Result<(), OpenError> {
    let rows = {
        let mut statement = connection
            .prepare(
                "SELECT command_id, request, response FROM command_receipts ORDER BY command_id",
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

    for (command, request, response) in rows {
        let request = migrate_request(&command, &request)?;
        let response = migrate_response(&command, &response)?;
        connection
            .execute(
                "UPDATE command_receipts SET request = ?1, response = ?2 WHERE command_id = ?3",
                params![request, response, command],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn migrate_request(row: &str, bytes: &[u8]) -> Result<Vec<u8>, OpenError> {
    let mut document = decode("command_receipts", row, bytes)?;
    rewrite_nested_keys("command_receipts", row, &mut document)?;
    let bytes = intermediate("command_receipts", row, &document)?;
    let command: wire::Command<Value> = wire::decode(&bytes).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("migrated request is not current command wire: {error}"),
        )
    })?;
    command
        .canonical_json()
        .map(String::into_bytes)
        .map_err(|error| {
            invalid(
                "command_receipts",
                row,
                &format!("migrated request cannot encode canonically: {error}"),
            )
        })
}

fn migrate_response(row: &str, bytes: &[u8]) -> Result<Vec<u8>, OpenError> {
    let mut document = decode("command_receipts", row, bytes)?;
    rewrite_nested_keys("command_receipts", row, &mut document)?;
    let bytes = intermediate("command_receipts", row, &document)?;
    let response: wire::Response<Value> = wire::decode(&bytes).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("migrated response is not current response wire: {error}"),
        )
    })?;
    wire::encode(&response).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("migrated response cannot encode canonically: {error}"),
        )
    })
}

fn rewrite_nested_keys(
    table: &'static str,
    row: &str,
    value: &mut Value,
) -> Result<bool, OpenError> {
    match value {
        Value::Array(values) => {
            let mut changed = false;
            for value in values {
                changed |= rewrite_nested_keys(table, row, value)?;
            }
            Ok(changed)
        }
        Value::Object(object) => {
            let mut changed = rename_key(table, row, object, OLD_TASK_IDENTITY, TASK_IDENTITY)?;
            changed |= rename_key(table, row, object, OLD_REMINDER_PROSE, REMINDER_PROSE)?;
            for value in object.values_mut() {
                changed |= rewrite_nested_keys(table, row, value)?;
            }
            Ok(changed)
        }
        _ => Ok(false),
    }
}

fn rename_key(
    table: &'static str,
    row: &str,
    object: &mut Map<String, Value>,
    old: &str,
    current: &str,
) -> Result<bool, OpenError> {
    let legacy = object.remove(old);
    if object.contains_key(current) {
        return if legacy.is_some() {
            Err(invalid(
                table,
                row,
                &format!("object carries both {old:?} and {current:?}"),
            ))
        } else {
            Ok(false)
        };
    }
    let Some(value) = legacy else {
        return Ok(false);
    };
    object.insert(current.to_owned(), value);
    Ok(true)
}

fn decode(table: &'static str, row: &str, bytes: &[u8]) -> Result<Value, OpenError> {
    serde_json::from_slice(bytes)
        .map_err(|error| invalid(table, row, &format!("stored JSON is invalid: {error}")))
}

fn intermediate(table: &'static str, row: &str, value: &Value) -> Result<Vec<u8>, OpenError> {
    serde_json::to_vec(value)
        .map_err(|error| invalid(table, row, &format!("migrated JSON cannot encode: {error}")))
}

fn object<'a>(
    table: &'static str,
    row: &str,
    value: &'a mut Value,
) -> Result<&'a mut Map<String, Value>, OpenError> {
    value
        .as_object_mut()
        .ok_or_else(|| invalid(table, row, "stored value is not an object"))
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
