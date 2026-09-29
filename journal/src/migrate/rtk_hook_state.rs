//! Schema 31 to 32 retirement of the Codex-only RTK hook observation.

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Map, Value};

use super::{MigrationError, OpenError};

const FROM: u32 = 31;
const TO: u32 = 32;

pub(super) fn migrate(connection: &Connection) -> Result<(), OpenError> {
    migrate_facts(connection)?;
    migrate_residency_head(connection)?;
    migrate_receipts(connection)
}

fn migrate_facts(connection: &Connection) -> Result<(), OpenError> {
    let mut statement = connection
        .prepare(
            "SELECT seq, envelope FROM fact_envelopes \
             WHERE kind = 'lane::declared' \
               AND instr(CAST(envelope AS TEXT), '\"hook\":') > 0 \
             ORDER BY seq",
        )
        .map_err(OpenError::Sqlite)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(OpenError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)?;
    drop(statement);

    for (seq, original) in rows {
        let row = seq.to_string();
        let mut document = decode("fact_envelopes", &row, &original)?;
        let fact = document
            .get_mut("fact")
            .ok_or_else(|| invalid("fact_envelopes", &row, "envelope.fact is missing"))?;
        let changed = retire_health(
            "fact_envelopes",
            &row,
            object("fact_envelopes", &row, fact)?,
        )?;
        let envelope =
            canonical::<wire::Envelope<wire::AnyFact>>("fact_envelopes", &row, &document)?;
        if envelope.fact.kind() != wire::FactKind::LaneSessionDeclared {
            return Err(invalid(
                "fact_envelopes",
                &row,
                "row kind and decoded lane declaration disagree",
            ));
        }
        store_changed(
            connection,
            "fact_envelopes",
            &row,
            changed,
            &original,
            &envelope,
            "UPDATE fact_envelopes SET envelope = ?1 WHERE seq = ?2",
            seq,
        )?;
    }
    Ok(())
}

fn migrate_residency_head(connection: &Connection) -> Result<(), OpenError> {
    let Some(original) = connection
        .query_row(
            "SELECT body FROM residency_head WHERE singleton = 1 \
             AND instr(CAST(body AS TEXT), '\"hook\":') > 0",
            [],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(OpenError::Sqlite)?
    else {
        return Ok(());
    };
    let mut document = decode("residency_head", "1", &original)?;
    let lanes = document
        .get_mut("lanes")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| invalid("residency_head", "1", "body.lanes is not an array"))?;
    let mut changed = false;
    for (index, lane) in lanes.iter_mut().enumerate() {
        let lane = object("residency_head", &format!("1 lane[{index}]"), lane)?;
        let Some(session) = lane.get_mut("session") else {
            continue;
        };
        changed |= retire_health(
            "residency_head",
            &format!("1 lane[{index}].session"),
            object(
                "residency_head",
                &format!("1 lane[{index}].session"),
                session,
            )?,
        )?;
    }
    let head = canonical::<wire::ResidencyHead>("residency_head", "1", &document)?;
    store_changed(
        connection,
        "residency_head",
        "1",
        changed,
        &original,
        &head,
        "UPDATE residency_head SET body = ?1 WHERE singleton = ?2",
        1_i64,
    )
}

fn migrate_receipts(connection: &Connection) -> Result<(), OpenError> {
    let mut statement = connection
        .prepare(
            "SELECT command_id, command_kind, response FROM command_receipts \
             WHERE command_kind IN ( \
                 'lane::read', 'lane::seat', 'lane::arrange', 'lane::state', \
                 'lane::retune', 'team::configure', 'team::arrange' \
             ) AND instr(CAST(response AS TEXT), '\"hook\":') > 0 \
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
    drop(statement);

    for (command, kind, original) in rows {
        let kind = kind.parse::<wire::CommandKind>().map_err(|error| {
            invalid(
                "command_receipts",
                &command,
                &format!("lane command kind is invalid: {error}"),
            )
        })?;
        let mut document = decode("command_receipts", &command, &original)?;
        let outcome = document
            .get_mut("outcome")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                invalid(
                    "command_receipts",
                    &command,
                    "response.outcome is not an object",
                )
            })?;
        let changed = if outcome.get("kind").and_then(Value::as_str) == Some("reply") {
            let body = outcome.get_mut("body").ok_or_else(|| {
                invalid(
                    "command_receipts",
                    &command,
                    "response.outcome.body is missing",
                )
            })?;
            retire_answer(&command, kind, body)?
        } else {
            false
        };
        let response = canonical::<wire::Response<Value>>("command_receipts", &command, &document)?;
        if let wire::Outcome::Reply(reply) = &response.outcome {
            let body = wire::encode(&reply.body).map_err(|error| {
                invalid(
                    "command_receipts",
                    &command,
                    &format!("migrated reply body cannot encode: {error}"),
                )
            })?;
            kind.binding().answer.decode(&body).map_err(|error| {
                invalid(
                    "command_receipts",
                    &command,
                    &format!("migrated reply body does not match {kind}: {error}"),
                )
            })?;
        }
        store_changed(
            connection,
            "command_receipts",
            &command,
            changed,
            &original,
            &response,
            "UPDATE command_receipts SET response = ?1 WHERE command_id = ?2",
            &command,
        )?;
    }
    Ok(())
}

fn retire_answer(
    command: &str,
    kind: wire::CommandKind,
    body: &mut Value,
) -> Result<bool, OpenError> {
    match kind {
        wire::CommandKind::LaneRead
        | wire::CommandKind::LaneArrange
        | wire::CommandKind::LaneRetune
        | wire::CommandKind::TeamConfigure
        | wire::CommandKind::TeamArrange => {
            let rows = body
                .get_mut("lanes")
                .and_then(Value::as_array_mut)
                .ok_or_else(|| {
                    invalid(
                        "command_receipts",
                        command,
                        "lane reply rows are not an array",
                    )
                })?;
            let mut changed = false;
            for (index, row) in rows.iter_mut().enumerate() {
                changed |= retire_health(
                    "command_receipts",
                    &format!("{command} row[{index}]"),
                    object("command_receipts", &format!("{command} row[{index}]"), row)?,
                )?;
            }
            Ok(changed)
        }
        wire::CommandKind::LaneSeat | wire::CommandKind::LaneState => retire_health(
            "command_receipts",
            command,
            object("command_receipts", command, body)?,
        ),
        _ => Err(invalid(
            "command_receipts",
            command,
            "selected command kind has no RTK answer carrier",
        )),
    }
}

fn retire_health(
    table: &'static str,
    row: &str,
    owner: &mut Map<String, Value>,
) -> Result<bool, OpenError> {
    let Some(health) = owner.get_mut("rtk") else {
        return Ok(false);
    };
    let health = object(table, row, health)?;
    match health.remove("hook") {
        None => Ok(false),
        Some(Value::String(value)) if value == "installed" => Ok(true),
        Some(value) => Err(invalid(
            table,
            row,
            &format!("retired RTK hook state is not installed: {value}"),
        )),
    }
}

fn store_changed<T: serde::Serialize, K: rusqlite::ToSql>(
    connection: &Connection,
    table: &'static str,
    row: &str,
    changed: bool,
    original: &[u8],
    value: &T,
    sql: &str,
    key: K,
) -> Result<(), OpenError> {
    let canonical = wire::encode(value).map_err(|error| {
        invalid(
            table,
            row,
            &format!("migrated value cannot encode canonically: {error}"),
        )
    })?;
    if changed {
        connection
            .execute(sql, params![canonical, key])
            .map_err(OpenError::Sqlite)?;
    } else if canonical != original {
        return Err(invalid(
            table,
            row,
            "unchanged schema-31 value is not canonical current wire",
        ));
    }
    Ok(())
}

fn canonical<T>(table: &'static str, row: &str, value: &Value) -> Result<T, OpenError>
where
    T: serde::de::DeserializeOwned,
{
    let bytes = serde_json::to_vec(value)
        .map_err(|error| invalid(table, row, &format!("migrated JSON cannot encode: {error}")))?;
    wire::decode(&bytes).map_err(|error| {
        invalid(
            table,
            row,
            &format!("migrated value is not current wire: {error}"),
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
        .ok_or_else(|| invalid(table, row, "RTK carrier is not an object"))
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

#[cfg(test)]
mod tests;
