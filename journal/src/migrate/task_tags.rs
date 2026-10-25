//! Lift every retained system meaning out of statement tags, then discard the
//! unused generic tag field.

use rusqlite::{Transaction, params};
use serde_json::{Map, Value};
use wire::FactKind;

use super::{OpenError, residual, task_markers, task_standing, taste_holds};

/// Last release whose statement schema retained a generic tag field.
const TASK_TAGS_RELEASE: &str = "b1c4a6044";

/// Select a JSON object that still carries the retired tag field.
fn carries_tags(column: &str, path: &str) -> String {
    format!("json_type(CAST({column} AS TEXT), '{path}') IS NOT NULL")
}

/// Cross every retained task creation fact after older fact shapes are current,
/// then prove every statement head has the current strict shape.
///
/// A hold a creation fact's tags declared takes its evidence from `audits` in
/// the same rewrite, because the rewritten fact must decode as current.
pub(super) fn cross_facts(
    transaction: &Transaction<'_>,
    audits: &taste_holds::Audits,
) -> Result<(), OpenError> {
    residual::rewrite_facts(
        transaction,
        FactKind::TaskAdded,
        &carries_tags("envelope", "$.fact.tags"),
        TASK_TAGS_RELEASE,
        |seq, fact| {
            cross_statement(fact)?;
            taste_holds::lift_opened(seq, fact, audits)
        },
    )?;
    canonicalize_heads(transaction)
}

fn cross_statement(statement: &mut Map<String, Value>) -> Result<(), String> {
    let tags = statement
        .remove("tags")
        .ok_or("the statement carries no tags")?;
    let mut tags: Vec<String> = serde_json::from_value(tags)
        .map_err(|error| format!("the stored tags do not decode: {error}"))?;

    let (priority, taste) = task_standing::lift(&mut tags)?;
    insert_lifted(statement, "priority", priority, "priority")?;
    insert_lifted(statement, "taste", taste, "taste-review standing")?;

    let document_slug = task_markers::lift(&mut tags)?;
    insert_lifted(statement, "document_slug", document_slug, "document slug")?;
    Ok(())
}

fn insert_lifted<T: serde::Serialize>(
    statement: &mut Map<String, Value>,
    field: &'static str,
    value: Option<T>,
    meaning: &str,
) -> Result<(), String> {
    let Some(value) = value else {
        return Ok(());
    };
    if statement.contains_key(field) {
        return Err(format!(
            "the statement carries both {meaning} tags and its typed field"
        ));
    }
    let value = serde_json::to_value(value).map_err(|error| error.to_string())?;
    statement.insert(field.to_owned(), value);
    Ok(())
}

pub(super) fn cross_heads(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let rows = {
        let mut statement = transaction
            .prepare(&format!(
                "SELECT stamp, head_seq, body FROM task_heads WHERE {} ORDER BY stamp",
                carries_tags("body", "$.tags")
            ))
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
    for (stamp, head_seq, body) in rows {
        let refuse = |detail: String| {
            residual::retained("task_heads", stamp.clone(), TASK_TAGS_RELEASE, detail)
        };
        let mut body: Value = serde_json::from_slice(&body)
            .map_err(|error| refuse(format!("the stored statement does not decode: {error}")))?;
        let statement = body
            .as_object_mut()
            .ok_or_else(|| refuse("the stored statement is not an object".to_owned()))?;
        cross_statement(statement).map_err(refuse)?;
        let bytes = serde_json::to_vec(&body)
            .map_err(|error| refuse(format!("the crossed statement cannot encode: {error}")))?;
        let changed = transaction
            .execute(
                "UPDATE task_heads SET body = ?1 WHERE stamp = ?2 AND head_seq = ?3",
                params![bytes, stamp, head_seq],
            )
            .map_err(OpenError::Sqlite)?;
        if changed != 1 {
            return Err(refuse(
                "the row changed under the exclusive crossing".to_owned(),
            ));
        }
    }
    Ok(())
}

fn canonicalize_heads(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let rows = {
        let mut statement = transaction
            .prepare("SELECT stamp, head_seq, body FROM task_heads ORDER BY stamp")
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
    for (stamp, head_seq, body) in rows {
        let refuse = |detail: String| {
            residual::retained("task_heads", stamp.clone(), TASK_TAGS_RELEASE, detail)
        };
        let head: wire::TaskHead = wire::decode(&body)
            .map_err(|error| refuse(format!("the crossed statement is not current: {error}")))?;
        let bytes = wire::encode(&head)
            .map_err(|error| refuse(format!("the crossed statement cannot encode: {error}")))?;
        let changed = transaction
            .execute(
                "UPDATE task_heads SET body = ?1 WHERE stamp = ?2 AND head_seq = ?3",
                params![bytes, stamp, head_seq],
            )
            .map_err(OpenError::Sqlite)?;
        if changed != 1 {
            return Err(refuse(
                "the row changed under the exclusive crossing".to_owned(),
            ));
        }
    }
    Ok(())
}
