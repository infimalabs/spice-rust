//! Retired receipt authority, never an executable command registry.
//!
//! A retired kind's receipts stay verifiable evidence after its command is
//! gone. `gate::findings` receipts stand in their own table; `steer::tell`
//! and `preparation::authorize` receipts stay among the command receipts they
//! were committed with. A `task::edit` receipt whose request carried a retired
//! evidence change stands in the historical table while current edits keep
//! their live receipts. The retained registry never makes these receipts
//! executable again.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::OpenError;
use crate::replay::{ReceiptRow, VerifyError, Violation, ViolationClass};

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
enum HistoricalKind {
    #[serde(rename = "gate::findings")]
    GateFindings,
    #[serde(rename = "preparation::authorize")]
    PreparationAuthorize,
    #[serde(rename = "steer::tell")]
    SteerTell,
    #[serde(rename = "task::edit")]
    TaskEdit,
}

impl HistoricalKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::GateFindings => "gate::findings",
            Self::PreparationAuthorize => "preparation::authorize",
            Self::SteerTell => "steer::tell",
            Self::TaskEdit => "task::edit",
        }
    }

    /// Whether receipts of this kind stand among the command receipts rather
    /// than in the historical table.
    const fn in_place(self) -> bool {
        matches!(self, Self::PreparationAuthorize | Self::SteerTell)
    }
}

/// Whether a command receipt row carries a retired kind that stays in place.
pub(crate) fn retired_in_place(kind: &str) -> bool {
    matches!(kind, "preparation::authorize" | "steer::tell")
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HistoricalCommand {
    v: wire::WireVersion,
    id: wire::CanonicalUuid,
    epoch: wire::CanonicalUuid,
    kind: HistoricalKind,
    expected: Vec<wire::Witness>,
    body: serde_json::Value,
}

pub(crate) fn create_schema(connection: &Connection) -> Result<(), OpenError> {
    connection
        .execute_batch(TABLE_SQL)
        .map_err(OpenError::Sqlite)
}

pub(crate) fn reserve_identities(connection: &Connection) -> Result<(), OpenError> {
    connection
        .execute_batch(IDENTITY_SQL)
        .map_err(OpenError::Sqlite)
}

pub(crate) fn verify(
    connection: &Connection,
    violations: &mut Vec<Violation>,
) -> Result<usize, VerifyError> {
    let mut statement = connection
        .prepare("SELECT command_id, actor, command_kind, request, evaluated_witnesses, response, first_seq, last_seq, snapshot_seq, epoch FROM historical_command_receipts ORDER BY command_id")
        .map_err(VerifyError::Sqlite)?;
    let rows = statement
        .query_map([], ReceiptRow::read)
        .map_err(VerifyError::Sqlite)?;
    let mut count = 0;
    for row in rows {
        let row = row.map_err(VerifyError::Sqlite)?;
        verify_receipt(&row, false, violations);
        count += 1;
    }
    let mut overlaps = connection.prepare(
        "SELECT command_id FROM historical_command_receipts INNER JOIN command_receipts USING (command_id)",
    ).map_err(VerifyError::Sqlite)?;
    for id in overlaps
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(VerifyError::Sqlite)?
    {
        violations.push(Violation::new(
            ViolationClass::Receipt,
            id.map_err(VerifyError::Sqlite)?,
            "historical command identity is also bound to a live receipt",
        ));
    }
    Ok(count)
}

/// Verify one receipt of a retired kind, standing in the command receipts
/// when `in_place` and in the historical table otherwise.
pub(crate) fn verify_receipt(row: &ReceiptRow, in_place: bool, violations: &mut Vec<Violation>) {
    let command = match wire::decode::<HistoricalCommand>(&row.request) {
        Ok(command) => command,
        Err(error) => {
            crate::replay::receipt_violation(
                row,
                violations,
                format!("historical request does not decode: {error}"),
            );
            return;
        }
    };
    let canonical = serde_json::to_value(&command).and_then(|mut value| {
        if let Some(expected) = value
            .get_mut("expected")
            .and_then(serde_json::Value::as_array_mut)
        {
            expected.sort_by_key(std::string::ToString::to_string);
        }
        serde_json::to_vec(&value)
    });
    if command.kind.as_str() != row.kind
        || command.kind.in_place() != in_place
        || command.id.to_string() != row.id
        || command.epoch.to_string() != row.epoch
        || !command.v.is_current()
        || !canonical.is_ok_and(|bytes| bytes == row.request)
        || row.actor.parse::<wire::Actor>().is_err()
    {
        crate::replay::receipt_violation(
            row,
            violations,
            "historical request identity or canonical bytes disagree with the receipt",
        );
    }
    crate::replay::verify_receipt_witnesses(row, &command.expected, violations);
    crate::replay::verify_response(row, violations);
}

const TABLE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS historical_command_receipts (
    command_id TEXT PRIMARY KEY,
    actor TEXT NOT NULL,
    command_kind TEXT NOT NULL,
    request BLOB NOT NULL,
    evaluated_witnesses BLOB NOT NULL,
    response BLOB NOT NULL,
    first_seq INTEGER REFERENCES fact_envelopes(seq),
    last_seq INTEGER REFERENCES fact_envelopes(seq),
    snapshot_seq INTEGER REFERENCES fact_envelopes(seq),
    epoch TEXT NOT NULL,
    CHECK (
        (first_seq IS NOT NULL AND last_seq IS NOT NULL AND snapshot_seq IS NULL AND first_seq <= last_seq)
        OR
        (first_seq IS NULL AND last_seq IS NULL AND snapshot_seq IS NOT NULL)
    )
) STRICT;
"#;

const IDENTITY_SQL: &str = r#"
CREATE TRIGGER IF NOT EXISTS command_receipts_historical_insert
BEFORE INSERT ON command_receipts
WHEN EXISTS (SELECT 1 FROM historical_command_receipts WHERE command_id = NEW.command_id)
BEGIN
    SELECT RAISE(ABORT, 'command identity belongs to historical receipt authority');
END;
CREATE TRIGGER IF NOT EXISTS command_receipts_historical_update
BEFORE UPDATE OF command_id ON command_receipts
WHEN EXISTS (SELECT 1 FROM historical_command_receipts WHERE command_id = NEW.command_id)
BEGIN
    SELECT RAISE(ABORT, 'command identity belongs to historical receipt authority');
END;
"#;
