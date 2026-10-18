//! Retained task creation contexts crossing onto the current closed reasons.
//!
//! A creation context is stored twice: on the creating `task::added` fact and
//! on the task's statement head. Current writers store either a transcript
//! boundary on the one registered stream or an unavailable context whose
//! reason the creating actor alone decides: Spice writes spice-actor, the
//! operator or a human writes operator-actor, and a lane always records its
//! transcript. A task holding a retired unavailable reason on either side
//! therefore crosses to the reason its creating fact's actor proves, once both
//! sides are checked against it; a lane creator, a missing creating fact, or a
//! side already contradicting that reason refuses the crossing. Every later
//! crossing step decodes statements and facts through the current types, so
//! this one edits stored JSON in place before any of them runs.

use std::collections::BTreeMap;

use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::Value;
use wire::Actor;
use wire::body::{TASK_CREATION_CONTEXT_STREAM, TaskCreationUnavailable};

use super::{CREATION_CONTEXT_RELEASE, OpenError, residual};

/// Last release that stored creation boundaries on streams other than the one
/// registered transcript stream.
const UNREGISTERED_STREAM_RELEASE: &str = "f8be0f9bb";

const FACTS: &str = "fact_envelopes";
const HEADS: &str = "task_heads";

/// Each retired unavailable reason a release wrote, with the last release
/// that wrote it: the schema-74 OOPS crossing stamped the pre-context evidence
/// reason, and the context crossings 9fdb23fc1 retired last wrote the
/// unregistered-stream reason. Any other retired spelling dates from the
/// release that made creation context explicit.
const RETIRED_REASONS: [(&str, &str); 2] = [
    ("legacy-evidence-missing", "1aa1fcf3f"),
    ("legacy-stream-unregistered", "f55963cab"),
];

/// One stored copy of a task's creation context.
struct Side {
    table: &'static str,
    row: String,
    /// Fact or head position the in-place rewrite must still find.
    seq: i64,
    kind: Option<String>,
    reason: Option<String>,
}

impl Side {
    /// The retired unavailable reason this copy holds, if any.
    fn retired(&self) -> Option<&str> {
        let reason = self.reason.as_deref()?;
        (self.kind.as_deref() == Some("unavailable") && retired(reason)).then_some(reason)
    }

    /// Whether this copy already stores exactly `reason`.
    fn holds(&self, reason: TaskCreationUnavailable) -> bool {
        self.kind.as_deref() == Some("unavailable")
            && self.reason.as_deref() == Some(reason.as_str())
    }

    fn describe(&self) -> String {
        match (self.kind.as_deref(), self.reason.as_deref()) {
            (Some("unavailable"), Some(reason)) => format!("unavailable reason {reason}"),
            (Some(kind), _) => format!("a {kind} context"),
            (None, _) => "no creation context".to_owned(),
        }
    }
}

/// A task's creating fact, its actor, and its statement head.
#[derive(Default)]
struct Pair {
    fact: Option<(Side, String)>,
    head: Option<Side>,
}

impl Pair {
    fn sides(&self) -> impl Iterator<Item = &Side> {
        self.fact
            .as_ref()
            .map(|(side, _)| side)
            .into_iter()
            .chain(&self.head)
    }
}

/// Cross every retained creation context, refusing before any rewrite.
pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    refuse_unregistered_streams(transaction)?;
    let pairs = pairs(transaction)?;
    let mut rewrites = Vec::new();
    for pair in pairs.values() {
        let Some((blamed, spelling)) = pair
            .sides()
            .find_map(|side| side.retired().map(|spelling| (side, spelling)))
        else {
            continue;
        };
        let reason = derive(pair, blamed, spelling)?;
        rewrites.extend(
            pair.sides()
                .filter(|side| side.retired().is_some())
                .map(|side| (side, reason)),
        );
    }
    for (side, reason) in rewrites {
        let (sql, key) = if side.table == FACTS {
            (
                "UPDATE fact_envelopes SET envelope = CAST(json_set(CAST(envelope AS TEXT), \
                 '$.fact.creation.reason', ?1) AS BLOB) WHERE kind = ?2 AND seq = ?3",
                wire::FactKind::TaskAdded.as_str(),
            )
        } else {
            (
                "UPDATE task_heads SET body = CAST(json_set(CAST(body AS TEXT), \
                 '$.creation.reason', ?1) AS BLOB) WHERE stamp = ?2 AND head_seq = ?3",
                side.row.as_str(),
            )
        };
        set_reason(
            transaction,
            side.table,
            side.row.clone(),
            sql,
            params![reason.as_str(), key, side.seq],
        )?;
    }
    Ok(())
}

/// The one current reason `pair`'s creating actor proves, refusing on
/// `blamed`, the copy holding retired `spelling`, when its creator proves none
/// or another copy already disagrees with it.
fn derive(
    pair: &Pair,
    blamed: &Side,
    spelling: &str,
) -> Result<TaskCreationUnavailable, OpenError> {
    let refuse = |detail: String| {
        residual::retained(
            blamed.table,
            blamed.row.clone(),
            last_writer(spelling),
            detail,
        )
    };
    let Some((_, actor)) = &pair.fact else {
        return Err(refuse(format!(
            "creation reason {spelling} has no task::added fact naming its creator"
        )));
    };
    let reason = match actor.parse::<Actor>() {
        Ok(Actor::Spice) => TaskCreationUnavailable::SpiceActor,
        Ok(Actor::Operator | Actor::Human(_)) => TaskCreationUnavailable::OperatorActor,
        Ok(Actor::Lane(_)) => {
            return Err(refuse(format!(
                "creation reason {spelling} on a task {actor} created names no current reason: \
                 a lane records its transcript"
            )));
        }
        Err(error) => {
            return Err(refuse(format!(
                "creating actor {actor} does not decode: {error}"
            )));
        }
    };
    if let Some(current) = pair
        .sides()
        .find(|side| side.retired().is_none() && !side.holds(reason))
    {
        return Err(refuse(format!(
            "{actor} created the task, proving {}, but {} row {} records {}",
            reason.as_str(),
            current.table,
            current.row,
            current.describe()
        )));
    }
    Ok(reason)
}

/// The release that last wrote retired reason `spelling`.
fn last_writer(spelling: &str) -> &'static str {
    RETIRED_REASONS
        .iter()
        .find(|(retired, _)| *retired == spelling)
        .map_or(CREATION_CONTEXT_RELEASE, |(_, release)| release)
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
        return Err(refuse(FACTS, seq.to_string()));
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
        return Err(refuse(HEADS, stamp));
    }
    Ok(())
}

/// Every task's creating fact, with its actor, and statement head, by stamp.
fn pairs(transaction: &Transaction<'_>) -> Result<BTreeMap<String, Pair>, OpenError> {
    let mut pairs = BTreeMap::<String, Pair>::new();
    let mut facts = transaction
        .prepare(
            "SELECT json_extract(CAST(envelope AS TEXT), '$.fact.task'), seq, actor, \
             json_extract(CAST(envelope AS TEXT), '$.fact.creation.kind'), \
             json_extract(CAST(envelope AS TEXT), '$.fact.creation.reason') \
             FROM fact_envelopes WHERE kind = ?1",
        )
        .map_err(OpenError::Sqlite)?;
    let rows = facts
        .query_map(params![wire::FactKind::TaskAdded.as_str()], |row| {
            let seq = row.get::<_, i64>(1)?;
            let side = Side {
                table: FACTS,
                row: seq.to_string(),
                seq,
                kind: row.get(3)?,
                reason: row.get(4)?,
            };
            Ok((row.get::<_, String>(0)?, side, row.get::<_, String>(2)?))
        })
        .map_err(OpenError::Sqlite)?;
    for row in rows {
        let (task, side, actor) = row.map_err(OpenError::Sqlite)?;
        pairs.entry(task).or_default().fact = Some((side, actor));
    }
    let mut heads = transaction
        .prepare(
            "SELECT stamp, head_seq, json_extract(CAST(body AS TEXT), '$.creation.kind'), \
             json_extract(CAST(body AS TEXT), '$.creation.reason') FROM task_heads",
        )
        .map_err(OpenError::Sqlite)?;
    let rows = heads
        .query_map([], |row| {
            let stamp = row.get::<_, String>(0)?;
            Ok(Side {
                table: HEADS,
                row: stamp,
                seq: row.get(1)?,
                kind: row.get(2)?,
                reason: row.get(3)?,
            })
        })
        .map_err(OpenError::Sqlite)?;
    for row in rows {
        let side = row.map_err(OpenError::Sqlite)?;
        let stamp = side.row.clone();
        pairs.entry(stamp).or_default().head = Some(side);
    }
    Ok(pairs)
}
