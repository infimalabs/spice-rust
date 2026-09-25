//! Schema 27 to 28 materialization of per-worktree speech playback in lane replies.
//!
//! Speech is a live worktree projection, but retained command receipts still
//! have to decode through the current strict reply shape. Historical lane rows
//! therefore gain the inert `disabled` spelling once; runtime reads replace it
//! with the worktree's current admitted configuration.

use rusqlite::{Connection, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use super::{MigrationError, OpenError};

const FROM: u32 = 27;
const TO: u32 = 28;
const DIRECT_KINDS: &[&str] = &[
    "lane::read",
    "lane::arrange",
    "lane::retune",
    "team::configure",
    "team::arrange",
];
const RECALL_KIND: &str = "lane::recall";

pub(super) fn migrate(connection: &Connection) -> Result<(), OpenError> {
    let mut statement = connection
        .prepare(
            "SELECT command_id, command_kind, response FROM command_receipts \
             WHERE command_kind IN ('lane::read', 'lane::arrange', 'lane::retune', \
                                    'team::configure', 'team::arrange', 'lane::recall') \
             ORDER BY command_id",
        )
        .map_err(OpenError::Sqlite)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .map_err(OpenError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)?;

    for (command, kind, bytes) in rows {
        migrate_receipt(connection, &command, &kind, &bytes)?;
    }
    Ok(())
}

/// Validates a pre-schema-28 lane answer through the current wire without
/// leaking the future `speech` field into an earlier migration's output.
pub(super) fn canonical_predecessor(document: &Value) -> Result<Vec<u8>, String> {
    let mut validation = document.clone();
    set_direct_speech(&mut validation, true)?;
    let bytes = super::team::current(&validation)
        .map_err(|error| format!("migrated JSON cannot encode: {error}"))?;
    let response: wire::Response<wire::intent::LaneAnswer> = wire::decode(&bytes)
        .map_err(|error| format!("migrated response is not current reply wire: {error}"))?;
    let canonical = wire::encode(&response)
        .map_err(|error| format!("migrated response cannot encode canonically: {error}"))?;
    let mut predecessor: Value = serde_json::from_slice(&canonical)
        .map_err(|error| format!("canonical response JSON is invalid: {error}"))?;
    set_direct_speech(&mut predecessor, false)?;
    let bytes = super::team::current(&predecessor)
        .map_err(|error| format!("predecessor JSON cannot encode: {error}"))?;
    let response: wire::Response<Value> = wire::decode(&bytes)
        .map_err(|error| format!("predecessor response is not generic reply wire: {error}"))?;
    wire::encode(&response)
        .map_err(|error| format!("predecessor response cannot encode canonically: {error}"))
}

fn set_direct_speech(document: &mut Value, present: bool) -> Result<(), String> {
    let Some(body) = document
        .get_mut("outcome")
        .and_then(|outcome| outcome.get_mut("body"))
    else {
        return Ok(());
    };
    let lanes = body
        .as_object_mut()
        .and_then(|body| body.get_mut("lanes"))
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "response.outcome.body.lanes is missing or not an array".to_owned())?;
    for (index, lane) in lanes.iter_mut().enumerate() {
        let lane = lane
            .as_object_mut()
            .ok_or_else(|| format!("lane row {index} is not an object"))?;
        if present {
            lane.insert("speech".to_owned(), json!({"backend": "disabled"}));
        } else {
            lane.remove("speech");
        }
    }
    Ok(())
}

fn migrate_receipt(
    connection: &Connection,
    command: &str,
    kind: &str,
    bytes: &[u8],
) -> Result<(), OpenError> {
    let mut document: Value = serde_json::from_slice(bytes).map_err(|error| {
        invalid(
            command,
            &format!("stored response JSON is invalid: {error}"),
        )
    })?;
    let changed = normalize_response(command, kind, &mut document)?;
    let canonical = if DIRECT_KINDS.contains(&kind) {
        canonical::<wire::intent::LaneAnswer>(command, &document)?
    } else if kind == RECALL_KIND {
        canonical::<wire::intent::RecallAnswer>(command, &document)?
    } else {
        return Err(invalid(
            command,
            "speech response command kind is not registered by the migration",
        ));
    };
    if changed {
        connection
            .execute(
                "UPDATE command_receipts SET response = ?1 WHERE command_id = ?2",
                params![canonical, command],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn normalize_response(command: &str, kind: &str, document: &mut Value) -> Result<bool, OpenError> {
    let Some(body) = document
        .get_mut("outcome")
        .and_then(|outcome| outcome.get_mut("body"))
    else {
        return Ok(false);
    };
    let lanes = if DIRECT_KINDS.contains(&kind) {
        lane_rows(command, body, "response.outcome.body.lanes")?
    } else if kind == RECALL_KIND {
        let residency = body
            .get_mut("residency")
            .ok_or_else(|| invalid(command, "response.outcome.body.residency is missing"))?;
        lane_rows(command, residency, "response.outcome.body.residency.lanes")?
    } else {
        return Err(invalid(
            command,
            "speech response command kind is not registered by the migration",
        ));
    };
    let mut changed = false;
    for (index, lane) in lanes.iter_mut().enumerate() {
        let lane = lane
            .as_object_mut()
            .ok_or_else(|| invalid(command, &format!("lane row {index} is not an object")))?;
        if !lane.contains_key("speech") {
            lane.insert("speech".to_owned(), json!({"backend": "disabled"}));
            changed = true;
        }
    }
    Ok(changed)
}

fn lane_rows<'a>(
    command: &str,
    value: &'a mut Value,
    locus: &str,
) -> Result<&'a mut Vec<Value>, OpenError> {
    value
        .as_object_mut()
        .and_then(|body: &mut Map<String, Value>| body.get_mut("lanes"))
        .and_then(Value::as_array_mut)
        .ok_or_else(|| invalid(command, &format!("{locus} is missing or not an array")))
}

fn canonical<B>(command: &str, document: &Value) -> Result<Vec<u8>, OpenError>
where
    B: DeserializeOwned + Serialize,
{
    let bytes = super::team::current(document)
        .map_err(|error| invalid(command, &format!("migrated JSON cannot encode: {error}")))?;
    let response: wire::Response<B> = wire::decode(&bytes).map_err(|error| {
        invalid(
            command,
            &format!("migrated response is not current reply wire: {error}"),
        )
    })?;
    let canonical = wire::encode(&response).map_err(|error| {
        invalid(
            command,
            &format!("migrated response cannot encode canonically: {error}"),
        )
    })?;
    let generic: wire::Response<Value> = wire::decode(&canonical).map_err(|error| {
        invalid(
            command,
            &format!("typed response cannot project current receipt wire: {error}"),
        )
    })?;
    wire::encode(&generic).map_err(|error| {
        invalid(
            command,
            &format!("current receipt cannot encode canonically: {error}"),
        )
    })
}

fn invalid(row: &str, detail: &str) -> OpenError {
    MigrationError::StoredContent {
        from: FROM,
        to: TO,
        table: "command_receipts",
        row: row.to_owned(),
        detail: detail.to_owned(),
    }
    .into()
}
