//! Shared bounds and SQL for indexed room reads.

use super::{FactReadBudget, GuardError, JournalHead};

/// Room authority includes every projection head, each Design request and
/// nudge, and each steering record a directed publication or delivered
/// directive names.
macro_rules! room_authority {
    () => {
        "WITH directed(seq) AS ( \
             SELECT p.seq FROM room_publications p \
             JOIN fact_envelopes f ON f.seq = p.seq WHERE f.kind = ?1 \
         ), recorded(seq) AS ( \
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
             UNION SELECT seq FROM fact_envelopes WHERE kind IN (?2, ?3) \
             UNION SELECT min(seq) FROM fact_envelopes WHERE kind = ?4 \
             UNION SELECT seq - 1 FROM directed \
             UNION SELECT h.head_seq FROM directed d \
             JOIN steering_heads h ON h.created_seq = d.seq - 1 \
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

pub(super) fn require_room<T>(
    head: Option<JournalHead<T>>,
    room: &wire::TaskStamp,
    named: impl Fn(&T) -> &wire::TaskStamp,
) -> Result<Option<JournalHead<T>>, GuardError> {
    match head {
        Some(head) if named(&head.body) != room => Err(GuardError::Invalid(format!(
            "room coordination head indexed as {room} names room {}",
            named(&head.body)
        ))),
        head => Ok(head),
    }
}
