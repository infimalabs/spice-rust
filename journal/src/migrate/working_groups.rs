//! Materialize the activity stance retained working groups implicitly carried.

use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::Value;
use wire::FactKind;

use super::{OpenError, residual};

/// Release that introduced activity stance without crossing retained groups.
const STANCE_RELEASE: &str = "41af25853";

/// A predecessor's omitted stance meant Explore. Bind that value once in both
/// historical partitions and the projected head, preserving explicit stances.
pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    for kind in [FactKind::WorkingGroupMoved, FactKind::WorkingGroupsArranged] {
        residual::rewrite_facts(
            transaction,
            kind,
            "EXISTS (SELECT 1 FROM json_each(CAST(envelope AS TEXT), '$.fact.partition.groups') \
             WHERE json_type(value, '$.stance') IS NULL) OR \
             EXISTS (SELECT 1 FROM json_each(CAST(envelope AS TEXT), '$.fact.from.groups') \
             WHERE json_type(value, '$.stance') IS NULL)",
            STANCE_RELEASE,
            |_, fact| {
                explicit_stance(fact.get_mut("partition").ok_or("partition is missing")?)?;
                if kind == FactKind::WorkingGroupsArranged {
                    explicit_stance(fact.get_mut("from").ok_or("prior partition is missing")?)?;
                }
                Ok(())
            },
        )?;
    }
    cross_head(transaction)
}

fn explicit_stance(partition: &mut Value) -> Result<bool, String> {
    let groups = partition
        .get_mut("groups")
        .and_then(Value::as_array_mut)
        .ok_or("partition groups are not an array")?;
    let mut changed = false;
    for group in groups {
        let group = group
            .as_object_mut()
            .ok_or("working group is not an object")?;
        if !group.contains_key("stance") {
            group.insert("stance".to_owned(), Value::String("explore".to_owned()));
            changed = true;
        }
    }
    Ok(changed)
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
    let changed = explicit_stance(&mut value).map_err(&refuse)?;
    let _: wire::rooms::WorkingGroupPartition = serde_json::from_value(value.clone())
        .map_err(|error| refuse(format!("the crossed partition head is invalid: {error}")))?;
    if changed {
        let bytes = serde_json::to_vec(&value)
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
