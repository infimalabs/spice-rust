//! Retire Design shadows into inert authority.
//!
//! Schemas 102 and 103 recorded `room::shadowed`: the best Design a session
//! fell short of when nothing cleared its floor. Notices already carry each
//! admitted score and floor, so shadows cross as
//! [`wire::body::JournalRetained`] evidence by the ordinary rule for removed
//! fact kinds and their per-Design index drops.

use super::room_coordination::{read_rows, retire_row};
use crate::{MigrationError, OpenError};

/// Last release whose decoders admitted Design shadows.
const RELEASE: &str = "6e0121328";

pub(super) fn cross(transaction: &rusqlite::Transaction<'_>) -> Result<(), OpenError> {
    let rows = read_rows(
        transaction,
        "SELECT seq, wire_version, instant, actor, origin, cause, envelope, kind \
         FROM fact_envelopes WHERE kind = 'room::shadowed' ORDER BY seq",
    )?;
    for row in rows {
        retire_row(transaction, row, RELEASE, retained_fact_error)?;
    }
    transaction
        .execute_batch("DROP TABLE IF EXISTS design_shadows;")
        .map_err(OpenError::Sqlite)
}

fn retained_fact_error(seq: i64, detail: String) -> OpenError {
    MigrationError::RetiredFact {
        seq,
        kind: "retired Design shadow",
        release: RELEASE,
        detail,
    }
    .into()
}
