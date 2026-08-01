use std::collections::BTreeSet;

use rusqlite::{Connection, params};
use serde_json::json;
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, ClaimHead, Command, CommandKind, Envelope, Fact,
    FactKind, FindingHead, FindingState, Instant, LaneResidency, LeaseHead, Origin, Outcome, Phase,
    Reply, ReplyPosition, ResidencyHead, ResidentState, Response, Seq, SeqRange, TaskHandle,
    TaskHead, TrustHead, TrustState, WireVersion, Witness, WitnessHead, WorkflowHead,
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

    let error = journal
        .verify()
        .expect_err("corrupt authority must fail verification");
    let classes = error
        .violations()
        .iter()
        .map(|violation| violation.class)
        .collect::<BTreeSet<_>>();
    assert!(classes.contains(&ViolationClass::Envelope));
    assert!(classes.contains(&ViolationClass::Head));
    assert!(classes.contains(&ViolationClass::Receipt));
    assert!(classes.contains(&ViolationClass::Cursor));
    assert!(classes.contains(&ViolationClass::Thread));
}

fn authority_fixture() -> Journal {
    let journal = Journal::in_memory().expect("create authority fixture");
    insert_fact(
        &journal,
        fact(1, None, FactKind::TaskAdded, json!({"task": "JOURNAL-a1"})),
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

fn insert_fact(journal: &Journal, envelope: Envelope<serde_json::Value>) {
    let bytes = wire::encode(&envelope).expect("encode fact fixture");
    let value: serde_json::Value = serde_json::from_slice(&bytes).expect("fact fixture is JSON");
    let kind = value["fact"]["kind"].as_str().expect("fact kind");
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
) -> Envelope<serde_json::Value> {
    Envelope {
        v: WireVersion::CURRENT,
        seq: Seq::new(seq).expect("nonzero fixture seq"),
        at: instant(),
        actor: Actor::Spice,
        origin: "task:JOURNAL-a1".parse::<Origin>().expect("fixture origin"),
        cause: cause.map(|cause| Seq::new(cause).expect("nonzero fixture cause")),
        fact: Fact::new(kind, body),
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
    insert_head(
        &journal.connection,
        "trust_heads",
        "digest",
        "sha256:fixture",
        2,
        &TrustHead {
            digest: "sha256:fixture".to_owned(),
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
            receipt: "receipt:fixture".to_owned(),
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
        kind: wire::head::WitnessKind::Review,
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

fn instant() -> Instant {
    "2030-04-12T18:44:09.271Z".parse().expect("fixture instant")
}

fn phase(text: &str) -> Phase {
    text.parse().expect("fixture phase")
}
