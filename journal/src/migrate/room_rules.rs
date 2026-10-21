//! Drop the retired automated-resume rule from every retained room
//! coordination record, direction head, and coordination receipt.
//!
//! Schema 92 and earlier let `room start` and `room resume` record a rule
//! that re-issued the coordination to the same lane up to N times, and each
//! automated record named the step it spent. Item-scoped Design outreach
//! bounds automation by the statement instead, so the rule retires: a
//! retained marker keeps its room, publication, key, and target and crosses
//! as the plain operator coordination it links; the retired members are
//! unrecoverable, as the directions they repeated still carry their text.

use rusqlite::{Transaction, params};
use serde_json::{Map, Value};

use super::{OpenError, residual};

/// Last schema whose coordination records carried a resume rule.
const RULE_RELEASE: &str = "schema-92";
/// Members a retained record or direction head loses.
const RECORD_MEMBERS: [&str; 2] = ["rule", "automated"];

pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    residual::rewrite_facts(
        transaction,
        wire::FactKind::RoomCoordinationStarted,
        "json_type(CAST(envelope AS TEXT), '$.fact.rule') IS NOT NULL \
         OR json_type(CAST(envelope AS TEXT), '$.fact.automated') IS NOT NULL",
        RULE_RELEASE,
        |_, fact| {
            strip(fact, &RECORD_MEMBERS);
            Ok(())
        },
    )?;
    cross_direction_heads(transaction)?;
    cross_receipts(transaction)
}

/// Remove each of `members` from `object`, answering whether any was present.
fn strip(object: &mut Map<String, Value>, members: &[&str]) -> bool {
    let mut stripped = false;
    for member in members {
        stripped |= object.remove(*member).is_some();
    }
    stripped
}

/// Re-encode each direction head that still carries a rule through the
/// current strict DTO. A predecessor older than the head table itself has no
/// heads to cross; the shape crossing creates the table afterwards.
fn cross_direction_heads(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    if !super::table_exists(transaction, "room_directions")? {
        return Ok(());
    }
    let heads = {
        let mut statement = transaction
            .prepare(
                "SELECT room, body FROM room_directions \
                 WHERE json_type(CAST(body AS TEXT), '$.rule') IS NOT NULL \
                 OR json_type(CAST(body AS TEXT), '$.automated') IS NOT NULL \
                 ORDER BY room",
            )
            .map_err(OpenError::Sqlite)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(OpenError::Sqlite)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };
    for (room, bytes) in heads {
        let refuse = |detail: String| {
            residual::retained("room_directions", room.clone(), RULE_RELEASE, detail)
        };
        let mut body: Map<String, Value> = wire::decode(&bytes).map_err(|error| {
            refuse(format!(
                "the stored direction head does not decode: {error}"
            ))
        })?;
        strip(&mut body, &RECORD_MEMBERS);
        let current: wire::rooms::RoomCoordinationStarted =
            serde_json::from_value(Value::Object(body)).map_err(|error| {
                refuse(format!(
                    "the crossed direction head is not current: {error}"
                ))
            })?;
        let bytes = wire::encode(&current).map_err(|error| {
            refuse(format!("the crossed direction head cannot encode: {error}"))
        })?;
        let changed = transaction
            .execute(
                "UPDATE room_directions SET body = ?1 WHERE room = ?2",
                params![bytes, room],
            )
            .map_err(OpenError::Sqlite)?;
        if changed != 1 {
            return Err(refuse(
                "the row changed under the exclusive crossing".to_owned(),
            ));
        }
    }
    Ok(())
}

/// Drop the retired rule budget from every coordination view a receipt
/// answered with, and the retired repeat count from every coordinate request.
fn cross_receipts(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    for command in [
        wire::CommandKind::RoomCoordinate,
        wire::CommandKind::RoomPutAside,
        wire::CommandKind::RoomCoordinationRead,
    ] {
        let receipts = {
            let mut statement = transaction
                .prepare(
                    "SELECT command_id, request, response FROM command_receipts \
                     WHERE command_kind = ?1 ORDER BY command_id",
                )
                .map_err(OpenError::Sqlite)?;
            let rows = statement
                .query_map(params![command.as_str()], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                })
                .map_err(OpenError::Sqlite)?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(OpenError::Sqlite)?
        };
        for (command_id, request, response) in receipts {
            let refuse = |detail: String| {
                residual::retained("command_receipts", command_id.clone(), RULE_RELEASE, detail)
            };
            let request = (command == wire::CommandKind::RoomCoordinate)
                .then(|| cross_request(&request))
                .transpose()
                .map_err(&refuse)?
                .flatten();
            let response = cross_response(command, &response).map_err(&refuse)?;
            if request.is_none() && response.is_none() {
                continue;
            }
            let changed = transaction
                .execute(
                    "UPDATE command_receipts \
                     SET request = coalesce(?1, request), response = coalesce(?2, response) \
                     WHERE command_id = ?3 AND command_kind = ?4",
                    params![request, response, command_id, command.as_str()],
                )
                .map_err(OpenError::Sqlite)?;
            if changed != 1 {
                return Err(refuse(
                    "the row changed under the exclusive crossing".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

/// The coordinate request without its repeat count, through the current
/// strict DTO, or `None` when it never carried one.
fn cross_request(bytes: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let mut request: Value = wire::decode(bytes)
        .map_err(|error| format!("the stored request does not decode: {error}"))?;
    let body = request
        .get_mut("body")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| "the stored request has no body object".to_owned())?;
    if !strip(body, &["auto_resume"]) {
        return Ok(None);
    }
    let request: wire::Command<wire::intent::RoomCoordinate> = serde_json::from_value(request)
        .map_err(|error| format!("the crossed request is not current: {error}"))?;
    request
        .canonical_json()
        .map(String::into_bytes)
        .map(Some)
        .map_err(|error| format!("the crossed request cannot encode: {error}"))
}

/// The response without its reply's rule budget, or `None` when it is a
/// refusal or never carried one.
fn cross_response(command: wire::CommandKind, bytes: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let mut response: wire::Response<Value> = wire::decode(bytes)
        .map_err(|error| format!("the stored response does not decode: {error}"))?;
    let wire::Outcome::Reply(reply) = &mut response.outcome else {
        return Ok(None);
    };
    let answer = reply
        .body
        .as_object_mut()
        .ok_or_else(|| "the stored answer is not a JSON object".to_owned())?;
    if !strip(answer, &["rule"]) {
        return Ok(None);
    }
    wire::encode(&reply.body)
        .and_then(|body| command.binding().answer.decode(&body))
        .map_err(|error| format!("the crossed answer is not current: {error}"))?;
    wire::encode(&response)
        .map(Some)
        .map_err(|error| format!("the crossed response cannot encode: {error}"))
}
