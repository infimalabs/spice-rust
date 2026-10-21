//! Give every retained item-bearing `room::contributed` fact its question
//! item's text identity.
//!
//! Schema 90 contributions named a question item by position only. Schema 91
//! keys each item's charge series by the identity of the item's text at the
//! statement the contribution witnessed. The journal keeps only each task's
//! current statement, so a retained position crosses only while its witnessed
//! statement is still the Design's current head; any other refuses.

use rusqlite::{OptionalExtension, Transaction};
use serde_json::{Map, Value};
use wire::rooms::{RoomQuestion, RoomQuestionItem};

use super::{OpenError, residual};

/// Last schema whose contributions named an item without its text identity.
const POSITION_RELEASE: &str = "schema-90";

pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    residual::rewrite_facts(
        transaction,
        wire::FactKind::RoomContributed,
        "json_type(CAST(envelope AS TEXT), '$.fact.question') IS NOT NULL \
         AND json_type(CAST(envelope AS TEXT), '$.fact.item') IS NULL",
        POSITION_RELEASE,
        |_, fact| identify(transaction, fact),
    )
}

/// Record `fact`'s item identity from its Design's current statement.
fn identify(transaction: &Transaction<'_>, fact: &mut Map<String, Value>) -> Result<(), String> {
    let room = fact
        .get("room")
        .and_then(Value::as_str)
        .ok_or_else(|| "the retained contribution names no room".to_owned())?;
    let question = fact
        .get("question")
        .cloned()
        .ok_or_else(|| "the retained contribution names no question item".to_owned())
        .and_then(|value| {
            serde_json::from_value::<RoomQuestion>(value)
                .map_err(|error| format!("the retained question item is invalid: {error}"))
        })?;
    let statement = fact
        .get("statement")
        .and_then(Value::as_u64)
        .ok_or_else(|| "the retained item-bearing contribution names no statement".to_owned())?;
    let head = transaction
        .query_row(
            "SELECT head_seq, body FROM task_heads WHERE stamp = ?1",
            [room],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()
        .map_err(|error| format!("the Design statement does not read: {error}"))?
        .ok_or_else(|| format!("Design {room} has no current statement"))?;
    if u64::try_from(head.0).ok() != Some(statement) {
        return Err(format!(
            "the contribution witnessed statement {statement} of Design {room}, whose text the \
             journal no longer keeps"
        ));
    }
    let head: wire::TaskHead = wire::decode(&head.1)
        .map_err(|error| format!("the Design statement does not decode: {error}"))?;
    let item = question.item(&head.acceptance).ok_or_else(|| {
        format!("question item {question} is outside Design {room}'s current statement")
    })?;
    let item = serde_json::to_value(RoomQuestionItem::of(item))
        .map_err(|error| format!("the item identity cannot encode: {error}"))?;
    fact.insert("item".to_owned(), item);
    Ok(())
}
