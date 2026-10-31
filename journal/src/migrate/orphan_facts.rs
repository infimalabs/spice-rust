//! Retire fact kinds that no current release produces.
//!
//! Schema 107 and earlier still registered `consent::floored`,
//! `vendor::armed`, `commit::landed`, `gate::found`, `gate::cleared`, the five
//! graceful-shutdown transitions `up::quiescing` through `up::verifying`, the
//! lane-global catch-up start `room::horizon`, the provenance-less judge
//! rulings `taste::confirmed` and `taste::cleared`, the lease warning
//! `claim::warned`, the Design pass critique `proposal::critiqued`, coverage
//! `proposal::covered`, and operator nudge `proposal::nudged`, and the task
//! note `task::noted`. No current path writes any of them. Each retained row
//! crosses as [`wire::body::JournalRetained`] evidence naming schema-107 by
//! the ordinary rule for removed fact kinds. The horizon projection,
//! `room_horizons`, which no current reader consults, drops; a retained ruling
//! keeps its place in the standing judgment tally, which reads it back from
//! the retained evidence; a lease warning takes its whole steering directive
//! with it (see [`claim_warnings`]); a critique or coverage statement,
//! evidence that never gated a decision, drops its write-once proposal head; a
//! retained nudge still readies its Design through
//! [`wire::rooms::RetainedNudge`]; and a retained note keeps its room
//! publication and its place in its task's generation history through
//! [`wire::body::RetainedTaskNote`]. Later crossings that read notes, the
//! taste-hold audits and the schema 76 room index, read the retained spelling.

use wire::body::RetainedTaskNote;

use super::retire::{self, COLUMNS, Retirement};
use crate::OpenError;

mod claim_warnings;

/// The kinds schema 107 last admitted.
const SCHEMA_107: Retirement = Retirement {
    release: "schema-107",
    what: "retired orphan fact",
};

/// The task notes schema 107 last admitted.
const SCHEMA_107_NOTES: Retirement = Retirement {
    release: "schema-107",
    what: "retired task note",
};

/// Every retired kind with no dependent authority beyond its own row.
const RETIRED_KINDS: [&str; 16] = [
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
    "taste::confirmed",
    "taste::cleared",
    "proposal::critiqued",
    "proposal::covered",
    "proposal::nudged",
];

/// Retired proposal evidence whose write-once head drops with it.
const PROPOSAL_EVIDENCE_HEADS: &str = "DELETE FROM proposal_heads WHERE head_seq IN ( \
     SELECT seq FROM fact_envelopes WHERE kind = 'journal::retained' \
     AND json_extract(CAST(envelope AS TEXT), '$.fact.fact.kind') \
     IN ('proposal::critiqued', 'proposal::covered'))";

pub(super) fn cross(transaction: &rusqlite::Transaction<'_>) -> Result<(), OpenError> {
    retire_kinds(transaction, &RETIRED_KINDS, SCHEMA_107)?;
    transaction
        .execute_batch("DROP TABLE IF EXISTS room_horizons;")
        .map_err(OpenError::Sqlite)?;
    claim_warnings::cross(transaction, SCHEMA_107)?;
    retire_kinds(transaction, &[RetainedTaskNote::KIND], SCHEMA_107_NOTES)?;
    if super::table_exists(transaction, "proposal_heads")? {
        transaction
            .execute_batch(PROPOSAL_EVIDENCE_HEADS)
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn retire_kinds(
    transaction: &rusqlite::Transaction<'_>,
    kinds: &[&str],
    retirement: Retirement,
) -> Result<(), OpenError> {
    let kinds = kinds
        .iter()
        .map(|kind| format!("'{kind}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let rows = retire::read_rows(
        transaction,
        &format!("SELECT {COLUMNS} FROM fact_envelopes WHERE kind IN ({kinds}) ORDER BY seq"),
        [],
    )?;
    for row in rows {
        retire::retire_row(transaction, row, retirement)?;
    }
    Ok(())
}
