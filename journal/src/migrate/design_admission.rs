//! Cross every retained `room::noticed` reach and match onto one admission.
//!
//! Through schema 102 a Design request recorded an optional reach and, from
//! the last schema-101 releases on, an optional exact match. Schema 103 records
//! one required admission instead. A recorded match crosses whole as `scored`
//! with its reach. The score and fit generation of an older request were never
//! kept and replay cannot recover them, because selection weighed resident
//! transcript profiles the journal does not retain: such a request crosses as
//! `unscored` with its reach, or `unreached` when it kept none, naming
//! [`UNSCORED_RELEASE`] instead of an invented value.

use rusqlite::Transaction;
use serde_json::{Map, Value, json};

use super::{OpenError, residual};

/// Last schema whose Design requests could omit their score.
const UNSCORED_RELEASE: &str = "schema-102";

pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    residual::rewrite_facts(
        transaction,
        wire::FactKind::DesignNoticed,
        "json_type(CAST(envelope AS TEXT), '$.fact.admission') IS NULL",
        UNSCORED_RELEASE,
        |_, fact| admit(fact),
    )
}

/// Replace a retained notice's reach and match with its admission. A notice
/// already carrying one is current and stays as it is, so every crossing that
/// rewrites a notice may call this before its strict current decode.
pub(super) fn admit(fact: &mut Map<String, Value>) -> Result<(), String> {
    if fact.contains_key("admission") {
        return Ok(());
    }
    let reach = fact.remove("reach");
    let admission = match (fact.remove("matching"), reach) {
        (Some(Value::Object(mut matching)), Some(reach)) => {
            matching.insert("reach".to_owned(), reach);
            json!({ "scored": matching })
        }
        (Some(Value::Object(_)), None) => {
            return Err("the retained match carries no reach".to_owned());
        }
        (Some(other), _) => return Err(format!("the retained match {other} is not an object")),
        (None, Some(reach)) => {
            json!({ "unscored": { "reach": reach, "release": UNSCORED_RELEASE } })
        }
        (None, None) => json!({ "unreached": { "release": UNSCORED_RELEASE } }),
    };
    fact.insert("admission".to_owned(), admission);
    Ok(())
}
