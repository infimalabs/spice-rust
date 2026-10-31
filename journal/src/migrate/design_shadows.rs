//! Retire Design shadows into inert authority.
//!
//! Schemas 102 and 103 recorded `room::shadowed`: the best Design a session
//! fell short of when nothing cleared its floor. Notices already carry each
//! admitted score and floor, so shadows cross as
//! [`wire::body::JournalRetained`] evidence by the ordinary rule for removed
//! fact kinds and their per-Design index drops.

use super::retire::{self, COLUMNS, Retirement};
use crate::OpenError;

/// Design shadows and the last release whose decoders admitted them.
const RETIREMENT: Retirement = Retirement {
    release: "6e0121328",
    what: "retired Design shadow",
};

pub(super) fn cross(transaction: &rusqlite::Transaction<'_>) -> Result<(), OpenError> {
    let rows = retire::read_rows(
        transaction,
        &format!("SELECT {COLUMNS} FROM fact_envelopes WHERE kind = 'room::shadowed' ORDER BY seq"),
        [],
    )?;
    for row in rows {
        retire::retire_row(transaction, row, RETIREMENT)?;
    }
    transaction
        .execute_batch("DROP TABLE IF EXISTS design_shadows;")
        .map_err(OpenError::Sqlite)
}
