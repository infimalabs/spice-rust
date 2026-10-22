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
use crate::migrate::residual;
use crate::replay::{ReceiptRow, VerifyError, Violation, ViolationClass};

/// Release that retired the room question commands into inert authority.
const RETIRED_ROOM_RELEASE: &str = "963a54c37";
/// Release that retired room coordination into inert authority.
const RETIRED_COORDINATION_RELEASE: &str = "54e357267";

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
enum HistoricalKind {
    #[serde(rename = "gate::findings")]
    GateFindings,
    #[serde(rename = "preparation::authorize")]
    PreparationAuthorize,
    #[serde(rename = "room::close")]
    RoomClose,
    #[serde(rename = "room::control")]
    RoomControl,
    #[serde(rename = "room::coordinate")]
    RoomCoordinate,
    #[serde(rename = "room::coordination-read")]
    RoomCoordinationRead,
    #[serde(rename = "room::put-aside")]
    RoomPutAside,
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
            Self::RoomClose => "room::close",
            Self::RoomControl => "room::control",
            Self::RoomCoordinate => "room::coordinate",
            Self::RoomCoordinationRead => "room::coordination-read",
            Self::RoomPutAside => "room::put-aside",
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

/// Move receipts for the retired operator question protocol into inert,
/// verifiable authority while preserving their exact body and response.
pub(crate) fn retire_room_questions(connection: &Connection) -> Result<(), OpenError> {
    retire_receipts(
        connection,
        "SELECT command_id, command_kind, request \
         FROM command_receipts \
         WHERE command_kind = 'room::close' \
         OR (command_kind = 'room::control' AND (\
             json_extract(CAST(request AS TEXT), '$.body.control') = 'answer' \
             OR json_type(CAST(response AS TEXT), '$.outcome.body.answer') IS NOT NULL \
             OR json_type(CAST(response AS TEXT), '$.outcome.body.results') IS NOT NULL\
         )) \
         ORDER BY command_id",
        RETIRED_ROOM_RELEASE,
    )
}

/// Move receipts for the retired room coordination commands into inert,
/// verifiable authority while preserving their exact body and response.
pub(crate) fn retire_room_coordination(connection: &Connection) -> Result<(), OpenError> {
    retire_receipts(
        connection,
        "SELECT command_id, command_kind, request \
         FROM command_receipts \
         WHERE command_kind IN ('room::coordinate', 'room::coordination-read', 'room::put-aside') \
         ORDER BY command_id",
        RETIRED_COORDINATION_RELEASE,
    )
}

fn retire_receipts(
    connection: &Connection,
    select: &str,
    release: &'static str,
) -> Result<(), OpenError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct RetiredCommand {
        v: wire::WireVersion,
        id: wire::CanonicalUuid,
        epoch: wire::CanonicalUuid,
        kind: serde_json::Value,
        expected: Vec<wire::Witness>,
        body: serde_json::Value,
    }

    let retained =
        |row: String, detail: String| residual::retained("command_receipts", row, release, detail);
    let rows = {
        let mut statement = connection.prepare(select).map_err(OpenError::Sqlite)?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(OpenError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };

    for (id, kind, request) in rows {
        let retired: RetiredCommand = serde_json::from_slice(&request).map_err(|error| {
            retained(
                id.clone(),
                format!("retired room command does not decode: {error}"),
            )
        })?;
        let historical_kind = match kind.as_str() {
            "room::close" => HistoricalKind::RoomClose,
            "room::control" => HistoricalKind::RoomControl,
            "room::coordinate" => HistoricalKind::RoomCoordinate,
            "room::coordination-read" => HistoricalKind::RoomCoordinationRead,
            "room::put-aside" => HistoricalKind::RoomPutAside,
            _ => unreachable!("query admits only retired room command kinds"),
        };
        if retired.kind.as_str() != Some(kind.as_str()) || retired.id.to_string() != id {
            return Err(retained(
                id,
                "retired room command identity disagrees with its receipt".to_owned(),
            ));
        }
        let request = serde_json::to_value(HistoricalCommand {
            v: retired.v,
            id: retired.id,
            epoch: retired.epoch,
            kind: historical_kind,
            expected: retired.expected,
            body: retired.body,
        })
        .and_then(|mut value| {
            if let Some(expected) = value
                .get_mut("expected")
                .and_then(serde_json::Value::as_array_mut)
            {
                expected.sort_by_key(std::string::ToString::to_string);
            }
            serde_json::to_vec(&value)
        })
        .map_err(|error| {
            retained(
                id.clone(),
                format!("retired room command cannot become inert authority: {error}"),
            )
        })?;
        let inserted = connection
            .execute(
                "INSERT INTO historical_command_receipts \
                 (command_id, actor, command_kind, request, evaluated_witnesses, response, \
                  first_seq, last_seq, snapshot_seq, epoch) \
                 SELECT command_id, actor, command_kind, ?1, evaluated_witnesses, response, \
                        first_seq, last_seq, snapshot_seq, epoch \
                 FROM command_receipts WHERE command_id = ?2",
                rusqlite::params![request, id],
            )
            .map_err(OpenError::Sqlite)?;
        let deleted = connection
            .execute("DELETE FROM command_receipts WHERE command_id = ?1", [&id])
            .map_err(OpenError::Sqlite)?;
        if inserted != 1 || deleted != 1 {
            return Err(retained(
                id,
                "retired room receipt changed during exclusive migration".to_owned(),
            ));
        }
    }
    Ok(())
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
