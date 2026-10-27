//! Retained working-group stances cross onto the Paused/Running brake.

use super::*;

const STANCE_COMMAND: &str = "018f82d4-6f59-7d06-92e3-bce913116321";
const ARRANGE_COMMAND: &str = "018f82d4-6f59-7d06-92e3-bce913116322";
const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce9131163c3";

fn partition(revision: u64, stance: &str) -> serde_json::Value {
    json!({
        "revision": revision,
        "groups": [{"group": "1kKRRsDR1", "anchors": ["1kKRRsDP"], "stance": stance}]
    })
}

/// Plant one retained `command` receipt whose request body is `body`.
fn plant_receipt(journal: &Journal, id: &str, command: CommandKind, body: serde_json::Value) {
    let command = Command {
        v: WireVersion::CURRENT,
        id: uuid(id),
        epoch: uuid(EPOCH),
        kind: command,
        expected: Vec::new(),
        body,
    };
    let response = Response {
        v: WireVersion::CURRENT,
        id: command.id,
        epoch: command.epoch,
        outcome: Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(4),
                last: seq(4),
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
             ) VALUES (?1, 'operator', ?2, ?3, ?4, ?5, 4, 4, NULL, ?6)",
            params![
                command.id.to_string(),
                command.kind.as_str(),
                command
                    .canonical_json()
                    .expect("canonical request")
                    .into_bytes(),
                wire::encode(&Vec::<Witness>::new()).expect("encode empty witnesses"),
                wire::encode(&response).expect("encode retained response"),
                command.epoch.to_string(),
            ],
        )
        .expect("insert retained receipt");
}

/// A schema-99 authority holding every retired stance: a Converge move, an
/// arrangement from a group whose stance predates schema 66 to a Passive one,
/// and Converge and Explore requests.
fn predecessor(path: &std::path::Path) -> Journal {
    let journal = authority_file_fixture(path);
    let running = partition(1, "running");
    insert_fact(
        &journal,
        fact(
            3,
            Some(2),
            FactKind::WorkingGroupMoved,
            json!({"design": "1kKRRsDP", "to": "1kKRRsDR1", "partition": running}),
        ),
    );
    insert_fact(
        &journal,
        fact(
            4,
            Some(3),
            FactKind::WorkingGroupsArranged,
            json!({"from": running, "partition": partition(2, "paused")}),
        ),
    );
    journal
        .connection
        .execute(
            "INSERT INTO group_partition(singleton, head_seq, body) VALUES (1, 4, ?1)",
            [serde_json::to_vec(&partition(2, "passive")).unwrap()],
        )
        .unwrap();
    journal
        .connection
        .execute_batch(
            "UPDATE fact_envelopes SET envelope = CAST(json_set(CAST(envelope AS TEXT),
                 '$.fact.partition.groups[0].stance', 'converge') AS BLOB) WHERE seq = 3;
             UPDATE fact_envelopes SET envelope = CAST(json_remove(json_set(
                 CAST(envelope AS TEXT), '$.fact.partition.groups[0].stance', 'passive'),
                 '$.fact.from.groups[0].stance') AS BLOB) WHERE seq = 4;
             INSERT INTO thread_index(seq, origin, cause, root, depth) VALUES
                 (3, 'task:1kKRRsDP', 2, 1, 2),
                 (4, 'task:1kKRRsDP', 3, 1, 3);
             UPDATE cursor_head SET seq = 4, wake = 4 WHERE singleton = 1;
             UPDATE steering_index SET indexed_seq = 4 WHERE singleton = 1;",
        )
        .unwrap();
    plant_receipt(
        &journal,
        STANCE_COMMAND,
        CommandKind::WorkingGroupStance,
        json!({"group": "1kKRRsDR1", "stance": "converge"}),
    );
    plant_receipt(
        &journal,
        ARRANGE_COMMAND,
        CommandKind::WorkingGroupArrange,
        json!({"groups": partition(2, "explore")["groups"]}),
    );
    restore_predecessor(&journal, 99);
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

#[test]
fn writable_open_reads_passive_as_paused_and_every_other_stance_as_running() {
    let scratch = Scratch::new();
    let path = scratch.path().join("working-group-brake.sqlite3");
    drop(predecessor(&path));

    let journal = Journal::open(&path).expect("schema-99 working groups cross");
    assert_eq!(
        journal
            .migration()
            .map(|migration| (migration.from(), migration.to())),
        Some((99, SCHEMA_VERSION))
    );
    let moved = envelope(&journal, 3);
    assert_eq!(moved["fact"]["partition"], partition(1, "running"));
    let arranged = envelope(&journal, 4);
    assert_eq!(arranged["fact"]["from"], partition(1, "running"));
    assert_eq!(arranged["fact"]["partition"], partition(2, "paused"));
    let head = journal
        .read(|read| read.group_partition())
        .expect("partition reads")
        .expect("partition stands")
        .into_body();
    assert_eq!(
        head.groups[0].stance,
        wire::rooms::WorkingGroupStance::Paused
    );
    assert_eq!(
        request_body(&journal, STANCE_COMMAND),
        json!({"group": "1kKRRsDR1", "stance": "running"})
    );
    assert_eq!(
        request_body(&journal, ARRANGE_COMMAND)["groups"],
        partition(2, "running")["groups"]
    );
    journal.verify().expect("crossed authority verifies");
    drop(journal);

    let reopened = Journal::open(&path).expect("crossed authority reopens");
    assert_eq!(reopened.migration(), None, "the crossing runs exactly once");
}

#[test]
fn an_unknown_retained_stance_refuses_the_whole_crossing() {
    let scratch = Scratch::new();
    let path = scratch.path().join("working-group-unknown.sqlite3");
    let journal = predecessor(&path);
    journal
        .connection
        .execute_batch(
            "UPDATE group_partition SET body = CAST(json_set(CAST(body AS TEXT),
             '$.groups[0].stance', 'hold') AS BLOB);",
        )
        .unwrap();
    drop(journal);
    let before = fs::read(&path).expect("authority bytes read");

    let error = Journal::open(&path).expect_err("an unknown stance refuses");
    assert!(matches!(error, OpenError::Migration(_)), "{error}");
    assert_eq!(fs::read(&path).expect("authority bytes reread"), before);
}
