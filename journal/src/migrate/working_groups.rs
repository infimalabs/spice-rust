//! Cross every retained working-group stance onto the Paused/Running brake.
//!
//! Before schema 66 a retained group omitted its stance, which meant Explore;
//! through schema 99 a group stood Passive, Explore, or Converge. Passive
//! becomes Paused, and Explore, Converge, and an omitted stance become Running,
//! in every historical partition, the projected head, and every retained
//! `group::stance` and `group::arrange` request.

use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::{Map, Value};
use wire::FactKind;

use super::{OpenError, residual};

/// Last schema whose working groups stood Passive, Explore, or Converge.
const STANCE_RELEASE: &str = "schema-99";

pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let predicate = format!(
        "{} OR {}",
        retired_in("$.fact.partition.groups"),
        retired_in("$.fact.from.groups")
    );
    for kind in [FactKind::WorkingGroupMoved, FactKind::WorkingGroupsArranged] {
        residual::rewrite_facts(transaction, kind, &predicate, STANCE_RELEASE, |_, fact| {
            brake_partition(fact.get_mut("partition").ok_or("partition is missing")?)?;
            if kind == FactKind::WorkingGroupsArranged {
                brake_partition(fact.get_mut("from").ok_or("prior partition is missing")?)?;
            }
            Ok(())
        })?;
    }
    residual::rewrite_receipts(
        transaction,
        wire::CommandKind::WorkingGroupStance,
        None,
        STANCE_RELEASE,
        |request, _| {
            Ok((
                residual::edit_request::<wire::rooms::WorkingGroupStanceSet, _>(request, brake)?,
                None,
            ))
        },
    )?;
    residual::rewrite_receipts(
        transaction,
        wire::CommandKind::WorkingGroupArrange,
        None,
        STANCE_RELEASE,
        |request, _| {
            Ok((
                residual::edit_request::<wire::rooms::WorkingGroupArrange, _>(
                    request,
                    brake_groups,
                )?,
                None,
            ))
        },
    )?;
    cross_head(transaction)
}

/// Selects an envelope with a group at `path` whose stance is not current.
fn retired_in(path: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM json_each(CAST(envelope AS TEXT), '{path}') \
         WHERE json_extract(value, '$.stance') IS NULL \
         OR json_extract(value, '$.stance') NOT IN ('paused', 'running'))"
    )
}

/// Put `object`'s stance on the brake, answering whether it changed.
fn brake(object: &mut Map<String, Value>) -> Result<bool, String> {
    let current = match object.get("stance") {
        None => "running",
        Some(Value::String(stance)) => match stance.as_str() {
            "paused" | "running" => return Ok(false),
            "passive" => "paused",
            "explore" | "converge" => "running",
            other => return Err(format!("the retained stance {other:?} is not a stance")),
        },
        Some(other) => return Err(format!("the retained stance {other} is not a spelling")),
    };
    object.insert("stance".to_owned(), Value::String(current.to_owned()));
    Ok(true)
}

/// Brake every group `object` lists, answering whether any changed.
fn brake_groups(object: &mut Map<String, Value>) -> Result<bool, String> {
    let groups = object
        .get_mut("groups")
        .and_then(Value::as_array_mut)
        .ok_or("working groups are not an array")?;
    let mut changed = false;
    for group in groups {
        changed |= brake(
            group
                .as_object_mut()
                .ok_or("working group is not an object")?,
        )?;
    }
    Ok(changed)
}

fn brake_partition(partition: &mut Value) -> Result<bool, String> {
    brake_groups(
        partition
            .as_object_mut()
            .ok_or("partition is not an object")?,
    )
}

fn cross_head(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let stored: Option<Vec<u8>> = transaction
        .query_row(
            "SELECT body FROM group_partition WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(OpenError::Sqlite)?;
    let Some(bytes) = stored else {
        return Ok(());
    };
    let refuse = |detail: String| {
        residual::retained("group_partition", "1".to_owned(), STANCE_RELEASE, detail)
    };
    let mut value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| refuse(format!("the partition head does not decode: {error}")))?;
    let changed = brake_partition(&mut value).map_err(&refuse)?;
    let partition: wire::rooms::WorkingGroupPartition = serde_json::from_value(value)
        .map_err(|error| refuse(format!("the crossed partition head is invalid: {error}")))?;
    if changed {
        let bytes = wire::encode(&partition)
            .map_err(|error| refuse(format!("the crossed partition cannot encode: {error}")))?;
        transaction
            .execute(
                "UPDATE group_partition SET body = ?1 WHERE singleton = 1",
                params![bytes],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}
