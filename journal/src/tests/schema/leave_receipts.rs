//! A retained leave answer crosses without its retired repeat-leave flag or
//! the retired duties it kept.

use super::*;

const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce9131163c3";
const LEAVE_COMMAND: &str = "018f82d4-6f59-7d06-92e3-bce913116311";

#[test]
fn writable_open_drops_the_repeat_leave_flag_and_retired_duties_from_a_retained_leave_answer() {
    let scratch = Scratch::new();
    let path = scratch.path().join("leave-receipts.sqlite3");
    let journal = authority_file_fixture(&path);
    let born: wire::TaskStamp = "1kKRRsDP".parse().expect("room");
    let room = wire::TaskHandle::for_project("spice.rooms", &born);
    let answer = wire::intent::RoomLeaveAnswer {
        room: room.clone(),
        publication: wire::rooms::RoomAuthorityStamp::for_micros(1_760_000_000_000_000)
            .expect("fixture publication"),
        kept: vec![wire::intent::RoomListeningDuty::Stop {
            key: "dWJyMVkz4".to_owned(),
        }],
    };
    let mut retained = serde_json::to_value(&answer).expect("answer encodes");
    retained["recorded"] = json!(true);
    retained["kept"] = json!([
        {"duty": "result", "key": "dWJyMVkz3"},
        {"duty": "stop", "key": "dWJyMVkz4"},
        {"duty": "receipt", "key": "dWJyMVkz5"},
    ]);
    let command = Command {
        v: WireVersion::CURRENT,
        id: uuid(LEAVE_COMMAND),
        epoch: uuid(EPOCH),
        kind: CommandKind::RoomLeave,
        expected: Vec::new(),
        body: json!({"room": room, "clause": "not my problem space"}),
    };
    let response = Response {
        v: WireVersion::CURRENT,
        id: command.id,
        epoch: command.epoch,
        outcome: Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(2),
                last: seq(2),
            }),
            body: retained,
        }),
    };
    journal
        .connection
        .execute(
            "INSERT INTO command_receipts(
                 command_id, actor, command_kind, request, evaluated_witnesses, response,
                 first_seq, last_seq, snapshot_seq, epoch
             ) VALUES (?1, 'operator', 'room::leave', ?2, ?3, ?4, 2, 2, NULL, ?5)",
            params![
                command.id.to_string(),
                command
                    .canonical_json()
                    .expect("canonical leave request")
                    .into_bytes(),
                wire::encode(&Vec::<Witness>::new()).expect("encode empty witnesses"),
                wire::encode(&response).expect("encode retained leave response"),
                command.epoch.to_string(),
            ],
        )
        .expect("insert retained leave receipt");
    restore_predecessor(&journal, 94);
    drop(journal);

    let crossed = Journal::open(&path).expect("schema-94 leave receipts cross");
    let stored = crossed
        .connection
        .query_row(
            "SELECT response FROM command_receipts WHERE command_id = ?1",
            [LEAVE_COMMAND],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .expect("crossed receipt reads");
    let response: Response<serde_json::Value> =
        wire::decode(&stored).expect("crossed response decodes");
    let Outcome::Reply(reply) = response.outcome else {
        panic!("a retained leave keeps its reply");
    };
    assert_eq!(
        serde_json::from_value::<wire::intent::RoomLeaveAnswer>(reply.body)
            .expect("the crossed answer is current"),
        answer
    );
}
