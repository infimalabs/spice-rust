//! Move every retained `room::leave` answer to the current answer shape.
//!
//! Schema 94 and earlier answered a repeated leave without recording it, so a
//! leave answer said whether this command recorded one. Schema 95 admits a
//! leave only as the decline of an owed Design request, so every answer
//! records its leave and the flag is dropped. Answers from releases that still
//! owed room receipts and ASK results also list `receipt` and `result` duties
//! among those the leave kept; neither duty exists any longer, so they are
//! dropped from `kept` with it.

use rusqlite::Transaction;
use serde_json::Value;

use super::{OpenError, residual};

/// Last schema whose leave answers carried `recorded`.
const REPEAT_LEAVE_RELEASE: &str = "schema-94";

/// Kept duties no current release owes.
const RETIRED_DUTIES: [&str; 2] = ["receipt", "result"];

pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let command = wire::CommandKind::RoomLeave;
    residual::rewrite_receipts(
        transaction,
        command,
        None,
        REPEAT_LEAVE_RELEASE,
        |_, response| {
            Ok((
                None,
                residual::edit_reply(command, response, |answer| {
                    let mut edited = answer.remove("recorded").is_some();
                    if let Some(Value::Array(kept)) = answer.get_mut("kept") {
                        let before = kept.len();
                        kept.retain(|duty| {
                            !duty
                                .get("duty")
                                .and_then(Value::as_str)
                                .is_some_and(|duty| RETIRED_DUTIES.contains(&duty))
                        });
                        edited |= kept.len() != before;
                    }
                    Ok::<_, String>(edited)
                })?,
            ))
        },
    )
}
