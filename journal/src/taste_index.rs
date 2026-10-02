//! Compact predicates for bounded taste evidence reads.

use rusqlite::{Connection, OptionalExtension, params};

pub(crate) const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS taste_fires (
    seq INTEGER PRIMARY KEY REFERENCES fact_envelopes(seq),
    taste TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal > 0),
    UNIQUE(taste, ordinal)
) STRICT;
CREATE INDEX IF NOT EXISTS taste_fires_taste_seq ON taste_fires(taste, seq DESC);
CREATE TABLE IF NOT EXISTS taste_judgments (
    seq INTEGER PRIMARY KEY REFERENCES fact_envelopes(seq),
    taste TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal > 0),
    confirmed INTEGER NOT NULL CHECK (confirmed >= 0),
    cleared INTEGER NOT NULL CHECK (cleared >= 0),
    UNIQUE(taste, ordinal)
) STRICT;
CREATE INDEX IF NOT EXISTS taste_judgments_taste_seq ON taste_judgments(taste, seq DESC);
CREATE TABLE IF NOT EXISTS lane_seats (
    lane TEXT PRIMARY KEY,
    vendor TEXT NOT NULL,
    seq INTEGER NOT NULL REFERENCES fact_envelopes(seq)
) STRICT;
"#;

pub(crate) fn create(connection: &Connection) -> Result<(), rusqlite::Error> {
    connection.execute_batch(SCHEMA_SQL)
}

pub(crate) fn record(
    connection: &Connection,
    fact: &wire::AnyFact,
    seq: wire::Seq,
) -> Result<(), rusqlite::Error> {
    if let Some(body) = fact.read::<wire::body::LaneSeated>() {
        connection.execute(
            "INSERT INTO lane_seats(lane, vendor, seq) VALUES (?1, ?2, ?3) \
             ON CONFLICT(lane) DO UPDATE SET vendor = excluded.vendor, seq = excluded.seq",
            params![body.lane.to_string(), body.vendor.to_string(), sql_seq(seq)],
        )?;
        return Ok(());
    }
    if let Some(body) = fact.read::<wire::body::TasteReminded>() {
        insert_fire(connection, &body.taste, seq)?;
        return Ok(());
    }
    if let Some(body) = fact.read::<wire::body::TasteJudged>() {
        let confirmed = matches!(body.decision, wire::body::TasteDecision::Confirmed { .. });
        insert_judgment(connection, &body.taste, seq, confirmed)?;
        if confirmed {
            insert_fire(connection, &body.taste, seq)?;
        }
        return Ok(());
    }
    if let Some(body) = fact.read::<wire::body::TasteConfirmed>() {
        insert_judgment(connection, &body.taste, seq, true)?;
    } else if let Some(body) = fact.read::<wire::body::TasteCleared>() {
        insert_judgment(connection, &body.taste, seq, false)?;
    }
    Ok(())
}

fn insert_fire(
    connection: &Connection,
    taste: &str,
    seq: wire::Seq,
) -> Result<(), rusqlite::Error> {
    let ordinal = connection
        .query_row(
            "SELECT ordinal FROM taste_fires WHERE taste = ?1 ORDER BY seq DESC LIMIT 1",
            [taste],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0);
    connection.execute(
        "INSERT INTO taste_fires(seq, taste, ordinal) VALUES (?1, ?2, ?3)",
        params![sql_seq(seq), taste, ordinal + 1],
    )?;
    Ok(())
}

fn insert_judgment(
    connection: &Connection,
    taste: &str,
    seq: wire::Seq,
    is_confirmed: bool,
) -> Result<(), rusqlite::Error> {
    let previous = connection
        .query_row(
            "SELECT ordinal, confirmed, cleared FROM taste_judgments \
             WHERE taste = ?1 ORDER BY ordinal DESC LIMIT 1",
            [taste],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?
        .unwrap_or((0, 0, 0));
    connection.execute(
        "INSERT INTO taste_judgments(seq, taste, ordinal, confirmed, cleared) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            sql_seq(seq),
            taste,
            previous.0 + 1,
            previous.1 + i64::from(is_confirmed),
            previous.2 + i64::from(!is_confirmed),
        ],
    )?;
    Ok(())
}

fn sql_seq(seq: wire::Seq) -> i64 {
    i64::try_from(seq.get()).expect("journal positions fit SQLite")
}
