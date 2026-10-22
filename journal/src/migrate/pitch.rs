//! Schema 94: every retained task statement carries a pitch.
//!
//! Statements written before the pitch field derive theirs once here from the
//! stored title and description, the same rule a friction report uses at
//! creation. The crossing raises no hold: a derived sentence is not authored
//! prose. A head that stores neither a title nor a description has nothing to
//! derive from and refuses the crossing naming its row. Each retained
//! `task::added` fact then takes its task's head pitch, so the fact and the
//! statement it opened agree, or derives from its own text when no head
//! remains.

use rusqlite::{OptionalExtension as _, Transaction, params};
use serde_json::{Map, Value};

use super::{OpenError, residual};

/// Last schema whose task statements carried no pitch.
const PITCH_RELEASE: &str = "schema-95";

/// Derive the pitch one stored statement object carries from its own title and
/// description, or say why it cannot.
fn derived_pitch(statement: &Map<String, Value>) -> Result<String, String> {
    let title = statement
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let description = statement
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty());
    if title.is_empty() && description.is_none() {
        return Err(
            "the stored statement has neither a title nor a description to derive a pitch from"
                .to_owned(),
        );
    }
    Ok(wire::pitch::derive(title, description))
}

/// Fill each retained `task::added` fact's pitch from its task's crossed head,
/// or from the fact's own text when the head is gone.
pub(super) fn cross_facts(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    residual::rewrite_facts(
        transaction,
        wire::FactKind::TaskAdded,
        "json_type(CAST(envelope AS TEXT), '$.fact.pitch') IS NULL",
        PITCH_RELEASE,
        |_, fact| {
            let stamp = fact
                .get("task")
                .and_then(Value::as_str)
                .ok_or_else(|| "the stored fact names no task".to_owned())?;
            let head_pitch: Option<String> = transaction
                .query_row(
                    "SELECT json_extract(CAST(body AS TEXT), '$.pitch') FROM task_heads \
                     WHERE stamp = ?1",
                    [stamp],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| format!("the task head cannot be read: {error}"))?
                .flatten();
            let pitch = match head_pitch {
                Some(pitch) => pitch,
                None => derived_pitch(fact)?,
            };
            fact.insert("pitch".to_owned(), Value::String(pitch));
            Ok(())
        },
    )
}

pub(super) fn cross_heads(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let rows = {
        let mut statement = transaction
            .prepare(
                "SELECT stamp, head_seq, body FROM task_heads
                 WHERE json_type(CAST(body AS TEXT), '$.pitch') IS NULL ORDER BY stamp",
            )
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
            residual::retained(
                "task_heads",
                format!("{stamp} head_seq={head_seq}"),
                PITCH_RELEASE,
                detail,
            )
        };
        let mut body: Value = serde_json::from_slice(&body)
            .map_err(|error| refuse(format!("the stored statement does not decode: {error}")))?;
        let statement = body
            .as_object_mut()
            .ok_or_else(|| refuse("the stored statement is not an object".to_owned()))?;
        let pitch = derived_pitch(statement).map_err(refuse)?;
        statement.insert("pitch".to_owned(), Value::String(pitch));
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
