//! Schema 18 to 19 retirement of project-backed lifetime scope.

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::{MigrationError, OpenError};

const FROM: u32 = 18;
const TO: u32 = 19;
const RETIRED_FIELDS: [&str; 3] = ["manual_projects", "auto_projects", "from_manual_projects"];

pub(super) fn migrate(connection: &Connection) -> Result<(), OpenError> {
    migrate_residency_head(connection)?;
    migrate_lineage_facts(connection)?;
    migrate_receipts(connection)
}

fn migrate_residency_head(connection: &Connection) -> Result<(), OpenError> {
    let Some(original) = connection
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
    let mut value = decode("residency_head", "1", &original)?;
    retire_fields(&mut value);
    let canonical = canonical::<wire::ResidencyHead>("residency_head", "1", &value)?;
    connection
        .execute(
            "UPDATE residency_head SET body = ?1 WHERE singleton = 1",
            params![canonical],
        )
        .map_err(OpenError::Sqlite)?;
    Ok(())
}

fn migrate_lineage_facts(connection: &Connection) -> Result<(), OpenError> {
    let mut query = connection
        .prepare(
            "SELECT seq, envelope FROM fact_envelopes \
             WHERE kind = 'lineage::configured' ORDER BY seq",
        )
        .map_err(OpenError::Sqlite)?;
    let rows = query
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(OpenError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)?;
    for (seq, original) in rows {
        let row = seq.to_string();
        let mut value = decode("fact_envelopes", &row, &original)?;
        retire_fields(&mut value);
        let bytes = intermediate("fact_envelopes", &row, &value)?;
        let envelope: wire::Envelope<wire::AnyFact> = wire::decode(&bytes).map_err(|error| {
            invalid(
                "fact_envelopes",
                &row,
                &format!("migrated envelope is not current fact wire: {error}"),
            )
        })?;
        if u64::try_from(seq).ok() != Some(envelope.seq.get())
            || envelope.fact.kind() != wire::FactKind::TeamConfigured
        {
            return Err(invalid(
                "fact_envelopes",
                &row,
                "indexed lineage fact identity disagrees with its envelope",
            ));
        }
        let canonical = wire::encode(&envelope).map_err(|error| {
            invalid(
                "fact_envelopes",
                &row,
                &format!("migrated envelope cannot encode canonically: {error}"),
            )
        })?;
        connection
            .execute(
                "UPDATE fact_envelopes SET envelope = ?1 WHERE seq = ?2",
                params![canonical, seq],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn migrate_receipts(connection: &Connection) -> Result<(), OpenError> {
    let mut query = connection
        .prepare(
            "SELECT command_id, command_kind, request, response \
             FROM command_receipts ORDER BY command_id",
        )
        .map_err(OpenError::Sqlite)?;
    let rows = query
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })
        .map_err(OpenError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)?;
    for (command, kind, request, response) in rows {
        let mut request = decode("command_receipts", &command, &request)?;
        let request = if retire_fields(&mut request) == 0 {
            None
        } else if kind == "lineage::configure" {
            Some(canonical_request::<wire::intent::TeamConfigure>(
                &command, &request,
            )?)
        } else {
            return Err(invalid(
                "command_receipts",
                &command,
                &format!("retired lifetime fields occur in unregistered request kind {kind}"),
            ));
        };

        let mut response = decode("command_receipts", &command, &response)?;
        let response = if retire_fields(&mut response) == 0 {
            None
        } else {
            Some(match kind.as_str() {
                "board::read" => {
                    canonical_receipt::<wire::intent::BoardAnswer>(&command, &response)
                }
                "lane::read" | "lane::arrange" | "lineage::configure" | "lineage::arrange" => {
                    canonical_receipt::<wire::intent::LaneAnswer>(&command, &response)
                }
                _ => Err(invalid(
                    "command_receipts",
                    &command,
                    &format!("retired lifetime fields occur in unregistered response kind {kind}"),
                )),
            }?)
        };
        if request.is_none() && response.is_none() {
            continue;
        }
        connection
            .execute(
                "UPDATE command_receipts \
                 SET request = coalesce(?1, request), response = coalesce(?2, response) \
                 WHERE command_id = ?3",
                params![request, response, command],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn canonical_request<B>(row: &str, value: &Value) -> Result<Vec<u8>, OpenError>
where
    B: DeserializeOwned + Serialize,
{
    let bytes = intermediate("command_receipts", row, value)?;
    let request: wire::Command<B> = wire::decode(&bytes).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("migrated request is not its command-kind wire: {error}"),
        )
    })?;
    request
        .canonical_json()
        .map(String::into_bytes)
        .map_err(|error| {
            invalid(
                "command_receipts",
                row,
                &format!("migrated request cannot encode canonically: {error}"),
            )
        })
}

fn retire_fields(value: &mut Value) -> usize {
    match value {
        Value::Object(object) => {
            let removed = RETIRED_FIELDS
                .into_iter()
                .filter(|field| object.remove(*field).is_some())
                .count();
            removed + object.values_mut().map(retire_fields).sum::<usize>()
        }
        Value::Array(array) => array.iter_mut().map(retire_fields).sum(),
        _ => 0,
    }
}

fn canonical_receipt<B>(row: &str, value: &Value) -> Result<Vec<u8>, OpenError>
where
    B: DeserializeOwned + Serialize,
{
    let bytes = intermediate("command_receipts", row, value)?;
    let response: wire::Response<B> = wire::decode(&bytes).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("migrated response is not its command-kind wire: {error}"),
        )
    })?;
    wire::encode(&response).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("migrated response cannot encode canonically: {error}"),
        )
    })
}

fn canonical<T>(table: &'static str, row: &str, value: &Value) -> Result<Vec<u8>, OpenError>
where
    T: DeserializeOwned + Serialize,
{
    let bytes = intermediate(table, row, value)?;
    let typed: T = wire::decode(&bytes).map_err(|error| {
        invalid(
            table,
            row,
            &format!("migrated carrier is not current typed wire: {error}"),
        )
    })?;
    wire::encode(&typed).map_err(|error| {
        invalid(
            table,
            row,
            &format!("migrated carrier cannot encode canonically: {error}"),
        )
    })
}

fn decode(table: &'static str, row: &str, bytes: &[u8]) -> Result<Value, OpenError> {
    serde_json::from_slice(bytes)
        .map_err(|error| invalid(table, row, &format!("stored carrier is not JSON: {error}")))
}

fn intermediate(table: &'static str, row: &str, value: &Value) -> Result<Vec<u8>, OpenError> {
    super::team::current(value).map_err(|error| {
        invalid(
            table,
            row,
            &format!("normalized carrier cannot encode: {error}"),
        )
    })
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn retired_lineage_configuration_request_normalizes_to_current_wire() {
        let uuid = |suffix: &str| {
            format!("018f82d4-6f59-7d06-92e3-{suffix}")
                .parse()
                .expect("fixture UUID is canonical")
        };
        let request = wire::Command {
            v: wire::WireVersion::CURRENT,
            id: uuid("bce913116301"),
            epoch: uuid("bce913116302"),
            kind: wire::CommandKind::TeamConfigure,
            expected: Vec::new(),
            body: wire::intent::TeamConfigure {
                topology_revision: 3,
                team: uuid("bce913116303"),
                member: None,
                life: Some(wire::intent::TeamLife::Drive),
            },
        };
        let mut legacy = serde_json::to_value(request).expect("request becomes JSON");
        legacy["body"]
            .as_object_mut()
            .expect("request body is an object")
            .insert("manual_projects".to_owned(), json!(["task.allocation"]));

        assert_eq!(retire_fields(&mut legacy), 1);
        let canonical = canonical_request::<wire::intent::TeamConfigure>("fixture", &legacy)
            .expect("retired request normalizes");
        let decoded: wire::Command<wire::intent::TeamConfigure> =
            wire::decode(&canonical).expect("normalized request is current typed wire");
        assert_eq!(decoded.body.life, Some(wire::intent::TeamLife::Drive));
        assert_eq!(decoded.body.topology_revision, 3);
    }
}
