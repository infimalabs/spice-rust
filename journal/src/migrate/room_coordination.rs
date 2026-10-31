//! Retire room coordination into inert authority.
//!
//! Schema 96 and earlier recorded `room::directed` and `room::shelved`
//! publications and answered `room::coordinate`, `room::put-aside`, and
//! `room::coordination-read`. A Design nudge now selects and messages its
//! recipient, so those records cross as [`wire::body::JournalRetained`]
//! evidence by the ordinary rule for removed fact kinds, their receipts move
//! to the historical table, and their projection heads drop. A room delivery
//! whose cursor named only a retired publication retires with it, exactly as
//! the question retirement did, so replay never cites a position that no
//! longer indexes.

use std::collections::BTreeSet;

use rusqlite::OptionalExtension;

use super::retire::{self, COLUMNS, Retirement};
use super::room_questions;
use crate::OpenError;

/// The retired coordination records and the last release that admitted them.
const RETIREMENT: Retirement = Retirement {
    release: "54e357267",
    what: "retired room coordination authority",
};

pub(super) fn cross(transaction: &rusqlite::Transaction<'_>) -> Result<(), OpenError> {
    crate::historical_receipts::create_schema(transaction)?;
    crate::historical_receipts::retire_room_coordination(transaction)?;
    crate::historical_receipts::reserve_identities(transaction)?;
    let retired = retire_facts(transaction)?;
    repair_projections(transaction, &retired)?;
    transaction
        .execute_batch("DROP TABLE IF EXISTS room_directions; DROP TABLE IF EXISTS room_shelvings;")
        .map_err(OpenError::Sqlite)
}

/// Retire every coordination publication and each delivery that advanced
/// only to one, answering their positions.
fn retire_facts(transaction: &rusqlite::Transaction<'_>) -> Result<BTreeSet<i64>, OpenError> {
    let rows = retire::read_rows(
        transaction,
        &format!(
            "SELECT {COLUMNS} FROM fact_envelopes \
             WHERE kind IN ('room::directed', 'room::shelved') ORDER BY seq"
        ),
        [],
    )?;
    let mut retired = BTreeSet::new();
    let mut positions = BTreeSet::new();
    for row in rows {
        if let Ok(seq) = u64::try_from(row.seq) {
            positions.insert(wire::SequenceCode::new(seq).to_string());
        }
        retired.insert(row.seq);
        retire::retire_row(transaction, row, RETIREMENT)?;
    }
    if positions.is_empty() {
        return Ok(retired);
    }
    let deliveries = retire::read_rows(
        transaction,
        &format!(
            "SELECT {COLUMNS} FROM fact_envelopes WHERE kind = 'room::delivered' ORDER BY seq"
        ),
        [],
    )?;
    for row in deliveries {
        let through = serde_json::from_slice::<wire::Envelope<serde_json::Value>>(&row.envelope)
            .ok()
            .and_then(|envelope| {
                envelope
                    .fact
                    .get("through")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            });
        if through.is_some_and(|through| positions.contains(&through)) {
            retired.insert(row.seq);
            retire::retire_row(transaction, row, RETIREMENT)?;
        }
    }
    Ok(retired)
}

fn repair_projections(
    transaction: &rusqlite::Transaction<'_>,
    retired: &BTreeSet<i64>,
) -> Result<(), OpenError> {
    for &seq in retired {
        if super::table_exists(transaction, "room_publications")? {
            transaction
                .execute("DELETE FROM room_publications WHERE seq = ?1", [seq])
                .map_err(OpenError::Sqlite)?;
        }
        if !super::table_exists(transaction, "room_cursors")? {
            continue;
        }
        let cursor = transaction
            .query_row(
                "SELECT lane FROM room_cursors WHERE head_seq = ?1",
                [seq],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(OpenError::Sqlite)?;
        if let Some(lane) = cursor {
            room_questions::restore_cursor(transaction, &lane, seq, |seq, detail| {
                RETIREMENT.error(seq, detail)
            })?;
        }
    }
    Ok(())
}
