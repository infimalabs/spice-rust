//! Schema 22 to 23 materialization of lane-scoped head predicates.

use rusqlite::{Connection, params};

use super::{MigrationError, OpenError};

const FROM: u32 = 22;
const TO: u32 = 23;

pub(super) fn migrate(connection: &Connection) -> Result<(), OpenError> {
    connection
        .execute_batch(
            r#"CREATE TABLE claim_index_v23 (
                 stamp TEXT PRIMARY KEY REFERENCES claim_heads(stamp),
                 actor TEXT NOT NULL,
                 lane TEXT NOT NULL,
                 disposition TEXT NOT NULL CHECK (disposition IN ('active', 'freed', 'lapsed')),
                 head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq)
             ) STRICT;
             CREATE TABLE transcript_source_heads_v23 (
                 stream TEXT PRIMARY KEY,
                 lane TEXT NOT NULL,
                 head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
                 body BLOB NOT NULL
             ) STRICT;"#,
        )
        .map_err(OpenError::Sqlite)?;

    migrate_claims(connection)?;
    migrate_transcripts(connection)?;

    connection
        .execute_batch(
            r#"DROP TABLE claim_index;
             ALTER TABLE claim_index_v23 RENAME TO claim_index;
             CREATE INDEX claim_index_actor_disposition_stamp
                 ON claim_index(actor, disposition, stamp, lane, head_seq);
             CREATE INDEX claim_index_disposition_actor
                 ON claim_index(disposition, actor);
             CREATE INDEX claim_index_disposition_lane_stamp
                 ON claim_index(disposition, lane, stamp, head_seq);
             DROP TABLE transcript_source_heads;
             ALTER TABLE transcript_source_heads_v23 RENAME TO transcript_source_heads;
             CREATE INDEX transcript_source_heads_lane_stream
                 ON transcript_source_heads(lane, stream, head_seq);"#,
        )
        .map_err(OpenError::Sqlite)
}

fn migrate_claims(connection: &Connection) -> Result<(), OpenError> {
    let rows = {
        let mut statement = connection
            .prepare(
                "SELECT keys.stamp, heads.head_seq, heads.body, indexed.actor, \
                        indexed.disposition, indexed.head_seq \
                 FROM (SELECT stamp FROM claim_heads UNION SELECT stamp FROM claim_index) AS keys \
                 LEFT JOIN claim_heads AS heads ON heads.stamp = keys.stamp \
                 LEFT JOIN claim_index AS indexed ON indexed.stamp = keys.stamp \
                 ORDER BY keys.stamp",
            )
            .map_err(OpenError::Sqlite)?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                ))
            })
            .map_err(OpenError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };

    for (stamp, head_seq, bytes, actor, disposition, indexed_seq) in rows {
        let (Some(head_seq), Some(bytes), Some(actor), Some(disposition), Some(indexed_seq)) =
            (head_seq, bytes, actor, disposition, indexed_seq)
        else {
            return Err(invalid(
                "claim_index",
                &stamp,
                "claim head and index are not one-to-one",
            ));
        };
        let head: wire::ClaimHead = canonical("claim_heads", &stamp, &bytes)?;
        let position = positive_seq("claim_heads", &stamp, head_seq)?;
        let expected = disposition_name(head.disposition);
        if head.fact != position
            || indexed_seq != head_seq
            || actor != head.actor.to_string()
            || disposition != expected
        {
            return Err(invalid(
                "claim_index",
                &stamp,
                "predicate or head sequence disagrees with canonical claim head",
            ));
        }
        connection
            .execute(
                "INSERT INTO claim_index_v23(stamp, actor, lane, disposition, head_seq) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![stamp, actor, head.lane.to_string(), disposition, head_seq],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn migrate_transcripts(connection: &Connection) -> Result<(), OpenError> {
    let rows = {
        let mut statement = connection
            .prepare("SELECT stream, head_seq, body FROM transcript_source_heads ORDER BY stream")
            .map_err(OpenError::Sqlite)?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(OpenError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };

    for (stream, head_seq, bytes) in rows {
        positive_seq("transcript_source_heads", &stream, head_seq)?;
        let head: wire::TranscriptSourceHead =
            canonical("transcript_source_heads", &stream, &bytes)?;
        if head.key().as_str() != stream {
            return Err(invalid(
                "transcript_source_heads",
                &stream,
                "stream key disagrees with canonical transcript head",
            ));
        }
        connection
            .execute(
                "INSERT INTO transcript_source_heads_v23(stream, lane, head_seq, body) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![stream, head.lane.to_string(), head_seq, bytes],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn canonical<T: serde::de::DeserializeOwned + serde::Serialize>(
    table: &'static str,
    row: &str,
    bytes: &[u8],
) -> Result<T, OpenError> {
    let body: T = wire::decode(bytes)
        .map_err(|error| invalid(table, row, &format!("body is invalid: {error}")))?;
    let encoded = wire::encode(&body)
        .map_err(|error| invalid(table, row, &format!("body cannot encode: {error}")))?;
    if encoded != bytes {
        return Err(invalid(
            table,
            row,
            "body is not stored as canonical wire bytes",
        ));
    }
    Ok(body)
}

fn positive_seq(table: &'static str, row: &str, value: i64) -> Result<wire::Seq, OpenError> {
    u64::try_from(value)
        .ok()
        .and_then(wire::Seq::new)
        .ok_or_else(|| invalid(table, row, "head_seq is not a positive journal position"))
}

const fn disposition_name(disposition: wire::ClaimDisposition) -> &'static str {
    match disposition {
        wire::ClaimDisposition::Active => "active",
        wire::ClaimDisposition::Freed => "freed",
        wire::ClaimDisposition::Lapsed => "lapsed",
    }
}

fn invalid(table: &'static str, row: &str, detail: &str) -> OpenError {
    MigrationError::StoredContent {
        from: FROM,
        to: TO,
        table,
        row: row.to_owned(),
        detail: detail.to_owned(),
    }
    .into()
}
