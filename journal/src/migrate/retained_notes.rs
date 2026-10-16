//! Index every retained task note into its task's room once.
//!
//! The room fold indexes each `task::noted` fact that is not a taste-review
//! audit into its task's room. A predecessor's projection holds none of them,
//! so the crossing folds each retained note against the stored authority in
//! journal order, exactly as verification replays it. A note whose
//! publication identity its room already holds refuses the crossing.

use rusqlite::Transaction;
use wire::{AnyFact, Envelope, FactKind};

use super::{OpenError, residual};

/// Last schema whose rooms held no retained task note.
const ROOMLESS_NOTES_RELEASE: &str = "schema-75";

pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let mut statement = transaction
        .prepare("SELECT seq, envelope FROM fact_envelopes WHERE kind = ?1 ORDER BY seq")
        .map_err(OpenError::Sqlite)?;
    let rows = statement
        .query_map([FactKind::TaskNoted.as_str()], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(OpenError::Sqlite)?;
    let mut stored = crate::rooms::Stored::new(transaction);
    for row in rows {
        let (seq, envelope) = row.map_err(OpenError::Sqlite)?;
        let refuse = |detail: String| {
            residual::retained(
                "fact_envelopes",
                seq.to_string(),
                ROOMLESS_NOTES_RELEASE,
                detail,
            )
        };
        let envelope = wire::decode::<Envelope<AnyFact>>(&envelope)
            .map_err(|error| refuse(format!("the retained note does not decode: {error}")))?;
        crate::rooms::fold(
            &mut stored,
            envelope.seq,
            envelope.at,
            &envelope.actor,
            &envelope.fact,
        )
        .map_err(|refusal| refuse(refusal.to_string()))?;
    }
    Ok(())
}
