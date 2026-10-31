//! Retire room relevance observations and assessments into inert authority.
//!
//! Schemas 49 through 104 recorded `room::observed`, a source-backed
//! classification of room activity, and `room::assessed`, the operator's
//! one verdict on such an observation. The question-result protocol that
//! produced observations retired at 4f95c1405, so nothing current writes
//! either kind. Both cross as [`wire::body::JournalRetained`] evidence by the
//! ordinary rule for removed fact kinds, and their projection tables drop.

use super::room_coordination::{read_rows, retire_row};
use crate::{MigrationError, OpenError};

/// Last release whose decoders admitted room relevance records.
const RELEASE: &str = "947abad82";

pub(super) fn cross(transaction: &rusqlite::Transaction<'_>) -> Result<(), OpenError> {
    let rows = read_rows(
        transaction,
        "SELECT seq, wire_version, instant, actor, origin, cause, envelope, kind \
         FROM fact_envelopes WHERE kind IN ('room::observed', 'room::assessed') ORDER BY seq",
    )?;
    for row in rows {
        retire_row(transaction, row, RELEASE, retained_fact_error)?;
    }
    transaction
        .execute_batch(
            "DROP TABLE IF EXISTS room_relevance_assessments; \
             DROP TABLE IF EXISTS room_relevance_observations;",
        )
        .map_err(OpenError::Sqlite)
}

fn retained_fact_error(seq: i64, detail: String) -> OpenError {
    MigrationError::RetiredFact {
        seq,
        kind: "retired room relevance record",
        release: RELEASE,
        detail,
    }
    .into()
}
