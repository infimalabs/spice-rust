//! Cross retained Design notices to provenance-bound statement evidence.

use std::collections::BTreeMap;

use rusqlite::Transaction;
use serde_json::{Map, Value};
use wire::{AnyFact, Envelope, FactKind};

use super::{MigrationError, OpenError};

/// Release that bound every Design notice to its selection path and statement.
const NOTICE_PROVENANCE_RELEASE: &str = "ROOMS-dW4XHbhwK";

/// Add the exact statement current when each retained notice was recorded and
/// classify the former term-matched path as organic.
pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let rows = fact_rows(transaction)?;
    let mut revisions = BTreeMap::<String, u64>::new();
    let mut notice_heads = BTreeMap::<i64, u64>::new();
    for (seq, kind, bytes) in rows {
        if kind == FactKind::DesignNoticed.as_str() {
            let fact = fact_object(&bytes, seq)?;
            let design = fact
                .get("design")
                .and_then(Value::as_str)
                .ok_or_else(|| refusal(seq, "the retained notice names no Design"))?;
            let revision = revisions.get(design).copied().ok_or_else(|| {
                refusal(
                    seq,
                    "the retained notice predates any provable Design statement",
                )
            })?;
            notice_heads.insert(seq, revision);
            continue;
        }
        let envelope: Envelope<AnyFact> = wire::decode(&bytes).map_err(|error| {
            refusal(
                seq,
                &format!("a retained statement fact does not decode: {error}"),
            )
        })?;
        for task in envelope.fact.revised_tasks() {
            revisions.insert(task.as_str().to_owned(), envelope.seq.get());
        }
    }

    super::residual::rewrite_facts(
        transaction,
        FactKind::DesignNoticed,
        "json_type(CAST(envelope AS TEXT), '$.fact.provenance') IS NULL OR \
         json_type(CAST(envelope AS TEXT), '$.fact.statement') IS NULL",
        NOTICE_PROVENANCE_RELEASE,
        |seq, fact| cross_notice(seq, fact, &notice_heads),
    )
}

fn fact_rows(transaction: &Transaction<'_>) -> Result<Vec<(i64, String, Vec<u8>)>, OpenError> {
    let kinds = [
        FactKind::TaskAdded,
        FactKind::TaskFiled,
        FactKind::TaskEdited,
        FactKind::OutputControlled,
        FactKind::ExecutionAuthorized,
        FactKind::DesignNoticed,
    ];
    let mut statement = transaction
        .prepare(
            "SELECT seq, kind, envelope FROM fact_envelopes \
             WHERE kind IN (?1, ?2, ?3, ?4, ?5, ?6) ORDER BY seq",
        )
        .map_err(OpenError::Sqlite)?;
    let rows = statement
        .query_map(kinds.map(|kind| kind.as_str()), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(OpenError::Sqlite)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)
}

fn fact_object(bytes: &[u8], seq: i64) -> Result<Map<String, Value>, OpenError> {
    serde_json::from_slice::<Value>(bytes)
        .map_err(|error| {
            refusal(
                seq,
                &format!("the retained envelope does not decode: {error}"),
            )
        })?
        .get_mut("fact")
        .and_then(Value::as_object_mut)
        .cloned()
        .ok_or_else(|| refusal(seq, "the retained envelope carries no fact object"))
}

fn cross_notice(
    seq: i64,
    fact: &mut Map<String, Value>,
    notice_heads: &BTreeMap<i64, u64>,
) -> Result<(), String> {
    if fact.contains_key("provenance") || fact.contains_key("statement") {
        return Err("the retained notice is only partly provenance-bound".to_owned());
    }
    let statement = notice_heads
        .get(&seq)
        .copied()
        .ok_or_else(|| "the retained notice has no recovered statement head".to_owned())?;
    fact.insert("statement".to_owned(), Value::from(statement));
    fact.insert("provenance".to_owned(), Value::from("organic"));
    Ok(())
}

fn refusal(seq: i64, detail: &str) -> OpenError {
    MigrationError::RetainedRow {
        table: "fact_envelopes",
        row: seq.to_string(),
        release: NOTICE_PROVENANCE_RELEASE,
        detail: detail.to_owned(),
    }
    .into()
}
