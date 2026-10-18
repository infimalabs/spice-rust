//! Bind retained completion-only Design notices to exact sessions and sources.

use std::collections::BTreeMap;

use rusqlite::Transaction;
use serde_json::{Map, Value, json};
use wire::{AnyFact, Envelope, FactKind, Seq, TaskStamp};

use super::{MigrationError, OpenError};

/// Release that separated the invitation opportunity from its supporting card.
const DESIGN_INVITATION_RELEASE: &str = "ROOMS-dWKcWx0xN";

#[derive(Clone)]
pub(super) struct Crossing {
    session: Value,
    opportunity: Value,
    evidence: Value,
}

/// Rewrite every retained completion-only notice to the session-targeted shape.
pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let crossings = derive(transaction)?;
    super::residual::rewrite_facts(
        transaction,
        FactKind::DesignNoticed,
        "json_type(CAST(envelope AS TEXT), '$.fact.session') IS NULL",
        DESIGN_INVITATION_RELEASE,
        |seq, fact| cross_notice(seq, fact, &crossings),
    )
}

pub(super) fn derive(transaction: &Transaction<'_>) -> Result<BTreeMap<i64, Crossing>, OpenError> {
    let rows = fact_rows(transaction)?;
    let mut statements = BTreeMap::<TaskStamp, Seq>::new();
    let mut completions = BTreeMap::<TaskStamp, Seq>::new();
    let mut sessions = BTreeMap::<(wire::CanonicalUuid, wire::CanonicalUuid), Value>::new();
    let mut crossings = BTreeMap::new();

    for (seq, kind, bytes) in rows {
        if kind == FactKind::DesignNoticed.as_str() {
            let fact = fact_object(&bytes, seq)?;
            if fact.contains_key("session") {
                continue;
            }
            let task = required_string(&fact, "task", seq)?
                .parse::<TaskStamp>()
                .map_err(|error| refusal(seq, &format!("the retained task is invalid: {error}")))?;
            let lane = required_string(&fact, "lane", seq)?
                .parse::<wire::CanonicalUuid>()
                .map_err(|error| refusal(seq, &format!("the retained lane is invalid: {error}")))?;
            let occupant = required_string(&fact, "occupant", seq)?
                .parse::<wire::CanonicalUuid>()
                .map_err(|error| {
                    refusal(seq, &format!("the retained occupant is invalid: {error}"))
                })?;
            fact.get("claim")
                .ok_or_else(|| refusal(seq, "the retained notice names no claim"))?;
            let statement = statements.get(&task).copied().ok_or_else(|| {
                refusal(seq, "the retained notice task has no provable statement")
            })?;
            let completed = completions.get(&task).copied().ok_or_else(|| {
                refusal(seq, "the retained notice task has no successful completion")
            })?;
            let session = sessions.get(&(lane, occupant)).cloned().ok_or_else(|| {
                refusal(
                    seq,
                    "the retained notice recipient has no declared vendor session",
                )
            })?;
            crossings.insert(
                seq,
                Crossing {
                    session,
                    opportunity: json!({
                        "kind": "task-completion",
                        "task": task,
                        "completed": completed,
                    }),
                    evidence: json!({
                        "kind": "task",
                        "task": task,
                        "statement": statement,
                        "completed": completed,
                    }),
                },
            );
            continue;
        }

        let envelope: Envelope<AnyFact> = wire::decode(&bytes).map_err(|error| {
            refusal(
                seq,
                &format!("a retained invitation witness does not decode: {error}"),
            )
        })?;
        for task in envelope.fact.revised_tasks() {
            statements.insert(task.clone(), envelope.seq);
        }
        if let Some(done) = envelope.fact.read::<wire::body::TaskDone>()
            && done.terminal.as_ref().is_some_and(|terminal| {
                terminal.status == wire::body::TaskCompletionStatus::Completed
            })
        {
            completions.insert(done.task, envelope.seq);
        }
        if let Some(declared) = envelope.fact.read::<wire::body::LaneSessionDeclared>() {
            sessions.insert(
                (declared.lane, declared.occupant),
                serde_json::to_value(wire::rooms::DesignSession {
                    vendor: declared.vendor,
                    identity: declared.identity,
                })
                .map_err(|error| {
                    refusal(
                        seq,
                        &format!("the retained session identity cannot encode: {error}"),
                    )
                })?,
            );
        }
    }
    Ok(crossings)
}

fn fact_rows(transaction: &Transaction<'_>) -> Result<Vec<(i64, String, Vec<u8>)>, OpenError> {
    let mut statement = transaction
        .prepare("SELECT seq, kind, envelope FROM fact_envelopes ORDER BY seq")
        .map_err(OpenError::Sqlite)?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
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

fn required_string<'a>(
    fact: &'a Map<String, Value>,
    field: &str,
    seq: i64,
) -> Result<&'a str, OpenError> {
    fact.get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| refusal(seq, &format!("the retained notice names no {field}")))
}

pub(super) fn cross_notice(
    seq: i64,
    fact: &mut Map<String, Value>,
    crossings: &BTreeMap<i64, Crossing>,
) -> Result<(), String> {
    let crossed = crossings
        .get(&seq)
        .ok_or_else(|| "the retained notice has no derived invitation witnesses".to_owned())?;
    fact.insert("session".to_owned(), crossed.session.clone());
    fact.insert("opportunity".to_owned(), crossed.opportunity.clone());
    fact.insert("evidence".to_owned(), crossed.evidence.clone());
    fact.remove("task");
    fact.remove("claim");
    Ok(())
}

fn refusal(seq: i64, detail: &str) -> OpenError {
    MigrationError::RetainedRow {
        table: "fact_envelopes",
        row: seq.to_string(),
        release: DESIGN_INVITATION_RELEASE,
        detail: detail.to_owned(),
    }
    .into()
}
