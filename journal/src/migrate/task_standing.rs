//! Lift task priority and taste-review standing out of statement tags into the
//! typed statement fields.

use rusqlite::{Transaction, params};
use serde_json::{Map, Value};
use wire::{FactKind, TaskPriority, TasteHold};

use super::{OpenError, residual};

/// Last release whose build stored priority and taste-review standing as
/// statement tags.
const STANDING_TAGS_RELEASE: &str = "b9df8e0c8";
/// Retired tag prefix that declared a priority.
const PRIORITY_PREFIX: &str = "priority:";
/// Retired tag that held a statement for taste review.
const TASTE_HELD: &str = "taste-review:required";
/// Retired tag marking a held statement whose plan phase the hold prepended.
const TASTE_PREPENDED: &str = "taste-review:plan-prepended";

/// Selects rows whose tags at `path` carry a retired standing tag.
fn carries_standing(column: &str, path: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM json_each(CAST({column} AS TEXT), '{path}') \
         WHERE value GLOB '{PRIORITY_PREFIX}*' OR value IN ('{TASTE_HELD}', '{TASTE_PREPENDED}'))"
    )
}

/// Lift standing from every retained task statement and task-added fact.
///
/// Every other tag stays in its authored order. A retained statement that
/// declares more than one priority, a priority the allocator never scored, or a
/// prepended plan phase without its hold has no one current spelling, so the
/// whole crossing refuses rather than guess.
pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    residual::rewrite_facts(
        transaction,
        FactKind::TaskAdded,
        &carries_standing("envelope", "$.fact.tags"),
        STANDING_TAGS_RELEASE,
        |_, fact| lift_fact(fact),
    )?;
    cross_heads(transaction)
}

fn lift_fact(fact: &mut Map<String, Value>) -> Result<(), String> {
    if fact.contains_key("priority") || fact.contains_key("taste") {
        return Err("the fact carries both standing tags and standing fields".to_owned());
    }
    let mut tags: Vec<String> = fact
        .get("tags")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| format!("the stored tags do not decode: {error}"))?
        .ok_or("the fact carries no tags")?;
    let (priority, taste) = lift(&mut tags)?;
    fact.insert("tags".to_owned(), encode(&tags)?);
    if let Some(priority) = priority {
        fact.insert("priority".to_owned(), encode(&priority)?);
    }
    if let Some(taste) = taste {
        fact.insert("taste".to_owned(), encode(&taste)?);
    }
    Ok(())
}

fn encode(value: &impl serde::Serialize) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|error| error.to_string())
}

fn cross_heads(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let rows = {
        let mut statement = transaction
            .prepare(&format!(
                "SELECT stamp, head_seq, body FROM task_heads WHERE {} ORDER BY stamp",
                carries_standing("body", "$.tags")
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
            residual::retained("task_heads", stamp.clone(), STANDING_TAGS_RELEASE, detail)
        };
        let mut head: wire::TaskHead = wire::decode(&body)
            .map_err(|error| refuse(format!("the stored statement does not decode: {error}")))?;
        if head.priority.is_some() || head.taste.is_some() {
            return Err(refuse(
                "the statement carries both standing tags and standing fields".to_owned(),
            ));
        }
        (head.priority, head.taste) = lift(&mut head.tags).map_err(refuse)?;
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

/// Remove every retired standing tag from `tags` and return what they declared.
fn lift(tags: &mut Vec<String>) -> Result<(Option<TaskPriority>, Option<TasteHold>), String> {
    let mut priority = None;
    let mut held = false;
    let mut prepended = false;
    let mut refusal = None;
    tags.retain(|tag| {
        if let Some(word) = tag.strip_prefix(PRIORITY_PREFIX) {
            match (priority, TaskPriority::from_wire(word)) {
                (None, Some(declared)) => priority = Some(declared),
                (Some(_), _) => {
                    refusal.get_or_insert_with(|| "more than one priority tag".to_owned());
                }
                (None, None) => {
                    refusal.get_or_insert_with(|| format!("priority tag {tag} names no priority"));
                }
            }
            false
        } else if tag == TASTE_HELD {
            held = true;
            false
        } else if tag == TASTE_PREPENDED {
            prepended = true;
            false
        } else {
            true
        }
    });
    if let Some(refusal) = refusal {
        return Err(refusal);
    }
    let taste = match (held, prepended) {
        (false, false) => None,
        (true, false) => Some(TasteHold::Authored),
        (true, true) => Some(TasteHold::Prepended),
        (false, true) => return Err("a prepended plan marker without its taste hold".to_owned()),
    };
    Ok((priority, taste))
}
