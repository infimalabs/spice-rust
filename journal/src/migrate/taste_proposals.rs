//! Schema 99: Taste mining is retired.
//!
//! A mined proposal was an ordinary `taste.policy` task whose statement also
//! carried `taste_proposal` evidence. The task stays; the crossing drops that
//! member from every retained head and `task::added` fact before any later
//! crossing decodes them strictly. A retained `taste::proposal-add` receipt
//! names a command no current release can evaluate, so it refuses the crossing
//! with the last release that admitted it.

use rusqlite::Transaction;

use super::{OpenError, residual};

/// Last release that filed mined taste proposals.
const TASTE_MINING_RELEASE: &str = "e2a056064";

pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    residual::refuse_first(
        transaction,
        "command_receipts",
        "SELECT command_id FROM command_receipts \
         WHERE command_kind = 'taste::proposal-add' ORDER BY command_id LIMIT 1",
        [],
        TASTE_MINING_RELEASE,
        "the receipt records a mined taste proposal, which no current release evaluates",
    )?;
    for (table, column, path) in [
        ("task_heads", "body", "$.taste_proposal"),
        ("fact_envelopes", "envelope", "$.fact.taste_proposal"),
    ] {
        transaction
            .execute(
                &format!(
                    "UPDATE {table} \
                     SET {column} = CAST(json_remove(CAST({column} AS TEXT), '{path}') AS BLOB) \
                     WHERE json_type(CAST({column} AS TEXT), '{path}') IS NOT NULL"
                ),
                [],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}
