//! Lift document slugs and taste-proposal evidence out of statement tags into
//! the typed statement fields.

use rusqlite::{Transaction, params};
use serde_json::{Map, Value};
use wire::{Digest, FactKind, TasteProposal};

use super::{OpenError, residual};

/// Last release whose build stored document slugs and taste-proposal evidence
/// as statement tags.
const MARKER_TAGS_RELEASE: &str = "1ebbdf5a5";
/// Retired tag prefix naming a task's document node slug.
const DOCUMENT_SLUG_PREFIX: &str = "document-slug:";
/// Retired tag marking a resident-filed taste proposal.
const PROPOSAL: &str = "taste-proposal";
/// Retired tag prefix naming a proposal's finding identity.
const FINDING_PREFIX: &str = "taste-finding:";
/// Retired tag prefix naming a proposal's occurrence identity.
const OCCURRENCE_PREFIX: &str = "taste-occurrence:";
/// Retired tag prefix naming a proposal's recurrence count.
const RECURRENCE_PREFIX: &str = "taste-recurrence:";

/// Selects rows whose tags at `path` carry a retired marker tag.
fn carries_markers(column: &str, path: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM json_each(CAST({column} AS TEXT), '{path}') \
         WHERE value = '{PROPOSAL}' OR value GLOB '{DOCUMENT_SLUG_PREFIX}*' \
         OR value GLOB '{FINDING_PREFIX}*' OR value GLOB '{OCCURRENCE_PREFIX}*' \
         OR value GLOB '{RECURRENCE_PREFIX}*')"
    )
}

/// Lift markers from every retained task statement and task-added fact.
///
/// Every other tag stays in its authored order. A retained statement with more
/// than one slug, an empty slug, or an incomplete, repeated, or unparsable
/// proposal marker set has no one current spelling, so the whole crossing
/// refuses rather than guess.
pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    residual::rewrite_facts(
        transaction,
        FactKind::TaskAdded,
        &carries_markers("envelope", "$.fact.tags"),
        MARKER_TAGS_RELEASE,
        |_, fact| lift_fact(fact),
    )?;
    cross_heads(transaction)
}

fn lift_fact(fact: &mut Map<String, Value>) -> Result<(), String> {
    if fact.contains_key("document_slug") || fact.contains_key("taste_proposal") {
        return Err("the fact carries both marker tags and marker fields".to_owned());
    }
    let mut tags: Vec<String> = fact
        .get("tags")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| format!("the stored tags do not decode: {error}"))?
        .ok_or("the fact carries no tags")?;
    let (slug, proposal) = lift(&mut tags)?;
    fact.insert("tags".to_owned(), encode(&tags)?);
    if let Some(slug) = slug {
        fact.insert("document_slug".to_owned(), Value::String(slug));
    }
    if let Some(proposal) = proposal {
        fact.insert("taste_proposal".to_owned(), encode(&proposal)?);
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
                carries_markers("body", "$.tags")
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
            residual::retained("task_heads", stamp.clone(), MARKER_TAGS_RELEASE, detail)
        };
        let mut head: wire::TaskHead = wire::decode(&body)
            .map_err(|error| refuse(format!("the stored statement does not decode: {error}")))?;
        if head.document_slug.is_some() || head.taste_proposal.is_some() {
            return Err(refuse(
                "the statement carries both marker tags and marker fields".to_owned(),
            ));
        }
        (head.document_slug, head.taste_proposal) = lift(&mut head.tags).map_err(refuse)?;
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

/// Remove every retired marker tag from `tags` and return what they declared.
fn lift(tags: &mut Vec<String>) -> Result<(Option<String>, Option<TasteProposal>), String> {
    let mut slug = None;
    let mut marked = false;
    let mut finding = None;
    let mut occurrence = None;
    let mut recurrence = None;
    let mut retained = Vec::with_capacity(tags.len());
    for tag in tags.drain(..) {
        if let Some(value) = tag.strip_prefix(DOCUMENT_SLUG_PREFIX) {
            if value.is_empty() {
                return Err("the statement declares an empty document slug".to_owned());
            }
            once(&mut slug, value.to_owned(), "document slug")?;
        } else if tag == PROPOSAL {
            if std::mem::replace(&mut marked, true) {
                return Err("the statement repeats the taste-proposal marker".to_owned());
            }
        } else if let Some(value) = tag.strip_prefix(FINDING_PREFIX) {
            once(&mut finding, digest(value, "finding")?, "proposal finding")?;
        } else if let Some(value) = tag.strip_prefix(OCCURRENCE_PREFIX) {
            once(
                &mut occurrence,
                digest(value, "occurrence")?,
                "proposal occurrence",
            )?;
        } else if let Some(value) = tag.strip_prefix(RECURRENCE_PREFIX) {
            let count = value
                .parse::<usize>()
                .ok()
                .filter(|count| *count >= 2)
                .ok_or_else(|| format!("the proposal recurrence {value:?} is not at least two"))?;
            once(&mut recurrence, count, "proposal recurrence")?;
        } else {
            retained.push(tag);
        }
    }
    *tags = retained;
    let proposal = match (marked, finding, occurrence, recurrence) {
        (false, None, None, None) => None,
        (true, Some(finding), Some(occurrence), Some(recurrence)) => Some(TasteProposal {
            finding,
            occurrence,
            recurrence,
        }),
        _ => {
            return Err("the statement carries an incomplete taste-proposal marker set".to_owned());
        }
    };
    Ok((slug, proposal))
}

fn once<T>(slot: &mut Option<T>, value: T, what: &str) -> Result<(), String> {
    match slot.replace(value) {
        Some(_) => Err(format!("the statement declares more than one {what}")),
        None => Ok(()),
    }
}

fn digest(value: &str, what: &str) -> Result<Digest, String> {
    value
        .parse()
        .map_err(|_| format!("the proposal {what} {value:?} is not a digest"))
}
