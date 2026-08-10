//! Schema 5 to 6 materialization of optimistic Canvas topology authority.
//!
//! Schema 5 already records every topology mutation as exactly one resident
//! fact. This step derives each historical revision from that ordered fact
//! history, then writes the required field into the current residency head and
//! every affected command receipt. A carrier that already names the exact
//! derived revision crosses byte-for-byte; any different value refuses rather
//! than opening a second revision authority.

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Map, Value};

use super::{MigrationError, OpenError};

const FROM: u32 = 5;
const TO: u32 = 6;

const TOPOLOGY_FACT_KINDS: &[&str] = &[
    "lane::arranged",
    "lane::seated",
    "lineage::arranged",
    "lineage::configured",
];

const TOPOLOGY_COMMAND_KINDS: &[&str] = &[
    "lane::arrange",
    "lane::seat",
    "lineage::arrange",
    "lineage::configure",
];

const LANE_RESPONSE_COMMAND_KINDS: &[&str] = &[
    "lane::arrange",
    "lane::seat",
    "lane::state",
    "lineage::arrange",
    "lineage::configure",
];

pub(super) fn migrate(connection: &Connection) -> Result<(), OpenError> {
    let topology = topology_positions(connection)?;
    migrate_residency_head(connection, &topology)?;
    migrate_receipts(connection, &topology)
}

fn topology_positions(connection: &Connection) -> Result<Vec<u64>, OpenError> {
    let mut statement = connection
        .prepare(
            "SELECT seq, kind, envelope FROM fact_envelopes \
             WHERE kind IN ('lane::arranged', 'lane::seated', \
                            'lineage::arranged', 'lineage::configured') \
             ORDER BY seq",
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

    rows.into_iter()
        .map(|(seq, kind, bytes)| {
            let position = u64::try_from(seq).map_err(|_| {
                invalid(
                    "fact_envelopes",
                    &seq.to_string(),
                    "topology fact position is not a positive wire sequence",
                )
            })?;
            let envelope: wire::Envelope<wire::AnyFact> =
                wire::decode(&bytes).map_err(|error| {
                    invalid(
                        "fact_envelopes",
                        &seq.to_string(),
                        &format!("topology envelope is not current wire: {error}"),
                    )
                })?;
            if envelope.seq.get() != position || envelope.fact.kind().as_str() != kind {
                return Err(invalid(
                    "fact_envelopes",
                    &seq.to_string(),
                    "topology fact row and envelope identity disagree",
                ));
            }
            if !TOPOLOGY_FACT_KINDS.contains(&kind.as_str()) {
                return Err(invalid(
                    "fact_envelopes",
                    &seq.to_string(),
                    "topology fact kind is not registered by the migration",
                ));
            }
            Ok(position)
        })
        .collect()
}

fn migrate_residency_head(connection: &Connection, topology: &[u64]) -> Result<(), OpenError> {
    let Some((head_seq, bytes)) = connection
        .query_row(
            "SELECT head_seq, body FROM residency_head WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()
        .map_err(OpenError::Sqlite)?
    else {
        return Ok(());
    };
    let head_seq = sequence("residency_head", "1", head_seq)?;
    let revision = revision_at(topology, head_seq);
    let mut document = decode("residency_head", "1", &bytes)?;
    let changed = normalize_revision(
        "residency_head",
        "1",
        object("residency_head", "1", &mut document)?,
        revision,
    )?;
    let canonical = canonical_residency("1", &document)?;
    if changed {
        connection
            .execute(
                "UPDATE residency_head SET body = ?1 WHERE singleton = 1",
                params![canonical],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn migrate_receipts(connection: &Connection, topology: &[u64]) -> Result<(), OpenError> {
    let mut statement = connection
        .prepare(
            "SELECT command_id, command_kind, request, response, first_seq, last_seq \
             FROM command_receipts \
             WHERE command_kind IN ('lane::arrange', 'lane::seat', 'lane::state', \
                                    'lineage::arrange', 'lineage::configure') \
             ORDER BY command_id",
        )
        .map_err(OpenError::Sqlite)?;
    let rows = statement
        .query_map([], |row| {
            Ok(Receipt {
                command: row.get(0)?,
                kind: row.get(1)?,
                request: row.get(2)?,
                response: row.get(3)?,
                first: row.get(4)?,
                last: row.get(5)?,
            })
        })
        .map_err(OpenError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)?;

    for receipt in rows {
        migrate_receipt(connection, topology, receipt)?;
    }
    Ok(())
}

struct Receipt {
    command: String,
    kind: String,
    request: Vec<u8>,
    response: Vec<u8>,
    first: Option<i64>,
    last: Option<i64>,
}

fn migrate_receipt(
    connection: &Connection,
    topology: &[u64],
    receipt: Receipt,
) -> Result<(), OpenError> {
    if !LANE_RESPONSE_COMMAND_KINDS.contains(&receipt.kind.as_str()) {
        return Err(invalid(
            "command_receipts",
            &receipt.command,
            "lane response command kind is not registered by the migration",
        ));
    }
    let first = committed_sequence(&receipt, receipt.first, "first_seq")?;
    let last = committed_sequence(&receipt, receipt.last, "last_seq")?;
    if first > last {
        return Err(invalid(
            "command_receipts",
            &receipt.command,
            "lane receipt has a reversed committed range",
        ));
    }
    let before = revision_before(topology, first);
    let after = revision_at(topology, last);
    let topology_mutation = TOPOLOGY_COMMAND_KINDS.contains(&receipt.kind.as_str());
    let expected_after = before
        .checked_add(u64::from(topology_mutation))
        .ok_or_else(|| {
            invalid(
                "command_receipts",
                &receipt.command,
                "derived topology revision overflows",
            )
        })?;
    if after != expected_after {
        return Err(invalid(
            "command_receipts",
            &receipt.command,
            "lane receipt range does not contain its one expected topology mutation",
        ));
    }

    let request = if topology_mutation {
        let mut document = decode("command_receipts", &receipt.command, &receipt.request)?;
        let body = body(
            "command_receipts",
            &receipt.command,
            &mut document,
            "request.body",
        )?;
        let changed = normalize_revision("command_receipts", &receipt.command, body, before)?;
        let canonical = canonical_request(&receipt.command, &receipt.kind, &document)?;
        changed.then_some(canonical)
    } else {
        None
    };

    let mut document = decode("command_receipts", &receipt.command, &receipt.response)?;
    let outcome = document.get_mut("outcome").ok_or_else(|| {
        invalid(
            "command_receipts",
            &receipt.command,
            "response.outcome is missing",
        )
    })?;
    let response_body = body(
        "command_receipts",
        &receipt.command,
        outcome,
        "response.outcome.body",
    )?;
    let response_changed =
        normalize_revision("command_receipts", &receipt.command, response_body, after)?;
    let canonical = canonical_response(&receipt.command, &receipt.kind, &document)?;
    let response = response_changed.then_some(canonical);

    match (request, response) {
        (Some(request), Some(response)) => connection
            .execute(
                "UPDATE command_receipts SET request = ?1, response = ?2 WHERE command_id = ?3",
                params![request, response, receipt.command],
            )
            .map(|_| ())
            .map_err(OpenError::Sqlite),
        (Some(request), None) => connection
            .execute(
                "UPDATE command_receipts SET request = ?1 WHERE command_id = ?2",
                params![request, receipt.command],
            )
            .map(|_| ())
            .map_err(OpenError::Sqlite),
        (None, Some(response)) => connection
            .execute(
                "UPDATE command_receipts SET response = ?1 WHERE command_id = ?2",
                params![response, receipt.command],
            )
            .map(|_| ())
            .map_err(OpenError::Sqlite),
        (None, None) => Ok(()),
    }
}

fn committed_sequence(
    receipt: &Receipt,
    value: Option<i64>,
    field: &str,
) -> Result<u64, OpenError> {
    let value = value.ok_or_else(|| {
        invalid(
            "command_receipts",
            &receipt.command,
            &format!("{} receipt has no {field}", receipt.kind),
        )
    })?;
    sequence("command_receipts", &receipt.command, value)
}

fn sequence(table: &'static str, row: &str, value: i64) -> Result<u64, OpenError> {
    u64::try_from(value).map_err(|_| invalid(table, row, "journal position is not positive"))
}

fn revision_before(topology: &[u64], seq: u64) -> u64 {
    u64::try_from(topology.partition_point(|position| *position < seq))
        .expect("topology position count fits u64")
}

fn revision_at(topology: &[u64], seq: u64) -> u64 {
    u64::try_from(topology.partition_point(|position| *position <= seq))
        .expect("topology position count fits u64")
}

fn normalize_revision(
    table: &'static str,
    row: &str,
    body: &mut Map<String, Value>,
    revision: u64,
) -> Result<bool, OpenError> {
    match body.get("topology_revision") {
        None => {
            body.insert("topology_revision".to_owned(), Value::from(revision));
            Ok(true)
        }
        Some(value) if value.as_u64() == Some(revision) => Ok(false),
        Some(_) => Err(invalid(
            table,
            row,
            &format!("topology_revision does not equal derived revision {revision}"),
        )),
    }
}

fn canonical_residency(row: &str, value: &Value) -> Result<Vec<u8>, OpenError> {
    let bytes = intermediate("residency_head", row, value)?;
    let head: wire::ResidencyHead = wire::decode(&bytes).map_err(|error| {
        invalid(
            "residency_head",
            row,
            &format!("migrated body is not current residency wire: {error}"),
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

fn canonical_request(row: &str, kind: &str, value: &Value) -> Result<Vec<u8>, OpenError> {
    let bytes = intermediate("command_receipts", row, value)?;
    match kind {
        "lane::arrange" => encode_command::<wire::intent::LaneArrange>(row, &bytes),
        "lane::seat" => encode_command::<wire::intent::LaneSeat>(row, &bytes),
        "lineage::arrange" => encode_command::<wire::intent::LineageArrange>(row, &bytes),
        "lineage::configure" => encode_command::<wire::intent::LineageConfigure>(row, &bytes),
        _ => Err(invalid(
            "command_receipts",
            row,
            "topology request command kind is not registered by the migration",
        )),
    }
}

fn encode_command<B>(row: &str, bytes: &[u8]) -> Result<Vec<u8>, OpenError>
where
    B: serde::de::DeserializeOwned + serde::Serialize,
{
    let request: wire::Command<B> = wire::decode(bytes).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("migrated request is not current command wire: {error}"),
        )
    })?;
    request
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

fn canonical_response(row: &str, kind: &str, value: &Value) -> Result<Vec<u8>, OpenError> {
    let bytes = intermediate("command_receipts", row, value)?;
    match kind {
        "lane::seat" | "lane::state" => {
            encode_response::<wire::intent::LaneChangeAnswer>(row, &bytes)
        }
        "lane::arrange" | "lineage::arrange" | "lineage::configure" => {
            encode_response::<wire::intent::LaneAnswer>(row, &bytes)
        }
        _ => Err(invalid(
            "command_receipts",
            row,
            "lane response command kind is not registered by the migration",
        )),
    }
}

fn encode_response<B>(row: &str, bytes: &[u8]) -> Result<Vec<u8>, OpenError>
where
    B: serde::de::DeserializeOwned + serde::Serialize,
{
    let _: wire::Response<B> = wire::decode(bytes).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("migrated response is not current reply wire: {error}"),
        )
    })?;
    let response: wire::Response<Value> = wire::decode(bytes).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("migrated response is not generic receipt wire: {error}"),
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

fn intermediate(table: &'static str, row: &str, value: &Value) -> Result<Vec<u8>, OpenError> {
    serde_json::to_vec(value)
        .map_err(|error| invalid(table, row, &format!("migrated JSON cannot encode: {error}")))
}

fn decode(table: &'static str, row: &str, bytes: &[u8]) -> Result<Value, OpenError> {
    serde_json::from_slice(bytes)
        .map_err(|error| invalid(table, row, &format!("stored JSON is invalid: {error}")))
}

fn body<'a>(
    table: &'static str,
    row: &str,
    value: &'a mut Value,
    locus: &str,
) -> Result<&'a mut Map<String, Value>, OpenError> {
    value
        .get_mut("body")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| invalid(table, row, &format!("{locus} is missing or not an object")))
}

fn object<'a>(
    table: &'static str,
    row: &str,
    value: &'a mut Value,
) -> Result<&'a mut Map<String, Value>, OpenError> {
    value
        .as_object_mut()
        .ok_or_else(|| invalid(table, row, "topology carrier is not an object"))
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
