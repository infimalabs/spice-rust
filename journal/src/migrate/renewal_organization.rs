//! Schema 16 to 17 retirement of the renewal-state `team_slot` key.
//!
//! Renewal state can rest in a fact, the residency singleton, or a typed
//! command answer. Each carrier is normalized at its declared path, decoded
//! through its current wire type, and written only when its predecessor bytes
//! differ by the retired key.

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use super::{MigrationError, OpenError};

const FROM: u32 = 16;
const TO: u32 = 17;
const CURRENT_SLOT: &[u8] = br#""org_slot":"#;
const RETIRED_SLOT: &[u8] = br#""team_slot":"#;
const RECEIPT_KINDS: &[&str] = &[
    "board::read",
    "lane::read",
    "lane::arrange",
    "lineage::configure",
    "lineage::arrange",
    "lane::renewal",
];

pub(super) fn migrate(connection: &Connection) -> Result<(), OpenError> {
    migrate_facts(connection)?;
    migrate_residency_head(connection)?;
    migrate_receipts(connection)
}

fn migrate_facts(connection: &Connection) -> Result<(), OpenError> {
    let rows = fact_rows(connection)?;
    for (seq, indexed_kind, original) in rows {
        let row = seq.to_string();
        let mut document = decode("fact_envelopes", &row, &original)?;
        let fact = required_object_field(
            "fact_envelopes",
            &row,
            object("fact_envelopes", &row, &mut document)?,
            "fact",
            "envelope.fact",
        )?;
        let mut retired = Vec::new();
        normalize_optional_record(
            "fact_envelopes",
            &row,
            fact,
            "renewal",
            "fact.renewal",
            &mut retired,
        )?;
        let canonical = canonical_fact(&row, seq, &indexed_kind, &document)?;
        if let Some(canonical) =
            changed_bytes("fact_envelopes", &row, &original, canonical, &retired)?
        {
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
    let Some(original) = connection
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
    let mut document = decode("residency_head", "1", &original)?;
    let mut retired = Vec::new();
    normalize_records(
        "residency_head",
        "1",
        object("residency_head", "1", &mut document)?,
        "renewals",
        "body.renewals",
        &mut retired,
    )?;
    let canonical = canonical::<wire::ResidencyHead>("residency_head", "1", &document)?;
    if let Some(canonical) = changed_bytes("residency_head", "1", &original, canonical, &retired)? {
        connection
            .execute(
                "UPDATE residency_head SET body = ?1 WHERE singleton = 1",
                params![canonical],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn migrate_receipts(connection: &Connection) -> Result<(), OpenError> {
    let rows = receipt_rows(connection)?;
    for (command, kind, original) in rows {
        if !RECEIPT_KINDS.contains(&kind.as_str()) {
            return Err(invalid(
                "command_receipts",
                &command,
                "renewal-bearing response kind is not registered by the migration",
            ));
        }
        let mut document = decode("command_receipts", &command, &original)?;
        let outcome_kind = response_outcome_kind(&command, &mut document)?;
        let mut retired = Vec::new();
        match outcome_kind.as_str() {
            "refusal" | "fault" => {}
            "reply" => {
                let body = reply_body("command_receipts", &command, &mut document)?;
                match kind.as_str() {
                    "board::read" => {
                        let residency = required_object_field(
                            "command_receipts",
                            &command,
                            body,
                            "residency",
                            "response.outcome.body.residency",
                        )?;
                        let residency = required_object_field(
                            "command_receipts",
                            &command,
                            residency,
                            "body",
                            "response.outcome.body.residency.body",
                        )?;
                        normalize_records(
                            "command_receipts",
                            &command,
                            residency,
                            "renewals",
                            "response.outcome.body.residency.body.renewals",
                            &mut retired,
                        )?;
                    }
                    "lane::read" | "lane::arrange" | "lineage::configure" | "lineage::arrange" => {
                        normalize_records(
                            "command_receipts",
                            &command,
                            body,
                            "renewals",
                            "response.outcome.body.renewals",
                            &mut retired,
                        )?;
                    }
                    "lane::renewal" => normalize_optional_record(
                        "command_receipts",
                        &command,
                        body,
                        "renewal",
                        "response.outcome.body.renewal",
                        &mut retired,
                    )?,
                    _ => unreachable!("receipt query and registry are the same closed set"),
                }
            }
            _ => {
                return Err(invalid(
                    "command_receipts",
                    &command,
                    &format!("response outcome kind {outcome_kind:?} is unknown"),
                ));
            }
        }
        let canonical = canonical_response(&command, &kind, &document)?;
        if let Some(canonical) =
            changed_bytes("command_receipts", &command, &original, canonical, &retired)?
        {
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

fn normalize_records(
    table: &'static str,
    row: &str,
    parent: &mut Map<String, Value>,
    field: &str,
    path: &str,
    retired: &mut Vec<bool>,
) -> Result<(), OpenError> {
    let Some(value) = parent.get_mut(field) else {
        return Ok(());
    };
    let records = value
        .as_array_mut()
        .ok_or_else(|| invalid(table, row, &format!("{path} is not an array")))?;
    for (index, record) in records.iter_mut().enumerate() {
        normalize_record(table, row, record, &format!("{path}[{index}]"), retired)?;
    }
    Ok(())
}

fn normalize_optional_record(
    table: &'static str,
    row: &str,
    parent: &mut Map<String, Value>,
    field: &str,
    path: &str,
    retired: &mut Vec<bool>,
) -> Result<(), OpenError> {
    match parent.get_mut(field) {
        None | Some(Value::Null) => Ok(()),
        Some(record) => normalize_record(table, row, record, path, retired),
    }
}

fn normalize_record(
    table: &'static str,
    row: &str,
    record: &mut Value,
    path: &str,
    retired: &mut Vec<bool>,
) -> Result<(), OpenError> {
    let record = object_at(table, row, record, path)?;
    let state = record
        .get_mut("state")
        .ok_or_else(|| invalid(table, row, &format!("{path}.state is missing")))?;
    let state = object_at(table, row, state, &format!("{path}.state"))?;
    let has_retired = state.contains_key("team_slot");
    let has_current = state.contains_key("org_slot");
    match (has_retired, has_current) {
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
        (true, false) => {
            let slot = state
                .remove("team_slot")
                .expect("the retired slot was just observed");
            state.insert("org_slot".to_owned(), slot);
            retired.push(true);
            Ok(())
        }
        (false, true) => {
            retired.push(false);
            Ok(())
        }
    }
}

fn changed_bytes(
    table: &'static str,
    row: &str,
    original: &[u8],
    canonical: Vec<u8>,
    retired: &[bool],
) -> Result<Option<Vec<u8>>, OpenError> {
    let predecessor = predecessor_bytes(table, row, &canonical, retired)?;
    if predecessor != original {
        return Err(invalid(
            table,
            row,
            "stored bytes are not canonical schema-16 renewal wire",
        ));
    }
    Ok(retired.iter().any(|value| *value).then_some(canonical))
}

fn predecessor_bytes(
    table: &'static str,
    row: &str,
    canonical: &[u8],
    retired: &[bool],
) -> Result<Vec<u8>, OpenError> {
    let mut output = Vec::with_capacity(canonical.len() + retired.len());
    let mut cursor = 0;
    for &was_retired in retired {
        let Some(relative) = find(&canonical[cursor..], CURRENT_SLOT) else {
            return Err(invalid(
                table,
                row,
                "typed renewal state has no current slot key",
            ));
        };
        let start = cursor + relative;
        output.extend_from_slice(&canonical[cursor..start]);
        output.extend_from_slice(if was_retired {
            RETIRED_SLOT
        } else {
            CURRENT_SLOT
        });
        cursor = start + CURRENT_SLOT.len();
    }
    if find(&canonical[cursor..], CURRENT_SLOT).is_some() {
        return Err(invalid(
            table,
            row,
            "typed carrier contains an unvisited renewal slot",
        ));
    }
    output.extend_from_slice(&canonical[cursor..]);
    Ok(output)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn canonical_fact(
    row: &str,
    seq: i64,
    indexed_kind: &str,
    value: &Value,
) -> Result<Vec<u8>, OpenError> {
    let bytes = intermediate("fact_envelopes", row, value)?;
    let envelope: wire::Envelope<wire::AnyFact> = wire::decode(&bytes).map_err(|error| {
        invalid(
            "fact_envelopes",
            row,
            &format!("migrated envelope is not current fact wire: {error}"),
        )
    })?;
    if u64::try_from(seq).ok() != Some(envelope.seq.get())
        || indexed_kind != envelope.fact.kind().as_str()
        || envelope.fact.kind() != wire::FactKind::LaneRenewalChanged
    {
        return Err(invalid(
            "fact_envelopes",
            row,
            "indexed renewal fact identity disagrees with its envelope",
        ));
    }
    wire::encode(&envelope).map_err(|error| {
        invalid(
            "fact_envelopes",
            row,
            &format!("migrated envelope cannot encode canonically: {error}"),
        )
    })
}

fn canonical_response(row: &str, kind: &str, value: &Value) -> Result<Vec<u8>, OpenError> {
    match kind {
        "board::read" => canonical_receipt::<wire::intent::BoardAnswer>(row, value),
        "lane::read" | "lane::arrange" | "lineage::configure" | "lineage::arrange" => {
            canonical_receipt::<wire::intent::LaneAnswer>(row, value)
        }
        "lane::renewal" => canonical_receipt::<wire::intent::LaneRenewalAnswer>(row, value),
        _ => Err(invalid(
            "command_receipts",
            row,
            "renewal-bearing response kind is not registered by the migration",
        )),
    }
}

fn canonical_receipt<B>(row: &str, value: &Value) -> Result<Vec<u8>, OpenError>
where
    B: DeserializeOwned + Serialize,
{
    let bytes = intermediate("command_receipts", row, value)?;
    let response: wire::Response<B> = wire::decode(&bytes).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("migrated response is not its command-kind wire: {error}"),
        )
    })?;
    let canonical = wire::encode(&response).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("migrated response cannot encode canonically: {error}"),
        )
    })?;
    // Receipts are stored and verified through the generic envelope. Project
    // only after the typed round trip so command-specific defaults and
    // validation govern the body while the durable envelope keeps its wire.
    let generic: wire::Response<Value> = wire::decode(&canonical).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("typed response cannot project predecessor wire: {error}"),
        )
    })?;
    wire::encode(&generic).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("typed response cannot encode canonical receipt wire: {error}"),
        )
    })
}

fn canonical<T>(table: &'static str, row: &str, value: &Value) -> Result<Vec<u8>, OpenError>
where
    T: DeserializeOwned + Serialize,
{
    let bytes = intermediate(table, row, value)?;
    let typed: T = wire::decode(&bytes).map_err(|error| {
        invalid(
            table,
            row,
            &format!("migrated carrier is not current typed wire: {error}"),
        )
    })?;
    wire::encode(&typed).map_err(|error| {
        invalid(
            table,
            row,
            &format!("migrated carrier cannot encode canonically: {error}"),
        )
    })
}

fn reply_body<'a>(
    table: &'static str,
    row: &str,
    value: &'a mut Value,
) -> Result<&'a mut Map<String, Value>, OpenError> {
    let root = object(table, row, value)?;
    let outcome = required_object_field(table, row, root, "outcome", "response.outcome")?;
    required_object_field(table, row, outcome, "body", "response.outcome.body")
}

fn response_outcome_kind(row: &str, value: &mut Value) -> Result<String, OpenError> {
    let root = object("command_receipts", row, value)?;
    let outcome =
        required_object_field("command_receipts", row, root, "outcome", "response.outcome")?;
    outcome
        .get("kind")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            invalid(
                "command_receipts",
                row,
                "response outcome kind is missing or not a string",
            )
        })
}

fn required_object_field<'a>(
    table: &'static str,
    row: &str,
    parent: &'a mut Map<String, Value>,
    field: &str,
    path: &str,
) -> Result<&'a mut Map<String, Value>, OpenError> {
    let value = parent
        .get_mut(field)
        .ok_or_else(|| invalid(table, row, &format!("{path} is missing")))?;
    object_at(table, row, value, path)
}

fn object<'a>(
    table: &'static str,
    row: &str,
    value: &'a mut Value,
) -> Result<&'a mut Map<String, Value>, OpenError> {
    object_at(table, row, value, "document")
}

fn object_at<'a>(
    table: &'static str,
    row: &str,
    value: &'a mut Value,
    path: &str,
) -> Result<&'a mut Map<String, Value>, OpenError> {
    value
        .as_object_mut()
        .ok_or_else(|| invalid(table, row, &format!("{path} is not an object")))
}

fn decode(table: &'static str, row: &str, bytes: &[u8]) -> Result<Value, OpenError> {
    serde_json::from_slice(bytes)
        .map_err(|error| invalid(table, row, &format!("stored JSON is invalid: {error}")))
}

fn intermediate(table: &'static str, row: &str, value: &Value) -> Result<Vec<u8>, OpenError> {
    serde_json::to_vec(value)
        .map_err(|error| invalid(table, row, &format!("migrated JSON cannot encode: {error}")))
}

type FactRow = (i64, String, Vec<u8>);

fn fact_rows(connection: &Connection) -> Result<Vec<FactRow>, OpenError> {
    let mut statement = connection
        .prepare(
            "SELECT seq, kind, envelope FROM fact_envelopes \
             WHERE kind = 'lane::renewal' ORDER BY seq",
        )
        .map_err(OpenError::Sqlite)?;
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(OpenError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)
}

type Receipt = (String, String, Vec<u8>);

fn receipt_rows(connection: &Connection) -> Result<Vec<Receipt>, OpenError> {
    let mut statement = connection
        .prepare(
            "SELECT command_id, command_kind, response FROM command_receipts \
             WHERE command_kind IN ('board::read', 'lane::read', 'lane::arrange', \
                                    'lineage::configure', 'lineage::arrange', \
                                    'lane::renewal') \
             ORDER BY command_id",
        )
        .map_err(OpenError::Sqlite)?;
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
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
