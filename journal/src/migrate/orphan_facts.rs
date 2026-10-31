//! Retire fact kinds that no current release produces.
//!
//! Schema 103 and earlier still registered `consent::floored`,
//! `vendor::armed`, `commit::landed`, `gate::found`, `gate::cleared`, the five
//! graceful-shutdown transitions `up::quiescing` through `up::verifying`, and
//! the lane-global catch-up start `room::horizon`. Schema 104 and earlier
//! still registered the provenance-less judge rulings `taste::confirmed` and
//! `taste::cleared` and the lease warning `claim::warned`. Schema 105 and
//! earlier still registered the Design pass critique `proposal::critiqued`,
//! coverage `proposal::covered`, and operator nudge `proposal::nudged`. No
//! current path writes any of them. Each retained row crosses as
//! [`wire::body::JournalRetained`] evidence by the ordinary rule for removed
//! fact kinds. The horizon projection, `room_horizons`, which no current reader
//! consults, drops; a retained ruling keeps its place in the standing judgment
//! tally, which reads it back from the retained evidence; a lease warning
//! takes its whole steering directive with it (see [`claim_warnings`]); a
//! critique or coverage statement, evidence that never gated a decision, drops
//! its write-once proposal head; and a retained nudge still readies its Design
//! through [`wire::rooms::RetainedNudge`].

use super::retire::{self, COLUMNS, Retirement};
use crate::OpenError;

mod claim_warnings;

/// The kinds schema 103 last admitted.
const SCHEMA_103: Retirement = Retirement {
    release: "schema-103",
    what: "retired orphan fact",
};

/// The kinds schema 104 last admitted.
const SCHEMA_104: Retirement = Retirement {
    release: "schema-104",
    what: "retired orphan fact",
};

/// The kinds schema 105 last admitted.
const SCHEMA_105: Retirement = Retirement {
    release: "schema-105",
    what: "retired orphan fact",
};

/// Every kind retired as of [`SCHEMA_103`], with no dependent authority.
const SCHEMA_103_KINDS: [&str; 11] = [
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

/// Every kind retired as of [`SCHEMA_104`] with no dependent authority.
const SCHEMA_104_KINDS: [&str; 2] = ["taste::confirmed", "taste::cleared"];

/// Every kind retired as of [`SCHEMA_105`].
const SCHEMA_105_KINDS: [&str; 3] = [
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
    retire_kinds(transaction, &SCHEMA_103_KINDS, SCHEMA_103)?;
    transaction
        .execute_batch("DROP TABLE IF EXISTS room_horizons;")
        .map_err(OpenError::Sqlite)?;
    retire_kinds(transaction, &SCHEMA_104_KINDS, SCHEMA_104)?;
    claim_warnings::cross(transaction, SCHEMA_104)?;
    retire_kinds(transaction, &SCHEMA_105_KINDS, SCHEMA_105)?;
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
