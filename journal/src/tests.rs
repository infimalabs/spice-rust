use std::collections::BTreeSet;

use rusqlite::{Connection, params};
use serde_json::json;
use wire::{
    Actor, AnyFact, CanonicalUuid, ClaimDisposition, ClaimHead, Command, CommandKind, Digest,
    Envelope, FactKind, FindingHead, FindingState, Instant, LaneResidency, LeaseHead, Origin,
    Outcome, Phase, Reply, ReplyPosition, ResidencyHead, ResidentState, Response, Seq, SeqRange,
    TaskHandle, TaskHead, TrustHead, TrustState, WireVersion, Witness, WitnessClass, WitnessHead,
    WorkflowHead,
};

use crate::{Journal, MigrationError, OpenError, SCHEMA_VERSION, ViolationClass};

#[test]
fn empty_database_reaches_the_same_finalized_baseline_once() {
    let mut journal = Journal::in_memory().expect("empty journal migrates");
    assert_eq!(
        journal.schema_version().expect("schema version"),
        SCHEMA_VERSION
    );

    crate::migrate::baseline(&mut journal.connection).expect("baseline is idempotent");
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
fn coherent_authority_verifies_every_required_plane() {
    let journal = authority_fixture();

    let verified = journal.verify().expect("coherent fixture verifies");
    assert_eq!(verified.facts, 2);
    assert_eq!(verified.threads, 2);
    assert_eq!(verified.receipts, 1);
    assert_eq!(verified.heads, 8);
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
    let journal = Journal::in_memory().expect("create authority fixture");
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
                "project": "task.journal",
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
            owner: "task.journal".to_owned(),
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
        project: "task.journal".to_owned(),
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
            occupant: Some(uuid("018f82d4-6f59-7d06-92e3-bce913116303")),
            lineage: Vec::new(),
            capabilities: Vec::new(),
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
