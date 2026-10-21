//! Cross every retained `room::contributed` fact and `room::contribute`
//! receipt from the retired contribution labels to charges.
//!
//! Schema 89 and earlier labelled a contribution `reason-for`,
//! `reason-against`, `evidence`, or `question`. The labels retire into charges:
//! reason-for is +1, reason-against is -1, evidence is 0, and a question stays
//! a question. The fact, its command request, and its reply answer cross
//! together, because the receipt proves the fact only while all three name the
//! same kind. Retained posts gain no question item, so they stay Design-level
//! and owe no citation.

use rusqlite::Transaction;
use serde_json::{Map, Value};

use super::{OpenError, residual};

/// Last schema whose contributions carried labels instead of charges.
const LABEL_RELEASE: &str = "schema-89";

pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    residual::rewrite_facts(
        transaction,
        wire::FactKind::RoomContributed,
        "json_type(CAST(envelope AS TEXT), '$.fact.contribution') = 'text' \
         AND json_extract(CAST(envelope AS TEXT), '$.fact.contribution') != 'question'",
        LABEL_RELEASE,
        |_, fact| relabel(fact, "contribution").map(drop),
    )?;
    let command = wire::CommandKind::RoomContribute;
    residual::rewrite_receipts(
        transaction,
        command,
        None,
        LABEL_RELEASE,
        |request, response| {
            Ok((
                residual::edit_request::<wire::intent::RoomContribute, _>(request, |body| {
                    relabel(body, "kind")
                })?,
                residual::edit_reply(command, response, |answer| relabel(answer, "kind"))?,
            ))
        },
    )
}

/// Replace `member`'s retired label with its charge, answering whether it held
/// one. A charge or question is already current.
fn relabel(object: &mut Map<String, Value>, member: &str) -> Result<bool, String> {
    let charge = match object.get(member).and_then(Value::as_str) {
        Some("reason-for") => 1,
        Some("reason-against") => -1,
        Some("evidence") => 0,
        Some("question") => return Ok(false),
        Some(other) => return Err(format!("the retained label {other:?} has no charge")),
        None if object.get(member).is_some_and(Value::is_i64) => return Ok(false),
        None => return Err(format!("the retained record carries no {member} kind")),
    };
    object.insert(member.to_owned(), Value::from(charge));
    Ok(true)
}
