//! Cross every retained `room::noticed` stance onto the reach that admitted it.
//!
//! Through schema 99 a Design request recorded the working-group stance whose
//! relevance floor admitted it. Schema 100 derives that floor from the Design's
//! maturity and records it as a reach: Converge's raised floor is narrow and
//! Explore's lowered floor is wide. Passive's middle floor no longer exists, so
//! its notice records no reach, as one older than the field does. A schema-92
//! or schema-93 notice also lists its one retained ask, so it crosses whole.

use rusqlite::Transaction;
use serde_json::Value;

use super::{OpenError, design_asks, residual};

/// Last schema whose Design requests recorded an admitting stance.
const STANCE_RELEASE: &str = "schema-99";

pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    residual::rewrite_facts(
        transaction,
        wire::FactKind::DesignNoticed,
        "json_type(CAST(envelope AS TEXT), '$.fact.stance') IS NOT NULL",
        STANCE_RELEASE,
        |_, fact| {
            let reach = match fact.remove("stance") {
                Some(Value::String(stance)) => match stance.as_str() {
                    "converge" => Some("narrow"),
                    "explore" => Some("wide"),
                    "passive" => None,
                    other => return Err(format!("the retained stance {other:?} is not a stance")),
                },
                other => return Err(format!("the retained stance {other:?} is not a spelling")),
            };
            if let Some(reach) = reach {
                fact.insert("reach".to_owned(), Value::String(reach.to_owned()));
            }
            if fact.contains_key("ask") {
                design_asks::list_single_ask(fact)?;
            }
            Ok(())
        },
    )
}
