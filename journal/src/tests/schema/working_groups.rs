//! The retired working-group brake crosses onto deferral.

use super::*;

const STANCE_COMMAND: &str = "018f82d4-6f59-7d06-92e3-bce913116321";
const ARRANGE_COMMAND: &str = "018f82d4-6f59-7d06-92e3-bce913116322";
const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce9131163c3";
const STOPPED: &str = "1kKRRsDR1";
const MOVING: &str = "1kKRRsDR2";

fn stamp(micros: i64) -> String {
    TaskStamp::for_micros(micros)
        .expect("fixture stamp")
        .to_string()
}

/// The open Design a stopping group holds.
fn held_design() -> String {
    stamp(3)
}

/// The open Design a running group holds.
fn moving_design() -> String {
    stamp(4)
}

/// A partition at `revision`: the stopping group holds the held Design and the
/// ordinary fixture task, and the running group holds the moving Design. A
/// `None` stance omits the member.
fn partition(revision: u64, stopped: Option<&str>, moving: Option<&str>) -> serde_json::Value {
    let group = |group: &str, anchors: Vec<String>, stance: Option<&str>| {
        let mut value = json!({"group": group, "anchors": anchors});
        if let Some(stance) = stance {
            value["stance"] = json!(stance);
        }
        value
    };
    json!({
        "revision": revision,
        "groups": [
            group(STOPPED, vec![held_design(), "1kKRRsDP".to_owned()], stopped),
            group(MOVING, vec![moving_design()], moving),
        ]
    })
}

/// Plant one retained `command` receipt whose request body is `body`.
fn plant_receipt(journal: &Journal, id: &str, command: &str, body: serde_json::Value) {
    let request = json!({
        "v": WireVersion::CURRENT,
        "id": id,
        "epoch": EPOCH,
        "kind": command,
        "expected": [],
        "body": body,
    });
    let response = Response {
        v: WireVersion::CURRENT,
        id: uuid(id),
        epoch: uuid(EPOCH),
        outcome: Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(6),
                last: seq(6),
            }),
            body: json!({"revision": 2, "status": "stanced", "owed": []}),
        }),
    };
    journal
        .connection
        .execute(
            "INSERT INTO command_receipts(
                 command_id, actor, command_kind, request, evaluated_witnesses, response,
                 first_seq, last_seq, snapshot_seq, epoch
             ) VALUES (?1, 'operator', ?2, ?3, ?4, ?5, 6, 6, NULL, ?6)",
            params![
                id,
                command,
                serde_json::to_vec(&request).expect("encode retained request"),
                wire::encode(&Vec::<Witness>::new()).expect("encode empty witnesses"),
                wire::encode(&response).expect("encode retained response"),
                EPOCH,
            ],
        )
        .expect("insert retained receipt");
}

/// Plant one open filed Design born at `at`.
fn plant_design(journal: &Journal, at: u64, design: &str) {
    insert_fact(
        journal,
        fact(
            at,
            None,
            FactKind::TaskAdded,
            json!({
                "task": design,
                "title": "Design fixture",
                "pitch": "A Design fixture needs a pitch.",
                "project": "journal",
                "intent": "design",
                "depends": [],
                "flow": ["design"],
                "phase": "design",
                "revision": 1,
                "creation": {"kind": "unavailable", "reason": "operator-actor"}
            }),
        ),
    );
    let statement = TaskHead {
        born: design.parse().expect("design stamp"),
        intent: Some(wire::TaskIntent::Design),
        title: "Design fixture".to_owned(),
        queue_origin: seq(at),
        revision: seq(at),
        ..task_head()
    };
    insert_head(
        &journal.connection,
        "task_heads",
        "stamp",
        design,
        at,
        &statement,
    );
    let workflow = WorkflowHead {
        flow: vec![phase("design")],
        phase: phase("design"),
        ..workflow_head()
    };
    insert_head(
        &journal.connection,
        "workflow_heads",
        "stamp",
        design,
        at,
        &workflow,
    );
}

/// A `version` authority whose projected partition holds one Design in a
/// group standing `stopped` and another in a Running group, with retained
/// stances in every partition fact and request spelling.
fn predecessor(path: &std::path::Path, version: u32, stopped: &str) -> Journal {
    let journal = authority_file_fixture(path);
    plant_design(&journal, 3, &held_design());
    plant_design(&journal, 4, &moving_design());
    let first = partition(1, None, None);
    insert_fact(
        &journal,
        fact(
            5,
            Some(3),
            FactKind::WorkingGroupMoved,
            json!({"design": held_design(), "to": STOPPED, "partition": first}),
        ),
    );
    insert_fact(
        &journal,
        fact(
            6,
            Some(5),
            FactKind::WorkingGroupsArranged,
            json!({"from": first, "partition": partition(2, None, None)}),
        ),
    );
    journal
        .connection
        .execute(
            "INSERT INTO group_partition(singleton, head_seq, body) VALUES (1, 6, ?1)",
            [serde_json::to_vec(&partition(2, Some(stopped), Some("running"))).unwrap()],
        )
        .unwrap();
    journal
        .connection
        .execute_batch(
            "INSERT INTO thread_index(seq, origin, cause, root, depth) VALUES
                 (3, 'task:1kKRRsDP', NULL, 3, 0),
                 (4, 'task:1kKRRsDP', NULL, 4, 0),
                 (5, 'task:1kKRRsDP', 3, 3, 1),
                 (6, 'task:1kKRRsDP', 5, 3, 2);
             UPDATE fact_envelopes SET envelope = CAST(json_set(CAST(envelope AS TEXT),
                 '$.fact.partition.groups[0].stance', 'converge') AS BLOB) WHERE seq = 5;
             UPDATE fact_envelopes SET envelope = CAST(json_set(CAST(envelope AS TEXT),
                 '$.fact.partition.groups[0].stance', 'paused',
                 '$.fact.partition.groups[1].stance', 'running') AS BLOB) WHERE seq = 6;
             UPDATE cursor_head SET seq = 6, wake = 6 WHERE singleton = 1;
             UPDATE steering_index SET indexed_seq = 6 WHERE singleton = 1;",
        )
        .unwrap();
    plant_receipt(
        &journal,
        STANCE_COMMAND,
        "group::stance",
        json!({"group": STOPPED, "stance": stopped}),
    );
    plant_receipt(
        &journal,
        ARRANGE_COMMAND,
        "group::arrange",
        json!({"groups": partition(2, Some("paused"), Some("running"))["groups"]}),
    );
    restore_predecessor(&journal, version);
    journal
}

fn envelope(journal: &Journal, seq: i64) -> serde_json::Value {
    let bytes: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE seq = ?1",
            [seq],
            |row| row.get(0),
        )
        .unwrap();
    let _: Envelope<AnyFact> = wire::decode(&bytes).expect("crossed fact is current");
    serde_json::from_slice(&bytes).unwrap()
}

fn workflow(journal: &Journal, task: &str) -> WorkflowHead {
    let bytes: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT body FROM workflow_heads WHERE stamp = ?1",
            [task],
            |row| row.get(0),
        )
        .unwrap();
    wire::decode(&bytes).expect("crossed workflow decodes")
}

fn receipt_kinds(journal: &Journal, table: &str, id: &str) -> Vec<String> {
    let mut statement = journal
        .connection
        .prepare(&format!(
            "SELECT command_kind FROM {table} WHERE command_id = ?1"
        ))
        .unwrap();
    statement
        .query_map([id], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn request_body(journal: &Journal, id: &str) -> serde_json::Value {
    let bytes: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT request FROM command_receipts WHERE command_id = ?1",
            [id],
            |row| row.get(0),
        )
        .unwrap();
    serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["body"].clone()
}

/// Open the `version` authority whose stopping group stands `stopped` and
/// prove the crossing deferred exactly its held open Design.
fn cross_and_prove_deferred(version: u32, stopped: &str) {
    let scratch = Scratch::new();
    let path = scratch.path().join("working-group-brake.sqlite3");
    drop(predecessor(&path, version, stopped));

    let journal = Journal::open(&path).expect("the retained brake crosses");
    assert_eq!(
        journal
            .migration()
            .map(|migration| (migration.from(), migration.to())),
        Some((version, SCHEMA_VERSION))
    );

    let held = workflow(&journal, &held_design());
    assert_eq!(held.deferral, Some(Instant::indefinite()));
    assert_eq!(held.generation, 1);
    let deferred = envelope(&journal, 7);
    assert_eq!(deferred["fact"]["kind"], "task::deferred");
    assert_eq!(deferred["fact"]["task"], json!(held_design()));
    assert_eq!(workflow(&journal, &moving_design()).deferral, None);
    assert_eq!(
        workflow(&journal, "1kKRRsDP").deferral,
        None,
        "a stopped group's ordinary anchor is no Design to defer"
    );

    assert_eq!(
        envelope(&journal, 5)["fact"]["partition"],
        partition(1, None, None)
    );
    let arranged = envelope(&journal, 6);
    assert_eq!(arranged["fact"]["from"], partition(1, None, None));
    assert_eq!(arranged["fact"]["partition"], partition(2, None, None));
    let head = journal
        .read(|read| read.group_partition())
        .expect("partition reads")
        .expect("partition stands")
        .into_body();
    assert_eq!(head.groups.len(), 2);
    assert_eq!(
        request_body(&journal, ARRANGE_COMMAND)["groups"],
        partition(2, None, None)["groups"]
    );
    assert!(receipt_kinds(&journal, "command_receipts", STANCE_COMMAND).is_empty());
    assert_eq!(
        receipt_kinds(&journal, "historical_command_receipts", STANCE_COMMAND),
        vec!["group::stance".to_owned()]
    );
    journal.verify().expect("crossed authority verifies");
    drop(journal);

    let reopened = Journal::open(&path).expect("crossed authority reopens");
    assert_eq!(reopened.migration(), None, "the crossing runs exactly once");
}

#[test]
fn a_paused_group_defers_its_open_design_and_every_stance_drops() {
    cross_and_prove_deferred(100, "paused");
}

#[test]
fn a_passive_group_defers_its_open_design_and_every_stance_drops() {
    cross_and_prove_deferred(99, "passive");
}

#[test]
fn a_design_already_deferred_keeps_its_own_deferral() {
    let scratch = Scratch::new();
    let path = scratch.path().join("working-group-deferred.sqlite3");
    let journal = predecessor(&path, 100, "paused");
    let until: Instant = "2031-01-01T00:00:00.000Z".parse().unwrap();
    let mut held = workflow(&journal, &held_design());
    held.deferral = Some(until);
    journal
        .connection
        .execute(
            "UPDATE workflow_heads SET body = ?1 WHERE stamp = ?2",
            params![wire::encode(&held).unwrap(), held_design()],
        )
        .unwrap();
    drop(journal);

    let journal = Journal::open(&path).expect("the retained brake crosses");
    assert_eq!(workflow(&journal, &held_design()), held);
    let last: i64 = journal
        .connection
        .query_row("SELECT max(seq) FROM fact_envelopes", [], |row| row.get(0))
        .unwrap();
    assert_eq!(last, 6, "no deferral is appended over a standing one");
    journal.verify().expect("crossed authority verifies");
}
