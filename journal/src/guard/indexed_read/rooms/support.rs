//! Shared bounds and SQL for indexed room reads.

use super::{FactReadBudget, GuardError};

/// Room authority includes every projection head, each Design request and
/// retained nudge, and each steering record a delivered directive names.
macro_rules! room_authority {
    () => {
        "WITH recorded(seq) AS ( \
             SELECT seq FROM room_publications \
             UNION SELECT head_seq FROM room_conversations \
         ), delivered(directive) AS ( \
             SELECT j.value FROM recorded r \
             JOIN fact_envelopes f ON f.seq = r.seq, \
             json_each(CAST(f.envelope AS TEXT), '$.fact.directed') j \
         ), authority(seq) AS ( \
             SELECT seq FROM recorded \
             UNION SELECT head_seq FROM group_partition \
             UNION SELECT head_seq FROM proposal_heads \
             UNION SELECT head_seq FROM authorization_heads \
             UNION SELECT head_seq FROM room_items \
             UNION SELECT head_seq FROM room_relevance_observations \
             UNION SELECT head_seq FROM room_relevance_assessments \
             UNION SELECT seq FROM fact_envelopes WHERE kind = ?1 \
             UNION SELECT seq FROM fact_envelopes WHERE kind = ?2 \
             AND json_extract(CAST(envelope AS TEXT), '$.fact.fact.kind') = ?4 \
             UNION SELECT min(seq) FROM fact_envelopes WHERE kind = ?3 \
             UNION SELECT h.created_seq FROM delivered d \
             JOIN steering_heads h ON h.directive = d.directive \
             UNION SELECT h.head_seq FROM delivered d \
             JOIN steering_heads h ON h.directive = d.directive \
         ) "
    };
}

pub(super) const ROOM_AUTHORITY_PAGE: &str = concat!(
    room_authority!(),
    "SELECT f.seq, f.kind, length(f.envelope), f.envelope FROM authority a \
     JOIN fact_envelopes f ON f.seq = a.seq \
     WHERE a.seq >= ?5 AND a.seq <= ?6 ORDER BY a.seq LIMIT ?7"
);

pub(super) const ROOM_AUTHORITY_REMAINING: &str = concat!(
    room_authority!(),
    "SELECT count(*) FROM authority WHERE seq >= ?5 AND seq <= ?6"
);

pub(super) fn probe_limit(budget: FactReadBudget) -> Result<i64, GuardError> {
    let limit = budget
        .rows()
        .checked_add(1)
        .expect("bounded room page probe");
    i64::try_from(limit)
        .map_err(|_| GuardError::Invalid("room read limit exceeds SQLite".to_owned()))
}

pub(super) fn indexed_length(
    row: &rusqlite::Row<'_>,
    column: usize,
    class: &str,
) -> Result<usize, GuardError> {
    let length = row.get::<_, i64>(column).map_err(GuardError::Store)?;
    usize::try_from(length)
        .map_err(|_| GuardError::Invalid(format!("{class} has an invalid byte length")))
}
