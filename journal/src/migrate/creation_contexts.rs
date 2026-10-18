//! Retained task creation contexts crossing onto the current closed reasons.
//!
//! A creation context is stored twice: on the creating `task::added` fact and
//! on the task's statement head. Current writers store either a transcript
//! boundary on the one registered stream or an unavailable context with a
//! current reason. A retired unavailable reason crosses to Spice authorship when
//! Spice wrote the creating fact, since Spice is then exactly why no lane
//! transcript exists; any other retired context refuses the crossing. Every
//! later crossing step decodes statements and facts through the current types,
//! so this one edits stored JSON in place before any of them runs.

use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::Value;
use wire::body::{TASK_CREATION_CONTEXT_STREAM, TaskCreationUnavailable};

use super::{CREATION_CONTEXT_RELEASE, OpenError, residual};

/// Last release that stored creation boundaries on streams other than the one
/// registered transcript stream.
const UNREGISTERED_STREAM_RELEASE: &str = "f8be0f9bb";

/// Cross every retained creation context, refusing before any rewrite.
pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    refuse_unregistered_streams(transaction)?;
    let facts = retired_facts(transaction)?;
    let heads = retired_heads(transaction)?;
    let spice = TaskCreationUnavailable::SpiceActor.as_str();
    for seq in facts {
        set_reason(
            transaction,
            "fact_envelopes",
            seq.to_string(),
            "UPDATE fact_envelopes SET envelope = CAST(json_set(CAST(envelope AS TEXT), \
             '$.fact.creation.reason', ?1) AS BLOB) WHERE kind = ?2 AND seq = ?3",
            params![spice, wire::FactKind::TaskAdded.as_str(), seq],
        )?;
    }
    for (stamp, head_seq) in heads {
        set_reason(
            transaction,
            "task_heads",
            stamp.clone(),
            "UPDATE task_heads SET body = CAST(json_set(CAST(body AS TEXT), \
             '$.creation.reason', ?1) AS BLOB) WHERE stamp = ?2 AND head_seq = ?3",
            params![spice, stamp, head_seq],
        )?;
    }
    Ok(())
}

/// Run one in-place reason rewrite that must touch exactly `row`.
fn set_reason(
    transaction: &Transaction<'_>,
    table: &'static str,
    row: String,
    sql: &str,
    parameters: impl rusqlite::Params,
) -> Result<(), OpenError> {
    let changed = transaction
        .execute(sql, parameters)
        .map_err(OpenError::Sqlite)?;
    if changed == 1 {
        Ok(())
    } else {
        Err(residual::retained(
            table,
            row,
            CREATION_CONTEXT_RELEASE,
            "the row changed under the exclusive crossing".to_owned(),
        ))
    }
}

/// Whether `reason` is a spelling no current reason decodes.
fn retired(reason: &str) -> bool {
    serde_json::from_value::<TaskCreationUnavailable>(Value::String(reason.to_owned())).is_err()
}

/// Refuse any fact or head whose transcript boundary names another stream: no
/// current context command can rehydrate it and no current reason records why.
fn refuse_unregistered_streams(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let refuse = |table, row: String| {
        residual::retained(
            table,
            row,
            UNREGISTERED_STREAM_RELEASE,
            format!(
                "the creation boundary names a stream other than {TASK_CREATION_CONTEXT_STREAM}"
            ),
        )
    };
    if let Some(seq) = transaction
        .query_row(
            "SELECT seq FROM fact_envelopes WHERE kind = ?1 \
             AND json_extract(CAST(envelope AS TEXT), '$.fact.creation.kind') = 'transcript' \
             AND json_extract(CAST(envelope AS TEXT), '$.fact.creation.boundary.stream') IS NOT ?2 \
             ORDER BY seq LIMIT 1",
            params![
                wire::FactKind::TaskAdded.as_str(),
                TASK_CREATION_CONTEXT_STREAM
            ],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(OpenError::Sqlite)?
    {
        return Err(refuse("fact_envelopes", seq.to_string()));
    }
    if let Some(stamp) = transaction
        .query_row(
            "SELECT stamp FROM task_heads \
             WHERE json_extract(CAST(body AS TEXT), '$.creation.kind') = 'transcript' \
             AND json_extract(CAST(body AS TEXT), '$.creation.boundary.stream') IS NOT ?1 \
             ORDER BY stamp LIMIT 1",
            params![TASK_CREATION_CONTEXT_STREAM],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(OpenError::Sqlite)?
    {
        return Err(refuse("task_heads", stamp));
    }
    Ok(())
}

/// Every `task::added` fact holding a retired unavailable reason; a fact Spice
/// did not write refuses.
fn retired_facts(transaction: &Transaction<'_>) -> Result<Vec<i64>, OpenError> {
    let rows = {
        let mut statement = transaction
            .prepare(
                "SELECT seq, actor, json_extract(CAST(envelope AS TEXT), '$.fact.creation.reason') \
                 FROM fact_envelopes WHERE kind = ?1 \
                 AND json_extract(CAST(envelope AS TEXT), '$.fact.creation.kind') = 'unavailable' \
                 ORDER BY seq",
            )
            .map_err(OpenError::Sqlite)?;
        let rows = statement
            .query_map(params![wire::FactKind::TaskAdded.as_str()], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(OpenError::Sqlite)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };
    let spice = wire::Actor::Spice.to_string();
    let mut retired_rows = Vec::new();
    for (seq, actor, reason) in rows {
        if !retired(&reason) {
            continue;
        }
        if actor != spice {
            return Err(residual::retained(
                "fact_envelopes",
                seq.to_string(),
                CREATION_CONTEXT_RELEASE,
                format!(
                    "creation reason {reason} on a task {actor} created names no current reason"
                ),
            ));
        }
        retired_rows.push(seq);
    }
    Ok(retired_rows)
}

/// Every statement head holding a retired unavailable reason, with its head
/// seq; its creating fact must be one Spice wrote with that same reason.
fn retired_heads(transaction: &Transaction<'_>) -> Result<Vec<(String, i64)>, OpenError> {
    let rows = {
        let mut statement = transaction
            .prepare(
                "SELECT stamp, head_seq, json_extract(CAST(body AS TEXT), '$.creation.reason') \
                 FROM task_heads \
                 WHERE json_extract(CAST(body AS TEXT), '$.creation.kind') = 'unavailable' \
                 ORDER BY stamp",
            )
            .map_err(OpenError::Sqlite)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(OpenError::Sqlite)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };
    let spice = wire::Actor::Spice.to_string();
    let mut retired_rows = Vec::new();
    for (stamp, head_seq, reason) in rows {
        if !retired(&reason) {
            continue;
        }
        let creator: Option<String> = transaction
            .query_row(
                "SELECT actor FROM fact_envelopes WHERE kind = ?1 \
                 AND json_extract(CAST(envelope AS TEXT), '$.fact.task') = ?2 \
                 AND json_extract(CAST(envelope AS TEXT), '$.fact.creation.reason') = ?3",
                params![wire::FactKind::TaskAdded.as_str(), stamp, reason],
                |row| row.get(0),
            )
            .optional()
            .map_err(OpenError::Sqlite)?;
        if creator.as_deref() != Some(spice.as_str()) {
            return Err(residual::retained(
                "task_heads",
                stamp,
                CREATION_CONTEXT_RELEASE,
                format!("creation reason {reason} has no Spice-created fact carrying it"),
            ));
        }
        retired_rows.push((stamp, head_seq));
    }
    Ok(retired_rows)
}
