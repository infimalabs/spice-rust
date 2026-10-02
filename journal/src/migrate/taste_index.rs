//! Schema 32 to 33: bounded taste evidence predicates and cumulative counts.

use rusqlite::Connection;

use super::OpenError;

pub(super) fn migrate(connection: &Connection) -> Result<(), OpenError> {
    crate::taste_index::create(connection).map_err(OpenError::Sqlite)?;
    connection
        .execute_batch(
            r#"
INSERT INTO lane_seats(lane, vendor, seq)
SELECT json_extract(CAST(envelope AS TEXT), '$.fact.lane'),
       json_extract(CAST(envelope AS TEXT), '$.fact.vendor'),
       seq
FROM fact_envelopes
WHERE kind = 'lane::seated'
ORDER BY seq
ON CONFLICT(lane) DO UPDATE SET vendor = excluded.vendor, seq = excluded.seq;

WITH fires AS (
    SELECT seq, json_extract(CAST(envelope AS TEXT), '$.fact.taste') AS taste
    FROM fact_envelopes
    WHERE kind = 'taste::reminded'
       OR (kind = 'taste::judged'
           AND json_extract(CAST(envelope AS TEXT), '$.fact.decision.verdict') = 'confirmed')
), numbered AS (
    SELECT seq, taste, row_number() OVER (PARTITION BY taste ORDER BY seq) AS ordinal
    FROM fires
)
INSERT INTO taste_fires(seq, taste, ordinal)
SELECT seq, taste, ordinal FROM numbered WHERE true
ON CONFLICT(seq) DO UPDATE SET taste = excluded.taste, ordinal = excluded.ordinal;

WITH evidence AS (
    SELECT seq,
           json_extract(CAST(envelope AS TEXT), '$.fact.taste') AS taste,
           CASE
               WHEN kind = 'taste::confirmed' THEN 1
               WHEN kind = 'taste::judged'
                    AND json_extract(CAST(envelope AS TEXT), '$.fact.decision.verdict') = 'confirmed'
                   THEN 1
               ELSE 0
           END AS is_confirmed
    FROM fact_envelopes
    WHERE kind IN ('taste::confirmed', 'taste::cleared', 'taste::judged')
), numbered AS (
    SELECT seq,
           taste,
           row_number() OVER (PARTITION BY taste ORDER BY seq) AS ordinal,
           sum(is_confirmed) OVER (PARTITION BY taste ORDER BY seq) AS confirmed,
           sum(1 - is_confirmed) OVER (PARTITION BY taste ORDER BY seq) AS cleared
    FROM evidence
)
INSERT INTO taste_judgments(seq, taste, ordinal, confirmed, cleared)
SELECT seq, taste, ordinal, confirmed, cleared FROM numbered WHERE true
ON CONFLICT(seq) DO UPDATE SET
    taste = excluded.taste,
    ordinal = excluded.ordinal,
    confirmed = excluded.confirmed,
    cleared = excluded.cleared;
"#,
        )
        .map_err(OpenError::Sqlite)
}
