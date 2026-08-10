//! Schema 3 to 4 collapse of steering resend rungs onto one stable identity.
//!
//! The whole old steering graph is validated before any row is rewritten. A
//! malformed or ambiguous graph therefore refuses the enclosing exclusive
//! migration transaction without leaving a mixture of old and current facts.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{Connection, params};
use serde_json::{Map, Value};

use super::{MigrationError, OpenError};

const FROM: u32 = 3;
const TO: u32 = 4;
const SENT: &str = "steer::sent";
const REMINDED: &str = "taste::reminded";
const RESENT: &str = "steer::resent";
const ESCALATED: &str = "steer::escalated";

#[derive(Clone)]
struct Node {
    root_directive: String,
    root_key: String,
    lane: String,
    stage: u32,
    at: wire::Instant,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Standing {
    Live,
    Acked,
    Declined,
    Lapsed,
    Canceled,
}

struct Root {
    standing: Standing,
    expires: String,
}

#[derive(Default)]
struct Graph {
    by_key: NodeIndex,
    by_directive: NodeIndex,
    roots: BTreeMap<String, Root>,
    descended: BTreeSet<String>,
}

struct FactRow {
    seq: i64,
    indexed_kind: String,
    document: Value,
    changed: bool,
}

type NodeIndex = BTreeMap<String, Node>;
type NodeIndexes = (NodeIndex, NodeIndex);

pub(super) fn migrate(connection: &Connection) -> Result<(), OpenError> {
    let mut rows = read_facts(connection)?;
    let (by_key, by_directive) = validate_and_rewrite_facts(&mut rows)?;
    let receipt_updates = rewrite_receipts(connection, &by_key, &by_directive)?;

    for row in rows.iter().filter(|row| row.changed) {
        let bytes = encode_fact(&row.seq.to_string(), &row.document)?;
        let kind = fact_object("fact_envelopes", &row.seq.to_string(), &row.document)?
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                invalid(
                    "fact_envelopes",
                    &row.seq.to_string(),
                    "fact.kind is missing",
                )
            })?;
        connection
            .execute(
                "UPDATE fact_envelopes SET kind = ?1, envelope = ?2 WHERE seq = ?3",
                params![kind, bytes, row.seq],
            )
            .map_err(OpenError::Sqlite)?;
    }
    for update in receipt_updates {
        connection
            .execute(
                "UPDATE command_receipts SET command_kind = ?1, request = ?2, \
                 evaluated_witnesses = ?3, response = ?4 WHERE command_id = ?5",
                params![
                    update.kind,
                    update.request,
                    update.witnesses,
                    update.response,
                    update.command
                ],
            )
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn read_facts(connection: &Connection) -> Result<Vec<FactRow>, OpenError> {
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
        .map(|row| {
            let (seq, indexed_kind, bytes) = row.map_err(OpenError::Sqlite)?;
            let document = decode("fact_envelopes", &seq.to_string(), &bytes)?;
            Ok(FactRow {
                seq,
                indexed_kind,
                document,
                changed: false,
            })
        })
        .collect()
}

fn validate_and_rewrite_facts(rows: &mut [FactRow]) -> Result<NodeIndexes, OpenError> {
    let mut graph = Graph::default();
    for row in rows {
        validate_and_rewrite_fact(row, &mut graph)?;
    }
    Ok((graph.by_key, graph.by_directive))
}

fn validate_and_rewrite_fact(row: &mut FactRow, graph: &mut Graph) -> Result<(), OpenError> {
    let row_name = row.seq.to_string();
    let at = envelope_at(&row_name, &row.document)?;
    let fact = fact_object_mut("fact_envelopes", &row_name, &mut row.document)?;
    let kind = string(fact, "kind", &row_name)?.to_owned();
    if kind != row.indexed_kind {
        return Err(invalid(
            "fact_envelopes",
            &row_name,
            "indexed kind disagrees with envelope.fact.kind",
        ));
    }
    let changed = match kind.as_str() {
        SENT | REMINDED => record_publication(fact, &row_name, at, graph)?,
        RESENT => rewrite_resent(fact, &row_name, at, graph)?,
        "steer::acked" => rewrite_identity_terminal(fact, &row_name, graph, Standing::Acked)?,
        "steer::declined" => rewrite_identity_terminal(fact, &row_name, graph, Standing::Declined)?,
        "steer::lapsed" => rewrite_lapse(fact, &row_name, graph)?,
        "steer::canceled" => rewrite_key_terminal(fact, &row_name, graph)?,
        "steer::recovered" => rewrite_recovery(fact, &row_name, graph)?,
        _ => false,
    };
    row.changed |= changed;
    Ok(())
}

fn record_publication(
    fact: &Map<String, Value>,
    row: &str,
    at: wire::Instant,
    graph: &mut Graph,
) -> Result<bool, OpenError> {
    let directive = uuid_text(fact, "directive", row)?;
    let key = key_text(fact, "key", row)?;
    let lane = uuid_text(fact, "lane", row)?;
    let expires = instant_text(fact, "expires", row)?;
    let node = Node {
        root_directive: directive.clone(),
        root_key: key.clone(),
        lane,
        stage: 0,
        at,
    };
    insert_node(
        row,
        &mut graph.by_key,
        &mut graph.by_directive,
        key.clone(),
        directive,
        node,
    )?;
    graph.roots.insert(
        key,
        Root {
            standing: Standing::Live,
            expires,
        },
    );
    Ok(false)
}

fn rewrite_resent(
    fact: &mut Map<String, Value>,
    row: &str,
    at: wire::Instant,
    graph: &mut Graph,
) -> Result<bool, OpenError> {
    let directive = uuid_text(fact, "directive", row)?;
    let key = key_text(fact, "key", row)?;
    let ancestor = key_text(fact, "ancestor", row)?;
    let lane = uuid_text(fact, "lane", row)?;
    let stage = u32_field(fact, "escalation", row)?;
    let below = validate_resent_parent(row, &ancestor, &lane, stage, at, graph)?;
    let node = Node {
        root_directive: below.root_directive.clone(),
        root_key: below.root_key.clone(),
        lane: below.lane,
        stage,
        at,
    };
    insert_node(
        row,
        &mut graph.by_key,
        &mut graph.by_directive,
        key,
        directive,
        node.clone(),
    )?;
    fact.insert("kind".to_owned(), Value::String(ESCALATED.to_owned()));
    fact.insert("directive".to_owned(), Value::String(node.root_directive));
    fact.insert("key".to_owned(), Value::String(node.root_key));
    fact.insert("lane".to_owned(), Value::String(node.lane));
    fact.insert(
        "stage".to_owned(),
        Value::String(stage_word(stage).to_owned()),
    );
    fact.remove("ancestor");
    fact.remove("escalation");
    Ok(true)
}

fn validate_resent_parent(
    row: &str,
    ancestor: &str,
    lane: &str,
    stage: u32,
    at: wire::Instant,
    graph: &mut Graph,
) -> Result<Node, OpenError> {
    let below = graph.by_key.get(ancestor).cloned().ok_or_else(|| {
        invalid(
            "fact_envelopes",
            row,
            "steer::resent ancestor does not name an earlier steering node",
        )
    })?;
    if !graph.descended.insert(ancestor.to_owned()) {
        return Err(invalid(
            "fact_envelopes",
            row,
            "steer::resent forks an ancestor into more than one child",
        ));
    }
    if graph
        .roots
        .get(&below.root_key)
        .is_none_or(|root| root.standing != Standing::Live)
    {
        return Err(invalid(
            "fact_envelopes",
            row,
            "steer::resent descends from an already terminal directive",
        ));
    }
    if lane != below.lane || stage != below.stage + 1 || !(1..=3).contains(&stage) {
        return Err(invalid(
            "fact_envelopes",
            row,
            "steer::resent changes lane or does not advance exactly one of three stages",
        ));
    }
    if at < below.at {
        return Err(invalid(
            "fact_envelopes",
            row,
            "steer::resent predates the ancestor it names",
        ));
    }
    Ok(below)
}

fn rewrite_identity_terminal(
    fact: &mut Map<String, Value>,
    row: &str,
    graph: &mut Graph,
    standing: Standing,
) -> Result<bool, OpenError> {
    terminal_with_identity(
        fact,
        row,
        &graph.by_key,
        &graph.by_directive,
        &mut graph.roots,
        standing,
    )?;
    rewrite_root_reference(fact, row, &graph.by_key, &graph.by_directive, true)
}

fn rewrite_key_terminal(
    fact: &mut Map<String, Value>,
    row: &str,
    graph: &mut Graph,
) -> Result<bool, OpenError> {
    terminal_by_key(
        fact,
        row,
        &graph.by_key,
        &mut graph.roots,
        Standing::Canceled,
    )?;
    rewrite_root_reference(fact, row, &graph.by_key, &graph.by_directive, false)
}

fn rewrite_lapse(
    fact: &mut Map<String, Value>,
    row: &str,
    graph: &mut Graph,
) -> Result<bool, OpenError> {
    let node = terminal_by_key(fact, row, &graph.by_key, &mut graph.roots, Standing::Lapsed)?;
    let expires = instant_text(fact, "expires", row)?;
    if graph
        .roots
        .get(&node.root_key)
        .is_none_or(|root| root.expires != expires)
    {
        return Err(invalid(
            "fact_envelopes",
            row,
            "steer::lapsed expiry disagrees with the stable root",
        ));
    }
    rewrite_root_reference(fact, row, &graph.by_key, &graph.by_directive, false)
}

fn rewrite_recovery(
    fact: &mut Map<String, Value>,
    row: &str,
    graph: &mut Graph,
) -> Result<bool, OpenError> {
    let node = exact_node(fact, row, &graph.by_key, &graph.by_directive)?;
    let root = graph
        .roots
        .get_mut(&node.root_key)
        .ok_or_else(|| invalid("fact_envelopes", row, "recovery root is missing"))?;
    if root.standing != Standing::Lapsed || root.expires != instant_text(fact, "lapsed_at", row)? {
        return Err(invalid(
            "fact_envelopes",
            row,
            "steer::recovered does not match one lapsed stable root",
        ));
    }
    let lapsed_at = instant(fact, "lapsed_at", row)?;
    let expires = instant(fact, "expires", row)?;
    if expires <= lapsed_at {
        return Err(invalid(
            "fact_envelopes",
            row,
            "steer::recovered expiry does not follow its lapse",
        ));
    }
    root.standing = Standing::Live;
    root.expires = expires.to_string();
    rewrite_root_reference(fact, row, &graph.by_key, &graph.by_directive, true)
}

fn insert_node(
    row: &str,
    by_key: &mut BTreeMap<String, Node>,
    by_directive: &mut BTreeMap<String, Node>,
    key: String,
    directive: String,
    node: Node,
) -> Result<(), OpenError> {
    if by_key.insert(key, node.clone()).is_some() || by_directive.insert(directive, node).is_some()
    {
        return Err(invalid(
            "fact_envelopes",
            row,
            "steering key or directive identity is used by more than one node",
        ));
    }
    Ok(())
}

fn terminal_with_identity(
    fact: &Map<String, Value>,
    row: &str,
    by_key: &BTreeMap<String, Node>,
    by_directive: &BTreeMap<String, Node>,
    roots: &mut BTreeMap<String, Root>,
    standing: Standing,
) -> Result<(), OpenError> {
    let node = exact_node(fact, row, by_key, by_directive)?;
    retire(row, roots, &node.root_key, standing)
}

fn terminal_by_key(
    fact: &Map<String, Value>,
    row: &str,
    by_key: &BTreeMap<String, Node>,
    roots: &mut BTreeMap<String, Root>,
    standing: Standing,
) -> Result<Node, OpenError> {
    let key = key_text(fact, "key", row)?;
    let node = by_key.get(&key).cloned().ok_or_else(|| {
        invalid(
            "fact_envelopes",
            row,
            "terminal fact names no steering node",
        )
    })?;
    let lane = uuid_text(fact, "lane", row)?;
    if lane != node.lane {
        return Err(invalid(
            "fact_envelopes",
            row,
            "terminal fact changes the stable root lane",
        ));
    }
    retire(row, roots, &node.root_key, standing)?;
    Ok(node)
}

fn exact_node(
    fact: &Map<String, Value>,
    row: &str,
    by_key: &BTreeMap<String, Node>,
    by_directive: &BTreeMap<String, Node>,
) -> Result<Node, OpenError> {
    let key = key_text(fact, "key", row)?;
    let directive = uuid_text(fact, "directive", row)?;
    let keyed = by_key
        .get(&key)
        .ok_or_else(|| invalid("fact_envelopes", row, "fact key names no steering node"))?;
    let identified = by_directive.get(&directive).ok_or_else(|| {
        invalid(
            "fact_envelopes",
            row,
            "fact identity names no steering node",
        )
    })?;
    if keyed.root_key != identified.root_key || keyed.stage != identified.stage {
        return Err(invalid(
            "fact_envelopes",
            row,
            "fact key and directive identity name different steering nodes",
        ));
    }
    let lane = uuid_text(fact, "lane", row)?;
    if lane != keyed.lane {
        return Err(invalid(
            "fact_envelopes",
            row,
            "fact lane disagrees with its stable steering root",
        ));
    }
    Ok(keyed.clone())
}

fn retire(
    row: &str,
    roots: &mut BTreeMap<String, Root>,
    root_key: &str,
    standing: Standing,
) -> Result<(), OpenError> {
    let root = roots
        .get_mut(root_key)
        .ok_or_else(|| invalid("fact_envelopes", row, "terminal steering root is missing"))?;
    if root.standing != Standing::Live {
        return Err(invalid(
            "fact_envelopes",
            row,
            "a terminal steering fact follows an already terminal state",
        ));
    }
    root.standing = standing;
    Ok(())
}

fn rewrite_root_reference(
    fact: &mut Map<String, Value>,
    row: &str,
    by_key: &BTreeMap<String, Node>,
    by_directive: &BTreeMap<String, Node>,
    has_directive: bool,
) -> Result<bool, OpenError> {
    let key = key_text(fact, "key", row)?;
    let node = by_key
        .get(&key)
        .ok_or_else(|| invalid("fact_envelopes", row, "steering reference has no root"))?;
    let mut changed = key != node.root_key;
    fact.insert("key".to_owned(), Value::String(node.root_key.clone()));
    if has_directive {
        let directive = uuid_text(fact, "directive", row)?;
        let identified = by_directive
            .get(&directive)
            .ok_or_else(|| invalid("fact_envelopes", row, "steering identity has no root"))?;
        if identified.root_key != node.root_key {
            return Err(invalid(
                "fact_envelopes",
                row,
                "steering key and identity resolve to different roots",
            ));
        }
        changed |= directive != node.root_directive;
        fact.insert(
            "directive".to_owned(),
            Value::String(node.root_directive.clone()),
        );
    }
    Ok(changed)
}

struct ReceiptUpdate {
    command: String,
    kind: String,
    request: Vec<u8>,
    witnesses: Vec<u8>,
    response: Vec<u8>,
}

fn rewrite_receipts(
    connection: &Connection,
    by_key: &BTreeMap<String, Node>,
    by_directive: &BTreeMap<String, Node>,
) -> Result<Vec<ReceiptUpdate>, OpenError> {
    let mut statement = connection
        .prepare(
            "SELECT command_id, command_kind, request, evaluated_witnesses, response \
             FROM command_receipts WHERE command_kind LIKE 'steer::%' ORDER BY command_id",
        )
        .map_err(OpenError::Sqlite)?;
    let rows = statement
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
        .map_err(OpenError::Sqlite)?;

    let mut updates = Vec::new();
    for (command, mut kind, request, witnesses, response) in rows {
        let mut request = decode("command_receipts", &command, &request)?;
        let mut witnesses = decode("command_receipts", &command, &witnesses)?;
        let mut response = decode("command_receipts", &command, &response)?;
        let mut changed = rewrite_value(&mut request, by_key, by_directive);
        changed |= rewrite_value(&mut response, by_key, by_directive);
        // Schema 3 exposed `steer::resend` only as the recovery command. Schema
        // 4 gives live re-delivery that name and moves recovery to its own kind,
        // so successful and refused historical receipts must both be renamed.
        let recovery = kind == "steer::resend";
        changed |= rewrite_expected_witnesses(&mut request, by_directive, recovery, &command)?;
        changed |= rewrite_witness_array(&mut witnesses, by_directive, recovery, &command)?;
        if recovery {
            kind = "steer::recover".to_owned();
            if let Some(object) = request.as_object_mut() {
                object.insert("kind".to_owned(), Value::String(kind.clone()));
            }
            changed = true;
        }
        if !changed {
            continue;
        }
        updates.push(ReceiptUpdate {
            command: command.clone(),
            kind,
            request: encode_command(&command, &request)?,
            witnesses: encode_witnesses(&command, &witnesses)?,
            response: encode_response(&command, &response)?,
        });
    }
    Ok(updates)
}

fn rewrite_value(
    value: &mut Value,
    by_key: &BTreeMap<String, Node>,
    by_directive: &BTreeMap<String, Node>,
) -> bool {
    let mut changed = false;
    match value {
        Value::Array(values) => {
            for value in values {
                changed |= rewrite_value(value, by_key, by_directive);
            }
        }
        Value::Object(object) => {
            if let Some(Value::String(key)) = object.get_mut("key")
                && let Some(node) = by_key.get(key)
                && *key != node.root_key
            {
                *key = node.root_key.clone();
                changed = true;
            }
            if let Some(Value::String(token)) = object.get_mut("token")
                && let Some(node) = by_key.get(token)
                && *token != node.root_key
            {
                *token = node.root_key.clone();
                changed = true;
            }
            if let Some(Value::String(directive)) = object.get_mut("directive")
                && let Some(node) = by_directive.get(directive)
                && *directive != node.root_directive
            {
                *directive = node.root_directive.clone();
                changed = true;
            }
            for value in object.values_mut() {
                changed |= rewrite_value(value, by_key, by_directive);
            }
        }
        _ => {}
    }
    changed
}

fn rewrite_expected_witnesses(
    request: &mut Value,
    by_directive: &BTreeMap<String, Node>,
    remove_lineage: bool,
    row: &str,
) -> Result<bool, OpenError> {
    let expected = request
        .as_object_mut()
        .and_then(|object| object.get_mut("expected"))
        .ok_or_else(|| invalid("command_receipts", row, "request.expected is missing"))?;
    rewrite_witness_array(expected, by_directive, remove_lineage, row)
}

fn rewrite_witness_array(
    witnesses: &mut Value,
    by_directive: &BTreeMap<String, Node>,
    recovery: bool,
    row: &str,
) -> Result<bool, OpenError> {
    let values = witnesses.as_array_mut().ok_or_else(|| {
        invalid(
            "command_receipts",
            row,
            "evaluated steering witnesses are not an array",
        )
    })?;
    let mut changed = false;
    let mut index = 0;
    while index < values.len() {
        let object = values[index].as_object_mut().ok_or_else(|| {
            invalid(
                "command_receipts",
                row,
                "a steering witness is not an object",
            )
        })?;
        let kind = object
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                invalid(
                    "command_receipts",
                    row,
                    "a steering witness kind is missing",
                )
            })?
            .to_owned();
        if kind == "lineage" && recovery {
            values.remove(index);
            changed = true;
            continue;
        }
        if kind == "directive" && recovery {
            if object.get("disposition").and_then(Value::as_str) != Some("lapsed") {
                return Err(invalid(
                    "command_receipts",
                    row,
                    "a historical recovery witness is not lapsed",
                ));
            }
            object.insert("kind".to_owned(), Value::String("recoverable".to_owned()));
            object.remove("disposition");
            changed = true;
        }
        if matches!(kind.as_str(), "directive" | "lineage" | "recoverable") {
            let steer = object
                .get_mut("steer")
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    invalid(
                        "command_receipts",
                        row,
                        "a steering witness identity is missing",
                    )
                })?
                .to_owned();
            let node = by_directive.get(&steer).ok_or_else(|| {
                invalid(
                    "command_receipts",
                    row,
                    "a steering witness identity has no durable node",
                )
            })?;
            if steer != node.root_directive {
                object.insert(
                    "steer".to_owned(),
                    Value::String(node.root_directive.clone()),
                );
                changed = true;
            }
        }
        index += 1;
    }
    Ok(changed)
}

fn stage_word(stage: u32) -> &'static str {
    match stage {
        1 => "reminded",
        2 => "escalated",
        3 => "overdue",
        _ => unreachable!("validated steering stage"),
    }
}

fn envelope_at(row: &str, document: &Value) -> Result<wire::Instant, OpenError> {
    document
        .as_object()
        .and_then(|object| object.get("at"))
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("fact_envelopes", row, "envelope.at is missing"))?
        .parse()
        .map_err(|error| {
            invalid(
                "fact_envelopes",
                row,
                &format!("envelope.at is invalid: {error}"),
            )
        })
}

fn string<'a>(
    object: &'a Map<String, Value>,
    field: &str,
    row: &str,
) -> Result<&'a str, OpenError> {
    object.get(field).and_then(Value::as_str).ok_or_else(|| {
        invalid(
            "fact_envelopes",
            row,
            &format!("fact.{field} is not a string"),
        )
    })
}

fn uuid_text(object: &Map<String, Value>, field: &str, row: &str) -> Result<String, OpenError> {
    let text = string(object, field, row)?;
    text.parse::<wire::CanonicalUuid>().map_err(|error| {
        invalid(
            "fact_envelopes",
            row,
            &format!("fact.{field} is invalid: {error}"),
        )
    })?;
    Ok(text.to_owned())
}

fn key_text(object: &Map<String, Value>, field: &str, row: &str) -> Result<String, OpenError> {
    let text = string(object, field, row)?;
    if text.len() != 8 || !text.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(invalid(
            "fact_envelopes",
            row,
            &format!("fact.{field} is not an eight-character steering key"),
        ));
    }
    Ok(text.to_owned())
}

fn instant_text(object: &Map<String, Value>, field: &str, row: &str) -> Result<String, OpenError> {
    let text = string(object, field, row)?;
    text.parse::<wire::Instant>().map_err(|error| {
        invalid(
            "fact_envelopes",
            row,
            &format!("fact.{field} is invalid: {error}"),
        )
    })?;
    Ok(text.to_owned())
}

fn instant(
    object: &Map<String, Value>,
    field: &str,
    row: &str,
) -> Result<wire::Instant, OpenError> {
    let text = instant_text(object, field, row)?;
    text.parse().map_err(|error| {
        invalid(
            "fact_envelopes",
            row,
            &format!("fact.{field} is invalid: {error}"),
        )
    })
}

fn u32_field(object: &Map<String, Value>, field: &str, row: &str) -> Result<u32, OpenError> {
    object
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| invalid("fact_envelopes", row, &format!("fact.{field} is not a u32")))
}

fn fact_object<'a>(
    table: &'static str,
    row: &str,
    document: &'a Value,
) -> Result<&'a Map<String, Value>, OpenError> {
    document
        .as_object()
        .and_then(|object| object.get("fact"))
        .and_then(Value::as_object)
        .ok_or_else(|| invalid(table, row, "envelope.fact is not an object"))
}

fn fact_object_mut<'a>(
    table: &'static str,
    row: &str,
    document: &'a mut Value,
) -> Result<&'a mut Map<String, Value>, OpenError> {
    document
        .as_object_mut()
        .and_then(|object| object.get_mut("fact"))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| invalid(table, row, "envelope.fact is not an object"))
}

fn decode(table: &'static str, row: &str, bytes: &[u8]) -> Result<Value, OpenError> {
    serde_json::from_slice(bytes)
        .map_err(|error| invalid(table, row, &format!("stored JSON is invalid: {error}")))
}

fn encode_fact(row: &str, value: &Value) -> Result<Vec<u8>, OpenError> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        invalid(
            "fact_envelopes",
            row,
            &format!("migrated JSON cannot encode: {error}"),
        )
    })?;
    let envelope: wire::Envelope<wire::AnyFact> = wire::decode(&bytes).map_err(|error| {
        invalid(
            "fact_envelopes",
            row,
            &format!("migrated envelope is not current fact wire: {error}"),
        )
    })?;
    wire::encode(&envelope).map_err(|error| {
        invalid(
            "fact_envelopes",
            row,
            &format!("migrated envelope cannot encode canonically: {error}"),
        )
    })
}

fn encode_command(row: &str, value: &Value) -> Result<Vec<u8>, OpenError> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("request cannot encode: {error}"),
        )
    })?;
    let command: wire::Command<Value> = wire::decode(&bytes).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("migrated request is invalid: {error}"),
        )
    })?;
    command.witnessed().map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("migrated request violates its command law: {error}"),
        )
    })?;
    command
        .canonical_json()
        .map(String::into_bytes)
        .map_err(|error| {
            invalid(
                "command_receipts",
                row,
                &format!("request is not canonical: {error}"),
            )
        })
}

fn encode_witnesses(row: &str, value: &Value) -> Result<Vec<u8>, OpenError> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("witnesses cannot encode: {error}"),
        )
    })?;
    let witnesses: Vec<wire::Witness> = wire::decode(&bytes).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("migrated witnesses are invalid: {error}"),
        )
    })?;
    wire::encode(&witnesses).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("witnesses are not canonical: {error}"),
        )
    })
}

fn encode_response(row: &str, value: &Value) -> Result<Vec<u8>, OpenError> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("response cannot encode: {error}"),
        )
    })?;
    let response: wire::Response<Value> = wire::decode(&bytes).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("migrated response is invalid: {error}"),
        )
    })?;
    wire::encode(&response).map_err(|error| {
        invalid(
            "command_receipts",
            row,
            &format!("response is not canonical: {error}"),
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
mod tests;
