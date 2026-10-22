//! Drop the retired `recorded` flag from every retained `room::leave` answer.
//!
//! Schema 94 and earlier answered a repeated leave without recording it, so a
//! leave answer said whether this command recorded one. Schema 95 admits a
//! leave only as the decline of an owed Design request, so every answer
//! records its leave.

use rusqlite::Transaction;

use super::{OpenError, residual};

/// Last schema whose leave answers carried `recorded`.
const REPEAT_LEAVE_RELEASE: &str = "schema-94";

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
                    Ok::<_, String>(answer.remove("recorded").is_some())
                })?,
            ))
        },
    )
}
