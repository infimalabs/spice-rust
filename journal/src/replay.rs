use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{Journal, JournalReader};

/// Counts proved by a successful authority verification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Verification {
    pub facts: usize,
    pub receipts: usize,
    pub heads: usize,
    pub threads: usize,
}

/// Stable verification planes.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ViolationClass {
    Schema,
    Envelope,
    Sequence,
    Thread,
    Head,
    Receipt,
    Cursor,
}

/// One authority invariant that did not hold.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Violation {
    pub class: ViolationClass,
    pub subject: String,
    pub detail: String,
}

impl Violation {
    fn new(class: ViolationClass, subject: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            class,
            subject: subject.into(),
            detail: detail.into(),
        }
    }
}

/// Verification either could not inspect SQLite or found authority defects.
#[derive(Debug)]
pub enum VerifyError {
    Open(crate::OpenError),
    Sqlite(rusqlite::Error),
    Violations(Vec<Violation>),
}

impl VerifyError {
    /// Returns all invariant defects when inspection completed.
    pub fn violations(&self) -> &[Violation] {
        match self {
            Self::Violations(violations) => violations,
            Self::Open(_) | Self::Sqlite(_) => &[],
        }
    }
}

impl fmt::Display for VerifyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open(error) => write!(formatter, "journal schema verification failed: {error}"),
            Self::Sqlite(error) => write!(formatter, "journal verification query failed: {error}"),
            Self::Violations(violations) => {
                write!(
                    formatter,
                    "journal verification found {} violation(s)",
                    violations.len()
                )
            }
        }
    }
}

impl Error for VerifyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Open(error) => Some(error),
            Self::Sqlite(error) => Some(error),
            Self::Violations(_) => None,
        }
    }
}

impl Journal {
    /// Verifies schema identity, immutable history, heads, receipts, and indexes.
    pub fn verify(&self) -> Result<Verification, VerifyError> {
        verify_connection(&self.connection)
    }
}

impl JournalReader {
    /// Verifies a finalized authority without acquiring writer capability.
    pub fn verify(&self) -> Result<Verification, VerifyError> {
        verify_connection(&self.connection)
    }
}

fn verify_connection(connection: &Connection) -> Result<Verification, VerifyError> {
    crate::migrate::finalized_version(connection).map_err(VerifyError::Open)?;
    let mut violations = Vec::new();
    verify_sqlite(connection, &mut violations)?;
    let facts = verify_facts(connection, &mut violations)?;
    let threads = verify_threads(connection, &facts, &mut violations)?;
    let heads = verify_heads(connection, &mut violations)?;
    let receipts = verify_receipts(connection, &mut violations)?;
    verify_cursor(connection, &facts, &mut violations)?;

    if violations.is_empty() {
        Ok(Verification {
            facts: facts.len(),
            receipts,
            heads,
            threads,
        })
    } else {
        Err(VerifyError::Violations(violations))
    }
}

fn verify_sqlite(
    connection: &Connection,
    violations: &mut Vec<Violation>,
) -> Result<(), VerifyError> {
    let mut statement = connection
        .prepare("PRAGMA integrity_check")
        .map_err(VerifyError::Sqlite)?;
    let checks = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(VerifyError::Sqlite)?;
    for check in checks {
        let check = check.map_err(VerifyError::Sqlite)?;
        if check != "ok" {
            violations.push(Violation::new(ViolationClass::Schema, "integrity", check));
        }
    }

    let mut foreign = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(VerifyError::Sqlite)?;
    let failures = foreign
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(VerifyError::Sqlite)?;
    for failure in failures {
        let (table, row, parent) = failure.map_err(VerifyError::Sqlite)?;
        violations.push(Violation::new(
            ViolationClass::Schema,
            format!("{table}:{row:?}"),
            format!("foreign key does not resolve in {parent}"),
        ));
    }
    Ok(())
}

#[derive(Clone)]
struct FactRecord {
    origin: String,
    cause: Option<u64>,
}

fn verify_facts(
    connection: &Connection,
    violations: &mut Vec<Violation>,
) -> Result<BTreeMap<u64, FactRecord>, VerifyError> {
    let mut statement = connection
        .prepare("SELECT seq, wire_version, instant, actor, origin, cause, kind, envelope FROM fact_envelopes ORDER BY seq")
        .map_err(VerifyError::Sqlite)?;
    let rows = statement
        .query_map([], |row| {
            Ok(FactRow {
                seq: row.get(0)?,
                version: row.get(1)?,
                instant: row.get(2)?,
                actor: row.get(3)?,
                origin: row.get(4)?,
                cause: row.get(5)?,
                kind: row.get(6)?,
                envelope: row.get(7)?,
            })
        })
        .map_err(VerifyError::Sqlite)?;
    let mut facts = BTreeMap::new();
    let mut expected = 1_i64;
    for row in rows {
        let row = row.map_err(VerifyError::Sqlite)?;
        if row.seq != expected {
            violations.push(Violation::new(
                ViolationClass::Sequence,
                row.seq.to_string(),
                format!("expected contiguous seq {expected}"),
            ));
        }
        expected = row.seq.saturating_add(1);
        verify_fact_row(&row, violations);
        if let Ok(seq) = u64::try_from(row.seq) {
            facts.insert(
                seq,
                FactRecord {
                    origin: row.origin,
                    cause: row.cause.and_then(|cause| u64::try_from(cause).ok()),
                },
            );
        }
    }
    Ok(facts)
}

struct FactRow {
    seq: i64,
    version: i64,
    instant: String,
    actor: String,
    origin: String,
    cause: Option<i64>,
    kind: String,
    envelope: Vec<u8>,
}

fn verify_fact_row(row: &FactRow, violations: &mut Vec<Violation>) {
    let subject = row.seq.to_string();
    let envelope = match decode_canonical::<wire::Envelope<wire::AnyFact>>(&row.envelope) {
        Ok(envelope) => envelope,
        Err(detail) => {
            violations.push(Violation::new(ViolationClass::Envelope, subject, detail));
            return;
        }
    };
    let cause = row.cause.and_then(|cause| u64::try_from(cause).ok());
    let agreed = envelope.seq.get() == u64::try_from(row.seq).unwrap_or_default()
        && i64::from(envelope.v.0) == row.version
        && envelope.at.to_string() == row.instant
        && envelope.actor.to_string() == row.actor
        && envelope.origin.as_str() == row.origin
        && envelope.cause.map(wire::Seq::get) == cause
        && envelope.fact.kind().as_str() == row.kind;
    if !envelope.v.is_current() || !agreed {
        violations.push(Violation::new(
            ViolationClass::Envelope,
            row.seq.to_string(),
            "decoded envelope does not agree with its indexed columns",
        ));
    }
}

fn verify_threads(
    connection: &Connection,
    facts: &BTreeMap<u64, FactRecord>,
    violations: &mut Vec<Violation>,
) -> Result<usize, VerifyError> {
    let rows = thread_rows(connection)?;
    let mut expected = BTreeMap::<u64, ThreadRecord>::new();
    for (seq, fact) in facts {
        let calculated = calculate_thread(*seq, fact, &expected, violations);
        if let Some(calculated) = calculated {
            expected.insert(*seq, calculated);
        }
    }
    for (seq, calculated) in &expected {
        match rows.get(seq) {
            Some(stored) if stored == calculated => {}
            Some(_) => violations.push(Violation::new(
                ViolationClass::Thread,
                seq.to_string(),
                "thread row disagrees with replayed origin, cause, root, or depth",
            )),
            None => violations.push(Violation::new(
                ViolationClass::Thread,
                seq.to_string(),
                "fact has no thread row",
            )),
        }
    }
    for seq in rows.keys().filter(|seq| !facts.contains_key(seq)) {
        violations.push(Violation::new(
            ViolationClass::Thread,
            seq.to_string(),
            "thread row has no fact envelope",
        ));
    }
    Ok(rows.len())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ThreadRecord {
    origin: String,
    cause: Option<u64>,
    root: u64,
    depth: u64,
}

fn calculate_thread(
    seq: u64,
    fact: &FactRecord,
    prior: &BTreeMap<u64, ThreadRecord>,
    violations: &mut Vec<Violation>,
) -> Option<ThreadRecord> {
    let Some(cause) = fact.cause else {
        return Some(ThreadRecord {
            origin: fact.origin.clone(),
            cause: None,
            root: seq,
            depth: 0,
        });
    };
    let Some(parent) = prior.get(&cause) else {
        violations.push(Violation::new(
            ViolationClass::Thread,
            seq.to_string(),
            format!("cause {cause} is absent or does not precede the fact"),
        ));
        return None;
    };
    if parent.origin != fact.origin {
        violations.push(Violation::new(
            ViolationClass::Thread,
            seq.to_string(),
            "cause crosses origin boundaries",
        ));
    }
    Some(ThreadRecord {
        origin: fact.origin.clone(),
        cause: Some(cause),
        root: parent.root,
        depth: parent.depth.saturating_add(1),
    })
}

fn thread_rows(connection: &Connection) -> Result<BTreeMap<u64, ThreadRecord>, VerifyError> {
    let mut statement = connection
        .prepare("SELECT seq, origin, cause, root, depth FROM thread_index ORDER BY seq")
        .map_err(VerifyError::Sqlite)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                ThreadRecord {
                    origin: row.get(1)?,
                    cause: row
                        .get::<_, Option<i64>>(2)?
                        .and_then(|v| u64::try_from(v).ok()),
                    root: u64::try_from(row.get::<_, i64>(3)?).unwrap_or_default(),
                    depth: u64::try_from(row.get::<_, i64>(4)?).unwrap_or_default(),
                },
            ))
        })
        .map_err(VerifyError::Sqlite)?;
    let mut indexed = BTreeMap::new();
    for row in rows {
        let (seq, record) = row.map_err(VerifyError::Sqlite)?;
        if let Ok(seq) = u64::try_from(seq) {
            indexed.insert(seq, record);
        }
    }
    Ok(indexed)
}

fn verify_heads(
    connection: &Connection,
    violations: &mut Vec<Violation>,
) -> Result<usize, VerifyError> {
    let mut count = 0;
    count += verify_keyed_heads::<wire::TaskHead>(
        connection,
        "task_heads",
        violations,
        |key, seq, head| {
            (head.handle.as_str() == key && head.revision.get() == seq)
                .then_some(())
                .ok_or_else(|| "task handle or revision disagrees with the row".to_owned())
        },
    )?;
    count += verify_keyed_heads::<wire::WorkflowHead>(
        connection,
        "workflow_heads",
        violations,
        |_key, _seq, head| {
            head.flow
                .contains(&head.phase)
                .then_some(())
                .ok_or_else(|| "workflow phase is absent from its flow".to_owned())
        },
    )?;
    count += verify_keyed_heads::<wire::ClaimHead>(
        connection,
        "claim_heads",
        violations,
        |_key, seq, head| {
            (head.fact.get() == seq)
                .then_some(())
                .ok_or_else(|| "claim winning fact disagrees with head_seq".to_owned())
        },
    )?;
    count += verify_keyed_heads::<wire::LeaseHead>(
        connection,
        "lease_heads",
        violations,
        |_key, _seq, head| lease_agrees(head),
    )?;
    count += verify_keyed_heads::<wire::WitnessHead>(
        connection,
        "witness_heads",
        violations,
        |key, seq, head| {
            (head.name == key && head.head.get() == seq)
                .then_some(())
                .ok_or_else(|| "witness name or fact disagrees with the row".to_owned())
        },
    )?;
    count += verify_keyed_heads::<wire::TrustHead>(
        connection,
        "trust_heads",
        violations,
        |key, _seq, head| {
            (head.digest.as_str() == key)
                .then_some(())
                .ok_or_else(|| "trust digest disagrees with the row".to_owned())
        },
    )?;
    count += verify_keyed_heads::<wire::FindingHead>(
        connection,
        "finding_heads",
        violations,
        |key, _seq, head| {
            (head.identity == key)
                .then_some(())
                .ok_or_else(|| "finding identity disagrees with the row".to_owned())
        },
    )?;
    count += verify_residency(connection, violations)?;
    Ok(count)
}

fn verify_keyed_heads<T>(
    connection: &Connection,
    table: &'static str,
    violations: &mut Vec<Violation>,
    agreement: impl Fn(&str, u64, &T) -> Result<(), String>,
) -> Result<usize, VerifyError>
where
    T: DeserializeOwned + Serialize,
{
    let sql = format!(
        "SELECT {key}, head_seq, body FROM {table} ORDER BY {key}",
        key = head_key(table)
    );
    let mut statement = connection.prepare(&sql).map_err(VerifyError::Sqlite)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .map_err(VerifyError::Sqlite)?;
    let mut count = 0;
    for row in rows {
        let (key, seq, body) = row.map_err(VerifyError::Sqlite)?;
        count += 1;
        match decode_canonical::<T>(&body) {
            Ok(head) => {
                let seq = u64::try_from(seq).unwrap_or_default();
                if let Err(detail) = agreement(&key, seq, &head) {
                    violations.push(Violation::new(
                        ViolationClass::Head,
                        format!("{table}:{key}"),
                        detail,
                    ));
                }
            }
            Err(detail) => violations.push(Violation::new(
                ViolationClass::Head,
                format!("{table}:{key}"),
                detail,
            )),
        }
    }
    Ok(count)
}

/// A lease's four instants are one ordered life, and that order is the
/// agreement this row owes.
///
/// Every other head table proves agreement against its key: a task head names
/// its own handle, a witness head names its own fact. A lease head carries
/// neither, so checking it against the row would only compare the row to
/// itself. What it does carry is the order its own fields declare: possession
/// begins, is renewed, expires at the instant the latest renewal bought, and is
/// never renewed past the maximum. A lease outside that order is not one this
/// build could have written, whichever column it sits under.
fn lease_agrees(head: &wire::LeaseHead) -> Result<(), String> {
    if head.renewal < head.start {
        return Err("lease renewal precedes the start of possession".to_owned());
    }
    if head.expiry < head.renewal {
        return Err("lease expiry precedes its most recent renewal".to_owned());
    }
    if head.maximum < head.expiry {
        return Err("lease expiry passes the maximum renewal may not extend beyond".to_owned());
    }
    Ok(())
}

fn head_key(table: &str) -> &'static str {
    match table {
        "task_heads" | "workflow_heads" | "claim_heads" | "lease_heads" => "handle",
        "witness_heads" => "name",
        "trust_heads" => "digest",
        "finding_heads" => "identity",
        _ => unreachable!("head table inventory is closed"),
    }
}

fn verify_residency(
    connection: &Connection,
    violations: &mut Vec<Violation>,
) -> Result<usize, VerifyError> {
    let row = connection
        .query_row(
            "SELECT head_seq, body FROM residency_head WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()
        .map_err(VerifyError::Sqlite)?;
    let Some((_seq, body)) = row else {
        return Ok(0);
    };
    if let Err(detail) = decode_canonical::<wire::ResidencyHead>(&body) {
        violations.push(Violation::new(
            ViolationClass::Head,
            "residency_head:1",
            detail,
        ));
    }
    Ok(1)
}

fn verify_receipts(
    connection: &Connection,
    violations: &mut Vec<Violation>,
) -> Result<usize, VerifyError> {
    let mut statement = connection
        .prepare("SELECT command_id, actor, command_kind, request, evaluated_witnesses, response, first_seq, last_seq, snapshot_seq, epoch FROM command_receipts ORDER BY command_id")
        .map_err(VerifyError::Sqlite)?;
    let rows = statement
        .query_map([], |row| {
            Ok(ReceiptRow {
                id: row.get(0)?,
                actor: row.get(1)?,
                kind: row.get(2)?,
                request: row.get(3)?,
                witnesses: row.get(4)?,
                response: row.get(5)?,
                first: row.get(6)?,
                last: row.get(7)?,
                snapshot: row.get(8)?,
                epoch: row.get(9)?,
            })
        })
        .map_err(VerifyError::Sqlite)?;
    let mut count = 0;
    for row in rows {
        let row = row.map_err(VerifyError::Sqlite)?;
        count += 1;
        verify_receipt(&row, violations);
    }
    Ok(count)
}

struct ReceiptRow {
    id: String,
    actor: String,
    kind: String,
    request: Vec<u8>,
    witnesses: Vec<u8>,
    response: Vec<u8>,
    first: Option<i64>,
    last: Option<i64>,
    snapshot: Option<i64>,
    epoch: String,
}

fn verify_receipt(row: &ReceiptRow, violations: &mut Vec<Violation>) {
    let command = match wire::decode::<wire::Command<serde_json::Value>>(&row.request) {
        Ok(command) => command,
        Err(error) => {
            receipt_violation(row, violations, format!("request does not decode: {error}"));
            return;
        }
    };
    let kind = serde_json::from_slice::<serde_json::Value>(&row.request)
        .ok()
        .and_then(|value| value["kind"].as_str().map(str::to_owned));
    let canonical_agrees = command
        .canonical_json()
        .is_ok_and(|canonical| canonical.as_bytes() == row.request);
    if command.id.to_string() != row.id
        || kind.as_deref() != Some(row.kind.as_str())
        || !command.v.is_current()
        || !canonical_agrees
        || row.actor.parse::<wire::Actor>().is_err()
    {
        receipt_violation(
            row,
            violations,
            "request identity or canonical bytes disagree with the receipt",
        );
    }
    match decode_canonical::<Vec<wire::Witness>>(&row.witnesses) {
        Ok(witnesses) if witnesses == command.expected => {}
        Ok(_) => receipt_violation(
            row,
            violations,
            "evaluated witnesses disagree with the canonical request",
        ),
        Err(_) => receipt_violation(
            row,
            violations,
            "evaluated witnesses are not canonical wire bytes",
        ),
    }
    verify_response(row, violations);
}

fn verify_response(row: &ReceiptRow, violations: &mut Vec<Violation>) {
    let response = match decode_canonical::<wire::Response<serde_json::Value>>(&row.response) {
        Ok(response) => response,
        Err(detail) => {
            receipt_violation(row, violations, detail);
            return;
        }
    };
    if response.id.to_string() != row.id
        || response.epoch.to_string() != row.epoch
        || !response.v.is_current()
    {
        receipt_violation(
            row,
            violations,
            "response command, epoch, or version disagrees with the receipt",
        );
    }
    let wire::Outcome::Reply(reply) = response.outcome else {
        receipt_violation(
            row,
            violations,
            "authoritative command receipt does not contain a successful reply",
        );
        return;
    };
    let agreed = match reply.position {
        wire::ReplyPosition::Committed(range) => {
            row.first.and_then(|v| u64::try_from(v).ok()) == Some(range.first.get())
                && row.last.and_then(|v| u64::try_from(v).ok()) == Some(range.last.get())
                && row.snapshot.is_none()
        }
        wire::ReplyPosition::Snapshot(cursor) => {
            row.first.is_none()
                && row.last.is_none()
                && row.snapshot.and_then(|v| u64::try_from(v).ok()) == Some(cursor.get())
        }
        wire::ReplyPosition::Direct {} => false,
    };
    if !agreed {
        receipt_violation(
            row,
            violations,
            "response position disagrees with receipt columns",
        );
    }
}

fn receipt_violation(row: &ReceiptRow, violations: &mut Vec<Violation>, detail: impl Into<String>) {
    violations.push(Violation::new(ViolationClass::Receipt, &row.id, detail));
}

fn verify_cursor(
    connection: &Connection,
    facts: &BTreeMap<u64, FactRecord>,
    violations: &mut Vec<Violation>,
) -> Result<(), VerifyError> {
    let cursor = connection
        .query_row(
            "SELECT seq, wake FROM cursor_head WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .optional()
        .map_err(VerifyError::Sqlite)?;
    let Some((seq, wake)) = cursor else {
        violations.push(Violation::new(
            ViolationClass::Cursor,
            "cursor_head:1",
            "cursor singleton is missing",
        ));
        return Ok(());
    };
    let seq = seq.and_then(|value| u64::try_from(value).ok());
    let wake = wake.and_then(|value| u64::try_from(value).ok());
    let maximum = facts.last_key_value().map(|(seq, _)| *seq);
    if seq != maximum || wake.is_some_and(|wake| seq.is_none_or(|seq| wake > seq)) {
        violations.push(Violation::new(
            ViolationClass::Cursor,
            "cursor_head:1",
            format!("cursor seq {seq:?}, wake {wake:?}, and fact maximum {maximum:?} disagree"),
        ));
    }
    Ok(())
}

fn decode_canonical<T>(bytes: &[u8]) -> Result<T, String>
where
    T: DeserializeOwned + Serialize,
{
    let decoded = wire::decode::<T>(bytes).map_err(|error| error.to_string())?;
    let encoded = wire::encode(&decoded).map_err(|error| error.to_string())?;
    if encoded != bytes {
        return Err("stored bytes are not the current canonical wire encoding".to_owned());
    }
    Ok(decoded)
}
