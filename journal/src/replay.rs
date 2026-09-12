use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{Journal, JournalReader};

mod schema;
mod steering;

/// Counts proved by a successful authority verification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Verification {
    pub facts: usize,
    pub receipts: usize,
    pub heads: usize,
    pub threads: usize,
}

/// Counts returned after rebuilding every derived thread-index row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReindexReport {
    pub facts: usize,
    pub threads: usize,
}

/// One verified cause thread, ordered from its root through its latest fact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadSnapshot {
    pub reference: String,
    pub root: wire::Seq,
    pub entries: Vec<wire::Envelope<wire::AnyFact>>,
}

/// Reindex either could not transact or found corrupt immutable history.
#[derive(Debug)]
pub enum ReindexError {
    Sqlite(rusqlite::Error),
    Violations(Vec<Violation>),
}

impl fmt::Display for ReindexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(formatter, "journal reindex failed: {error}"),
            Self::Violations(violations) => write_violations(
                formatter,
                "journal reindex refused",
                " immutable-history",
                violations,
            ),
        }
    }
}

impl Error for ReindexError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::Violations(_) => None,
        }
    }
}

/// Thread lookup either found no unambiguous reference or could not verify authority.
#[derive(Debug)]
pub enum ThreadError {
    Verify(VerifyError),
    Snapshot(crate::SnapshotError),
    Sqlite(rusqlite::Error),
    NotFound { reference: String },
    Ambiguous { reference: String, roots: Vec<u64> },
}

impl fmt::Display for ThreadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Verify(error) => write!(formatter, "thread authority is invalid: {error}"),
            Self::Snapshot(error) => write!(formatter, "thread facts are invalid: {error}"),
            Self::Sqlite(error) => write!(formatter, "thread lookup failed: {error}"),
            Self::NotFound { reference } => {
                write!(formatter, "thread reference {reference:?} was not found")
            }
            Self::Ambiguous { reference, roots } => write!(
                formatter,
                "thread reference {reference:?} names multiple roots {roots:?}"
            ),
        }
    }
}

impl Error for ThreadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Verify(error) => Some(error),
            Self::Snapshot(error) => Some(error),
            Self::Sqlite(error) => Some(error),
            Self::NotFound { .. } | Self::Ambiguous { .. } => None,
        }
    }
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

impl fmt::Display for Violation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:?}:{}:{}",
            self.class, self.subject, self.detail
        )
    }
}

fn write_violations(
    formatter: &mut fmt::Formatter<'_>,
    prefix: &str,
    qualifier: &str,
    violations: &[Violation],
) -> fmt::Result {
    write!(
        formatter,
        "{prefix} {}{qualifier} violation(s)",
        violations.len()
    )?;
    for (index, violation) in violations.iter().enumerate() {
        write!(
            formatter,
            "{}{violation}",
            if index == 0 { ": " } else { " | " }
        )?;
    }
    Ok(())
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
                write_violations(formatter, "journal verification found", "", violations)
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

    /// Rebuilds only the derived origin/cause thread index in one transaction.
    pub fn reindex(&mut self) -> Result<ReindexReport, ReindexError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(ReindexError::Sqlite)?;
        let mut violations = Vec::new();
        let facts = verify_facts(&transaction, &mut violations).map_err(|error| match error {
            VerifyError::Sqlite(error) => ReindexError::Sqlite(error),
            VerifyError::Open(error) => ReindexError::Violations(vec![Violation::new(
                ViolationClass::Schema,
                "schema",
                error.to_string(),
            )]),
            VerifyError::Violations(found) => ReindexError::Violations(found),
        })?;
        let mut expected = BTreeMap::<u64, ThreadRecord>::new();
        for (seq, fact) in &facts {
            if let Some(record) = calculate_thread(*seq, fact, &expected, &mut violations) {
                expected.insert(*seq, record);
            }
        }
        if !violations.is_empty() {
            return Err(ReindexError::Violations(violations));
        }

        transaction
            .execute("DELETE FROM thread_index", [])
            .map_err(ReindexError::Sqlite)?;
        for (seq, record) in &expected {
            transaction
                .execute(
                    concat!(
                        "INSERT INTO thread_index(seq, origin, cause, root, depth) ",
                        "VALUES (?1, ?2, ?3, ?4, ?5)"
                    ),
                    params![
                        sqlite_integer(*seq)?,
                        record.origin,
                        record.cause.map(sqlite_integer).transpose()?,
                        sqlite_integer(record.root)?,
                        sqlite_integer(record.depth)?,
                    ],
                )
                .map_err(ReindexError::Sqlite)?;
        }
        transaction.commit().map_err(ReindexError::Sqlite)?;
        Ok(ReindexReport {
            facts: facts.len(),
            threads: expected.len(),
        })
    }
}

impl JournalReader {
    /// Verifies a finalized authority inside one immutable read-only snapshot.
    pub fn verify(&self) -> Result<Verification, VerifyError> {
        verify_reader_connection(&self.connection)
    }

    /// Resolves one sequence or exact origin into a verified cause thread.
    pub fn thread(&self, reference: &str) -> Result<ThreadSnapshot, ThreadError> {
        thread_connection(&self.connection, reference)
    }
}

pub(crate) fn thread_connection(
    connection: &Connection,
    reference: &str,
) -> Result<ThreadSnapshot, ThreadError> {
    verify_connection(connection).map_err(ThreadError::Verify)?;
    let roots = if let Ok(seq) = reference.parse::<u64>() {
        let Some(seq) = wire::Seq::new(seq) else {
            return Err(ThreadError::NotFound {
                reference: reference.to_owned(),
            });
        };
        connection
            .query_row(
                "SELECT root FROM thread_index WHERE seq = ?1",
                params![sqlite_integer(seq.get()).map_err(|error| match error {
                    ReindexError::Sqlite(error) => ThreadError::Sqlite(error),
                    ReindexError::Violations(_) =>
                        unreachable!("integer conversion has no violations"),
                })?],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(ThreadError::Sqlite)?
            .and_then(|root| u64::try_from(root).ok())
            .into_iter()
            .collect::<Vec<_>>()
    } else {
        let mut statement = connection
            .prepare("SELECT DISTINCT root FROM thread_index WHERE origin = ?1 ORDER BY root")
            .map_err(ThreadError::Sqlite)?;
        let rows = statement
            .query_map(params![reference], |row| row.get::<_, i64>(0))
            .map_err(ThreadError::Sqlite)?;
        let mut roots = Vec::new();
        for row in rows {
            let root = row.map_err(ThreadError::Sqlite)?;
            if let Ok(root) = u64::try_from(root) {
                roots.push(root);
            }
        }
        roots
    };
    let root = match roots.as_slice() {
        [] => {
            return Err(ThreadError::NotFound {
                reference: reference.to_owned(),
            });
        }
        [root] => *root,
        _ => {
            return Err(ThreadError::Ambiguous {
                reference: reference.to_owned(),
                roots,
            });
        }
    };
    let snapshot = crate::read::read_snapshot(connection).map_err(ThreadError::Snapshot)?;
    let by_seq = snapshot
        .facts()
        .iter()
        .map(|envelope| (envelope.seq.get(), envelope))
        .collect::<BTreeMap<_, _>>();
    let mut statement = connection
        .prepare("SELECT seq FROM thread_index WHERE root = ?1 ORDER BY seq")
        .map_err(ThreadError::Sqlite)?;
    let rows = statement
        .query_map(
            params![sqlite_integer(root).map_err(|error| match error {
                ReindexError::Sqlite(error) => ThreadError::Sqlite(error),
                ReindexError::Violations(_) => unreachable!("integer conversion has no violations"),
            })?],
            |row| row.get::<_, i64>(0),
        )
        .map_err(ThreadError::Sqlite)?;
    let mut entries = Vec::new();
    for row in rows {
        let seq = row.map_err(ThreadError::Sqlite)?;
        let seq = u64::try_from(seq).map_err(|_| ThreadError::NotFound {
            reference: reference.to_owned(),
        })?;
        let envelope = by_seq.get(&seq).ok_or_else(|| ThreadError::NotFound {
            reference: reference.to_owned(),
        })?;
        entries.push((*envelope).clone());
    }
    Ok(ThreadSnapshot {
        reference: reference.to_owned(),
        root: wire::Seq::new(root).expect("verified thread roots are positive"),
        entries,
    })
}

fn sqlite_integer(value: u64) -> Result<i64, ReindexError> {
    i64::try_from(value).map_err(|error| {
        ReindexError::Sqlite(rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
    })
}

pub(crate) fn verify_connection(connection: &Connection) -> Result<Verification, VerifyError> {
    verify_connection_after_facts(connection, || {})
}

fn verify_connection_after_facts(
    connection: &Connection,
    after_facts: impl FnOnce(),
) -> Result<Verification, VerifyError> {
    crate::migrate::finalized_version(connection).map_err(VerifyError::Open)?;
    let mut violations = Vec::new();
    verify_sqlite(connection, &mut violations)?;
    let facts = verify_facts(connection, &mut violations)?;
    after_facts();
    let threads = verify_threads(connection, &facts, &mut violations)?;
    let mut heads = verify_heads(connection, &mut violations)?;
    heads += steering::verify(connection, &facts, &mut violations)?;
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

fn verify_reader_connection(connection: &Connection) -> Result<Verification, VerifyError> {
    verify_reader_connection_with(connection, verify_connection)
}

fn verify_reader_connection_with(
    connection: &Connection,
    verify: impl FnOnce(&Connection) -> Result<Verification, VerifyError>,
) -> Result<Verification, VerifyError> {
    let transaction = connection
        .unchecked_transaction()
        .map_err(VerifyError::Sqlite)?;
    let verification = verify(&transaction);
    transaction.commit().map_err(VerifyError::Sqlite)?;
    verification
}

#[cfg(test)]
pub(crate) fn verify_reader_connection_with_concurrent_commit(
    connection: &Connection,
    commit: impl FnOnce(),
) -> Result<Verification, VerifyError> {
    verify_reader_connection_with(connection, |snapshot| {
        verify_connection_after_facts(snapshot, commit)
    })
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
    schema::verify_fact_envelope_bound_objects(connection, violations)?;
    Ok(())
}

#[derive(Clone)]
struct FactRecord {
    origin: String,
    cause: Option<u64>,
    envelope: Option<wire::Envelope<wire::AnyFact>>,
}

fn verify_facts(
    connection: &Connection,
    violations: &mut Vec<Violation>,
) -> Result<BTreeMap<u64, FactRecord>, VerifyError> {
    let mut statement = connection
        .prepare("SELECT seq, wire_version, instant, actor, origin, cause, kind, length(envelope), envelope FROM fact_envelopes ORDER BY seq")
        .map_err(VerifyError::Sqlite)?;
    let mut rows = statement.query([]).map_err(VerifyError::Sqlite)?;
    let mut facts = BTreeMap::new();
    let mut expected = 1_i64;
    while let Some(stored) = rows.next().map_err(VerifyError::Sqlite)? {
        let mut row = FactRow {
            seq: stored.get(0).map_err(VerifyError::Sqlite)?,
            version: stored.get(1).map_err(VerifyError::Sqlite)?,
            instant: stored.get(2).map_err(VerifyError::Sqlite)?,
            actor: stored.get(3).map_err(VerifyError::Sqlite)?,
            origin: stored.get(4).map_err(VerifyError::Sqlite)?,
            cause: stored.get(5).map_err(VerifyError::Sqlite)?,
            kind: stored.get(6).map_err(VerifyError::Sqlite)?,
            envelope_length: stored.get(7).map_err(VerifyError::Sqlite)?,
            envelope: None,
        };
        if row.seq != expected {
            violations.push(Violation::new(
                ViolationClass::Sequence,
                row.seq.to_string(),
                format!("expected contiguous seq {expected}"),
            ));
        }
        expected = row.seq.saturating_add(1);
        if fact_envelope_length_violation(&row, violations) {
            continue;
        }
        row.envelope = Some(stored.get(8).map_err(VerifyError::Sqlite)?);
        let envelope = verify_fact_row(&row, violations);
        if let Ok(seq) = u64::try_from(row.seq) {
            facts.insert(
                seq,
                FactRecord {
                    origin: row.origin,
                    cause: row.cause.and_then(|cause| u64::try_from(cause).ok()),
                    envelope,
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
    envelope_length: i64,
    envelope: Option<Vec<u8>>,
}

fn fact_envelope_length_violation(row: &FactRow, violations: &mut Vec<Violation>) -> bool {
    let limit = u64::try_from(crate::MAX_DURABLE_FACT_ENVELOPE_BYTES)
        .expect("fact envelope limit fits the verifier counter");
    let Ok(observed) = u64::try_from(row.envelope_length) else {
        violations.push(Violation::new(
            ViolationClass::Envelope,
            row.seq.to_string(),
            format!("stored envelope has invalid length {}", row.envelope_length),
        ));
        return true;
    };
    if observed > limit {
        violations.push(Violation::new(
            ViolationClass::Envelope,
            row.seq.to_string(),
            format!(
                "stored envelope has {observed} bytes, exceeding the {limit}-byte durable limit"
            ),
        ));
        return true;
    }
    false
}

fn verify_fact_row(
    row: &FactRow,
    violations: &mut Vec<Violation>,
) -> Option<wire::Envelope<wire::AnyFact>> {
    let subject = row.seq.to_string();
    let envelope = match decode_canonical::<wire::Envelope<wire::AnyFact>>(
        row.envelope
            .as_deref()
            .expect("validated fact bytes are loaded"),
    ) {
        Ok(envelope) => envelope,
        Err(detail) => {
            violations.push(Violation::new(ViolationClass::Envelope, subject, detail));
            return None;
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
    Some(envelope)
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
            (head.born.as_str() == key && head.revision.get() == seq)
                .then_some(())
                .ok_or_else(|| "task stamp or revision disagrees with the row".to_owned())
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
    verify_claim_index(connection, violations)?;
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
    count += verify_transcript_heads(connection, violations)?;
    count += verify_residency(connection, violations)?;
    Ok(count)
}

fn verify_claim_index(
    connection: &Connection,
    violations: &mut Vec<Violation>,
) -> Result<(), VerifyError> {
    let mut statement = connection
        .prepare(
            "SELECT keys.stamp, heads.head_seq, heads.body, \
                    indexed.actor, indexed.lane, indexed.disposition, indexed.head_seq \
             FROM (SELECT stamp FROM claim_heads UNION SELECT stamp FROM claim_index) AS keys \
             LEFT JOIN claim_heads AS heads ON heads.stamp = keys.stamp \
             LEFT JOIN claim_index AS indexed ON indexed.stamp = keys.stamp \
             ORDER BY keys.stamp",
        )
        .map_err(VerifyError::Sqlite)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<i64>>(6)?,
            ))
        })
        .map_err(VerifyError::Sqlite)?;
    for row in rows {
        let (stamp, head_seq, body, actor, lane, disposition, indexed_seq) =
            row.map_err(VerifyError::Sqlite)?;
        let locus = format!("claim_index:{stamp}");
        let (Some(head_seq), Some(body)) = (head_seq, body) else {
            violations.push(Violation::new(
                ViolationClass::Head,
                locus,
                "claim index has no corresponding claim head",
            ));
            continue;
        };
        let (Some(actor), Some(lane), Some(disposition), Some(indexed_seq)) =
            (actor, lane, disposition, indexed_seq)
        else {
            violations.push(Violation::new(
                ViolationClass::Head,
                locus,
                "claim head has no corresponding claim index row",
            ));
            continue;
        };
        if indexed_seq != head_seq {
            violations.push(Violation::new(
                ViolationClass::Head,
                locus.clone(),
                "claim index position disagrees with claim head",
            ));
        }
        let Ok(head) = decode_canonical::<wire::ClaimHead>(&body) else {
            continue;
        };
        let expected_disposition = match head.disposition {
            wire::ClaimDisposition::Active => "active",
            wire::ClaimDisposition::Freed => "freed",
            wire::ClaimDisposition::Lapsed => "lapsed",
        };
        if actor != head.actor.to_string()
            || lane != head.lane.to_string()
            || disposition != expected_disposition
        {
            violations.push(Violation::new(
                ViolationClass::Head,
                locus,
                "claim index predicate columns disagree with claim head",
            ));
        }
    }
    Ok(())
}

fn verify_transcript_heads(
    connection: &Connection,
    violations: &mut Vec<Violation>,
) -> Result<usize, VerifyError> {
    let mut statement = connection
        .prepare(
            "SELECT stream, lane, head_seq, body \
             FROM transcript_source_heads ORDER BY stream",
        )
        .map_err(VerifyError::Sqlite)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })
        .map_err(VerifyError::Sqlite)?;
    let mut count = 0;
    for row in rows {
        let (stream, lane, _seq, body) = row.map_err(VerifyError::Sqlite)?;
        count += 1;
        let locus = format!("transcript_source_heads:{stream}");
        match decode_canonical::<wire::TranscriptSourceHead>(&body) {
            Ok(head) => {
                if head.key().as_str() != stream {
                    violations.push(Violation::new(
                        ViolationClass::Head,
                        locus.clone(),
                        "transcript stream disagrees with the row",
                    ));
                }
                if head.lane.to_string() != lane {
                    violations.push(Violation::new(
                        ViolationClass::Head,
                        locus,
                        "transcript lane predicate disagrees with the row",
                    ));
                }
            }
            Err(detail) => violations.push(Violation::new(ViolationClass::Head, locus, detail)),
        }
    }
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
/// its own stamp, a witness head names its own fact. A lease head carries
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
        "task_heads" | "workflow_heads" | "claim_heads" | "lease_heads" => "stamp",
        "witness_heads" => "name",
        "trust_heads" => "digest",
        "finding_heads" => "identity",
        "transcript_source_heads" => "stream",
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
