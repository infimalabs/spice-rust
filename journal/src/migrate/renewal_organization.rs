//! Schema 16 to 17 retirement of the durable `team_slot` spelling.
//!
//! Renewal state can rest in immutable renewal facts, the residency head, and
//! successful snapshot or mutation receipts. Every changed carrier is decoded
//! through its exact current DTO before canonical bytes replace the legacy
//! document. Runtime wire remains strict: compatibility ends at this migration.

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use super::{MigrationError, OpenError};

const FROM: u32 = 16;
const TO: u32 = 17;

const LANE_ANSWER_KINDS: &[&str] = &[
    "lane::read",
    "lane::arrange",
    "lineage::configure",
    "lineage::arrange",
];

pub(super) fn migrate(connection: &Connection) -> Result<(), OpenError> {
    migrate_facts(connection)?;
    migrate_residency_head(connection)?;
    migrate_receipts(connection)
}

fn migrate_facts(connection: &Connection) -> Result<(), OpenError> {
    let mut statement = connection
        .prepare(
            "SELECT seq, kind, envelope FROM fact_envelopes \
             WHERE kind = 'lane::renewal' ORDER BY seq",
        )
        .map_err(OpenError::Sqlite)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .map_err(OpenError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)?;

    for (seq, indexed_kind, bytes) in rows {
        let row = seq.to_string();
        let mut document = decode("fact_envelopes", &row, &bytes)?;
        let fact = member_object(
            "fact_envelopes",
            &row,
            object("fact_envelopes", &row, &mut document)?,
            "fact",
        )?;
        let changed = normalize_optional_record(
            "fact_envelopes",
            &row,
            fact.get_mut("renewal"),
            "fact.renewal",
        )?;
        let intermediate = encoded_value("fact_envelopes", &row, &document)?;
        let envelope: wire::Envelope<wire::AnyFact> =
            wire::decode(&intermediate).map_err(|error| {
                invalid(
                    "fact_envelopes",
                    &row,
                    &format!("migrated renewal envelope is not current wire: {error}"),
                )
            })?;
        if u64::try_from(seq).ok() != Some(envelope.seq.get())
            || indexed_kind != envelope.fact.kind().as_str()
        {
            return Err(invalid(
                "fact_envelopes",
                &row,
                "indexed renewal fact identity disagrees with its envelope",
            ));
        }
        let canonical = wire::encode(&envelope).map_err(|error| {
            invalid(
                "fact_envelopes",
                &row,
                &format!("migrated renewal envelope cannot encode canonically: {error}"),
            )
        })?;
        require_unchanged_canonical("fact_envelopes", &row, changed, &bytes, &canonical)?;
        if changed {
            connection
                .execute(
                    "UPDATE fact_envelopes SET envelope = ?1 WHERE seq = ?2",
                    params![canonical, seq],
                )
                .map_err(OpenError::Sqlite)?;
        }
    }
    Ok(())
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
    let head = object("residency_head", "1", &mut document)?;
    let changed = normalize_records("residency_head", "1", head.get_mut("renewals"), "renewals")?;
    let intermediate = encoded_value("residency_head", "1", &document)?;
    let head: wire::ResidencyHead = wire::decode(&intermediate).map_err(|error| {
        invalid(
            "residency_head",
            "1",
            &format!("migrated residency head is not current wire: {error}"),
        )
    })?;
    let canonical = wire::encode(&head).map_err(|error| {
        invalid(
            "residency_head",
            "1",
            &format!("migrated residency head cannot encode canonically: {error}"),
        )
    })?;
    require_unchanged_canonical("residency_head", "1", changed, &bytes, &canonical)?;
    if changed {
        connection
            .execute(
                "UPDATE residency_head SET body = ?1 WHERE singleton = 1",
                [canonical],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn migrate_receipts(connection: &Connection) -> Result<(), OpenError> {
    let mut statement = connection
        .prepare(
            "SELECT command_id, command_kind, response FROM command_receipts \
             WHERE command_kind IN (\
                 'board::read', 'lane::read', 'lane::arrange', 'lane::renewal', \
                 'lineage::configure', 'lineage::arrange'\
             ) ORDER BY command_id",
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
        let mut document = decode("command_receipts", &command, &bytes)?;
        let changed = normalize_response(&command, &kind, &mut document)?;
        let intermediate = encoded_value("command_receipts", &command, &document)?;
        let canonical = match kind.as_str() {
            "board::read" => canonical_response::<wire::intent::BoardAnswer>(
                "command_receipts",
                &command,
                &intermediate,
                "board response",
            )?,
            "lane::renewal" => canonical_response::<wire::intent::LaneRenewalAnswer>(
                "command_receipts",
                &command,
                &intermediate,
                "lane renewal response",
            )?,
            kind if LANE_ANSWER_KINDS.contains(&kind) => {
                canonical_response::<wire::intent::LaneAnswer>(
                    "command_receipts",
                    &command,
                    &intermediate,
                    "lane response",
                )?
            }
            _ => {
                return Err(invalid(
                    "command_receipts",
                    &command,
                    "renewal response kind is not registered by the migration",
                ));
            }
        };
        require_unchanged_canonical("command_receipts", &command, changed, &bytes, &canonical)?;
        if changed {
            connection
                .execute(
                    "UPDATE command_receipts SET response = ?1 WHERE command_id = ?2",
                    params![canonical, command],
                )
                .map_err(OpenError::Sqlite)?;
        }
    }
    Ok(())
}

fn normalize_response(row: &str, kind: &str, document: &mut Value) -> Result<bool, OpenError> {
    let response = object("command_receipts", row, document)?;
    let outcome = member_object("command_receipts", row, response, "outcome")?;
    match outcome.get("kind").and_then(Value::as_str) {
        Some("refusal" | "fault") => Ok(false),
        Some("reply") => {
            let body = member_object("command_receipts", row, outcome, "body")?;
            match kind {
                "board::read" => {
                    let residency = member_object("command_receipts", row, body, "residency")?;
                    let residency = member_object("command_receipts", row, residency, "body")?;
                    normalize_records(
                        "command_receipts",
                        row,
                        residency.get_mut("renewals"),
                        "outcome.body.residency.body.renewals",
                    )
                }
                "lane::renewal" => normalize_optional_record(
                    "command_receipts",
                    row,
                    body.get_mut("renewal"),
                    "outcome.body.renewal",
                ),
                kind if LANE_ANSWER_KINDS.contains(&kind) => normalize_records(
                    "command_receipts",
                    row,
                    body.get_mut("renewals"),
                    "outcome.body.renewals",
                ),
                _ => Err(invalid(
                    "command_receipts",
                    row,
                    "renewal response kind is not registered by the migration",
                )),
            }
        }
        Some(other) => Err(invalid(
            "command_receipts",
            row,
            &format!("response outcome kind {other:?} is unknown"),
        )),
        None => Err(invalid(
            "command_receipts",
            row,
            "response outcome kind is missing or not a string",
        )),
    }
}

fn normalize_records(
    table: &'static str,
    row: &str,
    value: Option<&mut Value>,
    path: &str,
) -> Result<bool, OpenError> {
    let Some(value) = value else {
        return Ok(false);
    };
    let records = value
        .as_array_mut()
        .ok_or_else(|| invalid(table, row, &format!("{path} is not an array")))?;
    let mut changed = false;
    for (index, record) in records.iter_mut().enumerate() {
        changed |= normalize_record(table, row, record, &format!("{path}[{index}]"))?;
    }
    Ok(changed)
}

fn normalize_optional_record(
    table: &'static str,
    row: &str,
    value: Option<&mut Value>,
    path: &str,
) -> Result<bool, OpenError> {
    match value {
        None | Some(Value::Null) => Ok(false),
        Some(record) => normalize_record(table, row, record, path),
    }
}

fn normalize_record(
    table: &'static str,
    row: &str,
    record: &mut Value,
    path: &str,
) -> Result<bool, OpenError> {
    let record = record
        .as_object_mut()
        .ok_or_else(|| invalid(table, row, &format!("{path} is not an object")))?;
    let state = member_object(table, row, record, "state")?;
    match state.get("state").and_then(Value::as_str) {
        Some("requested" | "handoff-pending" | "successor-started" | "launch-refused") => {}
        Some(found) => {
            return Err(invalid(
                table,
                row,
                &format!("{path}.state names unknown renewal state {found:?}"),
            ));
        }
        None => {
            return Err(invalid(
                table,
                row,
                &format!("{path}.state.state is missing or not a string"),
            ));
        }
    }
    match (
        state.contains_key("team_slot"),
        state.contains_key("org_slot"),
    ) {
        (true, false) => {
            let slot = state
                .remove("team_slot")
                .expect("retired slot was just observed");
            state.insert("org_slot".to_owned(), slot);
            Ok(true)
        }
        (false, true) => Ok(false),
        (true, true) => Err(invalid(
            table,
            row,
            &format!("{path}.state carries both team_slot and org_slot"),
        )),
        (false, false) => Err(invalid(
            table,
            row,
            &format!("{path}.state carries neither team_slot nor org_slot"),
        )),
    }
}

fn canonical_response<B>(
    table: &'static str,
    row: &str,
    bytes: &[u8],
    description: &str,
) -> Result<Vec<u8>, OpenError>
where
    B: DeserializeOwned + Serialize,
{
    let _: wire::Response<B> = wire::decode(bytes).map_err(|error| {
        invalid(
            table,
            row,
            &format!("migrated {description} is not current wire: {error}"),
        )
    })?;
    let response: wire::Response<Value> = wire::decode(bytes).map_err(|error| {
        invalid(
            table,
            row,
            &format!("migrated {description} is not a receipt envelope: {error}"),
        )
    })?;
    wire::encode(&response).map_err(|error| {
        invalid(
            table,
            row,
            &format!("migrated {description} cannot encode canonically: {error}"),
        )
    })
}

fn require_unchanged_canonical(
    table: &'static str,
    row: &str,
    changed: bool,
    stored: &[u8],
    canonical: &[u8],
) -> Result<(), OpenError> {
    if !changed && stored != canonical {
        return Err(invalid(
            table,
            row,
            "current-spelling carrier is not canonical current wire",
        ));
    }
    Ok(())
}

fn decode(table: &'static str, row: &str, bytes: &[u8]) -> Result<Value, OpenError> {
    serde_json::from_slice(bytes)
        .map_err(|error| invalid(table, row, &format!("stored JSON is invalid: {error}")))
}

fn encoded_value(table: &'static str, row: &str, value: &Value) -> Result<Vec<u8>, OpenError> {
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
        .ok_or_else(|| invalid(table, row, "carrier is not an object"))
}

fn member_object<'a>(
    table: &'static str,
    row: &str,
    object: &'a mut Map<String, Value>,
    member: &str,
) -> Result<&'a mut Map<String, Value>, OpenError> {
    object
        .get_mut(member)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| invalid(table, row, &format!("{member} is missing or not an object")))
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
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn all_retired_states_rename_once_and_current_states_stay_current() {
        for name in [
            "requested",
            "handoff-pending",
            "successor-started",
            "launch-refused",
        ] {
            let mut retired = json!({"state": {"state": name, "team_slot": 2}});
            assert!(normalize_record("fixture", "1", &mut retired, "renewal").unwrap());
            assert_eq!(retired["state"]["org_slot"], 2);
            assert!(retired["state"].get("team_slot").is_none());
            assert!(!normalize_record("fixture", "1", &mut retired, "renewal").unwrap());
        }
    }

    #[test]
    fn ambiguous_or_missing_slot_refuses() {
        for mut state in [
            json!({"state": {"state": "requested", "team_slot": 1, "org_slot": 1}}),
            json!({"state": {"state": "requested"}}),
        ] {
            let error = normalize_record("fixture", "row", &mut state, "renewal")
                .expect_err("ambiguous renewal refuses");
            assert!(matches!(
                error,
                OpenError::Migration(MigrationError::StoredContent {
                    from: 16,
                    to: 17,
                    ..
                })
            ));
        }
    }
}
