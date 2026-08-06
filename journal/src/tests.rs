use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{Connection, params};
use serde_json::json;
use wire::{
    Actor, AnyFact, CanonicalUuid, ClaimDisposition, ClaimHead, Command, CommandKind, Digest,
    Envelope, FactKind, FindingHead, FindingState, Instant, LaneResidency, LeaseHead, Origin,
    Outcome, Phase, Reply, ReplyPosition, ResidencyHead, ResidentState, Response, Seq, SeqRange,
    TaskHandle, TaskHead, TrustHead, TrustState, WireVersion, Witness, WitnessClass, WitnessHead,
    WorkflowHead,
};

use crate::{Journal, JournalReader, MigrationError, OpenError, SCHEMA_VERSION, ViolationClass};

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

#[test]
fn empty_database_reaches_the_same_finalized_baseline_once() {
    let mut journal = Journal::in_memory().expect("empty journal migrates");
    let migration = journal.migration().expect("empty journal migrated");
    assert_eq!((migration.from(), migration.to()), (0, SCHEMA_VERSION));
    assert_eq!(
        journal.schema_version().expect("schema version"),
        SCHEMA_VERSION
    );

    assert_eq!(
        crate::migrate::baseline(&mut journal.connection).expect("baseline is idempotent"),
        None
    );
    assert_eq!(
        journal.schema_version().expect("schema version"),
        SCHEMA_VERSION
    );
    assert_eq!(authority_tables(&journal), expected_tables());

    let verified = journal.verify().expect("empty baseline verifies");
    assert_eq!(verified.facts, 0);
    assert_eq!(verified.receipts, 0);
    assert_eq!(verified.heads, 0);
    assert_eq!(verified.threads, 0);
}

#[test]
fn prepared_and_forward_schema_states_are_refused_by_name() {
    let prepared = schema_fixture(1, "prepared", Some(0), Some(1), Some("plan:baseline"));
    let error = Journal::from_connection(prepared).expect_err("prepared migration refuses open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::Unfinalized {
            from: Some(0),
            to: Some(1)
        })
    ));
    assert!(error.to_string().contains("prepared and unfinalized"));

    let forward = schema_fixture(2, "finalized", None, None, None);
    let error = Journal::from_connection(forward).expect_err("forward schema refuses open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::ForwardSchema {
            found: 2,
            current: 1
        })
    ));
    assert!(error.to_string().contains("schema 2"));
}

#[test]
fn unknown_states_and_occupied_databases_are_refused_by_name() {
    // These two refusals exist for databases this build did not write, so the
    // baseline table's own CHECK constraints cannot stand in for them: a
    // foreign file reaches `finalized_version` carrying whatever it likes.
    let unknown = schema_fixture(1, "migrating", None, None, None);
    let error = Journal::from_connection(unknown).expect_err("unknown state refuses open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::UnknownState { .. })
    ));
    assert!(error.to_string().contains("\"migrating\" is unknown"));

    let occupied = Connection::open_in_memory().expect("open occupied fixture");
    occupied
        .execute_batch("CREATE TABLE somebody_elses_table(id INTEGER PRIMARY KEY) STRICT;")
        .expect("occupy the fixture database");
    let error =
        Journal::from_connection(occupied).expect_err("an occupied database refuses baselining");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::UnversionedAuthority { object_count: 1 })
    ));
    assert!(error.to_string().contains("empty authority"));
}

#[test]
fn malformed_schema_identity_is_refused_by_name() {
    let duplicate = schema_fixture(1, "finalized", None, None, None);
    duplicate
        .execute(
            "INSERT INTO journal_schema(singleton, version, state) VALUES (2, 1, 'finalized')",
            [],
        )
        .expect("insert duplicate schema row");
    let error = Journal::from_connection(duplicate).expect_err("two schema rows refuse open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::SchemaRowCount { count: 2 })
    ));
    assert!(error.to_string().contains("exactly one row, found 2"));

    let missing = schema_fixture(1, "finalized", None, None, None);
    missing
        .execute("UPDATE journal_schema SET singleton = 2", [])
        .expect("move schema row away from singleton one");
    let error = Journal::from_connection(missing).expect_err("missing singleton refuses open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::MissingSingleton)
    ));
    assert!(error.to_string().contains("singleton row 1 is missing"));

    let unknown = schema_fixture(0, "finalized", None, None, None);
    let error = Journal::from_connection(unknown).expect_err("unknown version refuses open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::UnknownVersion { found: 0 })
    ));
    assert!(error.to_string().contains("schema 0 has no migration path"));

    let disagreement = schema_fixture(1, "finalized", None, None, None);
    disagreement
        .pragma_update(None, "user_version", 0)
        .expect("make pragma disagree with schema row");
    let error =
        Journal::from_connection(disagreement).expect_err("version disagreement refuses open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::VersionDisagreement { row: 1, pragma: 0 })
    ));
    assert!(
        error
            .to_string()
            .contains("row names version 1, but SQLite user_version names 0")
    );
}

#[test]
fn read_only_handle_reports_schema_and_cursor_without_mutation() {
    let scratch = Scratch::new();
    let path = scratch.path().join("journal.sqlite3");
    drop(authority_file_fixture(&path));
    let before = FileState::read(&path);

    let reader = JournalReader::open(&path).expect("finalized authority opens read-only");

    assert_eq!(
        reader.schema_version().expect("read schema version"),
        SCHEMA_VERSION
    );
    assert_eq!(reader.cursor().expect("read cursor"), Seq::new(2));
    let query_only: bool = reader
        .connection
        .query_row("PRAGMA query_only", [], |row| row.get(0))
        .expect("query-only posture");
    assert!(query_only);
    let write = reader
        .connection
        .execute("UPDATE cursor_head SET wake = seq WHERE singleton = 1", [])
        .expect_err("SQLite rejects a write through the reader");
    assert_eq!(
        write.sqlite_error_code(),
        Some(rusqlite::ErrorCode::ReadOnly)
    );
    drop(reader);

    assert_eq!(FileState::read(&path), before);

    let empty = scratch.path().join("empty.sqlite3");
    drop(Journal::open(&empty).expect("create empty finalized authority"));
    let before = FileState::read(&empty);

    let reader = JournalReader::open(&empty).expect("empty finalized authority opens read-only");

    assert_eq!(reader.cursor().expect("read empty cursor"), None);
    drop(reader);
    assert_eq!(FileState::read(&empty), before);
}

#[test]
fn read_only_admission_refuses_schema_states_without_mutation() {
    let scratch = Scratch::new();
    let cases = [
        (
            "prepared.sqlite3",
            1,
            "prepared",
            Some(0),
            Some(1),
            Some("plan:baseline"),
            "prepared and unfinalized",
        ),
        (
            "newer.sqlite3",
            2,
            "finalized",
            None,
            None,
            None,
            "newer than this build",
        ),
        (
            "older.sqlite3",
            0,
            "finalized",
            None,
            None,
            None,
            "has no migration path",
        ),
    ];

    for (name, version, state, from, to, digest, expected) in cases {
        let path = scratch.path().join(name);
        drop(schema_file_fixture(&path, version, state, from, to, digest));
        let before = FileState::read(&path);

        let error = JournalReader::open(&path).expect_err("schema state refuses read-only open");

        assert!(
            error.to_string().contains(expected),
            "{name} did not name {expected:?}: {error}",
        );
        assert_eq!(FileState::read(&path), before, "{name} changed on refusal");
    }
}

#[test]
fn read_only_admission_never_creates_or_repairs_an_authority() {
    let scratch = Scratch::new();
    let absent = scratch.path().join("absent.sqlite3");
    let entries_before = directory_entries(scratch.path());

    let error = JournalReader::open(&absent).expect_err("absent authority refuses open");

    assert!(matches!(error, OpenError::Open { ref path, .. } if path == &absent));
    assert!(!absent.exists());
    assert_eq!(directory_entries(scratch.path()), entries_before);

    let malformed = scratch.path().join("malformed.sqlite3");
    fs::write(&malformed, b"not a SQLite authority").expect("write malformed fixture");
    let before = FileState::read(&malformed);

    let error = JournalReader::open(&malformed).expect_err("malformed authority refuses open");

    assert!(matches!(error, OpenError::Sqlite(_)));
    assert_eq!(FileState::read(&malformed), before);
}

#[test]
fn schema_probe_reads_finalized_identity_without_changing_the_store() {
    let scratch = Scratch::new();
    let path = scratch.path().join("authority.db");
    drop(Journal::open(&path).expect("create finalized authority"));
    let before = directory_bytes(scratch.path());

    assert_eq!(
        Journal::inspect_schema(&path).expect("inspect finalized authority"),
        SCHEMA_VERSION
    );
    assert_eq!(directory_bytes(scratch.path()), before);
}

#[test]
fn schema_probe_refuses_unfinalized_and_forward_state_without_changing_the_store() {
    for (name, version, state, from, to, digest, expected) in [
        (
            "prepared",
            1,
            "prepared",
            Some(0),
            Some(1),
            Some("plan:baseline"),
            MigrationError::Unfinalized {
                from: Some(0),
                to: Some(1),
            },
        ),
        (
            "forward",
            2,
            "finalized",
            None,
            None,
            None,
            MigrationError::ForwardSchema {
                found: 2,
                current: SCHEMA_VERSION,
            },
        ),
    ] {
        let scratch = Scratch::new();
        let path = scratch.path().join(format!("{name}.db"));
        drop(schema_file_fixture(&path, version, state, from, to, digest));
        let before = directory_bytes(scratch.path());

        let error = Journal::inspect_schema(&path).expect_err("schema state refuses");
        assert!(
            matches!(error, OpenError::Migration(ref found) if *found == expected),
            "unexpected schema refusal: {error}"
        );
        assert_eq!(directory_bytes(scratch.path()), before);
    }
}

#[test]
fn schema_probe_distinguishes_absence_from_sqlite_corruption_and_creates_nothing() {
    let scratch = Scratch::new();
    let missing = scratch.path().join("missing.db");

    let error = Journal::inspect_schema(&missing).expect_err("missing authority refuses");
    assert!(matches!(error, OpenError::Open { ref path, .. } if path == &missing));
    assert!(directory_bytes(scratch.path()).is_empty());

    let empty = scratch.path().join("empty.db");
    drop(Connection::open(&empty).expect("create empty SQLite fixture"));
    let before = directory_bytes(scratch.path());
    let error = Journal::inspect_schema(&empty).expect_err("missing schema identity refuses");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::MissingSchema)
    ));
    assert_eq!(directory_bytes(scratch.path()), before);

    let malformed = scratch.path().join("malformed.db");
    fs::write(&malformed, b"not a SQLite authority").expect("write malformed fixture");
    let before = directory_bytes(scratch.path());
    let error = Journal::inspect_schema(&malformed).expect_err("malformed SQLite refuses");
    assert!(matches!(error, OpenError::Sqlite(_)));
    assert_eq!(directory_bytes(scratch.path()), before);
}

#[test]
fn coherent_authority_verifies_every_required_plane() {
    let journal = authority_fixture();

    let verified = journal.verify().expect("coherent fixture verifies");
    assert_eq!(verified.facts, 2);
    assert_eq!(verified.threads, 2);
    assert_eq!(verified.receipts, 1);
    assert_eq!(verified.heads, 8);
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
            "SELECT body FROM task_heads WHERE handle = 'JOURNAL-a1'",
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
            "SELECT body FROM task_heads WHERE handle = 'JOURNAL-a1'",
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
fn scoped_read_returns_owned_cursor_complete_task_bundles_without_mutation() {
    let empty = Journal::in_memory().expect("empty journal");
    let empty_changes = empty.connection.total_changes();
    let (cursor, heads) = empty
        .read(|read| Ok((read.cursor()?, read.task_heads()?)))
        .expect("empty board read");
    assert_eq!(cursor, None);
    assert!(heads.is_empty());
    assert_eq!(empty.connection.total_changes(), empty_changes);

    let journal = authority_fixture();
    let mut second = task_head();
    second.handle = "JOURNAL-b2".parse().expect("second handle");
    second.title = "Second journal task".to_owned();
    insert_head(
        &journal.connection,
        "task_heads",
        "handle",
        "JOURNAL-b2",
        1,
        &second,
    );
    insert_head(
        &journal.connection,
        "workflow_heads",
        "handle",
        "JOURNAL-b2",
        2,
        &workflow_head(),
    );
    let changes = journal.connection.total_changes();

    let (cursor, heads) = journal
        .read(|read| Ok((read.cursor()?, read.task_heads()?)))
        .expect("coherent board read");
    let handles = heads
        .into_iter()
        .map(|heads| heads.into_bodies().0.handle.to_string())
        .collect::<Vec<_>>();

    assert_eq!(cursor, Seq::new(2));
    assert_eq!(handles, ["JOURNAL-a1", "JOURNAL-b2"]);
    assert_eq!(journal.connection.total_changes(), changes);
}

#[test]
fn task_bundle_read_names_every_incomplete_or_malformed_head_class() {
    let missing_workflow = authority_fixture();
    missing_workflow
        .connection
        .execute("DELETE FROM workflow_heads WHERE handle = 'JOURNAL-a1'", [])
        .expect("remove required workflow head");

    let loose_claim = authority_fixture();
    loose_claim
        .connection
        .pragma_update(None, "foreign_keys", false)
        .expect("disable foreign keys for corrupt fixture");
    insert_head(
        &loose_claim.connection,
        "claim_heads",
        "handle",
        "JOURNAL-b2",
        2,
        &claim_head(),
    );
    loose_claim
        .connection
        .pragma_update(None, "foreign_keys", true)
        .expect("restore foreign keys");

    let mismatched_key = authority_fixture();
    let mut wrong_task = task_head();
    wrong_task.handle = "JOURNAL-b2".parse().expect("alternate task handle");
    mismatched_key
        .connection
        .execute(
            "UPDATE task_heads SET body = ?1 WHERE handle = 'JOURNAL-a1'",
            params![wire::encode(&wrong_task).expect("encode wrong task")],
        )
        .expect("mismatch indexed key and body");

    let invalid_bytes = authority_fixture();
    invalid_bytes
        .connection
        .execute(
            "UPDATE workflow_heads SET body = X'00' WHERE handle = 'JOURNAL-a1'",
            [],
        )
        .expect("corrupt canonical body");

    let beyond_cursor = authority_fixture();
    insert_fact(
        &beyond_cursor,
        fact(
            3,
            Some(2),
            FactKind::TaskNoted,
            json!({"task": "JOURNAL-a1", "note": "future head fixture"}),
        ),
    );
    beyond_cursor
        .connection
        .execute(
            "UPDATE workflow_heads SET head_seq = 3 WHERE handle = 'JOURNAL-a1'",
            [],
        )
        .expect("move head beyond visible cursor");

    for (journal, expected) in [
        (
            missing_workflow,
            "task JOURNAL-a1 has a statement head but no workflow head",
        ),
        (loose_claim, "task JOURNAL-b2 has loose claim head(s)"),
        (
            mismatched_key,
            "task head indexed as JOURNAL-a1 carries body handle JOURNAL-b2",
        ),
        (
            invalid_bytes,
            "workflow_heads handle \"JOURNAL-a1\" is invalid",
        ),
        (
            beyond_cursor,
            "workflow_heads handle \"JOURNAL-a1\" head position 3 is beyond cursor Some(2)",
        ),
    ] {
        let error = journal
            .read(|read| read.task_heads().map(drop))
            .expect_err("malformed task heads refuse");
        assert!(
            error.to_string().contains(expected),
            "expected {expected:?}, found {error}"
        );
    }
}

#[test]
fn cursor_storage_refuses_backward_movement() {
    let journal = authority_fixture();
    let error = journal
        .connection
        .execute(
            "UPDATE cursor_head SET seq = 1, wake = 1 WHERE singleton = 1",
            [],
        )
        .expect_err("cursor cannot move backward");

    assert!(
        error
            .to_string()
            .contains("cursor head cannot move backward")
    );
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
    wrong_task.handle = "JOURNAL-b2".parse().expect("alternate task handle");
    journal
        .connection
        .execute(
            "UPDATE task_heads SET body = ?1 WHERE handle = 'JOURNAL-a1'",
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
            json!({"task": "JOURNAL-a1", "note": "Journal fixture note"}),
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
                "UPDATE lease_heads SET body = ?1 WHERE handle = 'JOURNAL-a1'",
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
                "task": "JOURNAL-a1",
                "uuid": "018f82d4-6f59-7d06-92e3-bce9131163c2",
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
            json!({"task": "JOURNAL-a1", "phase": "review"}),
        ),
    );
    journal
        .connection
        .execute(
            "INSERT INTO thread_index(seq, origin, cause, root, depth) VALUES (1, 'task:JOURNAL-a1', NULL, 1, 0), (2, 'task:JOURNAL-a1', 1, 1, 1)",
            [],
        )
        .expect("insert thread fixture");
    insert_heads(&journal);
    insert_receipt(&journal);
    journal
        .connection
        .execute(
            "UPDATE cursor_head SET seq = 2, wake = 1 WHERE singleton = 1",
            [],
        )
        .expect("advance cursor fixture");
    journal
}

fn insert_fact(journal: &Journal, envelope: Envelope<AnyFact>) {
    let bytes = wire::encode(&envelope).expect("encode fact fixture");
    let kind = envelope.fact.kind().as_str();
    let seq = i64::try_from(envelope.seq.get()).expect("fixture seq fits SQLite");
    let cause = envelope
        .cause
        .map(Seq::get)
        .map(|cause| i64::try_from(cause).expect("fixture cause fits SQLite"));
    journal
        .connection
        .execute(
            "INSERT INTO fact_envelopes(seq, wire_version, instant, actor, origin, cause, kind, envelope) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                seq, envelope.v.0, envelope.at.to_string(), envelope.actor.to_string(),
                envelope.origin.as_str(), cause, kind, bytes,
            ],
        )
        .expect("insert fact fixture");
}

fn fact(
    seq: u64,
    cause: Option<u64>,
    kind: FactKind,
    body: serde_json::Value,
) -> Envelope<AnyFact> {
    let mut fields = body.as_object().cloned().expect("a fact body is an object");
    fields.insert(
        "kind".to_owned(),
        serde_json::to_value(kind).expect("a fact kind encodes"),
    );
    let bytes = serde_json::to_vec(&fields).expect("fact fields encode");
    let fact = wire::decode(&bytes).expect("fact fields match the registered body");
    Envelope {
        v: WireVersion::CURRENT,
        seq: Seq::new(seq).expect("nonzero fixture seq"),
        at: instant(),
        actor: Actor::Spice,
        origin: "task:JOURNAL-a1".parse::<Origin>().expect("fixture origin"),
        cause: cause.map(|cause| Seq::new(cause).expect("nonzero fixture cause")),
        fact,
    }
}

fn insert_heads(journal: &Journal) {
    insert_head(
        &journal.connection,
        "task_heads",
        "handle",
        "JOURNAL-a1",
        1,
        &task_head(),
    );
    insert_head(
        &journal.connection,
        "workflow_heads",
        "handle",
        "JOURNAL-a1",
        2,
        &workflow_head(),
    );
    insert_head(
        &journal.connection,
        "claim_heads",
        "handle",
        "JOURNAL-a1",
        2,
        &claim_head(),
    );
    insert_head(
        &journal.connection,
        "lease_heads",
        "handle",
        "JOURNAL-a1",
        2,
        &lease_head(),
    );
    insert_head(
        &journal.connection,
        "witness_heads",
        "name",
        "review:JOURNAL-a1",
        2,
        &witness_head(),
    );
    let trust_digest = digest("a");
    insert_head(
        &journal.connection,
        "trust_heads",
        "digest",
        trust_digest.as_str(),
        2,
        &TrustHead {
            digest: trust_digest.clone(),
            state: TrustState::Pended,
        },
    );
    insert_head(
        &journal.connection,
        "finding_heads",
        "identity",
        "gate:fixture",
        2,
        &FindingHead {
            identity: "gate:fixture".to_owned(),
            severity: "error".to_owned(),
            locus: "journal:fixture".to_owned(),
            owner: "journal".to_owned(),
            receipt: digest("b"),
            state: FindingState::Open,
        },
    );
    journal
        .connection
        .execute(
            "INSERT INTO residency_head(singleton, head_seq, body) VALUES (1, 2, ?1)",
            params![wire::encode(&residency_head()).expect("encode residency")],
        )
        .expect("insert residency head");
}

fn insert_head<T: serde::Serialize>(
    connection: &Connection,
    table: &str,
    key_column: &str,
    key: &str,
    seq: u64,
    head: &T,
) {
    let sql = format!("INSERT INTO {table}({key_column}, head_seq, body) VALUES (?1, ?2, ?3)");
    let seq = i64::try_from(seq).expect("fixture head seq fits SQLite");
    connection
        .execute(
            &sql,
            params![key, seq, wire::encode(head).expect("encode head")],
        )
        .expect("insert head fixture");
}

fn insert_receipt(journal: &Journal) {
    let id = uuid("018f82d4-6f59-7d06-92e3-bce9131163c4");
    let epoch = uuid("018f82d4-6f59-7d06-92e3-bce9131163c3");
    let command = Command {
        v: WireVersion::CURRENT,
        id,
        epoch,
        kind: CommandKind::TaskDone,
        expected: Vec::new(),
        body: json!({"task": "JOURNAL-a1", "validation": "fixture"}),
    };
    let response = Response {
        v: WireVersion::CURRENT,
        id,
        epoch,
        outcome: Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: Seq::new(1).expect("first seq"),
                last: Seq::new(2).expect("last seq"),
            }),
            body: json!({"task": "JOURNAL-a1", "status": "review"}),
        }),
    };
    journal.connection.execute(
        "INSERT INTO command_receipts(command_id, actor, command_kind, request, evaluated_witnesses, response, first_seq, last_seq, snapshot_seq, epoch) VALUES (?1, 'operator', 'task::done', ?2, ?3, ?4, 1, 2, NULL, ?5)",
        params![
            id.to_string(), command.canonical_json().expect("canonical request").into_bytes(),
            wire::encode(&Vec::<Witness>::new()).expect("encode witnesses"),
            wire::encode(&response).expect("encode response"), epoch.to_string(),
        ],
    ).expect("insert command receipt");
}

fn task_head() -> TaskHead {
    TaskHead {
        handle: "JOURNAL-a1".parse::<TaskHandle>().expect("task handle"),
        uuid: uuid("018f82d4-6f59-7d06-92e3-bce913116301"),
        title: "Lay journal baseline".to_owned(),
        description: None,
        acceptance: Vec::new(),
        project: "journal".to_owned(),
        tags: vec!["journal".to_owned()],
        dependencies: Vec::new(),
        provenance: "task:JOURNAL-root".parse().expect("provenance"),
        revision: Seq::new(1).expect("revision"),
    }
}

fn workflow_head() -> WorkflowHead {
    WorkflowHead {
        flow: vec![phase("todo"), phase("review")],
        phase: phase("review"),
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
    }
}

fn claim_head() -> ClaimHead {
    ClaimHead {
        actor: Actor::Spice,
        fact: Seq::new(2).expect("claim fact"),
        lane: uuid("018f82d4-6f59-7d06-92e3-bce913116302"),
        occupant: uuid("018f82d4-6f59-7d06-92e3-bce913116303"),
        epoch: uuid("018f82d4-6f59-7d06-92e3-bce913116304"),
        disposition: ClaimDisposition::Active,
    }
}

fn lease_head() -> LeaseHead {
    LeaseHead {
        start: instant(),
        renewal: instant(),
        maximum: instant(),
        expiry: instant(),
        epoch: uuid("018f82d4-6f59-7d06-92e3-bce913116304"),
    }
}

fn witness_head() -> WitnessHead {
    WitnessHead {
        name: "review:JOURNAL-a1".to_owned(),
        kind: WitnessClass::Review,
        predicate: "workflow is review".to_owned(),
        head: Seq::new(2).expect("witness head"),
    }
}

fn residency_head() -> ResidencyHead {
    ResidencyHead {
        epoch: uuid("018f82d4-6f59-7d06-92e3-bce913116304"),
        state: ResidentState::Seated,
        lanes: vec![LaneResidency {
            lane: uuid("018f82d4-6f59-7d06-92e3-bce913116302"),
            lineage_id: uuid("018f82d4-6f59-7d06-92e3-bce913116302"),
            worktree: "/worktrees/lane-a".to_owned(),
            life: "drive".to_owned(),
            vendor: "claude".parse().expect("fixture vendor"),
            occupant: Some(uuid("018f82d4-6f59-7d06-92e3-bce913116303")),
            lineage: Vec::new(),
            capabilities: Vec::new(),
            session: None,
            disposition: "active".to_owned(),
        }],
    }
}

fn schema_fixture(
    version: u32,
    state: &str,
    from: Option<u32>,
    to: Option<u32>,
    digest: Option<&str>,
) -> Connection {
    let connection = Connection::open_in_memory().expect("open schema fixture");
    connection.execute_batch(
        "CREATE TABLE journal_schema(singleton INTEGER PRIMARY KEY, version INTEGER NOT NULL, state TEXT NOT NULL, from_version INTEGER, to_version INTEGER, plan_digest TEXT);",
    ).expect("create schema fixture");
    connection.execute(
        "INSERT INTO journal_schema(singleton, version, state, from_version, to_version, plan_digest) VALUES (1, ?1, ?2, ?3, ?4, ?5)",
        params![version, state, from, to, digest],
    ).expect("insert schema fixture");
    connection
        .pragma_update(None, "user_version", version)
        .expect("set fixture version");
    connection
}

fn schema_file_fixture(
    path: &Path,
    version: u32,
    state: &str,
    from: Option<u32>,
    to: Option<u32>,
    digest: Option<&str>,
) -> Connection {
    let connection = Connection::open(path).expect("open schema file fixture");
    connection.execute_batch(
        "CREATE TABLE journal_schema(singleton INTEGER PRIMARY KEY, version INTEGER NOT NULL, state TEXT NOT NULL, from_version INTEGER, to_version INTEGER, plan_digest TEXT);",
    ).expect("create schema file fixture");
    connection.execute(
        "INSERT INTO journal_schema(singleton, version, state, from_version, to_version, plan_digest) VALUES (1, ?1, ?2, ?3, ?4, ?5)",
        params![version, state, from, to, digest],
    ).expect("insert schema file fixture");
    connection
        .pragma_update(None, "user_version", version)
        .expect("set fixture version");
    connection
}

fn directory_bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fs::read_dir(root)
        .expect("read schema scratch")
        .map(|entry| {
            let path = entry.expect("read schema scratch entry").path();
            let relative = path
                .strip_prefix(root)
                .expect("entry stays below scratch")
                .to_path_buf();
            let bytes = fs::read(&path).expect("read schema scratch file");
            (relative, bytes)
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
struct FileState {
    bytes: Vec<u8>,
    length: u64,
    modified: std::time::SystemTime,
}

impl FileState {
    fn read(path: &Path) -> Self {
        let metadata = fs::metadata(path).expect("read fixture metadata");
        Self {
            bytes: fs::read(path).expect("read fixture bytes"),
            length: metadata.len(),
            modified: metadata.modified().expect("read modification instant"),
        }
    }
}

struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        for _ in 0..1024 {
            let candidate = std::env::temp_dir().join(format!(
                "spice-journal-readonly-{}-{}",
                std::process::id(),
                NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed),
            ));
            match fs::create_dir(&candidate) {
                Ok(()) => return Self { path: candidate },
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create scratch {}: {error}", candidate.display()),
            }
        }
        panic!("could not create a distinct read-only journal scratch directory");
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).expect("remove read-only journal scratch directory");
    }
}

fn directory_entries(path: &Path) -> Vec<String> {
    let mut entries = fs::read_dir(path)
        .expect("read scratch directory")
        .map(|entry| {
            entry
                .expect("read scratch entry")
                .file_name()
                .into_string()
                .expect("scratch entry is UTF-8")
        })
        .collect::<Vec<_>>();
    entries.sort();
    entries
}

fn authority_tables(journal: &Journal) -> BTreeSet<String> {
    let mut statement = journal.connection.prepare(
        "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    ).expect("query authority tables");
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("map table rows")
        .map(|row| row.expect("read table name"))
        .collect()
}

fn expected_tables() -> BTreeSet<String> {
    [
        "claim_heads",
        "command_receipts",
        "cursor_head",
        "fact_envelopes",
        "finding_heads",
        "journal_schema",
        "lease_heads",
        "residency_head",
        "task_heads",
        "thread_index",
        "trust_heads",
        "witness_heads",
        "workflow_heads",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn uuid(text: &str) -> CanonicalUuid {
    text.parse().expect("canonical fixture uuid")
}

fn digest(nibble: &str) -> Digest {
    nibble.repeat(64).parse().expect("canonical digest")
}

fn instant() -> Instant {
    "2030-04-12T18:44:09.271Z".parse().expect("fixture instant")
}

/// One millisecond before [`instant`], the finest step the spelling admits.
fn earlier() -> Instant {
    "2030-04-12T18:44:09.270Z".parse().expect("earlier instant")
}

fn phase(text: &str) -> Phase {
    text.parse().expect("fixture phase")
}
