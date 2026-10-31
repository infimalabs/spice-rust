//! Retire room relevance observations and assessments into inert authority.
//!
//! Schemas 49 through 105 recorded `room::observed`, a source-backed
//! classification of room activity, and `room::assessed`, the operator's
//! one verdict on such an observation. The question-result protocol that
//! produced observations retired at 4f95c1405, so nothing current writes
//! either kind. Both cross as [`wire::body::JournalRetained`] evidence by the
//! ordinary rule for removed fact kinds, and their projection tables drop.

use super::retire::{self, COLUMNS, Retirement};
use crate::OpenError;

/// Room relevance records and the last release whose decoders admitted them.
const RETIREMENT: Retirement = Retirement {
    release: "2bd90ad06",
    what: "retired room relevance record",
};

pub(super) fn cross(transaction: &rusqlite::Transaction<'_>) -> Result<(), OpenError> {
    let rows = retire::read_rows(
        transaction,
        &format!(
            "SELECT {COLUMNS} FROM fact_envelopes \
             WHERE kind IN ('room::observed', 'room::assessed') ORDER BY seq"
        ),
        [],
    )?;
    for row in rows {
        retire::retire_row(transaction, row, RETIREMENT)?;
    }
    transaction
        .execute_batch(
            "DROP TABLE IF EXISTS room_relevance_assessments; \
             DROP TABLE IF EXISTS room_relevance_observations;",
        )
        .map_err(OpenError::Sqlite)
}
