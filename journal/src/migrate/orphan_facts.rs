//! Retire fact kinds that no current release produces.
//!
//! Schema 103 and earlier still registered `consent::floored`,
//! `vendor::armed`, `commit::landed`, `gate::found`, `gate::cleared`, the five
//! graceful-shutdown transitions `up::quiescing` through `up::verifying`, and
//! the lane-global catch-up start `room::horizon`. No current path writes any
//! of them, and only the horizon fed a projection, `room_horizons`, which no
//! current reader consults. Each retained row crosses as
//! [`wire::body::JournalRetained`] evidence by the ordinary rule for removed
//! fact kinds, and the horizon projection drops.

use super::retire::{self, COLUMNS, Retirement};
use crate::OpenError;

/// The orphan kinds and the last schema whose decoders admitted them.
const RETIREMENT: Retirement = Retirement {
    release: "schema-103",
    what: "retired orphan fact",
};

/// Every kind this crossing retires.
const KINDS: [&str; 11] = [
    "consent::floored",
    "vendor::armed",
    "commit::landed",
    "gate::found",
    "gate::cleared",
    "up::quiescing",
    "up::draining",
    "up::terminating",
    "up::killing",
    "up::verifying",
    "room::horizon",
];

pub(super) fn cross(transaction: &rusqlite::Transaction<'_>) -> Result<(), OpenError> {
    let kinds = KINDS.map(|kind| format!("'{kind}'")).join(", ");
    let rows = retire::read_rows(
        transaction,
        &format!("SELECT {COLUMNS} FROM fact_envelopes WHERE kind IN ({kinds}) ORDER BY seq"),
    )?;
    for row in rows {
        retire::retire_row(transaction, row, RETIREMENT)?;
    }
    transaction
        .execute_batch("DROP TABLE IF EXISTS room_horizons;")
        .map_err(OpenError::Sqlite)
}
