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
        residual::rewrite_receipts(transaction, command, RULE_RELEASE, |request, response| {
            let request = if command == wire::CommandKind::RoomCoordinate {
                residual::edit_request::<wire::intent::RoomCoordinate>(request, |body| {
                    Ok(strip(body, &["auto_resume"]))
                })?
            } else {
                None
            };
            let response =
                residual::edit_reply(command, response, |answer| Ok(strip(answer, &["rule"])))?;
            Ok((request, response))
        })?;
    }
    Ok(())
}
