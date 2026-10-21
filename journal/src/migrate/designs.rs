//! Retained Designs cross once into the open Design lifecycle.
//!
//! A Design is open from creation until the operator concludes it; it walks
//! one open `design` phase that no lane step exhausts, and selecting its
//! outputs never concludes it. Predecessors walked `design,review`, completed
//! on a lane's clean review or on the operator's selection, and bound some
//! outputs to their Design through an ordinary dependency that only that
//! completion settled. Concluded Designs keep their heads: completion with a
//! closed review reads as collapsed, any other completion as done.

use std::collections::BTreeSet;

use rusqlite::{Transaction, params};

use super::{OpenError, residual};

/// Last release whose Designs walked `design,review` and concluded through a
/// lane review or an execution authorization.
const DESIGN_PHASES_RELEASE: &str = "8b9ad6994";

/// The one phase an open Design walks.
const OPEN_PHASE: &str = "design";

/// Move each open Design onto its one open phase, then drop each pending
/// output's ordinary dependency on the open Design that controls it.
pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let open = cross_open_workflows(transaction)?;
    if open.is_empty() {
        return Ok(());
    }
    cross_output_dependencies(transaction, &open)
}

/// Rewrite every pending Design workflow onto the open phase, returning every
/// open Design's stamp.
fn cross_open_workflows(transaction: &Transaction<'_>) -> Result<BTreeSet<String>, OpenError> {
    let rows = rows(
        transaction,
        "SELECT workflow.stamp, workflow.head_seq, workflow.body FROM workflow_heads AS workflow \
         JOIN task_heads AS statement ON statement.stamp = workflow.stamp \
         WHERE json_extract(CAST(statement.body AS TEXT), '$.intent') = 'design' \
         AND json_extract(CAST(workflow.body AS TEXT), '$.status') = 'pending' \
         ORDER BY workflow.stamp",
    )?;
    let phase: wire::Phase = OPEN_PHASE.parse().expect("the open phase is a valid phase");
    let mut open = BTreeSet::new();
    for (stamp, head_seq, body) in rows {
        let refuse = |detail: String| retained("workflow_heads", &stamp, detail);
        let mut head: wire::WorkflowHead = wire::decode(&body)
            .map_err(|error| refuse(format!("the stored workflow does not decode: {error}")))?;
        if head.flow != [phase.clone()] || head.phase != phase || head.status.reviewer().is_some() {
            head.flow = vec![phase.clone()];
            head.phase = phase.clone();
            head.status = wire::WorkflowStatus::pending();
            let bytes = wire::encode(&head)
                .map_err(|error| refuse(format!("the crossed workflow cannot encode: {error}")))?;
            update(
                transaction,
                "UPDATE workflow_heads SET body = ?1 WHERE stamp = ?2 AND head_seq = ?3",
                (&bytes, &stamp, head_seq),
                refuse,
            )?;
        }
        open.insert(stamp);
    }
    Ok(open)
}

/// Drop the ordinary dependency each pending output holds on the open Design
/// that controls it; that Design's selection, not its conclusion, releases it.
fn cross_output_dependencies(
    transaction: &Transaction<'_>,
    open: &BTreeSet<String>,
) -> Result<(), OpenError> {
    let rows = rows(
        transaction,
        "SELECT statement.stamp, statement.head_seq, statement.body FROM task_heads AS statement \
         JOIN workflow_heads AS workflow ON workflow.stamp = statement.stamp \
         WHERE json_extract(CAST(workflow.body AS TEXT), '$.status') = 'pending' \
         AND json_type(CAST(statement.body AS TEXT), '$.preparation') IS NOT NULL \
         ORDER BY statement.stamp",
    )?;
    for (stamp, head_seq, body) in rows {
        let refuse = |detail: String| retained("task_heads", &stamp, detail);
        let mut head: wire::TaskHead = wire::decode(&body)
            .map_err(|error| refuse(format!("the stored statement does not decode: {error}")))?;
        let Some(controller) = head
            .preparation
            .as_ref()
            .map(|preparation| preparation.controller.clone())
            .filter(|controller| open.contains(controller.as_str()))
        else {
            continue;
        };
        if !head.dependencies.contains(&controller) {
            continue;
        }
        head.dependencies
            .retain(|dependency| *dependency != controller);
        let bytes = wire::encode(&head)
            .map_err(|error| refuse(format!("the crossed statement cannot encode: {error}")))?;
        update(
            transaction,
            "UPDATE task_heads SET body = ?1 WHERE stamp = ?2 AND head_seq = ?3",
            (&bytes, &stamp, head_seq),
            refuse,
        )?;
    }
    Ok(())
}

fn rows(
    transaction: &Transaction<'_>,
    sql: &str,
) -> Result<Vec<(String, i64, Vec<u8>)>, OpenError> {
    let mut statement = transaction.prepare(sql).map_err(OpenError::Sqlite)?;
    let found = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(OpenError::Sqlite)?;
    found
        .collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)
}

/// Replace one row's body, refusing when the row moved under the crossing.
fn update(
    transaction: &Transaction<'_>,
    sql: &str,
    (body, stamp, head_seq): (&[u8], &str, i64),
    refuse: impl Fn(String) -> OpenError,
) -> Result<(), OpenError> {
    let changed = transaction
        .execute(sql, params![body, stamp, head_seq])
        .map_err(OpenError::Sqlite)?;
    if changed == 1 {
        return Ok(());
    }
    Err(refuse(
        "the head changed while its exclusive migration held the authority".to_owned(),
    ))
}

fn retained(table: &'static str, stamp: &str, detail: String) -> OpenError {
    residual::retained(table, stamp.to_owned(), DESIGN_PHASES_RELEASE, detail)
}
