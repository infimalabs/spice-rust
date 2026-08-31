//! Schema 5 to 6 contraction of task identity onto the birth stamp.
//!
//! The old schema indexed heads by a rendered handle and repeated a UUID in
//! statement facts and heads. The current schema stores only the fixed-width
//! stamp at authoritative crossings. Historical command input and output may
//! still contain actor-facing handles, but predicates, facts, current heads,
//! and task-origin realms are rewritten once before the current runtime reads
//! them.

use std::collections::BTreeSet;

use rusqlite::{Connection, params};
use serde_json::{Map, Value};

use super::{MigrationError, OpenError};

const FROM: u32 = 5;
const TO: u32 = 6;

const TASK_FACTS: &[&str] = &[
    "task::added",
    "task::phased",
    "task::done",
    "task::reviewed",
    "task::assigned",
    "task::noted",
    "task::deferred",
    "task::woken",
    "task::edited",
    "task::deleted",
    "claim::won",
    "claim::carried",
    "claim::freed",
    "claim::lapsed",
    "claim::renewed",
];

pub(super) fn migrate(connection: &Connection) -> Result<(), OpenError> {
    migrate_facts(connection)?;
    migrate_task_heads(connection)?;
    migrate_receipts(connection)
}

fn migrate_facts(connection: &Connection) -> Result<(), OpenError> {
    let rows = {
        let mut statement = connection
            .prepare("SELECT seq, kind, envelope FROM fact_envelopes ORDER BY seq")
            .map_err(OpenError::Sqlite)?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(OpenError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };
    for (seq, kind, bytes) in rows {
        let row = seq.to_string();
        let mut document = decode("fact_envelopes", &row, &bytes)?;
        let envelope = object("fact_envelopes", &row, &mut document)?;
        let origin = required_string("fact_envelopes", &row, envelope, "origin")?;
        let normalized_origin = normalize_origin("fact_envelopes", &row, origin)?;
        envelope.insert(
            "origin".to_owned(),
            Value::String(normalized_origin.clone()),
        );
        if TASK_FACTS.contains(&kind.as_str()) {
            let fact = envelope
                .get_mut("fact")
                .ok_or_else(|| invalid("fact_envelopes", &row, "envelope.fact is missing"))?;
            let fact = object("fact_envelopes", &row, fact)?;
            replace_handle("fact_envelopes", &row, fact, "task", "task")?;
            if kind == "task::added" {
                remove_uuid("fact_envelopes", &row, fact)?;
                replace_handle_array("fact_envelopes", &row, fact, "depends")?;
            }
        }
        let encoded = encode_fact(&row, &document)?;
        connection
            .execute(
                "UPDATE fact_envelopes SET origin = ?1, envelope = ?2 WHERE seq = ?3",
                params![normalized_origin, encoded, seq],
            )
            .map_err(OpenError::Sqlite)?;
        connection
            .execute(
                "UPDATE thread_index SET origin = ?1 WHERE seq = ?2",
                params![normalized_origin, seq],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn migrate_task_heads(connection: &Connection) -> Result<(), OpenError> {
    let task_rows = text_head_rows(connection, "task_heads")?;
    let workflow_rows = text_head_rows(connection, "workflow_heads")?;
    let claim_rows = text_head_rows(connection, "claim_heads")?;
    let lease_rows = text_head_rows(connection, "lease_heads")?;

    connection
        .execute_batch(
            "CREATE TABLE task_heads_identity_v6 (
                 stamp TEXT PRIMARY KEY,
                 head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
                 body BLOB NOT NULL
             ) STRICT;
             CREATE TABLE workflow_heads_identity_v6 (
                 stamp TEXT PRIMARY KEY REFERENCES task_heads_identity_v6(stamp),
                 head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
                 body BLOB NOT NULL
             ) STRICT;
             CREATE TABLE claim_heads_identity_v6 (
                 stamp TEXT PRIMARY KEY REFERENCES task_heads_identity_v6(stamp),
                 head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
                 body BLOB NOT NULL
             ) STRICT;
             CREATE TABLE lease_heads_identity_v6 (
                 stamp TEXT PRIMARY KEY REFERENCES task_heads_identity_v6(stamp),
                 head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
                 body BLOB NOT NULL
             ) STRICT;",
        )
        .map_err(OpenError::Sqlite)?;

    let mut stamps = BTreeSet::new();
    for (handle, head_seq, bytes) in task_rows {
        let row = handle.clone();
        let stamp = stamp_from_handle("task_heads", &row, &handle)?;
        if !stamps.insert(stamp.clone()) {
            return Err(invalid(
                "task_heads",
                &row,
                &format!("more than one rendered handle contracts to stamp {stamp}"),
            ));
        }
        let mut document = decode("task_heads", &row, &bytes)?;
        let head = object("task_heads", &row, &mut document)?;
        let body_handle = required_string("task_heads", &row, head, "handle")?;
        if body_handle != handle {
            return Err(invalid(
                "task_heads",
                &row,
                "indexed handle and statement body handle disagree",
            ));
        }
        head.remove("handle");
        head.insert("born".to_owned(), Value::String(stamp.clone()));
        remove_uuid("task_heads", &row, head)?;
        replace_handle_array("task_heads", &row, head, "dependencies")?;
        if let Some(origin) = head.get("provenance").and_then(Value::as_str) {
            let origin = normalize_origin("task_heads", &row, origin)?;
            head.insert("provenance".to_owned(), Value::String(origin));
        }
        let encoded = encode_task_head(&row, &document)?;
        connection
            .execute(
                "INSERT INTO task_heads_identity_v6(stamp, head_seq, body) VALUES (?1, ?2, ?3)",
                params![stamp, head_seq, encoded],
            )
            .map_err(OpenError::Sqlite)?;
    }

    copy_child_heads(
        connection,
        "workflow_heads",
        "workflow_heads_identity_v6",
        workflow_rows,
        &stamps,
    )?;
    copy_child_heads(
        connection,
        "claim_heads",
        "claim_heads_identity_v6",
        claim_rows,
        &stamps,
    )?;
    copy_child_heads(
        connection,
        "lease_heads",
        "lease_heads_identity_v6",
        lease_rows,
        &stamps,
    )?;

    connection
        .execute_batch(
            "DROP TABLE lease_heads;
             DROP TABLE claim_heads;
             DROP TABLE workflow_heads;
             DROP TABLE task_heads;
             ALTER TABLE task_heads_identity_v6 RENAME TO task_heads;
             ALTER TABLE workflow_heads_identity_v6 RENAME TO workflow_heads;
             ALTER TABLE claim_heads_identity_v6 RENAME TO claim_heads;
             ALTER TABLE lease_heads_identity_v6 RENAME TO lease_heads;",
        )
        .map_err(OpenError::Sqlite)
}

fn text_head_rows(
    connection: &Connection,
    table: &'static str,
) -> Result<Vec<(String, i64, Vec<u8>)>, OpenError> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT handle, head_seq, body FROM {table} ORDER BY handle"
        ))
        .map_err(OpenError::Sqlite)?;
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(OpenError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)
}

fn copy_child_heads(
    connection: &Connection,
    source: &'static str,
    target: &'static str,
    rows: Vec<(String, i64, Vec<u8>)>,
    task_stamps: &BTreeSet<String>,
) -> Result<(), OpenError> {
    for (handle, head_seq, bytes) in rows {
        let stamp = stamp_from_handle(source, &handle, &handle)?;
        if !task_stamps.contains(&stamp) {
            return Err(invalid(
                source,
                &handle,
                &format!("contracts to stamp {stamp} without a statement head"),
            ));
        }
        validate_child_head(source, &handle, &bytes)?;
        connection
            .execute(
                &format!("INSERT INTO {target}(stamp, head_seq, body) VALUES (?1, ?2, ?3)"),
                params![stamp, head_seq, bytes],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn validate_child_head(table: &'static str, row: &str, bytes: &[u8]) -> Result<(), OpenError> {
    let valid = match table {
        "workflow_heads" => wire::decode::<wire::WorkflowHead>(bytes).map(drop),
        "claim_heads" => wire::decode::<wire::ClaimHead>(bytes).map(drop),
        "lease_heads" => wire::decode::<wire::LeaseHead>(bytes).map(drop),
        _ => unreachable!("task identity migration names a closed head set"),
    };
    valid.map_err(|error| invalid(table, row, &format!("head body is invalid: {error}")))
}

fn migrate_receipts(connection: &Connection) -> Result<(), OpenError> {
    let rows = {
        let mut statement = connection
            .prepare(
                "SELECT command_id, command_kind, request, evaluated_witnesses, response \
                 FROM command_receipts ORDER BY command_id",
            )
            .map_err(OpenError::Sqlite)?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                ))
            })
            .map_err(OpenError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };
    for (command, kind, request, witnesses, response) in rows {
        let request = migrate_request(&command, &kind, &request)?;
        let witnesses = migrate_witnesses(&command, &witnesses)?;
        let response = migrate_response(&command, &kind, &response)?;
        connection
            .execute(
                "UPDATE command_receipts SET request = ?1, evaluated_witnesses = ?2, \
                 response = ?3 WHERE command_id = ?4",
                params![request, witnesses, response, command],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn migrate_request(row: &str, kind: &str, bytes: &[u8]) -> Result<Vec<u8>, OpenError> {
    let mut document = decode("command_receipts", row, bytes)?;
    let command = object("command_receipts", row, &mut document)?;
    let expected = command
        .get_mut("expected")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| invalid("command_receipts", row, "request.expected is not an array"))?;
    migrate_witness_values("command_receipts", row, expected)?;
    let body = command
        .get_mut("body")
        .ok_or_else(|| invalid("command_receipts", row, "request.body is missing"))?;
    if kind == "task::review" {
        let body = object("command_receipts", row, body)?;
        match body.get_mut("then") {
            None => {}
            Some(Value::Array(then)) => {
                for (index, followup) in then.iter_mut().enumerate() {
                    replace_handle(
                        "command_receipts",
                        &format!("{row} then[{index}]"),
                        object("command_receipts", row, followup)?,
                        "task",
                        "task",
                    )?;
                }
            }
            Some(_) => {
                return Err(invalid(
                    "command_receipts",
                    row,
                    "review body.then is not an array",
                ));
            }
        }
    } else if kind == "task::add" {
        let body = object("command_receipts", row, body)?;
        if let Some(origin) = body.get("origin").and_then(Value::as_str) {
            let origin = normalize_origin("command_receipts", row, origin)?;
            body.insert("origin".to_owned(), Value::String(origin));
        }
    }
    let bytes = intermediate("command_receipts", row, &document)?;
    let command: wire::Command<Value> = wire::decode(&bytes).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("migrated request is not current command wire: {error}"),
        )
    })?;
    command
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

fn migrate_witnesses(row: &str, bytes: &[u8]) -> Result<Vec<u8>, OpenError> {
    let mut document = decode("command_receipts", row, bytes)?;
    let witnesses = document.as_array_mut().ok_or_else(|| {
        invalid(
            "command_receipts",
            row,
            "evaluated_witnesses is not an array",
        )
    })?;
    migrate_witness_values("command_receipts", row, witnesses)?;
    witnesses.sort_by_key(std::string::ToString::to_string);
    let bytes = intermediate("command_receipts", row, &document)?;
    let witnesses: Vec<wire::Witness> = wire::decode(&bytes).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("migrated witnesses are not current wire: {error}"),
        )
    })?;
    wire::encode(&witnesses).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("migrated witnesses cannot encode canonically: {error}"),
        )
    })
}

fn migrate_witness_values(
    table: &'static str,
    row: &str,
    witnesses: &mut [Value],
) -> Result<(), OpenError> {
    for (index, witness) in witnesses.iter_mut().enumerate() {
        let subject = format!("{row} witness[{index}]");
        let witness = object(table, &subject, witness)?;
        if witness.contains_key("task") {
            replace_handle(table, &subject, witness, "task", "task")?;
        }
        if witness.contains_key("after") {
            replace_handle(table, &subject, witness, "after", "after")?;
        }
    }
    Ok(())
}

fn migrate_response(row: &str, kind: &str, bytes: &[u8]) -> Result<Vec<u8>, OpenError> {
    let mut document = decode("command_receipts", row, bytes)?;
    if kind == "board::read" {
        let body = reply_body(row, &mut document)?;
        let tasks = object("command_receipts", row, body)?
            .get_mut("tasks")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                invalid(
                    "command_receipts",
                    row,
                    "board response.tasks is not an array",
                )
            })?;
        for (index, task) in tasks.iter_mut().enumerate() {
            let subject = format!("{row} board task[{index}]");
            let statement = object("command_receipts", &subject, task)?
                .get_mut("statement")
                .and_then(|statement| statement.get_mut("body"))
                .ok_or_else(|| {
                    invalid(
                        "command_receipts",
                        &subject,
                        "board task statement.body is missing",
                    )
                })?;
            migrate_board_statement(&subject, statement)?;
        }
    } else if kind == "effort::read" {
        let body = reply_body(row, &mut document)?;
        let phases = object("command_receipts", row, body)?
            .get_mut("phases")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                invalid(
                    "command_receipts",
                    row,
                    "effort response.phases is not an array",
                )
            })?;
        for (index, phase) in phases.iter_mut().enumerate() {
            replace_handle(
                "command_receipts",
                &format!("{row} effort phase[{index}]"),
                object("command_receipts", row, phase)?,
                "task",
                "task",
            )?;
        }
    }
    let bytes = intermediate("command_receipts", row, &document)?;
    match kind {
        "board::read" => wire::decode::<wire::Response<wire::intent::BoardAnswer>>(&bytes)
            .map(drop)
            .map_err(|error| {
                invalid(
                    "command_receipts",
                    row,
                    &format!("migrated board response is not current wire: {error}"),
                )
            })?,
        "effort::read" => wire::decode::<wire::Response<wire::intent::EffortAnswer>>(&bytes)
            .map(drop)
            .map_err(|error| {
                invalid(
                    "command_receipts",
                    row,
                    &format!("migrated effort response is not current wire: {error}"),
                )
            })?,
        _ => wire::decode::<wire::Response<Value>>(&bytes)
            .map(drop)
            .map_err(|error| {
                invalid(
                    "command_receipts",
                    row,
                    &format!("stored response is not current generic wire: {error}"),
                )
            })?,
    }
    let response: wire::Response<Value> = wire::decode(&bytes).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("migrated response is not current generic wire: {error}"),
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

fn reply_body<'a>(row: &str, document: &'a mut Value) -> Result<&'a mut Value, OpenError> {
    let outcome = object("command_receipts", row, document)?
        .get_mut("outcome")
        .ok_or_else(|| invalid("command_receipts", row, "response.outcome is missing"))?;
    let outcome = object("command_receipts", row, outcome)?;
    if outcome.get("kind").and_then(Value::as_str) != Some("reply") {
        return Err(invalid(
            "command_receipts",
            row,
            "authoritative receipt response is not a reply",
        ));
    }
    outcome
        .get_mut("body")
        .ok_or_else(|| invalid("command_receipts", row, "reply body is missing"))
}

fn migrate_board_statement(row: &str, value: &mut Value) -> Result<(), OpenError> {
    let statement = object("command_receipts", row, value)?;
    let handle = required_string("command_receipts", row, statement, "handle")?.to_owned();
    let stamp = stamp_from_handle("command_receipts", row, &handle)?;
    let project = required_string("command_receipts", row, statement, "project")?;
    let rendered = wire::TaskHandle::for_project(
        project,
        &stamp
            .parse::<wire::TaskStamp>()
            .expect("validated stamp parses"),
    );
    statement.insert("handle".to_owned(), Value::String(rendered.to_string()));
    statement.insert("born".to_owned(), Value::String(stamp));
    remove_uuid("command_receipts", row, statement)?;
    replace_handle_array("command_receipts", row, statement, "dependencies")?;
    if let Some(origin) = statement.get("provenance").and_then(Value::as_str) {
        let origin = normalize_origin("command_receipts", row, origin)?;
        statement.insert("provenance".to_owned(), Value::String(origin));
    }
    Ok(())
}

fn replace_handle(
    table: &'static str,
    row: &str,
    object: &mut Map<String, Value>,
    field: &str,
    label: &str,
) -> Result<(), OpenError> {
    let handle = required_string(table, row, object, field)?;
    let stamp = stamp_from_handle(table, row, handle).map_err(|_| {
        invalid(
            table,
            row,
            &format!("{label} does not carry a canonical birth stamp"),
        )
    })?;
    object.insert(field.to_owned(), Value::String(stamp));
    Ok(())
}

fn replace_handle_array(
    table: &'static str,
    row: &str,
    object: &mut Map<String, Value>,
    field: &str,
) -> Result<(), OpenError> {
    let values = object
        .get_mut(field)
        .and_then(Value::as_array_mut)
        .ok_or_else(|| invalid(table, row, &format!("{field} is not an array")))?;
    for (index, value) in values.iter_mut().enumerate() {
        let handle = value
            .as_str()
            .ok_or_else(|| invalid(table, row, &format!("{field}[{index}] is not a string")))?;
        *value = Value::String(stamp_from_handle(table, row, handle)?);
    }
    Ok(())
}

fn remove_uuid(
    table: &'static str,
    row: &str,
    object: &mut Map<String, Value>,
) -> Result<(), OpenError> {
    let uuid = object
        .remove("uuid")
        .ok_or_else(|| invalid(table, row, "legacy uuid is missing"))?;
    let uuid = uuid
        .as_str()
        .ok_or_else(|| invalid(table, row, "legacy uuid is not a string"))?;
    uuid.parse::<wire::CanonicalUuid>()
        .map_err(|error| invalid(table, row, &format!("legacy uuid is invalid: {error}")))?;
    Ok(())
}

fn stamp_from_handle(table: &'static str, row: &str, handle: &str) -> Result<String, OpenError> {
    let handle = handle.parse::<wire::TaskHandle>().map_err(|error| {
        invalid(
            table,
            row,
            &format!("rendered task handle is invalid: {error}"),
        )
    })?;
    handle
        .born()
        .map(|stamp| stamp.to_string())
        .map_err(|error| {
            invalid(
                table,
                row,
                &format!("rendered task handle has no birth stamp: {error}"),
            )
        })
}

fn normalize_origin(table: &'static str, row: &str, origin: &str) -> Result<String, OpenError> {
    let Some(identity) = origin.strip_prefix("task:") else {
        return Ok(origin.to_owned());
    };
    if let Ok(stamp) = identity.parse::<wire::TaskStamp>() {
        return Ok(format!("task:{stamp}"));
    }
    stamp_from_handle(table, row, identity).map(|stamp| format!("task:{stamp}"))
}

fn required_string<'a>(
    table: &'static str,
    row: &str,
    object: &'a Map<String, Value>,
    field: &str,
) -> Result<&'a str, OpenError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(table, row, &format!("{field} is not a string")))
}

fn object<'a>(
    table: &'static str,
    row: &str,
    value: &'a mut Value,
) -> Result<&'a mut Map<String, Value>, OpenError> {
    value
        .as_object_mut()
        .ok_or_else(|| invalid(table, row, "stored value is not an object"))
}

fn decode(table: &'static str, row: &str, bytes: &[u8]) -> Result<Value, OpenError> {
    serde_json::from_slice(bytes)
        .map_err(|error| invalid(table, row, &format!("stored JSON is invalid: {error}")))
}

fn encode_fact(row: &str, value: &Value) -> Result<Vec<u8>, OpenError> {
    let bytes = intermediate("fact_envelopes", row, value)?;
    let fact: wire::Envelope<wire::AnyFact> = wire::decode(&bytes).map_err(|error| {
        invalid(
            "fact_envelopes",
            row,
            &format!("migrated envelope is not current fact wire: {error}"),
        )
    })?;
    wire::encode(&fact).map_err(|error| {
        invalid(
            "fact_envelopes",
            row,
            &format!("migrated envelope cannot encode canonically: {error}"),
        )
    })
}

fn encode_task_head(row: &str, value: &Value) -> Result<Vec<u8>, OpenError> {
    let bytes = intermediate("task_heads", row, value)?;
    let head: wire::TaskHead = wire::decode(&bytes).map_err(|error| {
        invalid(
            "task_heads",
            row,
            &format!("migrated body is not current task-head wire: {error}"),
        )
    })?;
    wire::encode(&head).map_err(|error| {
        invalid(
            "task_heads",
            row,
            &format!("migrated body cannot encode canonically: {error}"),
        )
    })
}

fn intermediate(table: &'static str, row: &str, value: &Value) -> Result<Vec<u8>, OpenError> {
    serde_json::to_vec(value)
        .map_err(|error| invalid(table, row, &format!("migrated JSON cannot encode: {error}")))
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
