//! Schema 4 to 5 representation of historical publication-note absence.
//!
//! Schema 4 release publication facts and command receipts predate the notes
//! digest. Their exact note bytes cannot be reconstructed, so this step writes
//! the required current field as JSON `null`. Rows already carrying an exact
//! digest are validated and retained byte-for-byte.

use rusqlite::{Connection, params};
use serde_json::{Map, Value};

use super::{MigrationError, OpenError};

const FROM: u32 = 4;
const TO: u32 = 5;

pub(super) fn migrate(connection: &Connection) -> Result<(), OpenError> {
    migrate_facts(connection)?;
    migrate_receipts(connection)
}

fn migrate_facts(connection: &Connection) -> Result<(), OpenError> {
    let rows = rows(
        connection,
        "SELECT seq, envelope FROM fact_envelopes \
         WHERE kind = 'release::published' ORDER BY seq",
    )?;
    for (seq, bytes) in rows {
        let row = seq.to_string();
        let mut document = decode("fact_envelopes", &row, &bytes)?;
        let body = document
            .get_mut("fact")
            .ok_or_else(|| invalid("fact_envelopes", &row, "envelope.fact is missing"))?;
        let changed = normalize(
            "fact_envelopes",
            &row,
            object("fact_envelopes", &row, body)?,
        )?;
        let intermediate = serde_json::to_vec(&document).map_err(|error| {
            invalid(
                "fact_envelopes",
                &row,
                &format!("migrated envelope cannot encode: {error}"),
            )
        })?;
        let envelope: wire::Envelope<wire::AnyFact> =
            wire::decode(&intermediate).map_err(|error| {
                invalid(
                    "fact_envelopes",
                    &row,
                    &format!("migrated envelope is not current release wire: {error}"),
                )
            })?;
        if envelope.fact.kind() != wire::FactKind::ReleasePublished {
            return Err(invalid(
                "fact_envelopes",
                &row,
                "row kind and decoded release fact disagree",
            ));
        }
        if changed {
            connection
                .execute(
                    "UPDATE fact_envelopes SET envelope = ?1 WHERE seq = ?2",
                    params![
                        wire::encode(&envelope).map_err(|error| {
                            invalid(
                                "fact_envelopes",
                                &row,
                                &format!("migrated envelope cannot encode canonically: {error}"),
                            )
                        })?,
                        seq
                    ],
                )
                .map_err(OpenError::Sqlite)?;
        }
    }
    Ok(())
}

fn migrate_receipts(connection: &Connection) -> Result<(), OpenError> {
    let rows = text_rows(
        connection,
        "SELECT command_id, request FROM command_receipts \
         WHERE command_kind = 'release::publish' ORDER BY command_id",
    )?;
    for (command, bytes) in rows {
        let mut document = decode("command_receipts", &command, &bytes)?;
        let body = document
            .get_mut("body")
            .ok_or_else(|| invalid("command_receipts", &command, "request.body is missing"))?;
        let changed = normalize(
            "command_receipts",
            &command,
            object("command_receipts", &command, body)?,
        )?;
        let intermediate = serde_json::to_vec(&document).map_err(|error| {
            invalid(
                "command_receipts",
                &command,
                &format!("migrated request cannot encode: {error}"),
            )
        })?;
        let request: wire::Command<wire::intent::ReleasePublish> = wire::decode(&intermediate)
            .map_err(|error| {
                invalid(
                    "command_receipts",
                    &command,
                    &format!("migrated request is not current release wire: {error}"),
                )
            })?;
        if changed {
            connection
                .execute(
                    "UPDATE command_receipts SET request = ?1 WHERE command_id = ?2",
                    params![
                        request
                            .canonical_json()
                            .map_err(|error| {
                                invalid(
                                    "command_receipts",
                                    &command,
                                    &format!("migrated request cannot encode canonically: {error}"),
                                )
                            })?
                            .into_bytes(),
                        command
                    ],
                )
                .map_err(OpenError::Sqlite)?;
        }
    }
    Ok(())
}

fn normalize(
    table: &'static str,
    row: &str,
    body: &mut Map<String, Value>,
) -> Result<bool, OpenError> {
    match body.get("notes") {
        None => {
            body.insert("notes".to_owned(), Value::Null);
            Ok(true)
        }
        Some(Value::String(_)) => Ok(false),
        Some(_) => Err(invalid(
            table,
            row,
            "schema-4 notes must be absent or an exact digest string",
        )),
    }
}

fn rows(connection: &Connection, sql: &str) -> Result<Vec<(i64, Vec<u8>)>, OpenError> {
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
        .ok_or_else(|| invalid(table, row, "release value is not an object"))
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
