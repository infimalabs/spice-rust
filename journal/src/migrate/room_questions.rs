use std::collections::BTreeSet;

use rusqlite::params;

use crate::{MigrationError, OpenError};

const RELEASE: &str = "963a54c37";

struct RetiredFactRow {
    seq: i64,
    wire_version: i64,
    instant: String,
    actor: String,
    origin: String,
    cause: Option<i64>,
    envelope: Vec<u8>,
    kind: String,
}

struct DecodedRow {
    row: RetiredFactRow,
    envelope: wire::Envelope<serde_json::Value>,
}

/// Retire the deleted question protocol before any current decoder observes it.
pub(super) fn cross(transaction: &rusqlite::Transaction<'_>) -> Result<(), OpenError> {
    crate::historical_receipts::create_schema(transaction)?;
    crate::historical_receipts::retire_room_questions(transaction)?;
    crate::historical_receipts::reserve_identities(transaction)?;
    retire_facts(transaction)?;
    drop_projections(transaction)
}

fn retire_facts(transaction: &rusqlite::Transaction<'_>) -> Result<(), OpenError> {
    let rows = read_candidate_rows(transaction)?;
    let mut results = BTreeSet::new();
    let mut publication_positions = BTreeSet::new();
    for decoded in &rows {
        let fact = &decoded.envelope.fact;
        if question_publication(fact) {
            if let Some(result) = operation_result(fact) {
                results.insert(result.to_owned());
            }
            if let Ok(seq) = u64::try_from(decoded.row.seq) {
                publication_positions.insert(wire::SequenceCode::new(seq).to_string());
            }
        }
        if matches!(decoded.row.kind.as_str(), "room::closed" | "room::timedout") {
            if let Some(result) = fact.get("result").and_then(serde_json::Value::as_str) {
                results.insert(result.to_owned());
            }
        }
    }

    let mut retired = BTreeSet::new();
    for decoded in rows {
        let fact = &decoded.envelope.fact;
        let initial = question_publication(fact)
            || matches!(decoded.row.kind.as_str(), "room::closed" | "room::timedout");
        let dependent = match decoded.row.kind.as_str() {
            "room::disposed" => fact
                .get("keys")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|keys| {
                    keys.iter()
                        .filter_map(serde_json::Value::as_str)
                        .any(|key| results.contains(key))
                }),
            "room::lapsed" | "room::recovered" => fact
                .get("receipt")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|receipt| results.contains(receipt)),
            "room::delivered" => fact
                .get("through")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|through| publication_positions.contains(through)),
            _ => false,
        };
        if !initial && !dependent {
            continue;
        }
        retired.insert(decoded.row.seq);
        retire_row(transaction, decoded)?;
    }
    repair_projections(transaction, &retired)
}

fn read_candidate_rows(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<Vec<DecodedRow>, OpenError> {
    let rows = {
        let mut statement = transaction
            .prepare(
                "SELECT seq, wire_version, instant, actor, origin, cause, envelope, kind \
                 FROM fact_envelopes \
                 WHERE kind IN ('room::closed', 'room::timedout', 'room::conversed', \
                                'room::posted', 'room::disposed', 'room::lapsed', \
                                'room::recovered', 'room::delivered') \
                 ORDER BY seq",
            )
            .map_err(OpenError::Sqlite)?;
        statement
            .query_map([], |row| {
                Ok(RetiredFactRow {
                    seq: row.get(0)?,
                    wire_version: row.get(1)?,
                    instant: row.get(2)?,
                    actor: row.get(3)?,
                    origin: row.get(4)?,
                    cause: row.get(5)?,
                    envelope: row.get(6)?,
                    kind: row.get(7)?,
                })
            })
            .map_err(OpenError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };
    rows.into_iter()
        .map(|row| {
            let envelope = serde_json::from_slice(&row.envelope).map_err(|error| {
                retained_fact_error(
                    row.seq,
                    format!("retired fact envelope does not decode: {error}"),
                )
            })?;
            validate_row(&row, &envelope)?;
            Ok(DecodedRow { row, envelope })
        })
        .collect()
}

fn validate_row(
    row: &RetiredFactRow,
    envelope: &wire::Envelope<serde_json::Value>,
) -> Result<(), OpenError> {
    let indexed_cause = row.cause.and_then(|cause| u64::try_from(cause).ok());
    if !envelope.v.is_current()
        || Some(envelope.seq.get()) != u64::try_from(row.seq).ok()
        || i64::from(envelope.v.0) != row.wire_version
        || envelope.at.to_string() != row.instant
        || envelope.actor.to_string() != row.actor
        || envelope.origin.as_str() != row.origin
        || envelope.cause.map(wire::Seq::get) != indexed_cause
    {
        return Err(retained_fact_error(
            row.seq,
            "retired fact envelope disagrees with its indexed columns",
        ));
    }
    Ok(())
}

fn question_publication(fact: &serde_json::Value) -> bool {
    fact.get("operation")
        .and_then(|operation| operation.get("verb"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|verb| matches!(verb, "ask" | "ans"))
}

fn operation_result(fact: &serde_json::Value) -> Option<&str> {
    fact.get("operation")
        .and_then(|operation| operation.get("result"))
        .and_then(serde_json::Value::as_str)
}

fn retire_row(
    transaction: &rusqlite::Transaction<'_>,
    decoded: DecodedRow,
) -> Result<(), OpenError> {
    let row = decoded.row;
    let envelope = decoded.envelope;
    let encoded = wire::encode(&wire::Fact::new(wire::body::JournalRetained {
        release: RELEASE.to_owned(),
        fact: envelope.fact,
    }))
    .map_err(|error| {
        retained_fact_error(
            row.seq,
            format!("retired fact cannot become inert authority: {error}"),
        )
    })?;
    let fact: wire::AnyFact = wire::decode(&encoded).map_err(|error| {
        retained_fact_error(
            row.seq,
            format!("inert replacement does not decode: {error}"),
        )
    })?;
    let replacement = wire::Envelope {
        v: envelope.v,
        seq: envelope.seq,
        at: envelope.at,
        actor: envelope.actor,
        origin: envelope.origin,
        cause: envelope.cause,
        fact,
    };
    let bytes = wire::encode(&replacement).map_err(|error| {
        retained_fact_error(
            row.seq,
            format!("inert replacement does not encode: {error}"),
        )
    })?;
    let changed = transaction
        .execute(
            "UPDATE fact_envelopes SET kind = ?1, envelope = ?2 \
             WHERE seq = ?3 AND kind = ?4",
            params![
                wire::FactKind::JournalRetained.as_str(),
                bytes,
                row.seq,
                row.kind
            ],
        )
        .map_err(OpenError::Sqlite)?;
    if changed != 1 {
        return Err(retained_fact_error(
            row.seq,
            "retired fact changed during exclusive migration",
        ));
    }
    Ok(())
}

fn repair_projections(
    transaction: &rusqlite::Transaction<'_>,
    retired: &BTreeSet<i64>,
) -> Result<(), OpenError> {
    for seq in retired {
        transaction
            .execute("DELETE FROM room_publications WHERE seq = ?1", [seq])
            .map_err(OpenError::Sqlite)?;
        transaction
            .execute("DELETE FROM room_cursors WHERE head_seq = ?1", [seq])
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn drop_projections(transaction: &rusqlite::Transaction<'_>) -> Result<(), OpenError> {
    transaction
        .execute_batch(
            "DELETE FROM room_items \
             WHERE json_extract(CAST(record AS TEXT), '$.publication') IN (\
                 SELECT publication FROM room_conversations \
                 WHERE json_extract(CAST(body AS TEXT), '$.kind.verb') IN ('ask', 'ans')\
             );
             DELETE FROM room_conversations \
             WHERE json_extract(CAST(body AS TEXT), '$.kind.verb') IN ('ask', 'ans');
             DROP TABLE IF EXISTS room_results;",
        )
        .map_err(OpenError::Sqlite)
}

fn retained_fact_error(seq: i64, detail: impl Into<String>) -> OpenError {
    MigrationError::RetiredFact {
        seq,
        kind: "retired room question authority",
        release: RELEASE,
        detail: detail.into(),
    }
    .into()
}
