//! The one schema-3 to schema-4 identity crossing.
//!
//! These fixtures are written in the retired wire shape on purpose. The test
//! opens them only through the migration door, then every assertion decodes
//! current types; no runtime decoder learns the old fields.

use super::*;

const STAMP: &str = "1kKRRsDP";
const FOLLOWUP_STAMP: &str = "1kKRRsDQ";
const HANDLE: &str = "JOURNAL-1kKRRsDP";
const FOLLOWUP_HANDLE: &str = "JOURNAL-1kKRRsDQ";

#[test]
fn schema_three_contracts_every_authoritative_task_carrier_once() {
    let scratch = Scratch::new();
    let path = scratch.path().join("identity-v3.sqlite3");
    let journal = authority_file_fixture(&path);
    downgrade_task_identity_to_three(&journal.connection);
    store_bare_stamp_origin(&journal.connection, 2);
    insert_legacy_review_receipt(&journal.connection);
    insert_legacy_board_receipt(&journal.connection);
    insert_legacy_effort_receipt(&journal.connection);
    journal
        .connection
        .execute_batch(
            "UPDATE journal_schema SET version = 3 WHERE singleton = 1;
             PRAGMA user_version = 3;",
        )
        .expect("mark legacy identity fixture as schema three");
    drop(journal);

    let journal = Journal::open(&path).expect("schema-three identity migrates");

    assert_eq!(
        journal
            .migration()
            .map(|migration| (migration.from(), migration.to())),
        Some((3, SCHEMA_VERSION))
    );
    assert_eq!(task_key_column(&journal.connection), "stamp");
    let (stamp, head): (String, Vec<u8>) = journal
        .connection
        .query_row("SELECT stamp, body FROM task_heads", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .expect("read migrated statement head");
    assert_eq!(stamp, STAMP);
    let head: TaskHead = wire::decode(&head).expect("migrated task head is current wire");
    assert_eq!(head.incepted.as_str(), STAMP);
    assert_eq!(head.provenance.as_str(), format!("task:{STAMP}"));
    let head_json = serde_json::to_value(&head).expect("task head JSON");
    assert!(
        !head_json
            .as_object()
            .expect("head object")
            .contains_key("handle")
    );
    assert!(
        !head_json
            .as_object()
            .expect("head object")
            .contains_key("uuid")
    );

    let fact_bytes: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE seq = 1",
            [],
            |row| row.get(0),
        )
        .expect("read migrated creation fact");
    let fact: Envelope<AnyFact> =
        wire::decode(&fact_bytes).expect("migrated creation fact is current wire");
    assert_eq!(fact.origin.as_str(), format!("task:{STAMP}"));
    assert_eq!(fact.fact.body()["task"], STAMP);
    assert!(fact.fact.body().get("uuid").is_none());
    let phased_origin: String = journal
        .connection
        .query_row(
            "SELECT origin FROM fact_envelopes WHERE seq = 2",
            [],
            |row| row.get(0),
        )
        .expect("read migrated pre-normalized task origin");
    assert_eq!(phased_origin, format!("task:{STAMP}"));

    assert_review_receipt(&journal.connection);
    assert_board_receipt(&journal.connection);
    assert_effort_receipt(&journal.connection);
    journal
        .verify()
        .expect("migrated identity authority verifies");
}

fn store_bare_stamp_origin(connection: &Connection, seq: i64) {
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE seq = ?1",
            [seq],
            |row| row.get(0),
        )
        .expect("read legacy fact selected for bare-stamp provenance");
    let mut document: Value = serde_json::from_slice(&bytes).expect("decode legacy fact");
    let origin = format!("task:{STAMP}");
    document["origin"] = Value::String(origin.clone());
    connection
        .execute(
            "UPDATE fact_envelopes SET origin = ?1, envelope = ?2 WHERE seq = ?3",
            params![
                origin,
                serde_json::to_vec(&document).expect("encode bare-stamp legacy fact"),
                seq,
            ],
        )
        .expect("store bare-stamp legacy fact origin");
    connection
        .execute(
            "UPDATE thread_index SET origin = ?1 WHERE seq = ?2",
            params![format!("task:{STAMP}"), seq],
        )
        .expect("store bare-stamp legacy thread origin");
}

#[test]
fn malformed_legacy_identity_refuses_and_rolls_back_the_whole_crossing() {
    let scratch = Scratch::new();
    let path = scratch.path().join("identity-v3-invalid.sqlite3");
    let journal = authority_file_fixture(&path);
    downgrade_task_identity_to_three(&journal.connection);
    let bytes: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE seq = 1",
            [],
            |row| row.get(0),
        )
        .expect("read legacy creation fact");
    let mut document: Value = serde_json::from_slice(&bytes).expect("decode legacy fact");
    document["fact"]["task"] = Value::String("JOURNAL-not-a-stamp".to_owned());
    journal
        .connection
        .execute(
            "UPDATE fact_envelopes SET envelope = ?1 WHERE seq = 1",
            params![serde_json::to_vec(&document).expect("encode malformed legacy fact")],
        )
        .expect("store malformed legacy fact");
    journal
        .connection
        .execute_batch(
            "UPDATE journal_schema SET version = 3 WHERE singleton = 1;
             PRAGMA user_version = 3;",
        )
        .expect("mark malformed fixture as schema three");
    drop(journal);
    let before = legacy_identity_state(&Connection::open(&path).expect("read malformed fixture"));

    let error = Journal::open(&path).expect_err("malformed inception suffix refuses migration");

    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::StoredContent {
            from: 3,
            to: 4,
            table: "fact_envelopes",
            ref row,
            ..
        }) if row == "1"
    ));
    assert!(error.to_string().contains("inception stamp"));
    let after = Connection::open(&path).expect("reopen refused authority");
    assert_eq!(legacy_identity_state(&after), before);
}

fn insert_legacy_review_receipt(connection: &Connection) {
    let id = uuid("018f82d4-6f59-7d06-92e3-bce913116320");
    let epoch = uuid("018f82d4-6f59-7d06-92e3-bce913116321");
    let stamp: TaskStamp = STAMP.parse().expect("fixture stamp");
    let followup: TaskStamp = FOLLOWUP_STAMP.parse().expect("fixture followup stamp");
    let expected = vec![
        Witness::Absence {
            task: followup.clone(),
        },
        Witness::Edge {
            task: followup.clone(),
            after: stamp,
        },
    ];
    let request = Command {
        v: WireVersion::CURRENT,
        id,
        epoch,
        kind: CommandKind::TaskReview,
        expected: expected.clone(),
        body: wire::intent::TaskReview {
            task: HANDLE.parse().expect("fixture handle"),
            finding: "changes".to_owned(),
            note: Some("follow up".to_owned()),
            then: vec![wire::intent::ReviewFollowup {
                task: followup,
                add: wire::intent::TaskAdd {
                    title: "Follow-up".to_owned(),
                    project: Some("task.journal".to_owned()),
                    description: None,
                    acceptance: Vec::new(),
                    origin: None,
                    flow: Some("todo".to_owned()),
                    depends: Vec::new(),
                },
            }],
            followup: Vec::new(),
        },
    };
    let mut request: Value = serde_json::from_str(
        &request
            .canonical_json()
            .expect("encode current review request"),
    )
    .expect("decode current review request");
    legacy_witness_values(
        request["expected"]
            .as_array_mut()
            .expect("request witnesses"),
    );
    request["body"]["then"][0]["task"] = Value::String(FOLLOWUP_HANDLE.to_owned());
    request["expected"]
        .as_array_mut()
        .expect("request witnesses")
        .sort_by_key(std::string::ToString::to_string);
    let mut witnesses = serde_json::to_value(expected).expect("encode current witnesses");
    legacy_witness_values(witnesses.as_array_mut().expect("evaluated witnesses"));
    let response = Response {
        v: WireVersion::CURRENT,
        id,
        epoch,
        outcome: Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: Seq::new(2).expect("review first"),
                last: Seq::new(2).expect("review last"),
            }),
            body: wire::intent::TaskAnswer {
                task: HANDLE.parse().expect("fixture handle"),
                status: "todo".to_owned(),
                learning: None,
                next: None,
            },
        }),
    };
    insert_receipt_row(
        connection,
        id,
        epoch,
        "task::review",
        serde_json::to_vec(&request).expect("legacy review request"),
        serde_json::to_vec(&witnesses).expect("legacy review witnesses"),
        wire::encode(&response).expect("review response"),
        Some((2, 2)),
    );
}

fn insert_legacy_board_receipt(connection: &Connection) {
    let id = uuid("018f82d4-6f59-7d06-92e3-bce913116322");
    let epoch = uuid("018f82d4-6f59-7d06-92e3-bce913116323");
    let request = Command {
        v: WireVersion::CURRENT,
        id,
        epoch,
        kind: CommandKind::BoardRead,
        expected: Vec::new(),
        body: wire::intent::BoardRead { family: None },
    };
    let board = wire::intent::BoardAnswer {
        schema: 3,
        cursor: Seq::new(2).expect("board cursor"),
        principal: Actor::Operator,
        project_policy: wire::intent::BoardProjectPolicy {
            public_stems: vec!["task".to_owned()],
            public_min_depth: 2,
            public_max_depth: 4,
        },
        actor_route: wire::intent::BoardActorRoute {
            actor: "operator".to_owned(),
            project: "agent.operator.task".to_owned(),
            origin_thread: "operator".to_owned(),
        },
        allocation_band: 0,
        tasks: vec![wire::intent::BoardTask::from_authoritative(
            wire::intent::ProjectVisibility::Public,
            "operator".to_owned(),
            wire::intent::Positioned {
                position: Seq::new(1).expect("statement position"),
                body: task_head(),
            },
            wire::intent::Positioned {
                position: Seq::new(2).expect("workflow position"),
                body: workflow_head(),
            },
            None,
            None,
        )],
        residency: wire::intent::Positioned {
            position: Seq::new(2).expect("residency position"),
            body: residency_head(),
        },
    };
    let response = Response {
        v: WireVersion::CURRENT,
        id,
        epoch,
        outcome: Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(Seq::new(2).expect("board snapshot")),
            body: board,
        }),
    };
    let mut response = serde_json::to_value(response).expect("encode current board response");
    let statement = response["outcome"]["body"]["tasks"][0]["statement"]["body"]
        .as_object_mut()
        .expect("board statement");
    statement
        .remove("incepted")
        .expect("current statement stamp");
    statement.insert(
        "uuid".to_owned(),
        Value::String("018f82d4-6f59-7d06-92e3-bce913116301".to_owned()),
    );
    statement.insert(
        "provenance".to_owned(),
        Value::String(format!("task:{HANDLE}")),
    );
    legacy_handle_array(statement, "dependencies");
    insert_receipt_row(
        connection,
        id,
        epoch,
        "board::read",
        request
            .canonical_json()
            .expect("board request")
            .into_bytes(),
        wire::encode(&Vec::<Witness>::new()).expect("board witnesses"),
        serde_json::to_vec(&response).expect("legacy board response"),
        None,
    );
}

fn insert_legacy_effort_receipt(connection: &Connection) {
    let id = uuid("018f82d4-6f59-7d06-92e3-bce913116324");
    let epoch = uuid("018f82d4-6f59-7d06-92e3-bce913116325");
    let request = Command {
        v: WireVersion::CURRENT,
        id,
        epoch,
        kind: CommandKind::EffortRead,
        expected: Vec::new(),
        body: wire::intent::EffortRead {
            task: Some(HANDLE.parse().expect("fixture handle")),
        },
    };
    let response = Response {
        v: WireVersion::CURRENT,
        id,
        epoch,
        outcome: Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(Seq::new(2).expect("effort snapshot")),
            body: wire::intent::EffortAnswer {
                phases: vec![wire::intent::EffortPhaseAnswer {
                    task: STAMP.parse().expect("fixture stamp"),
                    phase: "todo".parse().expect("fixture phase"),
                    phase_index: 0,
                    opening: Seq::new(1).expect("opening fact"),
                    closing: None,
                    renewals: 0,
                    opening_boundary: wire::body::SourceBoundary::Proven { heads: Vec::new() },
                    closing_boundary: None,
                    segments: Vec::new(),
                    availability: wire::intent::EffortAvailability::Unavailable {
                        reasons: Vec::new(),
                    },
                }],
                aggregates: Vec::new(),
            },
        }),
    };
    let mut response = serde_json::to_value(response).expect("current effort response");
    response["outcome"]["body"]["phases"][0]["task"] = Value::String(HANDLE.to_owned());
    insert_receipt_row(
        connection,
        id,
        epoch,
        "effort::read",
        request
            .canonical_json()
            .expect("effort request")
            .into_bytes(),
        wire::encode(&Vec::<Witness>::new()).expect("effort witnesses"),
        serde_json::to_vec(&response).expect("legacy effort response"),
        None,
    );
}

#[allow(
    clippy::too_many_arguments,
    reason = "receipt fixture states every stored carrier"
)]
fn insert_receipt_row(
    connection: &Connection,
    id: CanonicalUuid,
    epoch: CanonicalUuid,
    kind: &str,
    request: Vec<u8>,
    witnesses: Vec<u8>,
    response: Vec<u8>,
    committed: Option<(i64, i64)>,
) {
    let (first, last, snapshot) = committed.map_or((None, None, Some(2)), |(first, last)| {
        (Some(first), Some(last), None)
    });
    connection
        .execute(
            "INSERT INTO command_receipts(command_id, actor, command_kind, request, \
             evaluated_witnesses, response, first_seq, last_seq, snapshot_seq, epoch) \
             VALUES (?1, 'operator', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                id.to_string(),
                kind,
                request,
                witnesses,
                response,
                first,
                last,
                snapshot,
                epoch.to_string(),
            ],
        )
        .expect("insert legacy receipt");
}

fn legacy_witness_values(witnesses: &mut [Value]) {
    for witness in witnesses {
        if witness.get("task").and_then(Value::as_str) == Some(STAMP) {
            witness["task"] = Value::String(HANDLE.to_owned());
        } else if witness.get("task").and_then(Value::as_str) == Some(FOLLOWUP_STAMP) {
            witness["task"] = Value::String(FOLLOWUP_HANDLE.to_owned());
        }
        if witness.get("after").and_then(Value::as_str) == Some(STAMP) {
            witness["after"] = Value::String(HANDLE.to_owned());
        }
    }
}

fn assert_review_receipt(connection: &Connection) {
    let (request, witnesses): (Vec<u8>, Vec<u8>) = connection
        .query_row(
            "SELECT request, evaluated_witnesses FROM command_receipts \
             WHERE command_kind = 'task::review'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read migrated review receipt");
    let request: Command<wire::intent::TaskReview> =
        wire::decode(&request).expect("migrated review request is current wire");
    assert_eq!(request.body.then[0].task.as_str(), FOLLOWUP_STAMP);
    let witnesses: Vec<Witness> =
        wire::decode(&witnesses).expect("migrated review witnesses are current wire");
    assert!(witnesses.iter().any(
        |witness| matches!(witness, Witness::Absence { task } if task.as_str() == FOLLOWUP_STAMP)
    ));
}

fn assert_board_receipt(connection: &Connection) {
    let response: Vec<u8> = connection
        .query_row(
            "SELECT response FROM command_receipts WHERE command_kind = 'board::read'",
            [],
            |row| row.get(0),
        )
        .expect("read migrated board response");
    let response: Response<wire::intent::BoardAnswer> =
        wire::decode(&response).expect("migrated board response is current wire");
    let Outcome::Reply(reply) = response.outcome else {
        panic!("board receipt must remain a reply");
    };
    let statement = &reply.body.tasks[0].statement.body;
    assert_eq!(statement.incepted.as_str(), STAMP);
    assert_eq!(statement.handle.as_str(), HANDLE);
    assert_eq!(statement.provenance.as_str(), format!("task:{STAMP}"));
}

fn assert_effort_receipt(connection: &Connection) {
    let response: Vec<u8> = connection
        .query_row(
            "SELECT response FROM command_receipts WHERE command_kind = 'effort::read'",
            [],
            |row| row.get(0),
        )
        .expect("read migrated effort response");
    let response: Response<wire::intent::EffortAnswer> =
        wire::decode(&response).expect("migrated effort response is current wire");
    let Outcome::Reply(reply) = response.outcome else {
        panic!("effort receipt must remain a reply");
    };
    assert_eq!(reply.body.phases[0].task.as_str(), STAMP);
}

fn task_key_column(connection: &Connection) -> String {
    connection
        .query_row(
            "SELECT name FROM pragma_table_info('task_heads') WHERE pk = 1",
            [],
            |row| row.get(0),
        )
        .expect("read task head primary key")
}

fn legacy_identity_state(connection: &Connection) -> (u32, u32, String, Vec<u8>, Vec<u8>) {
    (
        connection
            .query_row(
                "SELECT version FROM journal_schema WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .expect("schema row"),
        connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("schema pragma"),
        task_key_column(connection),
        connection
            .query_row(
                "SELECT envelope FROM fact_envelopes WHERE seq = 1",
                [],
                |row| row.get(0),
            )
            .expect("legacy fact bytes"),
        connection
            .query_row("SELECT body FROM task_heads", [], |row| row.get(0))
            .expect("legacy head bytes"),
    )
}
