//! Schema 25 to 26: the lane group is a team.
//!
//! One durable spelling names the lane group across every carrier. The
//! retired `lineage` vocabulary is respelled in place: the object keys that
//! named the group and the fact and command kinds. Refusal and fault reasons
//! are never authoritative receipts, so none is stored; the occupant ancestry
//! `lineage` list is not the lane group and is untouched. Canonical wire
//! spells a key only as `"key":` and a kind only as `"kind":"…"`, so the pass
//! is one scan over the stored bytes with no decode for a carrier it leaves
//! alone; every respelled carrier re-encodes through its current typed wire
//! boundary, and the indexed kind columns follow the envelopes they describe.

use std::borrow::Cow;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::{MigrationError, OpenError};

const FROM: u32 = 25;
const TO: u32 = 26;
const RETIRED_PREFIX: &str = "lineage::";
const CURRENT_PREFIX: &str = "team::";
/// Every retired spelling as canonical wire bytes, and its current spelling.
const SPELLINGS: [(&[u8], &[u8]); 8] = [
    (br#""lineage_id":"#, br#""team":"#),
    (br#""from_lineage":"#, br#""from_team":"#),
    (br#""from_lineage_id":"#, br#""from_team":"#),
    (br#""lineages":"#, br#""teams":"#),
    (br#""lineage_ids":"#, br#""team_ids":"#),
    (br#""creator_lineage":"#, br#""creator_team":"#),
    (br#""org_slot":"#, br#""team_slot":"#),
    (br#""kind":"lineage::"#, br#""kind":"team::"#),
];

pub(super) fn migrate(connection: &Connection) -> Result<(), OpenError> {
    migrate_facts(connection)?;
    migrate_residency_head(connection)?;
    migrate_task_heads(connection)?;
    migrate_receipts(connection)
}

/// The current spelling of a stored kind.
pub(super) fn name(name: &str) -> String {
    name.strip_prefix(RETIRED_PREFIX)
        .map_or_else(|| name.to_owned(), |rest| format!("{CURRENT_PREFIX}{rest}"))
}

/// `bytes` in the current vocabulary, borrowed when nothing was retired.
///
/// A spelling is matched only at an unescaped quote, which in canonical wire
/// opens a key or a string value and never lies inside one.
pub(super) fn respell(bytes: &[u8]) -> Cow<'_, [u8]> {
    let mut output: Option<Vec<u8>> = None;
    let mut pending = 0;
    let mut at = 0;
    while at < bytes.len() {
        let opens = bytes[at] == b'"' && (at == 0 || bytes[at - 1] != b'\\');
        let spelling = opens
            .then(|| {
                SPELLINGS
                    .iter()
                    .find(|(retired, _)| bytes[at..].starts_with(retired))
            })
            .flatten();
        let Some((retired, current)) = spelling else {
            at += 1;
            continue;
        };
        let output = output.get_or_insert_with(|| Vec::with_capacity(bytes.len()));
        output.extend_from_slice(&bytes[pending..at]);
        output.extend_from_slice(current);
        at += retired.len();
        pending = at;
    }
    match output {
        Some(mut output) => {
            output.extend_from_slice(&bytes[pending..]);
            Cow::Owned(output)
        }
        None => Cow::Borrowed(bytes),
    }
}

/// `value` in the current vocabulary, encoded for a typed decode.
pub(super) fn current(value: &Value) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(value).map(|bytes| respell(&bytes).into_owned())
}

fn migrate_facts(connection: &Connection) -> Result<(), OpenError> {
    for (seq, kind, bytes) in fact_rows(connection)? {
        let row = seq.to_string();
        let current_kind = name(&kind);
        let respelled = respell(&bytes);
        if current_kind == kind && matches!(respelled, Cow::Borrowed(_)) {
            continue;
        }
        let canonical =
            canonical::<wire::Envelope<wire::AnyFact>>("fact_envelopes", &row, &respelled)?;
        connection
            .execute(
                "UPDATE fact_envelopes SET kind = ?1, envelope = ?2 WHERE seq = ?3",
                params![current_kind, canonical, seq],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn migrate_residency_head(connection: &Connection) -> Result<(), OpenError> {
    let Some(bytes) = connection
        .query_row(
            "SELECT body FROM residency_head WHERE singleton = 1",
            [],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(OpenError::Sqlite)?
    else {
        return Ok(());
    };
    let Cow::Owned(respelled) = respell(&bytes) else {
        return Ok(());
    };
    let canonical = canonical::<wire::ResidencyHead>("residency_head", "1", &respelled)?;
    connection
        .execute(
            "UPDATE residency_head SET body = ?1 WHERE singleton = 1",
            params![canonical],
        )
        .map_err(OpenError::Sqlite)?;
    Ok(())
}

fn migrate_task_heads(connection: &Connection) -> Result<(), OpenError> {
    for (stamp, bytes) in text_rows(connection, "SELECT stamp, body FROM task_heads")? {
        let Cow::Owned(respelled) = respell(&bytes) else {
            continue;
        };
        let canonical = canonical::<wire::TaskHead>("task_heads", &stamp, &respelled)?;
        connection
            .execute(
                "UPDATE task_heads SET body = ?1 WHERE stamp = ?2",
                params![canonical, stamp],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn migrate_receipts(connection: &Connection) -> Result<(), OpenError> {
    for (command, kind, request, witnesses, response) in receipt_rows(connection)? {
        let request = owned(respell(&request))
            .map(|respelled| canonical_request(&command, &respelled))
            .transpose()?;
        let witnesses = owned(respell(&witnesses))
            .map(|respelled| {
                canonical::<Vec<wire::Witness>>("command_receipts", &command, &respelled)
            })
            .transpose()?;
        let response = owned(respell(&response))
            .map(|respelled| {
                canonical::<wire::Response<Value>>("command_receipts", &command, &respelled)
            })
            .transpose()?;
        let current_kind = name(&kind);
        if request.is_none() && witnesses.is_none() && response.is_none() && current_kind == kind {
            continue;
        }
        connection
            .execute(
                "UPDATE command_receipts \
                 SET command_kind = ?1, request = coalesce(?2, request), \
                     evaluated_witnesses = coalesce(?3, evaluated_witnesses), \
                     response = coalesce(?4, response) \
                 WHERE command_id = ?5",
                params![current_kind, request, witnesses, response, command],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn owned(respelled: Cow<'_, [u8]>) -> Option<Vec<u8>> {
    match respelled {
        Cow::Owned(respelled) => Some(respelled),
        Cow::Borrowed(_) => None,
    }
}

fn canonical_request(row: &str, bytes: &[u8]) -> Result<Vec<u8>, OpenError> {
    let request: wire::Command<Value> = wire::decode(bytes).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("renamed request is not current command wire: {error}"),
        )
    })?;
    request
        .canonical_json()
        .map(String::into_bytes)
        .map_err(|error| {
            invalid(
                "command_receipts",
                row,
                &format!("renamed request cannot encode canonically: {error}"),
            )
        })
}

fn canonical<T>(table: &'static str, row: &str, bytes: &[u8]) -> Result<Vec<u8>, OpenError>
where
    T: DeserializeOwned + Serialize,
{
    let typed: T = wire::decode(bytes).map_err(|error| {
        invalid(
            table,
            row,
            &format!("renamed carrier is not current wire: {error}"),
        )
    })?;
    wire::encode(&typed).map_err(|error| {
        invalid(
            table,
            row,
            &format!("renamed carrier cannot encode canonically: {error}"),
        )
    })
}

type FactRow = (i64, String, Vec<u8>);

fn fact_rows(connection: &Connection) -> Result<Vec<FactRow>, OpenError> {
    let mut statement = connection
        .prepare("SELECT seq, kind, envelope FROM fact_envelopes ORDER BY seq")
        .map_err(OpenError::Sqlite)?;
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(OpenError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)
}

fn text_rows(connection: &Connection, sql: &str) -> Result<Vec<(String, Vec<u8>)>, OpenError> {
    let mut statement = connection.prepare(sql).map_err(OpenError::Sqlite)?;
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(OpenError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)
}

type Receipt = (String, String, Vec<u8>, Vec<u8>, Vec<u8>);

fn receipt_rows(connection: &Connection) -> Result<Vec<Receipt>, OpenError> {
    let mut statement = connection
        .prepare(
            "SELECT command_id, command_kind, request, evaluated_witnesses, response \
             FROM command_receipts ORDER BY command_id",
        )
        .map_err(OpenError::Sqlite)?;
    statement
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .map_err(OpenError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)
}

fn invalid(table: &'static str, row: &str, detail: &str) -> OpenError {
    MigrationError::StoredContent {
        from: FROM,
        to: TO,
        table,
        row: row.to_owned(),
        detail: detail.to_owned(),
    }
    .into()
}
