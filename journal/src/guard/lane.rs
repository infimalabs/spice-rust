//! Lane recovery reads scoped to their durable fact kind.

use super::{GuardError, JournalRead, read_metrics};

impl JournalRead<'_> {
    /// Whether the exact lane, occupant, stream, and source range was compacted.
    ///
    /// Recovery calls this both before proposing a fact and inside its write
    /// guard. Decode only compaction records; unrelated transcript accounting
    /// must not turn every recovered boundary into another full journal replay.
    pub fn lane_compaction_recorded(
        &self,
        expected: &wire::body::LaneCompacted,
    ) -> Result<bool, GuardError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT seq, kind, envelope FROM fact_envelopes \
                 WHERE kind = ?1 ORDER BY seq DESC",
            )
            .map_err(GuardError::Store)?;
        read_metrics::observe_query();
        let rows = statement
            .query_map([wire::FactKind::LaneCompacted.as_str()], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(GuardError::Store)?;
        for row in rows {
            let (seq, kind, bytes) = row.map_err(GuardError::Store)?;
            read_metrics::observe_row();
            read_metrics::observe_decode(bytes.len());
            let fact = crate::read::decode_fact(seq, &kind, &bytes)
                .map_err(|error| GuardError::Invalid(error.to_string()))?;
            if fact.fact.read::<wire::body::LaneCompacted>().as_ref() == Some(expected) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
