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

use rusqlite::{OptionalExtension, params};

use super::room_questions;
use crate::{MigrationError, OpenError};

/// Last release whose decoders admitted room coordination.
const RELEASE: &str = "54e357267";

pub(super) struct RetiredRow {
    seq: i64,
    wire_version: i64,
    instant: String,
    actor: String,
    origin: String,
    cause: Option<i64>,
    envelope: Vec<u8>,
    kind: String,
}

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
    let rows = read_rows(
        transaction,
        "SELECT seq, wire_version, instant, actor, origin, cause, envelope, kind \
         FROM fact_envelopes WHERE kind IN ('room::directed', 'room::shelved') ORDER BY seq",
    )?;
    let mut retired = BTreeSet::new();
    let mut positions = BTreeSet::new();
    for row in rows {
        if let Ok(seq) = u64::try_from(row.seq) {
            positions.insert(wire::SequenceCode::new(seq).to_string());
        }
        retired.insert(row.seq);
        retire_row(transaction, row, RELEASE, retained_fact_error)?;
    }
    if positions.is_empty() {
        return Ok(retired);
    }
    let deliveries = read_rows(
        transaction,
        "SELECT seq, wire_version, instant, actor, origin, cause, envelope, kind \
         FROM fact_envelopes WHERE kind = 'room::delivered' ORDER BY seq",
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
            retire_row(transaction, row, RELEASE, retained_fact_error)?;
        }
    }
    Ok(retired)
}

/// Each `fact_envelopes` row `select` names, as `seq, wire_version, instant,
/// actor, origin, cause, envelope, kind`.
pub(super) fn read_rows(
    transaction: &rusqlite::Transaction<'_>,
    select: &str,
) -> Result<Vec<RetiredRow>, OpenError> {
    let mut statement = transaction.prepare(select).map_err(OpenError::Sqlite)?;
    let rows = statement
        .query_map([], |row| {
            Ok(RetiredRow {
                seq: row.get(0)?,
                wire_version: row.get(1)?,
                instant: row.get(2)?,
                actor: row.get(3)?,
                origin: row.get(4)?,
                cause: row.get(5)?,
                envelope: row.get(6)?,
                kind: row.get(7)?,
            })
        })
        .map_err(OpenError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)?;
    Ok(rows)
}

/// Replace one retired row in place with inert [`wire::body::JournalRetained`]
/// evidence naming `release`, the last release whose decoders admitted it,
/// after proving its envelope agrees with its indexed columns.
pub(super) fn retire_row(
    transaction: &rusqlite::Transaction<'_>,
    row: RetiredRow,
    release: &str,
    retained_fact_error: fn(i64, String) -> OpenError,
) -> Result<(), OpenError> {
    let envelope: wire::Envelope<serde_json::Value> = serde_json::from_slice(&row.envelope)
        .map_err(|error| {
            retained_fact_error(
                row.seq,
                format!("retired fact envelope does not decode: {error}"),
            )
        })?;
    let indexed_cause = row.cause.and_then(|cause| u64::try_from(cause).ok());
    if !envelope.v.is_current()
        || Some(envelope.seq.get()) != u64::try_from(row.seq).ok()
        || i64::from(envelope.v.0) != row.wire_version
        || envelope.at.to_string() != row.instant
        || envelope.actor.to_string() != row.actor
        || envelope.origin.as_str() != row.origin
        || envelope.cause.map(wire::Seq::get) != indexed_cause
    {
        return Err(retained_fact_error(
            row.seq,
            "retired fact envelope disagrees with its indexed columns".to_owned(),
        ));
    }
    let encoded = wire::encode(&wire::Fact::new(wire::body::JournalRetained {
        release: release.to_owned(),
        fact: envelope.fact,
    }))
    .map_err(|error| {
        retained_fact_error(
            row.seq,
            format!("retired fact cannot become inert authority: {error}"),
        )
    })?;
    let fact: wire::AnyFact = wire::decode(&encoded).map_err(|error| {
        retained_fact_error(
            row.seq,
            format!("inert replacement does not decode: {error}"),
        )
    })?;
    let replacement = wire::Envelope {
        v: envelope.v,
        seq: envelope.seq,
        at: envelope.at,
        actor: envelope.actor,
        origin: envelope.origin,
        cause: envelope.cause,
        fact,
    };
    let bytes = wire::encode(&replacement).map_err(|error| {
        retained_fact_error(
            row.seq,
            format!("inert replacement does not encode: {error}"),
        )
    })?;
    let changed = transaction
        .execute(
            "UPDATE fact_envelopes SET kind = ?1, envelope = ?2 WHERE seq = ?3 AND kind = ?4",
            params![
                wire::FactKind::JournalRetained.as_str(),
                bytes,
                row.seq,
                row.kind
            ],
        )
        .map_err(OpenError::Sqlite)?;
    if changed != 1 {
        return Err(retained_fact_error(
            row.seq,
            "retired fact changed during exclusive migration".to_owned(),
        ));
    }
    Ok(())
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
            room_questions::restore_cursor(transaction, &lane, seq, retained_fact_error)?;
        }
    }
    Ok(())
}

fn retained_fact_error(seq: i64, detail: String) -> OpenError {
    MigrationError::RetiredFact {
        seq,
        kind: "retired room coordination authority",
        release: RELEASE,
        detail,
    }
    .into()
}
