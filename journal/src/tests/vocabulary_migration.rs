//! Schema 8 to 9 durable vocabulary migration.

use super::*;

use wire::body::TasteJudged;

const BOARD_COMMAND: &str = "018f82d4-6f59-7d06-92e3-bce913116330";

#[test]
fn schema_eight_vocabulary_moves_forward_once_across_every_authority_carrier() {
    let scratch = Scratch::new();
    let path = scratch.path().join("vocabulary.sqlite3");
    let journal = authority_file_fixture(&path);
    insert_taste_judgment(&journal);
    insert_board_receipt(&journal.connection);
    drop(journal);

    let connection = Connection::open(&path).expect("open schema-eight fixture");
    retire_task_head_key(&connection, false);
    retire_fact_key(&connection);
    retire_board_response_key(&connection);
    set_schema_eight(&connection);
    drop(connection);

    let journal = Journal::open(&path).expect("migrate schema-eight vocabulary");
    assert_eq!(
        journal.migration().map(|step| (step.from(), step.to())),
        Some((8, SCHEMA_VERSION))
    );
    assert_eq!(
        journal.schema_version().expect("schema version"),
        SCHEMA_VERSION,
        "the vocabulary rewrite continues through the source-locus boundary"
    );

    let head_bytes: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT body FROM task_heads WHERE stamp = '1kKRRsDP'",
            [],
            |row| row.get(0),
        )
        .expect("read migrated task head");
    assert_current_only(&head_bytes);
    let head: TaskHead = wire::decode(&head_bytes).expect("task head is current wire");
    assert_eq!(head.born.as_str(), "1kKRRsDP");

    let fact_bytes: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE seq = 3",
            [],
            |row| row.get(0),
        )
        .expect("read migrated taste judgment");
    assert_current_only(&fact_bytes);
    let envelope: Envelope<AnyFact> =
        wire::decode(&fact_bytes).expect("taste judgment is current wire");
    let judgment = envelope
        .fact
        .read::<TasteJudged>()
        .expect("fact remains a taste judgment");
    assert_eq!(judgment.vendor, "Wait for the terminal event.");

    let response_bytes: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT response FROM command_receipts WHERE command_id = ?1",
            [BOARD_COMMAND],
            |row| row.get(0),
        )
        .expect("read migrated board receipt");
    assert_current_only(&response_bytes);
    let response: Response<wire::intent::BoardAnswer> =
        wire::decode(&response_bytes).expect("board response is current wire");
    let Outcome::Reply(reply) = response.outcome else {
        panic!("board receipt remains a reply");
    };
    assert_eq!(reply.body.tasks[0].statement.body.born.as_str(), "1kKRRsDP");

    journal.verify().expect("migrated authority verifies");
}

#[test]
fn schema_eight_vocabulary_conflict_refuses_and_rolls_back() {
    let scratch = Scratch::new();
    let path = scratch.path().join("vocabulary-conflict.sqlite3");
    drop(authority_file_fixture(&path));

    let connection = Connection::open(&path).expect("open conflict fixture");
    let planted = retire_task_head_key(&connection, true);
    set_schema_eight(&connection);
    drop(connection);

    let error = Journal::open(&path).expect_err("dual vocabulary must be refused");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::StoredContent {
            from: 8,
            to: 9,
            table: "task_heads",
            ref detail,
            ..
        }) if detail.contains("both \"incepted\" and \"born\"")
    ));

    let connection = Connection::open(&path).expect("inspect rolled-back fixture");
    let version: u32 = connection
        .query_row(
            "SELECT version FROM journal_schema WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .expect("read rolled-back schema row");
    let pragma: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("read rolled-back schema pragma");
    let body: Vec<u8> = connection
        .query_row(
            "SELECT body FROM task_heads WHERE stamp = '1kKRRsDP'",
            [],
            |row| row.get(0),
        )
        .expect("read rolled-back task head");
    assert_eq!((version, pragma), (8, 8));
    assert_eq!(body, planted);
}

fn insert_taste_judgment(journal: &Journal) {
    insert_fact(
        journal,
        fact(
            3,
            Some(2),
            FactKind::TasteJudged,
            json!({
                "lane": "018f82d4-6f59-7d06-92e3-bce913116302",
                "occupant": "018f82d4-6f59-7d06-92e3-bce913116303",
                "compaction": 1,
                "taste": "wait",
                "content": digest("c"),
                "bag": digest("d"),
                "trigger": "polling",
                "vendor": "Wait for the terminal event.",
                "source": "Polling is not completion.",
                "locus": {
                    "member": 0,
                    "offset": 10,
                    "length": 32,
                    "event": 0,
                    "line": 1,
                    "column": 1
                },
                "attempts": [],
                "decision": {"verdict": "cleared"}
            }),
        ),
    );
    journal
        .connection
        .execute_batch(
            "INSERT INTO thread_index(seq, origin, cause, root, depth) \
             VALUES (3, 'task:1kKRRsDP', 2, 1, 2); \
             UPDATE cursor_head SET seq = 3 WHERE singleton = 1; \
             UPDATE steering_index SET indexed_seq = 3 WHERE singleton = 1;",
        )
        .expect("index taste judgment fixture");
}

fn insert_board_receipt(connection: &Connection) {
    let id = uuid(BOARD_COMMAND);
    let epoch = uuid("018f82d4-6f59-7d06-92e3-bce913116331");
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
        cursor: Seq::new(3).expect("board cursor"),
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
        recent_cell: None,
        trust: Vec::new(),
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
            position: ReplyPosition::Snapshot(Seq::new(3).expect("board snapshot")),
            body: board,
        }),
    };
    connection
        .execute(
            "INSERT INTO command_receipts(command_id, actor, command_kind, request, \
             evaluated_witnesses, response, first_seq, last_seq, snapshot_seq, epoch) \
             VALUES (?1, 'operator', 'board::read', ?2, ?3, ?4, NULL, NULL, 3, ?5)",
            params![
                id.to_string(),
                request
                    .canonical_json()
                    .expect("canonical board request")
                    .into_bytes(),
                wire::encode(&Vec::<Witness>::new()).expect("board witnesses"),
                wire::encode(&response).expect("board response"),
                epoch.to_string(),
            ],
        )
        .expect("insert board receipt");
}

fn retire_task_head_key(connection: &Connection, retain_current: bool) -> Vec<u8> {
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT body FROM task_heads WHERE stamp = '1kKRRsDP'",
            [],
            |row| row.get(0),
        )
        .expect("read current task head");
    let planted = if retain_current {
        let mut document: Value = serde_json::from_slice(&bytes).expect("task head JSON");
        let born = document["born"].clone();
        document
            .as_object_mut()
            .expect("task head object")
            .insert("incepted".to_owned(), born);
        serde_json::to_vec(&document).expect("conflicting task head JSON")
    } else {
        retired_key(bytes, "born", "incepted")
    };
    connection
        .execute(
            "UPDATE task_heads SET body = ?1 WHERE stamp = '1kKRRsDP'",
            [&planted],
        )
        .expect("plant schema-eight task head");
    planted
}

fn retire_fact_key(connection: &Connection) {
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE seq = 3",
            [],
            |row| row.get(0),
        )
        .expect("read current taste judgment");
    let retired = retired_key(bytes, "vendor", "driver");
    connection
        .execute(
            "UPDATE fact_envelopes SET envelope = ?1 WHERE seq = 3",
            [retired],
        )
        .expect("plant schema-eight taste judgment");
}

fn retire_board_response_key(connection: &Connection) {
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT response FROM command_receipts WHERE command_id = ?1",
            [BOARD_COMMAND],
            |row| row.get(0),
        )
        .expect("read current board response");
    let retired = retired_key(bytes, "born", "incepted");
    connection
        .execute(
            "UPDATE command_receipts SET response = ?1 WHERE command_id = ?2",
            params![retired, BOARD_COMMAND],
        )
        .expect("plant schema-eight board response");
}

fn retired_key(bytes: Vec<u8>, current: &str, retired: &str) -> Vec<u8> {
    let text = String::from_utf8(bytes).expect("wire JSON is UTF-8");
    let current = format!("\"{current}\":");
    assert!(text.contains(&current), "fixture carries {current}");
    text.replacen(&current, &format!("\"{retired}\":"), 1)
        .into_bytes()
}

fn set_schema_eight(connection: &Connection) {
    downgrade_steering_index_to_millis(connection);
    connection
        .execute_batch(
            "UPDATE journal_schema SET version = 8 WHERE singleton = 1; \
             PRAGMA user_version = 8;",
        )
        .expect("mark fixture as schema eight");
}

fn assert_current_only(bytes: &[u8]) {
    let text = std::str::from_utf8(bytes).expect("wire JSON is UTF-8");
    assert!(!text.contains("\"incepted\":"), "retired task key remains");
    assert!(!text.contains("\"driver\":"), "retired taste key remains");
}
