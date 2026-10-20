use super::*;

const CONTROL: &str = "018f82d4-6f59-7d06-92e3-bce913116310";
const CLOSE: &str = "018f82d4-6f59-7d06-92e3-bce913116311";
const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce9131163c3";

#[test]
fn retired_question_receipts_cross_once_and_reserve_their_identities() {
    let mut journal = authority_fixture();
    for (id, kind, body, answer) in [
        (
            CONTROL,
            "room::control",
            json!({"control": "answer", "result": "dVwVYC9yR", "text": "done"}),
            json!({"answer": "answered", "items": [], "results": ["dVwVYC9yR"], "returned": "published"}),
        ),
        (
            CLOSE,
            "room::close",
            json!({"result": "dVwVYC9yR", "clause": "retired"}),
            json!({"result": "dVwVYC9yR", "state": "declined"}),
        ),
    ] {
        let request = json!({
            "v": 1,
            "id": id,
            "epoch": EPOCH,
            "kind": kind,
            "expected": [],
            "body": body,
        });
        let response = Response {
            v: WireVersion::CURRENT,
            id: uuid(id),
            epoch: uuid(EPOCH),
            outcome: Outcome::Reply(Reply {
                position: ReplyPosition::Committed(SeqRange {
                    first: seq(1),
                    last: seq(2),
                }),
                body: answer,
            }),
        };
        journal
            .connection
            .execute(
                "INSERT INTO command_receipts \
                 (command_id, actor, command_kind, request, evaluated_witnesses, response, \
                  first_seq, last_seq, snapshot_seq, epoch) \
                 VALUES (?1, 'operator', ?2, ?3, ?4, ?5, 1, 2, NULL, ?6)",
                params![
                    id,
                    kind,
                    serde_json::to_vec(&request).unwrap(),
                    wire::encode(&Vec::<Witness>::new()).unwrap(),
                    wire::encode(&response).unwrap(),
                    EPOCH,
                ],
            )
            .unwrap();
    }
    journal
        .connection
        .execute_batch(
            "CREATE TABLE room_results (
                 result TEXT PRIMARY KEY,
                 head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
                 body BLOB NOT NULL
             ) STRICT;
             CREATE TABLE task_evidence (
                 task TEXT NOT NULL,
                 result TEXT NOT NULL,
                 head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
                 PRIMARY KEY (task, result)
             ) STRICT;",
        )
        .unwrap();
    restore_predecessor(&journal, SCHEMA_VERSION - 1);

    let migration = crate::migrate::admit(&mut journal.connection)
        .expect("predecessor crosses")
        .expect("crossing is reported");
    assert_eq!(
        (migration.from(), migration.to()),
        (SCHEMA_VERSION - 1, SCHEMA_VERSION)
    );
    journal
        .verify()
        .expect("retired receipts remain verifiable");
    for id in [CONTROL, CLOSE] {
        let historical: bool = journal
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM historical_command_receipts WHERE command_id = ?1)",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(historical);
        assert!(
            journal
                .connection
                .execute(
                    "INSERT INTO command_receipts SELECT * FROM historical_command_receipts \
                     WHERE command_id = ?1",
                    [id]
                )
                .is_err(),
            "retired identity {id} must not become executable again"
        );
    }
}
