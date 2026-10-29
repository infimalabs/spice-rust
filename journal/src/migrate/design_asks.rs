//! Give every retained single-ask `room::noticed` fact its one-element ask
//! list.
//!
//! Schemas 92 and 93 recorded at most one asked question item per Design
//! invitation; schema 94 records every item pending for the lane in one list.

use rusqlite::Transaction;
use serde_json::{Map, Value};

use super::{OpenError, residual};

/// Last schema whose Design notices named at most one asked item.
const SINGLE_ASK_RELEASE: &str = "schema-93";

pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    residual::rewrite_facts(
        transaction,
        wire::FactKind::DesignNoticed,
        "json_type(CAST(envelope AS TEXT), '$.fact.ask') IS NOT NULL",
        SINGLE_ASK_RELEASE,
        |_, fact| {
            list_single_ask(fact)?;
            super::design_admission::admit(fact)
        },
    )
}

/// Replace a notice's one retained `ask` with its one-element `asks` list.
pub(super) fn list_single_ask(fact: &mut Map<String, Value>) -> Result<(), String> {
    match fact.remove("ask") {
        Some(ask @ Value::Object(_)) => {
            fact.insert("asks".to_owned(), Value::Array(vec![ask]));
            Ok(())
        }
        other => Err(format!("ask {other:?} is not one asked item")),
    }
}
