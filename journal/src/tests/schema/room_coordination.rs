//! A predecessor holding room coordination crosses into inert evidence.

use super::*;

const COORDINATE: &str = "018f82d4-6f59-7d06-92e3-bce913116320";
const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce9131163c3";
const COORDINATION_PREDECESSOR: u32 = 96;
const RELEASE: &str = "54e357267";

pub(super) fn plant_fact(journal: &Journal, position: i64, kind: &str, fact: serde_json::Value) {
    let envelope = wire::Envelope {
        v: WireVersion::CURRENT,
        seq: seq(u64::try_from(position).unwrap()),
        at: instant(),
        actor: Actor::Operator,
        origin: "task:1kKRRsDP".parse().unwrap(),
        cause: Some(seq(2)),
        fact,
    };
    journal
        .connection
        .execute(
            "INSERT INTO fact_envelopes \
             (seq, wire_version, instant, actor, origin, cause, kind, envelope) \
             VALUES (?1, ?2, ?3, ?4, ?5, 2, ?6, ?7)",
            params![
                position,
                envelope.v.0,
                envelope.at.to_string(),
                envelope.actor.to_string(),
                envelope.origin.as_str(),
                kind,
                wire::encode(&envelope).unwrap(),
            ],
        )
        .unwrap();
    journal
        .connection
        .execute(
            "INSERT INTO thread_index(seq, origin, cause, root, depth) \
             VALUES (?1, 'task:1kKRRsDP', 2, 1, 2)",
            [position],
        )
        .unwrap();
}

/// Plant the committed `room::coordinate` receipt that answered position 3.
fn plant_coordinate_receipt(journal: &Journal) {
    let request = json!({
        "v": 1,
        "id": COORDINATE,
        "epoch": EPOCH,
        "kind": "room::coordinate",
        "expected": [],
        "body": {"room": "1kKRRsDP", "target": "6SNy", "text": "start"},
    });
    let response = Response {
        v: WireVersion::CURRENT,
        id: uuid(COORDINATE),
        epoch: uuid(EPOCH),
        outcome: Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(3),
                last: seq(3),
            }),
            body: json!({"room": "1kKRRsDP", "key": "dVsW5mRxg"}),
        }),
    };
    journal
        .connection
        .execute(
            "INSERT INTO command_receipts \
             (command_id, actor, command_kind, request, evaluated_witnesses, response, \
              first_seq, last_seq, snapshot_seq, epoch) \
             VALUES (?1, 'operator', 'room::coordinate', ?2, ?3, ?4, 3, 3, NULL, ?5)",
            params![
                COORDINATE,
                serde_json::to_vec(&request).unwrap(),
                wire::encode(&Vec::<Witness>::new()).unwrap(),
                wire::encode(&response).unwrap(),
                EPOCH,
            ],
        )
        .unwrap();
}

#[test]
fn retired_coordination_crosses_into_inert_evidence_and_leaves_room_history_once() {
    let scratch = Scratch::new();
    let path = scratch.path().join("room-coordination.sqlite3");
    let journal = authority_file_fixture(&path);
    let directed = json!({
        "kind": "room::directed",
        "room": "1kKRRsDP",
        "publication": "1760000000000000",
        "key": "dVsW5mRxg",
        "target": "6SNy",
    });
    let shelved = json!({
        "kind": "room::shelved",
        "room": "1kKRRsDP",
        "publication": "1760000000000001",
        "canceled": ["dVsW5mRxg"],
    });
    plant_fact(&journal, 3, "room::directed", directed.clone());
    plant_fact(&journal, 4, "room::shelved", shelved.clone());
    plant_coordinate_receipt(&journal);
    journal
        .connection
        .execute_batch(
            "INSERT INTO room_publications(room, publication, seq) \
             VALUES ('1kKRRsDP', '1760000000000000', 3), ('1kKRRsDP', '1760000000000001', 4);
             CREATE TABLE room_directions (
                 room TEXT PRIMARY KEY,
                 head_seq INTEGER NOT NULL UNIQUE REFERENCES fact_envelopes(seq),
                 body BLOB NOT NULL
             ) STRICT;
             CREATE TABLE room_shelvings (
                 room TEXT PRIMARY KEY,
                 head_seq INTEGER NOT NULL UNIQUE REFERENCES fact_envelopes(seq),
                 body BLOB NOT NULL
             ) STRICT;
             INSERT INTO room_directions(room, head_seq, body) VALUES ('1kKRRsDP', 3, X'7b7d');
             INSERT INTO room_shelvings(room, head_seq, body) VALUES ('1kKRRsDP', 4, X'7b7d');
             UPDATE cursor_head SET seq = 4, wake = 4 WHERE singleton = 1;
             UPDATE steering_index SET indexed_seq = 4 WHERE singleton = 1;",
        )
        .unwrap();
    restore_predecessor(&journal, COORDINATION_PREDECESSOR);
    drop(journal);

    let journal = Journal::open(&path).expect("coordination authority crosses on writable open");
    assert_eq!(
        journal
            .migration()
            .map(|migration| (migration.from(), migration.to())),
        Some((COORDINATION_PREDECESSOR, SCHEMA_VERSION)),
    );
    let snapshot = journal.fact_snapshot().expect("crossed facts read");
    for (index, original) in [(2, &directed), (3, &shelved)] {
        let retained = snapshot.facts()[index]
            .fact
            .read::<wire::body::JournalRetained>()
            .expect("coordination fact became inert evidence");
        assert_eq!(retained.release, RELEASE);
        assert_eq!(&retained.fact, original);
    }
    let room: TaskStamp = "1kKRRsDP".parse().unwrap();
    let latest = journal
        .read(|read| read.latest_room_publication(&room, &Actor::Operator))
        .expect("room publications read");
    assert!(
        latest.is_none(),
        "room history still indexes a retired coordination row: {latest:?}"
    );
    let historical: bool = journal
        .connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM historical_command_receipts WHERE command_id = ?1) \
             AND NOT EXISTS(SELECT 1 FROM command_receipts WHERE command_id = ?1)",
            [COORDINATE],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        historical,
        "the coordinate receipt moves to historical authority"
    );
    let tables = authority_tables(&journal);
    assert!(!tables.contains("room_directions") && !tables.contains("room_shelvings"));
    journal.verify().expect("crossed authority verifies");
    drop(journal);

    let journal = Journal::open(&path).expect("crossed authority reopens");
    assert!(journal.migration().is_none(), "the crossing runs once");
}
