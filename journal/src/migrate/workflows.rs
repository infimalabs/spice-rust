//! Cross retained terminal review fields before admitting typed workflow heads.

use rusqlite::{Transaction, params};
use serde_json::Value;

use super::{OpenError, residual};

const WORKFLOW_RELEASE: &str = "schema-72";

/// Normalize only retired terminal combinations, preserving settled standing.
///
/// Deletion used to retain the preceding review spelling. Positive completion
/// could retain `assigned` after its assignment was cleared, or `finding` from
/// an older review. Neither means clean review. The old positive predicate was
/// `completed && review != closed`, so these remain positive and unreviewed;
/// their exact verdict and finding stay in immutable facts. Unknown states,
/// live contradictions, assignments on terminal work, and missing terminal
/// instants still refuse the entire exclusive schema crossing.
pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let rows = {
        let mut statement = transaction
            .prepare("SELECT stamp, head_seq, body FROM workflow_heads ORDER BY stamp")
            .map_err(OpenError::Sqlite)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(OpenError::Sqlite)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };
    for (stamp, head_seq, bytes) in rows {
        let refuse = |detail: String| {
            residual::retained("workflow_heads", stamp.clone(), WORKFLOW_RELEASE, detail)
        };
        if wire::decode::<wire::WorkflowHead>(&bytes).is_ok() {
            continue;
        }
        let mut value: Value = wire::decode(&bytes)
            .map_err(|error| refuse(format!("the stored workflow does not decode: {error}")))?;
        let body = value
            .as_object_mut()
            .ok_or_else(|| refuse("the stored workflow is not an object".to_owned()))?;
        let review = match (
            body.get("status").and_then(Value::as_str),
            body.get("review").and_then(Value::as_str),
        ) {
            (Some("deleted"), Some("unassigned" | "assigned" | "finding" | "clean")) => {
                Some("closed")
            }
            (Some("completed"), Some("assigned" | "finding")) => Some("unassigned"),
            _ => None,
        };
        if let Some(review) = review {
            body.insert("review".to_owned(), Value::String(review.to_owned()));
        }
        let head: wire::WorkflowHead = serde_json::from_value(value)
            .map_err(|error| refuse(format!("the retained workflow cannot cross: {error}")))?;
        let crossed = wire::encode(&head)
            .map_err(|error| refuse(format!("the crossed workflow cannot encode: {error}")))?;
        let changed = transaction
            .execute(
                "UPDATE workflow_heads SET body = ?1 WHERE stamp = ?2 AND head_seq = ?3",
                params![crossed, stamp, head_seq],
            )
            .map_err(OpenError::Sqlite)?;
        if changed != 1 {
            return Err(refuse(
                "the head changed under the exclusive crossing".to_owned(),
            ));
        }
    }
    Ok(())
}
