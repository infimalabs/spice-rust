use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{Connection, params};
use serde_json::{Value, json};
use wire::{
    Actor, AnyFact, CanonicalUuid, ClaimDisposition, ClaimHead, Command, CommandKind, Digest,
    Envelope, FactKind, FindingHead, FindingState, Instant, LaneResidency, LeaseHead, Origin,
    Outcome, Phase, Reply, ReplyPosition, ResidencyHead, ResidentState, Response, Seq, SeqRange,
    TaskHandle, TaskHead, TaskStamp, TrustHead, TrustState, WireVersion, Witness, WitnessClass,
    WitnessHead, WorkflowHead,
};

use wire::body::TranscriptPosition;
use wire::head::TranscriptSourceHead;

use crate::{Journal, JournalReader, MigrationError, OpenError, SCHEMA_VERSION, ViolationClass};

mod allocation_migration;
mod boundary;
mod claim_index;
mod identity_migration;
mod lineage_migration;
mod public_vocabulary_migration;
mod read;
mod release_notes_migration;
mod schema;
mod steering_index;
mod steering_verification;
mod topology_revision_migration;
mod transcript;

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

#[test]
fn coherent_authority_verifies_every_required_plane() {
    let journal = authority_fixture();

    let verified = journal.verify().expect("coherent fixture verifies");
    assert_eq!(verified.facts, 2);
    assert_eq!(verified.threads, 2);
    assert_eq!(verified.receipts, 1);
    assert_eq!(verified.heads, 9);
}

#[test]
fn an_authoritative_receipt_cannot_claim_a_daemonless_direct_position() {
    let journal = authority_fixture();
    let response = Response {
        v: WireVersion::CURRENT,
        id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c4"),
        epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
        outcome: Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body: json!({"task": "JOURNAL-a1", "status": "review"}),
        }),
    };
    journal
        .connection
        .execute(
            "UPDATE command_receipts SET response = ?1",
            params![wire::encode(&response).expect("encode direct response")],
        )
        .expect("plant direct response in authoritative receipt");

    let violation = refused_violations(&journal)
        .into_iter()
        .find(|violation| violation.class == ViolationClass::Receipt)
        .expect("direct response disagrees with authoritative receipt columns");
    assert!(violation.detail.contains("position disagrees"));
}

#[test]
fn fact_snapshot_reads_the_canonical_authority_stream() {
    let journal = authority_fixture();
    let snapshot = journal.fact_snapshot().expect("fact snapshot");

    assert_eq!(snapshot.facts().len(), 2);
    assert_eq!(snapshot.horizon(), Some(Seq::new(2).expect("horizon")));
    assert_eq!(snapshot.facts()[0].fact.kind(), FactKind::TaskAdded);
    assert_eq!(snapshot.facts()[1].fact.kind(), FactKind::TaskPhased);
}

#[test]
fn read_only_snapshot_and_thread_follow_the_verified_cause_chain() {
    let scratch = Scratch::new();
    let path = scratch.path().join("authority.sqlite3");
    drop(authority_file_fixture(&path));

    let reader = JournalReader::open(&path).expect("open authority read-only");
    let snapshot = reader.fact_snapshot().expect("read-only fact snapshot");
    assert_eq!(snapshot.horizon(), Seq::new(2));

    let thread = reader.thread("2").expect("resolve descendant sequence");
    assert_eq!(thread.root, Seq::new(1).expect("root sequence"));
    assert_eq!(
        thread
            .entries
            .iter()
            .map(|entry| entry.seq.get())
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(thread.entries[1].cause, Seq::new(1));
}

#[test]
fn reindex_rebuilds_only_derived_threads_and_rolls_back_on_bad_history() {
    let mut journal = authority_fixture();
    let head_before = journal
        .connection
        .query_row(
            "SELECT body FROM task_heads WHERE stamp = '1kKRRsDP'",
            [],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .expect("read task head before reindex");
    journal
        .connection
        .execute("DELETE FROM thread_index", [])
        .expect("remove derived thread rows");

    let rebuilt = journal.reindex().expect("reindex valid history");
    assert_eq!((rebuilt.facts, rebuilt.threads), (2, 2));
    journal.verify().expect("rebuilt authority verifies");
    let head_after = journal
        .connection
        .query_row(
            "SELECT body FROM task_heads WHERE stamp = '1kKRRsDP'",
            [],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .expect("read task head after reindex");
    assert_eq!(head_after, head_before, "reindex changed an authority head");

    journal
        .connection
        .execute(
            "UPDATE fact_envelopes SET envelope = X'00' WHERE seq = 2",
            [],
        )
        .expect("corrupt immutable fact fixture");
    let threads_before = thread_index_bytes(&journal.connection);
    let error = journal
        .reindex()
        .expect_err("corrupt history must refuse reindex");
    assert!(matches!(error, crate::ReindexError::Violations(_)));
    assert_eq!(
        thread_index_bytes(&journal.connection),
        threads_before,
        "failed reindex changed derived rows"
    );
}

fn thread_index_bytes(connection: &Connection) -> Vec<(i64, String, Option<i64>, i64, i64)> {
    let mut statement = connection
        .prepare("SELECT seq, origin, cause, root, depth FROM thread_index ORDER BY seq")
        .expect("prepare thread snapshot");
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
        .expect("query thread snapshot")
        .map(|row| row.expect("read thread row"))
        .collect()
}

#[test]
fn verify_names_envelope_head_receipt_cursor_and_thread_defects() {
    let classes = refused_classes(&content_defect_fixture());
    assert!(classes.contains(&ViolationClass::Envelope));
    assert!(classes.contains(&ViolationClass::Head));
    assert!(classes.contains(&ViolationClass::Receipt));
    assert!(classes.contains(&ViolationClass::Cursor));
    assert!(classes.contains(&ViolationClass::Thread));
}

#[test]
fn verify_names_schema_and_sequence_defects() {
    let violations = refused_violations(&structure_defect_fixture());
    let sequence = violations
        .iter()
        .find(|violation| violation.class == ViolationClass::Sequence)
        .expect("sequence plane names the history gap");
    assert_eq!(sequence.subject, "4");
    assert!(sequence.detail.contains("expected contiguous seq 3"));

    let schema = violations
        .iter()
        .find(|violation| violation.class == ViolationClass::Schema)
        .expect("schema plane names the unresolved foreign key");
    assert!(schema.subject.contains("thread_index"));
    assert!(schema.detail.contains("fact_envelopes"));
}

#[test]
fn every_violation_plane_has_a_fixture_that_produces_it() {
    // `ViolationClass` declares itself closed, and a closed set earns nothing
    // unless something enforces it. `plane_fixture` matches it exhaustively
    // from outside `replay`, so a new plane fails the build here until it is
    // named. This assertion then proves each plane is actually reached, which
    // is what stops a plane from being declared and never verified.
    let mut observed = refused_classes(&content_defect_fixture());
    observed.extend(refused_classes(&structure_defect_fixture()));

    for class in [
        ViolationClass::Schema,
        ViolationClass::Envelope,
        ViolationClass::Sequence,
        ViolationClass::Thread,
        ViolationClass::Head,
        ViolationClass::Receipt,
        ViolationClass::Cursor,
    ] {
        assert!(
            observed.contains(&class),
            "no fixture produces the {class:?} plane; {} is where it belongs",
            plane_fixture(class),
        );
    }
}

/// The fixture that is supposed to produce each violation plane.
///
/// This is a match rather than a list so that adding a plane fails the build
/// naming the variant nobody assigned a fixture to.
fn plane_fixture(class: ViolationClass) -> &'static str {
    match class {
        ViolationClass::Envelope
        | ViolationClass::Thread
        | ViolationClass::Head
        | ViolationClass::Receipt
        | ViolationClass::Cursor => "content_defect_fixture",
        ViolationClass::Schema | ViolationClass::Sequence => "structure_defect_fixture",
    }
}

fn refused_classes(journal: &Journal) -> BTreeSet<ViolationClass> {
    refused_violations(journal)
        .into_iter()
        .map(|violation| violation.class)
        .collect()
}

fn refused_violations(journal: &Journal) -> Vec<crate::Violation> {
    journal
        .verify()
        .expect_err("a corrupt authority must fail verification")
        .violations()
        .to_vec()
}

/// An authority whose rows decode but disagree with what they are stored under.
fn content_defect_fixture() -> Journal {
    let journal = authority_fixture();
    journal
        .connection
        .execute(
            "UPDATE fact_envelopes SET envelope = X'00' WHERE seq = 2",
            [],
        )
        .expect("corrupt envelope fixture");

    let mut wrong_task = task_head();
    wrong_task.born = "1kKRRsDQ".parse().expect("alternate task stamp");
    journal
        .connection
        .execute(
            "UPDATE task_heads SET body = ?1 WHERE stamp = '1kKRRsDP'",
            params![wire::encode(&wrong_task).expect("encode wrong task")],
        )
        .expect("corrupt head fixture");
    journal
        .connection
        .execute(
            "UPDATE command_receipts SET command_id = '018f82d4-6f59-7d06-92e3-bce913116399'",
            [],
        )
        .expect("corrupt receipt fixture");
    journal
        .connection
        .execute("UPDATE thread_index SET depth = 9 WHERE seq = 2", [])
        .expect("corrupt thread fixture");
    journal
        .connection
        .execute_batch(
            "DROP TRIGGER cursor_head_monotone; UPDATE cursor_head SET seq = 1, wake = 1;",
        )
        .expect("corrupt cursor fixture");
    journal
}

/// An authority whose history has a hole and whose index points at nothing.
///
/// Both defects are invisible to the content checks: every stored row here
/// decodes and agrees with its own columns. What is wrong is the shape of the
/// history around them, which is the only thing the sequence and schema planes
/// look at.
fn structure_defect_fixture() -> Journal {
    let journal = authority_fixture();
    insert_fact(
        &journal,
        fact(
            4,
            None,
            FactKind::TaskNoted,
            json!({"task": "1kKRRsDP", "note": "Journal fixture note"}),
        ),
    );
    journal
        .connection
        .execute_batch(
            "INSERT INTO thread_index(seq, origin, cause, root, depth) VALUES (4, 'task:JOURNAL-a1', NULL, 4, 0);
             UPDATE cursor_head SET seq = 4 WHERE singleton = 1;
             PRAGMA foreign_keys = OFF;
             INSERT INTO thread_index(seq, origin, cause, root, depth) VALUES (9, 'task:JOURNAL-a1', NULL, 9, 0);
             PRAGMA foreign_keys = ON;",
        )
        .expect("open a hole at seq 3 and orphan a thread row");
    journal
}

#[test]
fn lease_heads_must_carry_an_ordered_life() {
    // One way to put a lease's instants out of order. The lease row is the one
    // head with no key to disagree with, so before this its body could say
    // anything at all and still verify.
    type Disorder = fn(&mut LeaseHead);

    let cases: [(Disorder, &str); 3] = [
        (
            |lease| lease.renewal = earlier(),
            "renewal precedes the start of possession",
        ),
        (
            |lease| lease.expiry = earlier(),
            "expiry precedes its most recent renewal",
        ),
        (
            |lease| lease.maximum = earlier(),
            "expiry passes the maximum renewal may not extend beyond",
        ),
    ];

    for (disorder, expected) in cases {
        let journal = authority_fixture();
        let mut lease = lease_head();
        disorder(&mut lease);
        journal
            .connection
            .execute(
                "UPDATE lease_heads SET body = ?1 WHERE stamp = '1kKRRsDP'",
                params![wire::encode(&lease).expect("encode disordered lease")],
            )
            .expect("corrupt lease fixture");

        let error = journal
            .verify()
            .expect_err("a disordered lease must fail verification");
        let details = error
            .violations()
            .iter()
            .filter(|violation| violation.class == ViolationClass::Head)
            .map(|violation| violation.detail.clone())
            .collect::<Vec<_>>();
        assert!(
            details.iter().any(|detail| detail.contains(expected)),
            "no head violation named {expected:?}, found {details:?}",
        );
    }
}

fn authority_fixture() -> Journal {
    populate_authority(Journal::in_memory().expect("create authority fixture"))
}

fn authority_file_fixture(path: &Path) -> Journal {
    populate_authority(Journal::open(path).expect("create file authority fixture"))
}

fn populate_authority(journal: Journal) -> Journal {
    insert_fact(
        &journal,
        fact(
            1,
            None,
            FactKind::TaskAdded,
            json!({
                "task": "1kKRRsDP",
                "title": "Journal fixture",
                "project": "journal",
                "tags": [],
                "depends": [],
                "flow": ["todo", "review"],
                "phase": "todo",
                "revision": 1
            }),
        ),
    );
    insert_fact(
        &journal,
        fact(
            2,
            Some(1),
            FactKind::TaskPhased,
            json!({"task": "1kKRRsDP", "phase": "review"}),
        ),
    );
    journal
        .connection
        .execute(
            "INSERT INTO thread_index(seq, origin, cause, root, depth) VALUES (1, 'task:1kKRRsDP', NULL, 1, 0), (2, 'task:1kKRRsDP', 1, 1, 1)",
            [],
        )
        .expect("insert thread fixture");
    insert_heads(&journal);
    insert_receipt(&journal);
    journal
        .connection
        .execute_batch(
            "UPDATE cursor_head SET seq = 2, wake = 1 WHERE singleton = 1;
             UPDATE steering_index SET indexed_seq = 2 WHERE singleton = 1;",
        )
        .expect("advance cursor fixture");
    journal
}

/// Turn the current task-identity carriers back into the exact schema-3 shape.
///
/// Older-schema tests start from the growing baseline so they inherit every
/// unrelated table. Identity is the one non-additive transition, so those
/// fixtures must also restore its old columns and bodies rather than lying
/// about their version while retaining schema-4 content.
pub(super) fn downgrade_task_identity_to_five(connection: &Connection) {
    connection
        .execute_batch("DROP TABLE IF EXISTS claim_index;")
        .expect("remove schema-8 claim index before restoring older task heads");
    downgrade_fact_identity_to_three(connection);
    downgrade_head_identity_to_three(connection);
}

fn downgrade_fact_identity_to_three(connection: &Connection) {
    for (seq, bytes) in current_fact_rows(connection) {
        let (document, origin) = legacy_fact(seq, &bytes);
        connection
            .execute(
                "UPDATE fact_envelopes SET origin = ?1, envelope = ?2 WHERE seq = ?3",
                params![
                    origin,
                    serde_json::to_vec(&document).expect("encode legacy fact"),
                    seq
                ],
            )
            .expect("store legacy fact");
        connection
            .execute(
                "UPDATE thread_index SET origin = ?1 WHERE seq = ?2",
                params![origin, seq],
            )
            .expect("store legacy thread origin");
    }
}

fn current_fact_rows(connection: &Connection) -> Vec<(i64, Vec<u8>)> {
    let mut statement = connection
        .prepare("SELECT seq, envelope FROM fact_envelopes ORDER BY seq")
        .expect("read current fact carriers");
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("map current facts")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect current facts")
}

fn legacy_fact(seq: i64, bytes: &[u8]) -> (Value, String) {
    let mut document: Value = serde_json::from_slice(bytes).expect("decode current fact");
    let object = document.as_object_mut().expect("fact envelope object");
    legacy_task_origin(object, "origin");
    let origin = object["origin"]
        .as_str()
        .expect("fact origin string")
        .to_owned();
    let fact = object
        .get_mut("fact")
        .and_then(Value::as_object_mut)
        .expect("fact body object");
    legacy_task_stamp(fact, "task");
    if fact.get("kind").and_then(Value::as_str) == Some("task::added") {
        fact.insert(
            "uuid".to_owned(),
            Value::String(format!("018f82d4-6f59-7d06-92e3-{seq:012x}")),
        );
        legacy_handle_array(fact, "depends");
    }
    (document, origin)
}

fn downgrade_head_identity_to_three(connection: &Connection) {
    let task_rows = current_head_rows(connection, "task_heads");
    let workflow_rows = current_head_rows(connection, "workflow_heads");
    let claim_rows = current_head_rows(connection, "claim_heads");
    let lease_rows = current_head_rows(connection, "lease_heads");
    restore_schema_three_head_tables(connection);
    store_legacy_task_heads(connection, task_rows);
    store_legacy_child_heads(
        connection,
        [
            ("workflow_heads", workflow_rows),
            ("claim_heads", claim_rows),
            ("lease_heads", lease_rows),
        ],
    );
}

fn restore_schema_three_head_tables(connection: &Connection) {
    connection
        .execute_batch(
            "DROP TABLE lease_heads;
             DROP TABLE claim_heads;
             DROP TABLE workflow_heads;
             DROP TABLE task_heads;
             CREATE TABLE task_heads (
                 handle TEXT PRIMARY KEY,
                 head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
                 body BLOB NOT NULL
             ) STRICT;
             CREATE TABLE workflow_heads (
                 handle TEXT PRIMARY KEY REFERENCES task_heads(handle),
                 head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
                 body BLOB NOT NULL
             ) STRICT;
             CREATE TABLE claim_heads (
                 handle TEXT PRIMARY KEY REFERENCES task_heads(handle),
                 head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
                 body BLOB NOT NULL
             ) STRICT;
             CREATE TABLE lease_heads (
                 handle TEXT PRIMARY KEY REFERENCES task_heads(handle),
                 head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
                 body BLOB NOT NULL
             ) STRICT;",
        )
        .expect("restore schema-three task head tables");
}

fn store_legacy_task_heads(connection: &Connection, rows: Vec<HeadRow>) {
    for (stamp, head_seq, bytes) in rows {
        let stamp: TaskStamp = stamp.parse().expect("current head stamp");
        let handle = legacy_handle(&stamp);
        let mut document: Value = serde_json::from_slice(&bytes).expect("decode current task head");
        let head = document.as_object_mut().expect("task head object");
        head.remove("born").expect("current task head stamp");
        head.insert("handle".to_owned(), Value::String(handle.to_string()));
        head.insert(
            "uuid".to_owned(),
            Value::String("018f82d4-6f59-7d06-92e3-bce913116301".to_owned()),
        );
        legacy_handle_array(head, "dependencies");
        if let Some(origin) = head.get("provenance").and_then(Value::as_str)
            && let Some(stamp) = origin.strip_prefix("task:")
            && let Ok(stamp) = stamp.parse::<TaskStamp>()
        {
            head.insert(
                "provenance".to_owned(),
                Value::String(format!("task:{}", legacy_handle(&stamp))),
            );
        }
        connection
            .execute(
                "INSERT INTO task_heads(handle, head_seq, body) VALUES (?1, ?2, ?3)",
                params![
                    handle.as_str(),
                    head_seq,
                    serde_json::to_vec(&document).expect("encode legacy task head")
                ],
            )
            .expect("store legacy task head");
    }
}

fn store_legacy_child_heads(connection: &Connection, tables: [(&str, Vec<HeadRow>); 3]) {
    for (table, rows) in tables {
        for (stamp, head_seq, bytes) in rows {
            let stamp: TaskStamp = stamp.parse().expect("current child stamp");
            connection
                .execute(
                    &format!("INSERT INTO {table}(handle, head_seq, body) VALUES (?1, ?2, ?3)"),
                    params![legacy_handle(&stamp).as_str(), head_seq, bytes],
                )
                .expect("store legacy child head");
        }
    }
}

type HeadRow = (String, i64, Vec<u8>);

fn current_head_rows(connection: &Connection, table: &str) -> Vec<HeadRow> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT stamp, head_seq, body FROM {table} ORDER BY stamp"
        ))
        .expect("read current head rows");
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("map current heads")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect current heads")
}

fn legacy_handle(stamp: &TaskStamp) -> TaskHandle {
    TaskHandle::for_project("task.journal", stamp)
}

fn legacy_task_origin(object: &mut serde_json::Map<String, Value>, field: &str) {
    let Some(stamp) = object
        .get(field)
        .and_then(Value::as_str)
        .and_then(|origin| origin.strip_prefix("task:"))
        .and_then(|stamp| stamp.parse::<TaskStamp>().ok())
    else {
        return;
    };
    object.insert(
        field.to_owned(),
        Value::String(format!("task:{}", legacy_handle(&stamp))),
    );
}

fn legacy_task_stamp(object: &mut serde_json::Map<String, Value>, field: &str) {
    let Some(stamp) = object
        .get(field)
        .and_then(Value::as_str)
        .and_then(|stamp| stamp.parse::<TaskStamp>().ok())
    else {
        return;
    };
    object.insert(
        field.to_owned(),
        Value::String(legacy_handle(&stamp).to_string()),
    );
}

fn legacy_handle_array(object: &mut serde_json::Map<String, Value>, field: &str) {
    let values = object
        .get_mut(field)
        .and_then(Value::as_array_mut)
        .expect("current identity list");
    for value in values {
        let stamp: TaskStamp = value
            .as_str()
            .expect("current stamp string")
            .parse()
            .expect("current stamp");
        *value = Value::String(legacy_handle(&stamp).to_string());
    }
}

include!("tests/authority_support.rs");
