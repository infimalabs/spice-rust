//! Schema 13 to 14 normalized steering-key microsecond high-water.
//!
//! Steering heads are rebuildable, but a schema crossing must remain coherent
//! before any resident has a chance to rebuild them. This step therefore
//! validates and rewrites only those derived head bodies, preserves immutable
//! facts and misses byte-for-byte, and renames the indexed moment in place.

use rusqlite::{Connection, params};

use super::{MigrationError, OpenError};

const FROM: u32 = 13;
const TO: u32 = 14;

pub(super) fn migrate(connection: &Connection) -> Result<(), OpenError> {
    let rows = heads(connection)?;
    connection
        .execute_batch(
            "DROP INDEX steering_heads_minted_millis; \
             ALTER TABLE steering_heads RENAME COLUMN minted_millis TO minted_micros; \
             CREATE INDEX steering_heads_minted_micros \
                 ON steering_heads(minted_micros) WHERE minted_micros IS NOT NULL;",
        )
        .map_err(OpenError::Sqlite)?;
    for row in rows {
        rewrite(connection, row)?;
    }
    Ok(())
}

struct LegacyHead {
    directive: String,
    indexed_millis: Option<i64>,
    body: Vec<u8>,
}

fn heads(connection: &Connection) -> Result<Vec<LegacyHead>, OpenError> {
    let mut statement = connection
        .prepare("SELECT directive, minted_millis, body FROM steering_heads ORDER BY directive")
        .map_err(OpenError::Sqlite)?;
    statement
        .query_map([], |row| {
            Ok(LegacyHead {
                directive: row.get(0)?,
                indexed_millis: row.get(1)?,
                body: row.get(2)?,
            })
        })
        .map_err(OpenError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)
}

fn rewrite(connection: &Connection, row: LegacyHead) -> Result<(), OpenError> {
    let mut head: wire::SteeringHead = wire::decode(&row.body).map_err(|error| {
        invalid(
            &row.directive,
            &format!("schema-13 steering head is not current-compatible wire: {error}"),
        )
    })?;
    if head.directive.to_string() != row.directive || head.minted_micros != row.indexed_millis {
        return Err(invalid(
            &row.directive,
            "indexed steering identity or mint moment disagrees with its body",
        ));
    }
    let micros = match row.indexed_millis {
        Some(millis) => Some(millis.checked_mul(1_000).ok_or_else(|| {
            invalid(
                &row.directive,
                "millisecond mint moment overflows microseconds",
            )
        })?),
        None => None,
    };
    head.minted_micros = micros;
    let body = wire::encode(&head).map_err(|error| {
        invalid(
            &row.directive,
            &format!("migrated steering head cannot encode: {error}"),
        )
    })?;
    connection
        .execute(
            "UPDATE steering_heads SET minted_micros = ?1, body = ?2 WHERE directive = ?3",
            params![micros, body, row.directive],
        )
        .map_err(OpenError::Sqlite)?;
    Ok(())
}

fn invalid(row: &str, detail: &str) -> OpenError {
    MigrationError::StoredContent {
        from: FROM,
        to: TO,
        table: "steering_heads",
        row: row.to_owned(),
        detail: detail.to_owned(),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::migrate;

    #[test]
    fn schema_thirteen_heads_normalize_once_without_touching_identity() {
        let connection = Connection::open_in_memory().expect("migration fixture");
        connection
            .execute_batch(
                "CREATE TABLE steering_heads ( \
                    directive TEXT PRIMARY KEY, steer_key TEXT NOT NULL UNIQUE, \
                    lane TEXT NOT NULL, disposition TEXT NOT NULL, created_seq INTEGER NOT NULL, \
                    minted_millis INTEGER, head_seq INTEGER NOT NULL, body BLOB NOT NULL \
                 ) STRICT; \
                 CREATE INDEX steering_heads_minted_millis \
                    ON steering_heads(minted_millis) WHERE minted_millis IS NOT NULL;",
            )
            .expect("schema-thirteen steering table");
        let directive = "018f82d4-6f59-7d06-92e3-bce913116399";
        let millis = 1_700_000_000_000_i64;
        let body = format!(
            concat!(
                "{{\"directive\":\"{directive}\",",
                "\"key\":\"1dzGkSrJ\",",
                "\"lane\":\"018f82d4-6f59-7d06-92e3-bce913116302\",",
                "\"text\":\"retained direction\",",
                "\"published\":\"2023-11-14T22:13:20.000Z\",",
                "\"expires\":\"2030-04-12T19:44:09.271Z\",",
                "\"stage\":\"published\",\"disposition\":\"live\",",
                "\"origin\":\"resident:018f82d4-6f59-7d06-92e3-bce913116304\",",
                "\"created\":1,\"minted_millis\":{millis}}}"
            ),
            directive = directive,
            millis = millis,
        );
        connection
            .execute(
                "INSERT INTO steering_heads( \
                    directive, steer_key, lane, disposition, created_seq, \
                    minted_millis, head_seq, body \
                 ) VALUES (?1, '1dzGkSrJ', \
                    '018f82d4-6f59-7d06-92e3-bce913116302', 'live', 1, ?2, 1, ?3)",
                rusqlite::params![directive, millis, body.as_bytes()],
            )
            .expect("legacy head");

        migrate(&connection).expect("schema thirteen migrates");
        let (key, micros, encoded): (String, i64, Vec<u8>) = connection
            .query_row(
                "SELECT steer_key, minted_micros, body FROM steering_heads",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("migrated head");
        let head: wire::SteeringHead = wire::decode(&encoded).expect("current head wire");
        assert_eq!(key, "1dzGkSrJ");
        assert_eq!(micros, millis * 1_000);
        assert_eq!(head.minted_micros, Some(micros));
        assert!(
            String::from_utf8(encoded)
                .unwrap()
                .contains("\"minted_micros\"")
        );
    }
}
