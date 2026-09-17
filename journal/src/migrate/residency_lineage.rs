//! Schema 2 to 3 completion of the lane-lineage field introduction and rename.
//!
//! The pre-field shape derives the initial lineage from the lane, matching the
//! original seat command's default. The intermediate name is moved once, and a
//! record carrying two different identities refuses the whole transaction.

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Map, Value};

use super::{MigrationError, OpenError};

const FROM: u32 = 2;
const TO: u32 = 3;

pub(super) fn migrate(connection: &Connection) -> Result<(), OpenError> {
    migrate_residency_head(connection)?;
    migrate_lane_seated_facts(connection)?;
    migrate_lane_seat_receipts(connection)
}

fn migrate_residency_head(connection: &Connection) -> Result<(), OpenError> {
    let Some(bytes) = connection
        .query_row(
            "SELECT body FROM residency_head WHERE singleton = 1",
            [],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(OpenError::Sqlite)?
    else {
        return Ok(());
    };
    let mut document = decode("residency_head", "1", &bytes)?;
    let lanes = document
        .get_mut("lanes")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| invalid("residency_head", "1", "body.lanes is not an array"))?;
    let mut changed = false;
    for (index, lane) in lanes.iter_mut().enumerate() {
        changed |= normalize(
            "residency_head",
            &format!("1 lane[{index}]"),
            object("residency_head", "1", lane)?,
        )?;
    }
    if changed {
        connection
            .execute(
                "UPDATE residency_head SET body = ?1 WHERE singleton = 1",
                params![encode_residency("1", &document)?],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn migrate_lane_seated_facts(connection: &Connection) -> Result<(), OpenError> {
    let rows = {
        let mut statement = connection
            .prepare(
                "SELECT seq, envelope FROM fact_envelopes \
                 WHERE kind = 'lane::seated' ORDER BY seq",
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
        let fact = document
            .get_mut("fact")
            .ok_or_else(|| invalid("fact_envelopes", &row, "envelope.fact is missing"))?;
        if normalize(
            "fact_envelopes",
            &row,
            object("fact_envelopes", &row, fact)?,
        )? {
            connection
                .execute(
                    "UPDATE fact_envelopes SET envelope = ?1 WHERE seq = ?2",
                    params![encode_fact(&row, &document)?, seq],
                )
                .map_err(OpenError::Sqlite)?;
        }
    }
    Ok(())
}

fn migrate_lane_seat_receipts(connection: &Connection) -> Result<(), OpenError> {
    let rows = {
        let mut statement = connection
            .prepare(
                "SELECT command_id, request FROM command_receipts \
                 WHERE command_kind = 'lane::seat' ORDER BY command_id",
            )
            .map_err(OpenError::Sqlite)?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(OpenError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };
    for (command, bytes) in rows {
        let mut document = decode("command_receipts", &command, &bytes)?;
        let body = document
            .get_mut("body")
            .ok_or_else(|| invalid("command_receipts", &command, "request.body is missing"))?;
        if normalize(
            "command_receipts",
            &command,
            object("command_receipts", &command, body)?,
        )? {
            connection
                .execute(
                    "UPDATE command_receipts SET request = ?1 WHERE command_id = ?2",
                    params![encode_command(&command, &document)?, command],
                )
                .map_err(OpenError::Sqlite)?;
        }
    }
    Ok(())
}

fn normalize(
    table: &'static str,
    row: &str,
    object: &mut Map<String, Value>,
) -> Result<bool, OpenError> {
    let lane = uuid(table, row, object, "lane")?.ok_or_else(|| {
        invalid(
            table,
            row,
            "the pre-lineage record has no lane from which to derive lineage_id",
        )
    })?;
    let lineage = uuid(table, row, object, "lineage_id")?;
    let team = uuid(table, row, object, "team")?;
    match (lineage, team) {
        (Some(lineage), Some(team)) if lineage != team => Err(invalid(
            table,
            row,
            "lineage_id and team name different lineages",
        )),
        (Some(_), Some(_)) => {
            object.remove("team");
            Ok(true)
        }
        (Some(_), None) => Ok(false),
        (None, Some(_)) => {
            let team = object
                .remove("team")
                .expect("validated team remains present");
            object.insert("lineage_id".to_owned(), team);
            Ok(true)
        }
        (None, None) => {
            object.insert("lineage_id".to_owned(), Value::String(lane.to_string()));
            Ok(true)
        }
    }
}

fn uuid(
    table: &'static str,
    row: &str,
    object: &Map<String, Value>,
    field: &str,
) -> Result<Option<wire::CanonicalUuid>, OpenError> {
    object
        .get(field)
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| invalid(table, row, &format!("{field} is not a string")))?
                .parse()
                .map_err(|error| invalid(table, row, &format!("{field} is invalid: {error}")))
        })
        .transpose()
}

fn object<'a>(
    table: &'static str,
    row: &str,
    value: &'a mut Value,
) -> Result<&'a mut Map<String, Value>, OpenError> {
    value
        .as_object_mut()
        .ok_or_else(|| invalid(table, row, "lineage-bearing value is not an object"))
}

fn decode(table: &'static str, row: &str, bytes: &[u8]) -> Result<Value, OpenError> {
    serde_json::from_slice(bytes)
        .map_err(|error| invalid(table, row, &format!("stored JSON is invalid: {error}")))
}

fn encode_residency(row: &str, value: &Value) -> Result<Vec<u8>, OpenError> {
    let bytes = intermediate("residency_head", row, value)?;
    let head: PreTopologyResidencyHead = wire::decode(&bytes).map_err(|error| {
        invalid(
            "residency_head",
            row,
            &format!("migrated body is not schema-three residency wire: {error}"),
        )
    })?;
    wire::encode(&head).map_err(|error| {
        invalid(
            "residency_head",
            row,
            &format!("migrated body cannot encode canonically: {error}"),
        )
    })
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PreTopologyResidencyHead {
    epoch: wire::CanonicalUuid,
    state: wire::ResidentState,
    lanes: Vec<wire::LaneResidency>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    subscriptions: Vec<wire::TeamSubscription>,
}

fn encode_fact(row: &str, value: &Value) -> Result<Vec<u8>, OpenError> {
    let bytes = intermediate("fact_envelopes", row, value)?;
    let fact: wire::Envelope<wire::AnyFact> = wire::decode(&bytes).map_err(|error| {
        invalid(
            "fact_envelopes",
            row,
            &format!("migrated envelope is not current fact wire: {error}"),
        )
    })?;
    wire::encode(&fact).map_err(|error| {
        invalid(
            "fact_envelopes",
            row,
            &format!("migrated envelope cannot encode canonically: {error}"),
        )
    })
}

fn encode_command(row: &str, value: &Value) -> Result<Vec<u8>, OpenError> {
    let bytes = intermediate("command_receipts", row, value)?;
    let command: wire::Command<PreTopologyLaneSeat> = wire::decode(&bytes).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("migrated request is not schema-three lane wire: {error}"),
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

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PreTopologyLaneSeat {
    lane: wire::CanonicalUuid,
    team: wire::CanonicalUuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    worktree: String,
    life: String,
    vendor: wire::Name,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resume: Option<String>,
}

fn intermediate(table: &'static str, row: &str, value: &Value) -> Result<Vec<u8>, OpenError> {
    super::team::current(value)
        .map_err(|error| invalid(table, row, &format!("migrated JSON cannot encode: {error}")))
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
