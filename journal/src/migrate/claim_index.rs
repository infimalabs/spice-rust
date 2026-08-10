//! Schema 7 to 8 materialization of current claim predicate columns.

use rusqlite::{Connection, params};

use super::{MigrationError, OpenError};

const FROM: u32 = 7;
const TO: u32 = 8;

pub(super) fn migrate(connection: &Connection) -> Result<(), OpenError> {
    connection
        .execute_batch(
            r#"CREATE TABLE claim_index (
                 stamp TEXT PRIMARY KEY REFERENCES claim_heads(stamp),
                 actor TEXT NOT NULL,
                 disposition TEXT NOT NULL CHECK (disposition IN ('active', 'freed', 'lapsed')),
                 head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq)
             ) STRICT;
             CREATE INDEX claim_index_actor_disposition_stamp
                 ON claim_index(actor, disposition, stamp);
             CREATE INDEX claim_index_disposition_actor
                 ON claim_index(disposition, actor);"#,
        )
        .map_err(OpenError::Sqlite)?;

    let rows = {
        let mut statement = connection
            .prepare("SELECT stamp, head_seq, body FROM claim_heads ORDER BY stamp")
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

    for (stamp, head_seq, bytes) in rows {
        let _: wire::TaskStamp = stamp
            .parse()
            .map_err(|error| invalid(&stamp, &format!("task stamp is invalid: {error}")))?;
        let position = u64::try_from(head_seq)
            .ok()
            .and_then(wire::Seq::new)
            .ok_or_else(|| invalid(&stamp, "head_seq is not a positive journal position"))?;
        let head: wire::ClaimHead = wire::decode(&bytes)
            .map_err(|error| invalid(&stamp, &format!("claim head is invalid: {error}")))?;
        let canonical = wire::encode(&head).map_err(|error| {
            invalid(
                &stamp,
                &format!("claim head cannot encode canonically: {error}"),
            )
        })?;
        if canonical != bytes {
            return Err(invalid(
                &stamp,
                "claim head is not stored as canonical wire bytes",
            ));
        }
        if head.fact != position {
            return Err(invalid(&stamp, "claim head fact disagrees with head_seq"));
        }
        connection
            .execute(
                "INSERT INTO claim_index(stamp, actor, disposition, head_seq) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    stamp,
                    head.actor.to_string(),
                    disposition_name(head.disposition),
                    head_seq
                ],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

const fn disposition_name(disposition: wire::ClaimDisposition) -> &'static str {
    match disposition {
        wire::ClaimDisposition::Active => "active",
        wire::ClaimDisposition::Freed => "freed",
        wire::ClaimDisposition::Lapsed => "lapsed",
    }
}

fn invalid(row: &str, detail: &str) -> OpenError {
    MigrationError::StoredContent {
        from: FROM,
        to: TO,
        table: "claim_heads",
        row: row.to_owned(),
        detail: detail.to_owned(),
    }
    .into()
}
