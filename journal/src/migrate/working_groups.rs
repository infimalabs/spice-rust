//! Retire the working-group brake onto deferral.
//!
//! Before schema 66 a retained group omitted its stance, which meant Explore;
//! through schema 99 a group stood Passive, Explore, or Converge, and through
//! schema 100 Paused or Running. Paused and Passive held their Designs
//! stopped. Deferral is now the one stop: each open filed Design the projected
//! partition held in a Paused or Passive group becomes deferred on the
//! indefinite horizon, appended as its own deferral so replay reaches the same
//! head. Every historical partition, the projected head, and every retained
//! `group::arrange` request then drop their stance, and every retained
//! `group::stance` receipt retires to historical authority.

use std::collections::BTreeSet;

use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::{Map, Value};
use wire::FactKind;

use super::{OpenError, residual};

/// Last schema whose working groups carried a stance.
const BRAKE_RELEASE: &str = "schema-100";

pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    defer_held_designs(transaction)?;
    let predicate = format!(
        "{} OR {}",
        stanced_in("$.fact.partition.groups"),
        stanced_in("$.fact.from.groups")
    );
    for kind in [FactKind::WorkingGroupMoved, FactKind::WorkingGroupsArranged] {
        residual::rewrite_facts(transaction, kind, &predicate, BRAKE_RELEASE, |_, fact| {
            unstance_partition(fact.get_mut("partition").ok_or("partition is missing")?)?;
            if kind == FactKind::WorkingGroupsArranged {
                unstance_partition(fact.get_mut("from").ok_or("prior partition is missing")?)?;
            }
            Ok(())
        })?;
    }
    crate::historical_receipts::retire_group_stance(transaction)?;
    residual::rewrite_receipts(
        transaction,
        wire::CommandKind::WorkingGroupArrange,
        None,
        BRAKE_RELEASE,
        |request, _| {
            Ok((
                residual::edit_request::<wire::rooms::WorkingGroupArrange, _>(
                    request,
                    unstance_groups,
                )?,
                None,
            ))
        },
    )?;
    cross_head(transaction)
}

/// Selects an envelope with a group at `path` that still carries a stance.
fn stanced_in(path: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM json_each(CAST(envelope AS TEXT), '{path}') \
         WHERE json_type(value, '$.stance') IS NOT NULL)"
    )
}

/// Drop the stance of every group `object` lists, answering whether any had one.
fn unstance_groups(object: &mut Map<String, Value>) -> Result<bool, String> {
    let groups = object
        .get_mut("groups")
        .and_then(Value::as_array_mut)
        .ok_or("working groups are not an array")?;
    let mut changed = false;
    for group in groups {
        changed |= group
            .as_object_mut()
            .ok_or("working group is not an object")?
            .remove("stance")
            .is_some();
    }
    Ok(changed)
}

fn unstance_partition(partition: &mut Value) -> Result<bool, String> {
    unstance_groups(
        partition
            .as_object_mut()
            .ok_or("partition is not an object")?,
    )
}

fn refuse_head(detail: String) -> OpenError {
    residual::retained("group_partition", "1".to_owned(), BRAKE_RELEASE, detail)
}

fn stored_head(transaction: &Transaction<'_>) -> Result<Option<Value>, OpenError> {
    let stored: Option<Vec<u8>> = transaction
        .query_row(
            "SELECT body FROM group_partition WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(OpenError::Sqlite)?;
    stored
        .map(|bytes| {
            serde_json::from_slice(&bytes).map_err(|error| {
                refuse_head(format!("the partition head does not decode: {error}"))
            })
        })
        .transpose()
}

fn cross_head(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let Some(mut value) = stored_head(transaction)? else {
        return Ok(());
    };
    if !unstance_partition(&mut value).map_err(refuse_head)? {
        return Ok(());
    }
    let partition: wire::rooms::WorkingGroupPartition = serde_json::from_value(value)
        .map_err(|error| refuse_head(format!("the crossed partition head is invalid: {error}")))?;
    let bytes = wire::encode(&partition)
        .map_err(|error| refuse_head(format!("the crossed partition cannot encode: {error}")))?;
    transaction
        .execute(
            "UPDATE group_partition SET body = ?1 WHERE singleton = 1",
            params![bytes],
        )
        .map_err(OpenError::Sqlite)?;
    Ok(())
}

/// Every anchor the projected partition holds in a group whose stance
/// stopped its Designs.
fn held_anchors(transaction: &Transaction<'_>) -> Result<BTreeSet<String>, OpenError> {
    let Some(head) = stored_head(transaction)? else {
        return Ok(BTreeSet::new());
    };
    let groups = head
        .get("groups")
        .and_then(Value::as_array)
        .ok_or_else(|| refuse_head("working groups are not an array".to_owned()))?;
    let mut anchors = BTreeSet::new();
    for group in groups {
        if !matches!(
            group.get("stance").and_then(Value::as_str),
            Some("paused" | "passive")
        ) {
            continue;
        }
        let members = group
            .get("anchors")
            .and_then(Value::as_array)
            .ok_or_else(|| refuse_head("working group anchors are not an array".to_owned()))?;
        for anchor in members {
            let anchor = anchor
                .as_str()
                .ok_or_else(|| refuse_head("working group anchor is not a stamp".to_owned()))?;
            anchors.insert(anchor.to_owned());
        }
    }
    Ok(anchors)
}

/// Defer each open filed Design a stopping group held and no deferral stops.
fn defer_held_designs(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    for anchor in held_anchors(transaction)? {
        let Some((statement, workflow)) = stored_task(transaction, &anchor)? else {
            continue;
        };
        if statement.filing == wire::TaskFiling::Filed
            && workflow.deferral.is_none()
            && wire::intent::is_open_design(statement.intent, &workflow)
        {
            append_deferral(transaction, &statement.born, workflow)?;
        }
    }
    Ok(())
}

fn refuse_task(table: &'static str, task: &str, detail: String) -> OpenError {
    residual::retained(table, task.to_owned(), BRAKE_RELEASE, detail)
}

fn stored_task(
    transaction: &Transaction<'_>,
    task: &str,
) -> Result<Option<(wire::TaskHead, wire::WorkflowHead)>, OpenError> {
    let rows: Option<(Vec<u8>, Vec<u8>)> = transaction
        .query_row(
            "SELECT task.body, workflow.body FROM task_heads AS task \
             JOIN workflow_heads AS workflow USING (stamp) WHERE stamp = ?1",
            [task],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(OpenError::Sqlite)?;
    let Some((statement, workflow)) = rows else {
        return Ok(None);
    };
    let statement = wire::decode(&statement).map_err(|error| {
        refuse_task(
            "task_heads",
            task,
            format!("statement does not decode: {error}"),
        )
    })?;
    let workflow = wire::decode(&workflow).map_err(|error| {
        refuse_task(
            "workflow_heads",
            task,
            format!("workflow does not decode: {error}"),
        )
    })?;
    Ok(Some((statement, workflow)))
}

/// Append `task`'s deferral at the next position, as the operator's own
/// deferral would land: the workflow head moves to it, its generation
/// advances, and an active claim stays held behind that generation.
fn append_deferral(
    transaction: &Transaction<'_>,
    task: &wire::TaskStamp,
    mut workflow: wire::WorkflowHead,
) -> Result<(), OpenError> {
    let refuse = |detail: String| refuse_task("fact_envelopes", task.as_str(), detail);
    let (next, instant): (i64, String) = transaction
        .query_row(
            "SELECT coalesce(max(seq), 0) + 1, \
             coalesce((SELECT instant FROM fact_envelopes ORDER BY seq DESC LIMIT 1), \
                      '1970-01-01T00:00:00.000Z') \
             FROM fact_envelopes",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(OpenError::Sqlite)?;
    let sequence = u64::try_from(next)
        .ok()
        .and_then(wire::Seq::new)
        .ok_or_else(|| refuse(format!("invalid next sequence {next}")))?;
    let at: wire::Instant = instant
        .parse()
        .map_err(|error| refuse(format!("invalid retained instant: {error}")))?;
    let until = wire::Instant::indefinite();
    let deferred = wire::body::TaskDeferred {
        task: task.clone(),
        until,
    };
    let fact: wire::AnyFact = wire::encode(&wire::Fact::new(deferred))
        .and_then(|bytes| wire::decode(&bytes))
        .map_err(|error| refuse(format!("deferral does not encode: {error}")))?;
    let origin: wire::Origin = format!("task:{task}")
        .parse()
        .map_err(|error| refuse(format!("invalid task origin: {error}")))?;
    let envelope = wire::Envelope {
        v: wire::WireVersion::CURRENT,
        seq: sequence,
        at,
        actor: wire::Actor::Spice,
        origin: origin.clone(),
        cause: None,
        fact,
    };
    let envelope = wire::encode(&envelope)
        .map_err(|error| refuse(format!("deferral envelope does not encode: {error}")))?;
    transaction
        .execute(
            "INSERT INTO fact_envelopes(seq, wire_version, instant, actor, origin, cause, kind, envelope) \
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7)",
            params![
                next,
                wire::WireVersion::CURRENT.0,
                at.to_string(),
                wire::Actor::Spice.to_string(),
                origin.as_str(),
                FactKind::TaskDeferred.as_str(),
                envelope
            ],
        )
        .map_err(OpenError::Sqlite)?;
    transaction
        .execute(
            "INSERT INTO thread_index(seq, origin, cause, root, depth) VALUES (?1, ?2, NULL, ?1, 0)",
            params![next, origin.as_str()],
        )
        .map_err(OpenError::Sqlite)?;

    workflow.deferral = Some(until);
    workflow.generation = workflow.generation.checked_add(1).ok_or_else(|| {
        refuse_task(
            "workflow_heads",
            task.as_str(),
            "workflow generation exhausted".to_owned(),
        )
    })?;
    let generation = i64::try_from(workflow.generation).map_err(|error| {
        refuse_task(
            "workflow_heads",
            task.as_str(),
            format!("workflow generation exceeds SQLite: {error}"),
        )
    })?;
    let body = wire::encode(&workflow).map_err(|error| {
        refuse_task(
            "workflow_heads",
            task.as_str(),
            format!("workflow does not encode: {error}"),
        )
    })?;
    transaction
        .execute(
            "UPDATE workflow_heads SET head_seq = ?1, body = ?2, revised_seq = NULL WHERE stamp = ?3",
            params![next, body, task.as_str()],
        )
        .map_err(OpenError::Sqlite)?;
    transaction
        .execute(
            "INSERT INTO task_generations(stamp, generation, seq, baseline) VALUES (?1, ?2, ?3, 0)",
            params![task.as_str(), generation, next],
        )
        .map_err(OpenError::Sqlite)?;
    transaction
        .execute(
            "UPDATE cursor_head SET seq = ?1 WHERE singleton = 1",
            [next],
        )
        .map_err(OpenError::Sqlite)?;
    transaction
        .execute(
            "UPDATE steering_index SET indexed_seq = ?1 WHERE singleton = 1",
            [next],
        )
        .map_err(OpenError::Sqlite)?;
    Ok(())
}
