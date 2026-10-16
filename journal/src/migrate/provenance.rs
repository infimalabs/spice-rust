//! Resolve every retained task statement's provenance into its closed type.

use rusqlite::{OptionalExtension, Transaction, params};

use super::{OpenError, residual};

/// Schema that closed task provenance and crossed every retained spelling.
const PROVENANCE_RELEASE: &str = "schema-67";

/// Rewrite each statement head whose provenance is not a canonical spelling.
///
/// The retained value is `<class>:<value>`, written when creation admitted
/// any class. Its value alone decides the cause, against this journal's own
/// indexes: a key of a retained steering directive becomes steering
/// provenance, a stamp or handle naming a retained task becomes that task's
/// provenance, and a value naming neither becomes parentless. A value without
/// a class, or naming both, refuses. Only the head changes; the creating
/// envelope keeps its original origin as evidence. This runs before any
/// crossing decodes a typed statement head.
pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    for (stamp, head_seq, stored) in retained_rows(transaction)? {
        let refuse = |detail: String| {
            residual::retained("task_heads", stamp.clone(), PROVENANCE_RELEASE, detail)
        };
        let Some(written) = stored else {
            return Err(refuse("the stored provenance is not a string".to_owned()));
        };
        if written.parse::<wire::TaskProvenance>().is_ok() {
            continue;
        }
        let resolved = resolve(transaction, &written)?.map_err(&refuse)?;
        let changed = transaction
            .execute(
                "UPDATE task_heads SET body = CAST(json_set(CAST(body AS TEXT), \
                 '$.provenance', ?1) AS BLOB) WHERE stamp = ?2 AND head_seq = ?3",
                params![resolved.to_string(), stamp, head_seq],
            )
            .map_err(OpenError::Sqlite)?;
        if changed != 1 {
            return Err(refuse(
                "the head changed while its exclusive migration held the authority".to_owned(),
            ));
        }
    }
    Ok(())
}

/// Every statement head with its stored provenance, `None` when not a string.
fn retained_rows(
    transaction: &Transaction<'_>,
) -> Result<Vec<(String, i64, Option<String>)>, OpenError> {
    let mut statement = transaction
        .prepare(
            "SELECT stamp, head_seq, \
             CASE json_type(CAST(body AS TEXT), '$.provenance') \
                 WHEN 'text' THEN json_extract(CAST(body AS TEXT), '$.provenance') \
             END \
             FROM task_heads ORDER BY stamp",
        )
        .map_err(OpenError::Sqlite)?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(OpenError::Sqlite)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)
}

/// The one cause a retired spelling's value names in this journal, or why it
/// names none deterministically.
fn resolve(
    transaction: &Transaction<'_>,
    written: &str,
) -> Result<Result<wire::TaskProvenance, String>, OpenError> {
    let Some((_, value)) = written.split_once(':') else {
        return Ok(Err(format!("provenance {written:?} has no class")));
    };
    let steering = (wire::steering_key(value) && exists(transaction, STEERING_KEY, value)?)
        .then(|| wire::TaskProvenance::Steering(value.to_owned()));
    // A stamp never contains `-` and a handle always does, so at most one parses.
    let named = value.parse::<wire::TaskStamp>().ok().or_else(|| {
        value
            .parse::<wire::TaskHandle>()
            .ok()
            .and_then(|handle| handle.born().ok())
    });
    let task = match named {
        Some(task) if exists(transaction, TASK_STAMP, task.as_str())? => {
            Some(wire::TaskProvenance::Task(task))
        }
        _ => None,
    };
    Ok(match (steering, task) {
        (Some(_), Some(_)) => Err(format!(
            "provenance {written:?} names both a steering directive and a task"
        )),
        (Some(cause), None) | (None, Some(cause)) => Ok(cause),
        (None, None) => Ok(wire::TaskProvenance::Parentless),
    })
}

const STEERING_KEY: &str = "SELECT 1 FROM steering_heads WHERE steer_key = ?1";
const TASK_STAMP: &str = "SELECT 1 FROM task_heads WHERE stamp = ?1";

fn exists(transaction: &Transaction<'_>, sql: &str, key: &str) -> Result<bool, OpenError> {
    transaction
        .query_row(sql, [key], |_| Ok(()))
        .optional()
        .map(|found| found.is_some())
        .map_err(OpenError::Sqlite)
}
