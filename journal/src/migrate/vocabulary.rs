//! Schema 8 to 9 retirement of durable public vocabulary.
//!
//! The wire schema has one current spelling for each renamed field. This step
//! rewrites the two retired object keys wherever current authority can retain
//! them, rejects ambiguous rows that already carry both spellings, and then
//! re-encodes every changed document through its current typed wire boundary.

use rusqlite::{Connection, params};
use serde_json::Value;

use super::{MigrationError, OpenError};

const FROM: u32 = 8;
const TO: u32 = 9;
const RENAMES: &[(&str, &str)] = &[("incepted", "born"), ("driver", "vendor")];

pub(super) fn migrate(connection: &Connection) -> Result<(), OpenError> {
    migrate_facts(connection)?;
    migrate_task_heads(connection)?;
    migrate_receipts(connection)
}

fn migrate_facts(connection: &Connection) -> Result<(), OpenError> {
    let rows = integer_rows(
        connection,
        "SELECT seq, envelope FROM fact_envelopes ORDER BY seq",
    )?;
    for (seq, bytes) in rows {
        let row = seq.to_string();
        let Some(document) = rewritten("fact_envelopes", &row, &bytes)? else {
            continue;
        };
        let intermediate = encode_value("fact_envelopes", &row, &document)?;
        let envelope: wire::Envelope<wire::AnyFact> =
            wire::decode(&intermediate).map_err(|error| {
                invalid(
                    "fact_envelopes",
                    &row,
                    &format!("renamed envelope is not current wire: {error}"),
                )
            })?;
        let canonical = wire::encode(&envelope).map_err(|error| {
            invalid(
                "fact_envelopes",
                &row,
                &format!("renamed envelope cannot encode canonically: {error}"),
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

fn migrate_task_heads(connection: &Connection) -> Result<(), OpenError> {
    let rows = text_rows(
        connection,
        "SELECT stamp, body FROM task_heads ORDER BY stamp",
    )?;
    for (stamp, bytes) in rows {
        let Some(document) = rewritten("task_heads", &stamp, &bytes)? else {
            continue;
        };
        let intermediate = encode_value("task_heads", &stamp, &document)?;
        let head: wire::TaskHead = wire::decode(&intermediate).map_err(|error| {
            invalid(
                "task_heads",
                &stamp,
                &format!("renamed task head is not current wire: {error}"),
            )
        })?;
        let canonical = wire::encode(&head).map_err(|error| {
            invalid(
                "task_heads",
                &stamp,
                &format!("renamed task head cannot encode canonically: {error}"),
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

fn migrate_receipts(connection: &Connection) -> Result<(), OpenError> {
    let rows = receipt_rows(connection)?;
    for (command, request, witnesses, response) in rows {
        let request = rewrite_request(&command, &request)?;
        let witnesses = rewrite_witnesses(&command, &witnesses)?;
        let response = rewrite_response(&command, &response)?;
        if request.is_none() && witnesses.is_none() && response.is_none() {
            continue;
        }
        connection
            .execute(
                "UPDATE command_receipts SET request = coalesce(?1, request), \
                 evaluated_witnesses = coalesce(?2, evaluated_witnesses), \
                 response = coalesce(?3, response) WHERE command_id = ?4",
                params![request, witnesses, response, command],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn rewrite_request(row: &str, bytes: &[u8]) -> Result<Option<Vec<u8>>, OpenError> {
    let Some(document) = rewritten("command_receipts", row, bytes)? else {
        return Ok(None);
    };
    let intermediate = encode_value("command_receipts", row, &document)?;
    let command: wire::Command<Value> = wire::decode(&intermediate).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("renamed request is not current command wire: {error}"),
        )
    })?;
    command
        .canonical_json()
        .map(String::into_bytes)
        .map(Some)
        .map_err(|error| {
            invalid(
                "command_receipts",
                row,
                &format!("renamed request cannot encode canonically: {error}"),
            )
        })
}

fn rewrite_witnesses(row: &str, bytes: &[u8]) -> Result<Option<Vec<u8>>, OpenError> {
    let Some(document) = rewritten("command_receipts", row, bytes)? else {
        return Ok(None);
    };
    let intermediate = encode_value("command_receipts", row, &document)?;
    let mut witnesses: Vec<wire::Witness> = wire::decode(&intermediate).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("renamed evaluated witnesses are not current wire: {error}"),
        )
    })?;
    witnesses.sort();
    wire::encode(&witnesses).map(Some).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("renamed evaluated witnesses cannot encode canonically: {error}"),
        )
    })
}

fn rewrite_response(row: &str, bytes: &[u8]) -> Result<Option<Vec<u8>>, OpenError> {
    let Some(document) = rewritten("command_receipts", row, bytes)? else {
        return Ok(None);
    };
    let intermediate = encode_value("command_receipts", row, &document)?;
    let response: wire::Response<Value> = wire::decode(&intermediate).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("renamed response is not current response wire: {error}"),
        )
    })?;
    wire::encode(&response).map(Some).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("renamed response cannot encode canonically: {error}"),
        )
    })
}

fn rewritten(table: &'static str, row: &str, bytes: &[u8]) -> Result<Option<Value>, OpenError> {
    let mut document: Value = serde_json::from_slice(bytes)
        .map_err(|error| invalid(table, row, &format!("stored JSON is invalid: {error}")))?;
    rename_keys(table, row, "$", &mut document).map(|changed| changed.then_some(document))
}

fn rename_keys(
    table: &'static str,
    row: &str,
    path: &str,
    value: &mut Value,
) -> Result<bool, OpenError> {
    match value {
        Value::Object(object) => {
            let mut changed = false;
            for (name, nested) in object.iter_mut() {
                changed |= rename_keys(table, row, &format!("{path}.{name}"), nested)?;
            }
            for &(retired, current) in RENAMES {
                if !object.contains_key(retired) {
                    continue;
                }
                if object.contains_key(current) {
                    return Err(invalid(
                        table,
                        row,
                        &format!("{path} carries both {retired:?} and {current:?}"),
                    ));
                }
                let field = object
                    .remove(retired)
                    .expect("the retired key was just observed");
                object.insert(current.to_owned(), field);
                changed = true;
            }
            Ok(changed)
        }
        Value::Array(values) => {
            let mut changed = false;
            for (index, nested) in values.iter_mut().enumerate() {
                changed |= rename_keys(table, row, &format!("{path}[{index}]"), nested)?;
            }
            Ok(changed)
        }
        _ => Ok(false),
    }
}

fn encode_value(table: &'static str, row: &str, value: &Value) -> Result<Vec<u8>, OpenError> {
    super::team::current(value)
        .map_err(|error| invalid(table, row, &format!("renamed JSON cannot encode: {error}")))
}

fn integer_rows(connection: &Connection, sql: &str) -> Result<Vec<(i64, Vec<u8>)>, OpenError> {
    let mut statement = connection.prepare(sql).map_err(OpenError::Sqlite)?;
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(OpenError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)
}

fn text_rows(connection: &Connection, sql: &str) -> Result<Vec<(String, Vec<u8>)>, OpenError> {
    let mut statement = connection.prepare(sql).map_err(OpenError::Sqlite)?;
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(OpenError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)
}

type Receipt = (String, Vec<u8>, Vec<u8>, Vec<u8>);

fn receipt_rows(connection: &Connection) -> Result<Vec<Receipt>, OpenError> {
    let mut statement = connection
        .prepare(
            "SELECT command_id, request, evaluated_witnesses, response \
             FROM command_receipts ORDER BY command_id",
        )
        .map_err(OpenError::Sqlite)?;
    statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .map_err(OpenError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)
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
