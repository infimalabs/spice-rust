//! Give every retained `task::reviewed` fact the workflow status its verdict
//! left, which schema 88 and earlier never recorded.
//!
//! A clean review that ends a flow is the actual settlement of its task: the
//! phase `task::done` before it carries `pending`. Fact-only folds such as the
//! Design pass cycle need the review itself to say whether it settled the
//! workflow, exactly as `task::done` does through its terminal status. The
//! crossing backfills `completed` only where the evidence proves it: a clean
//! verdict on a task whose current workflow head is settled positive, whose
//! current statement intent is not a Design (a clean Design review advances to
//! its design phase), and which no later review follows. Every other retained
//! review reads `pending`.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{OptionalExtension, Transaction};
use serde_json::Value;

use super::{OpenError, residual};

/// Last schema whose reviews recorded no workflow status.
const REVIEW_STATUS_RELEASE: &str = "schema-88";

pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let reviews = {
        let mut statement = transaction
            .prepare(
                "SELECT seq, \
                        json_extract(CAST(envelope AS TEXT), '$.fact.task'), \
                        json_extract(CAST(envelope AS TEXT), '$.fact.verdict') \
                 FROM fact_envelopes WHERE kind = ?1 ORDER BY seq",
            )
            .map_err(OpenError::Sqlite)?;
        let rows = statement
            .query_map([wire::FactKind::TaskReviewed.as_str()], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(OpenError::Sqlite)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };
    let mut last_review = BTreeMap::<String, i64>::new();
    for (seq, task, _) in &reviews {
        let task = task.clone().ok_or_else(|| {
            residual::retained(
                "fact_envelopes",
                seq.to_string(),
                REVIEW_STATUS_RELEASE,
                "the retained review names no task".to_owned(),
            )
        })?;
        last_review.insert(task, *seq);
    }
    let mut completed = BTreeSet::new();
    for (seq, task, verdict) in &reviews {
        let task = task.as_deref().expect("every review named its task above");
        if verdict.as_deref() != Some("clean") || last_review.get(task) != Some(seq) {
            continue;
        }
        if settled_positive_non_design(transaction, task, *seq)? {
            completed.insert(*seq);
        }
    }
    residual::rewrite_facts(
        transaction,
        wire::FactKind::TaskReviewed,
        "json_type(CAST(envelope AS TEXT), '$.fact.status') IS NULL",
        REVIEW_STATUS_RELEASE,
        |seq, fact| {
            let status = if completed.contains(&seq) {
                wire::body::TaskCompletionStatus::Completed
            } else {
                wire::body::TaskCompletionStatus::Pending
            };
            fact.insert(
                "status".to_owned(),
                Value::String(status.as_str().to_owned()),
            );
            Ok(())
        },
    )
}

/// Whether `task`'s current heads prove a clean final review settled it: a
/// positive settled workflow on a statement that is not a Design.
fn settled_positive_non_design(
    transaction: &Transaction<'_>,
    task: &str,
    seq: i64,
) -> Result<bool, OpenError> {
    let refuse = |table: &'static str, detail: String| {
        residual::retained(table, seq.to_string(), REVIEW_STATUS_RELEASE, detail)
    };
    let Some(workflow) = transaction
        .query_row(
            "SELECT body FROM workflow_heads WHERE stamp = ?1",
            [task],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(OpenError::Sqlite)?
    else {
        return Ok(false);
    };
    let workflow = wire::decode::<wire::WorkflowHead>(&workflow).map_err(|error| {
        refuse(
            "workflow_heads",
            format!("the reviewed task's workflow head does not decode: {error}"),
        )
    })?;
    if !matches!(
        workflow.status,
        wire::WorkflowStatus::Settled {
            conclusion: wire::WorkflowConclusion::Positive { .. },
            ..
        }
    ) {
        return Ok(false);
    }
    let Some(statement) = transaction
        .query_row(
            "SELECT body FROM task_heads WHERE stamp = ?1",
            [task],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(OpenError::Sqlite)?
    else {
        return Ok(false);
    };
    let statement = wire::decode::<wire::TaskHead>(&statement).map_err(|error| {
        refuse(
            "task_heads",
            format!("the reviewed task's statement head does not decode: {error}"),
        )
    })?;
    Ok(!statement.intent.is_some_and(wire::TaskIntent::is_design))
}
