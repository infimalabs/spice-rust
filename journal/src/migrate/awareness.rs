//! Bind retained Design notices to exact statement evidence and retire their
//! superseded delivery provenance.

use std::collections::BTreeMap;

use rusqlite::{Transaction, params};
use serde_json::{Map, Value};
use wire::{AnyFact, Envelope, FactKind};

use super::{OpenError, residual};

/// Release that made exact statement evidence the notice authority and
/// removed the superseded delivery-path classification.
const NOTICE_STATEMENT_RELEASE: &str = "ROOMS-dW6GpYK65";
/// Retained statement envelopes decoded in one bounded crossing step.
const FACT_PAGE_ROWS: i64 = 256;

/// Add the exact statement current when an old notice was recorded and remove
/// the superseded provenance field from every retained notice.
pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let mut revisions = BTreeMap::<String, u64>::new();
    let mut notice_heads = BTreeMap::<i64, u64>::new();
    let mut after = 0_i64;
    loop {
        let rows = fact_page(transaction, after)?;
        let Some((last, ..)) = rows.last() else {
            break;
        };
        after = *last;
        for (seq, kind, bytes) in rows {
            if kind == FactKind::DesignNoticed.as_str() {
                let fact = fact_object(&bytes, seq)?;
                let design = fact
                    .get("design")
                    .and_then(Value::as_str)
                    .ok_or_else(|| refusal(seq, "the retained notice names no Design"))?;
                if !fact.contains_key("statement") {
                    let revision = revisions.get(design).copied().ok_or_else(|| {
                        refusal(
                            seq,
                            "the retained notice predates any provable Design statement",
                        )
                    })?;
                    notice_heads.insert(seq, revision);
                }
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
    }

    super::residual::rewrite_facts(
        transaction,
        FactKind::DesignNoticed,
        "json_type(CAST(envelope AS TEXT), '$.fact.provenance') IS NOT NULL OR \
         json_type(CAST(envelope AS TEXT), '$.fact.statement') IS NULL",
        NOTICE_STATEMENT_RELEASE,
        |seq, fact| cross_notice(seq, fact, &notice_heads),
    )
}

fn fact_page(
    transaction: &Transaction<'_>,
    after: i64,
) -> Result<Vec<(i64, String, Vec<u8>)>, OpenError> {
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
             WHERE seq > ?1 AND kind IN (?2, ?3, ?4, ?5, ?6, ?7) \
             ORDER BY seq LIMIT ?8",
        )
        .map_err(OpenError::Sqlite)?;
    let rows = statement
        .query_map(
            params![
                after,
                kinds[0].as_str(),
                kinds[1].as_str(),
                kinds[2].as_str(),
                kinds[3].as_str(),
                kinds[4].as_str(),
                kinds[5].as_str(),
                FACT_PAGE_ROWS,
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
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
    if !fact.contains_key("statement") {
        let statement = notice_heads
            .get(&seq)
            .copied()
            .ok_or_else(|| "the retained notice has no recovered statement head".to_owned())?;
        fact.insert("statement".to_owned(), Value::from(statement));
    }
    fact.remove("provenance");
    super::design_admission::admit(fact)
}

fn refusal(seq: i64, detail: &str) -> OpenError {
    residual::retained(
        "fact_envelopes",
        seq.to_string(),
        NOTICE_STATEMENT_RELEASE,
        detail.to_owned(),
    )
}
