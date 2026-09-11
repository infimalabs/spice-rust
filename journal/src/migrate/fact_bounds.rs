//! Schema-21 certification and enforcement of the durable fact-envelope maximum.

use rusqlite::Connection;

use super::{IndexActivation, MigrationError, OpenError};
use crate::MAX_RETAINED_FACT_PAGE_BYTES;

pub(crate) const FACT_BOUND_VALIDATION_ROW_LIMIT: u64 = 4_096;
pub(crate) const FACT_BOUND_VALIDATION_BYTE_LIMIT: u64 = 16 * 1024 * 1024;

pub(crate) const FACT_ENVELOPE_INSERT_TRIGGER: &str = "fact_envelopes_envelope_size_insert";
pub(crate) const FACT_ENVELOPE_UPDATE_TRIGGER: &str = "fact_envelopes_envelope_size_update";

pub(crate) fn insert_sql() -> String {
    format!(
        r#"
CREATE TRIGGER fact_envelopes_envelope_size_insert
BEFORE INSERT ON fact_envelopes
WHEN length(NEW.envelope) > {limit}
BEGIN
    SELECT RAISE(ABORT, 'fact envelope exceeds {limit} bytes');
END
"#,
        limit = MAX_RETAINED_FACT_PAGE_BYTES,
    )
}

pub(crate) fn update_sql() -> String {
    format!(
        r#"
CREATE TRIGGER fact_envelopes_envelope_size_update
BEFORE UPDATE OF envelope ON fact_envelopes
WHEN length(NEW.envelope) > {limit}
BEGIN
    SELECT RAISE(ABORT, 'fact envelope exceeds {limit} bytes');
END
"#,
        limit = MAX_RETAINED_FACT_PAGE_BYTES,
    )
}

pub(crate) fn bounds_sql() -> String {
    format!(
        "DROP TRIGGER IF EXISTS {FACT_ENVELOPE_INSERT_TRIGGER};\n\
         DROP TRIGGER IF EXISTS {FACT_ENVELOPE_UPDATE_TRIGGER};\n{};\n{};",
        insert_sql(),
        update_sql(),
    )
}

/// Validate only SQLite length metadata, then publish the two exact triggers.
pub(super) fn migrate(
    connection: &Connection,
    activation: IndexActivation,
) -> Result<(), OpenError> {
    let query_limit = match activation {
        IndexActivation::Startup => FACT_BOUND_VALIDATION_ROW_LIMIT.saturating_add(1),
        IndexActivation::Maintenance => i64::MAX as u64,
    };
    let query_limit = i64::try_from(query_limit).expect("fact validation limit fits SQLite");
    let mut statement = connection
        .prepare("SELECT seq, length(envelope) FROM fact_envelopes ORDER BY seq LIMIT ?1")
        .map_err(OpenError::Sqlite)?;
    let rows = statement
        .query_map([query_limit], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(OpenError::Sqlite)?;
    let mut count = 0_u64;
    let mut cumulative = 0_u64;
    for row in rows {
        let (seq, length) = row.map_err(OpenError::Sqlite)?;
        count = count.saturating_add(1);
        if matches!(activation, IndexActivation::Startup) && count > FACT_BOUND_VALIDATION_ROW_LIMIT
        {
            return Err(MigrationError::FactBoundValidationRowsRequired {
                limit: FACT_BOUND_VALIDATION_ROW_LIMIT,
                observed: count,
            }
            .into());
        }
        let observed = u64::try_from(length).map_err(|_| MigrationError::StoredContent {
            from: 20,
            to: 21,
            table: "fact_envelopes",
            row: seq.to_string(),
            detail: format!("envelope length {length} is negative"),
        })?;
        let limit = u64::try_from(MAX_RETAINED_FACT_PAGE_BYTES)
            .expect("fact envelope limit fits the migration counter");
        if observed > limit {
            return Err(MigrationError::OversizedFactEnvelope {
                seq,
                observed,
                limit,
            }
            .into());
        }
        cumulative = cumulative.saturating_add(observed);
        if matches!(activation, IndexActivation::Startup)
            && cumulative > FACT_BOUND_VALIDATION_BYTE_LIMIT
        {
            return Err(MigrationError::FactBoundValidationBytesRequired {
                limit: FACT_BOUND_VALIDATION_BYTE_LIMIT,
                observed: cumulative,
            }
            .into());
        }
    }
    drop(statement);
    connection
        .execute_batch(&bounds_sql())
        .map_err(OpenError::Sqlite)
}
