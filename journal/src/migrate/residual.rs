//! Retained fact, receipt, and head shapes whose last decoder retired with the
//! release named beside each one.
//!
//! Shapes with an exact current spelling cross in place; shapes without one
//! refuse the crossing, which leaves the predecessor file untouched.

use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::{Map, Value};

use super::{MigrationError, OpenError};

/// Last release whose build recorded claimant notices on task edits.
const TASK_EDIT_NOTICE_RELEASE: &str = "a3cebe157";
/// Release from which lane fast facts spell both flags; earlier absence meant false.
const LANE_FAST_FLAGS_RELEASE: &str = "e904cfe7b";
/// Release from which every claim win carries its claim-time cell.
const CLAIM_CELL_RELEASE: &str = "ea3fe662a";
/// Release from which every publication names its release-note digest.
const RELEASE_NOTES_RELEASE: &str = "987f0d60b";
/// Release from which steering heads spell their mint instant in microseconds.
const STEERING_MICROS_RELEASE: &str = "d811711a3";
/// Release from which vendor-failure holds carry their exact refused input.
const VENDOR_FAILURE_INPUT_RELEASE: &str = "3903331e5";
/// Release from which lane session declarations name their tape generation.
const SESSION_SOURCE_RELEASE: &str = "a4abe071f";
/// Release from which resumable sessions carry exact launch identity.
const SESSION_LAUNCH_RELEASE: &str = "9f2487d5a";
/// Release from which steering transcript evidence names its tape generation.
const STEERING_SOURCE_RELEASE: &str = "cb609bd56";

/// Refuse every retained shape without a current spelling before mutation.
pub(super) fn refuse(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    refuse_first(
        transaction,
        "fact_envelopes",
        "SELECT CAST(seq AS TEXT) FROM fact_envelopes WHERE kind = ?1 \
         AND json_type(CAST(envelope AS TEXT), '$.fact.cell') IS NULL ORDER BY seq",
        [wire::FactKind::ClaimWon.as_str()],
        CLAIM_CELL_RELEASE,
        "the claim win omits its claim-time cell",
    )?;
    refuse_first(
        transaction,
        "fact_envelopes",
        "SELECT CAST(seq AS TEXT) FROM fact_envelopes WHERE kind = ?1 \
         AND json_type(CAST(envelope AS TEXT), '$.fact.notes') = 'null' ORDER BY seq",
        [wire::FactKind::ReleasePublished.as_str()],
        RELEASE_NOTES_RELEASE,
        "the publication has no release-note digest",
    )?;
    refuse_first(
        transaction,
        "command_receipts",
        "SELECT command_id FROM command_receipts WHERE command_kind = ?1 \
         AND json_type(CAST(request AS TEXT), '$.body.notes') = 'null' ORDER BY command_id",
        [wire::CommandKind::ReleasePublish.as_str()],
        RELEASE_NOTES_RELEASE,
        "the publication request has no release-note digest",
    )?;
    refuse_first(
        transaction,
        "steering_heads",
        "SELECT directive FROM steering_heads \
         WHERE json_type(CAST(body AS TEXT), '$.minted_millis') IS NOT NULL ORDER BY directive",
        (),
        STEERING_MICROS_RELEASE,
        "the steering head spells its mint instant in milliseconds",
    )?;
    refuse_first(
        transaction,
        "fact_envelopes",
        "SELECT CAST(seq AS TEXT) FROM fact_envelopes WHERE kind = ?1 \
         AND json_type(CAST(envelope AS TEXT), '$.fact.vendor_failure') IS NULL \
         AND instr(COALESCE(json_extract(CAST(envelope AS TEXT), '$.fact.evidence'), ''), \
             ' ended on a vendor error:') > 0 ORDER BY seq",
        [wire::FactKind::LaneHeld.as_str()],
        VENDOR_FAILURE_INPUT_RELEASE,
        "the vendor-failure hold omits its exact refused input",
    )?;
    refuse_first(
        transaction,
        "fact_envelopes",
        "SELECT CAST(seq AS TEXT) FROM fact_envelopes WHERE kind = ?1 \
         AND json_type(CAST(envelope AS TEXT), '$.fact.source') IS NULL ORDER BY seq",
        [wire::FactKind::LaneSessionDeclared.as_str()],
        SESSION_SOURCE_RELEASE,
        "the lane session declaration omits its tape generation",
    )?;
    refuse_first(
        transaction,
        "fact_envelopes",
        "SELECT CAST(seq AS TEXT) FROM fact_envelopes WHERE kind = ?1 \
         AND (json_type(CAST(envelope AS TEXT), '$.fact.model') IS NULL \
              OR json_type(CAST(envelope AS TEXT), '$.fact.effort') IS NULL) ORDER BY seq",
        [wire::FactKind::LaneSessionDeclared.as_str()],
        SESSION_LAUNCH_RELEASE,
        "the lane session declaration omits its launch identity",
    )?;
    refuse_first(
        transaction,
        "residency_head",
        "SELECT CAST(singleton AS TEXT) FROM residency_head, \
         json_each(CAST(body AS TEXT), '$.lanes') AS lane \
         WHERE json_type(lane.value, '$.session') = 'object' \
         AND (json_type(lane.value, '$.session.model') IS NULL \
              OR json_type(lane.value, '$.session.effort') IS NULL) ORDER BY singleton",
        (),
        SESSION_LAUNCH_RELEASE,
        "the resumable lane session omits its launch identity",
    )?;
    for kind in [
        wire::FactKind::SteerAcked,
        wire::FactKind::SteerDeclined,
        wire::FactKind::SteerMissed,
        wire::FactKind::SteerMessaged,
        wire::FactKind::SteerWithheld,
    ] {
        refuse_first(
            transaction,
            "fact_envelopes",
            "SELECT CAST(seq AS TEXT) FROM fact_envelopes WHERE kind = ?1 \
             AND json_type(CAST(envelope AS TEXT), '$.fact.locus.source') IS NULL ORDER BY seq",
            [kind.as_str()],
            STEERING_SOURCE_RELEASE,
            "the steering transcript locus omits its tape generation",
        )?;
    }
    Ok(())
}

/// Rewrite every retained shape that has one exact current spelling.
pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    rewrite_facts(
        transaction,
        wire::FactKind::TaskEdited,
        "json_type(CAST(envelope AS TEXT), '$.fact.notice') IS NOT NULL",
        TASK_EDIT_NOTICE_RELEASE,
        |fact| match fact.remove("notice") {
            Some(Value::Object(_)) => Ok(()),
            other => Err(format!("notice {other:?} is not a claimant notice object")),
        },
    )?;
    rewrite_facts(
        transaction,
        wire::FactKind::LaneFastRetuned,
        "json_type(CAST(envelope AS TEXT), '$.fact.from') IS NULL \
         OR json_type(CAST(envelope AS TEXT), '$.fact.fast_mode') IS NULL",
        LANE_FAST_FLAGS_RELEASE,
        |fact| {
            for flag in ["from", "fast_mode"] {
                fact.entry(flag).or_insert(Value::Bool(false));
            }
            Ok(())
        },
    )
}

/// Refuse naming the first row `sql` selects, if any.
fn refuse_first(
    transaction: &Transaction<'_>,
    table: &'static str,
    sql: &str,
    parameters: impl rusqlite::Params,
    release: &'static str,
    detail: &str,
) -> Result<(), OpenError> {
    match transaction
        .query_row(sql, parameters, |row| row.get::<_, String>(0))
        .optional()
        .map_err(OpenError::Sqlite)?
    {
        Some(row) => Err(retained(table, row, release, detail.to_owned())),
        None => Ok(()),
    }
}

/// Rewrite each `kind` fact `predicate` selects through `edit` on its body.
///
/// Each envelope keeps its seq, indexed columns, and every other member, and is
/// re-encoded canonically through the current strict DTO. No digest covers an
/// individual envelope, so nothing downstream disagrees with the new bytes.
pub(super) fn rewrite_facts(
    transaction: &Transaction<'_>,
    kind: wire::FactKind,
    predicate: &str,
    release: &'static str,
    edit: impl Fn(&mut Map<String, Value>) -> Result<(), String>,
) -> Result<(), OpenError> {
    let kind = kind.as_str();
    let rows = {
        let mut statement = transaction
            .prepare(&format!(
                "SELECT seq, envelope FROM fact_envelopes WHERE kind = ?1 AND ({predicate}) \
                 ORDER BY seq"
            ))
            .map_err(OpenError::Sqlite)?;
        let rows = statement
            .query_map([kind], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(OpenError::Sqlite)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };
    for (seq, bytes) in rows {
        let refuse = |detail: String| retained("fact_envelopes", seq.to_string(), release, detail);
        let mut envelope: Value = serde_json::from_slice(&bytes)
            .map_err(|error| refuse(format!("the stored envelope does not decode: {error}")))?;
        let fact = envelope
            .get_mut("fact")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| refuse("the stored envelope carries no fact object".to_owned()))?;
        edit(fact).map_err(refuse)?;
        let current = serde_json::to_vec(&envelope)
            .map_err(|error| refuse(format!("the crossed envelope cannot encode: {error}")))
            .and_then(|crossed| {
                wire::decode::<wire::Envelope<wire::AnyFact>>(&crossed).map_err(|error| {
                    refuse(format!("the crossed envelope is not current: {error}"))
                })
            })?;
        let bytes = wire::encode(&current)
            .map_err(|error| refuse(format!("the crossed envelope cannot encode: {error}")))?;
        let changed = transaction
            .execute(
                "UPDATE fact_envelopes SET envelope = ?1 WHERE seq = ?2 AND kind = ?3",
                params![bytes, seq, kind],
            )
            .map_err(OpenError::Sqlite)?;
        if changed != 1 {
            return Err(refuse(
                "the row changed under the exclusive crossing".to_owned(),
            ));
        }
    }
    Ok(())
}

fn retained(table: &'static str, row: String, release: &'static str, detail: String) -> OpenError {
    MigrationError::RetainedRow {
        table,
        row,
        release,
        detail,
    }
    .into()
}
