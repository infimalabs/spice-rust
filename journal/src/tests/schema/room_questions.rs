use super::*;

const CONTROL: &str = "018f82d4-6f59-7d06-92e3-bce913116310";
const CLOSE: &str = "018f82d4-6f59-7d06-92e3-bce913116311";
const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce9131163c3";
const QUESTION_PROTOCOL_PREDECESSOR: u32 = 86;

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
    restore_predecessor(&journal, QUESTION_PROTOCOL_PREDECESSOR);

    let migration = crate::migrate::admit(&mut journal.connection)
        .expect("predecessor crosses")
        .expect("crossing is reported");
    assert_eq!(
        (migration.from(), migration.to()),
        (QUESTION_PROTOCOL_PREDECESSOR, SCHEMA_VERSION)
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

#[test]
fn an_older_predecessor_with_question_authority_refuses_unchanged() {
    let mut journal = authority_fixture();
    let position = 3_i64;
    let envelope = wire::Envelope {
        v: WireVersion::CURRENT,
        seq: seq(u64::try_from(position).unwrap()),
        at: instant(),
        actor: Actor::Spice,
        origin: "task:1kKRRsDP".parse().unwrap(),
        cause: None,
        fact: json!({
            "kind": "room::conversed",
            "operation": {"verb": "ask", "result": "dVwVYC9yR"}
        }),
    };
    let envelope_bytes = wire::encode(&envelope).unwrap();
    journal
        .connection
        .execute(
            "INSERT INTO fact_envelopes \
             (seq, wire_version, instant, actor, origin, cause, kind, envelope) \
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, 'room::conversed', ?6)",
            params![
                position,
                envelope.v.0,
                envelope.at.to_string(),
                envelope.actor.to_string(),
                envelope.origin.as_str(),
                envelope_bytes,
            ],
        )
        .unwrap();
    let request = json!({
        "v": 1,
        "id": CLOSE,
        "epoch": EPOCH,
        "kind": "room::close",
        "expected": [],
        "body": {"result": "dVwVYC9yR", "clause": "retired"},
    });
    let response = Response {
        v: WireVersion::CURRENT,
        id: uuid(CLOSE),
        epoch: uuid(EPOCH),
        outcome: Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(1),
                last: seq(2),
            }),
            body: json!({"result": "dVwVYC9yR", "state": "declined"}),
        }),
    };
    journal
        .connection
        .execute(
            "INSERT INTO command_receipts \
             (command_id, actor, command_kind, request, evaluated_witnesses, response, \
              first_seq, last_seq, snapshot_seq, epoch) \
             VALUES (?1, 'operator', 'room::close', ?2, ?3, ?4, 1, 2, NULL, ?5)",
            params![
                CLOSE,
                serde_json::to_vec(&request).unwrap(),
                wire::encode(&Vec::<Witness>::new()).unwrap(),
                wire::encode(&response).unwrap(),
                EPOCH,
            ],
        )
        .unwrap();
    restore_predecessor(&journal, QUESTION_PROTOCOL_PREDECESSOR - 1);
    let before = retained_question_rows(&journal);

    let error = crate::migrate::admit(&mut journal.connection).unwrap_err();

    assert!(error.to_string().contains("963a54c37"), "{error}");
    assert_eq!(retained_question_rows(&journal), before);
}

fn retained_question_rows(journal: &Journal) -> (i64, String, Vec<u8>, String, Vec<u8>, Vec<u8>) {
    let version = journal
        .connection
        .query_row(
            "SELECT version FROM journal_schema WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let (kind, envelope) = journal
        .connection
        .query_row(
            "SELECT kind, envelope FROM fact_envelopes WHERE seq = 3",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let (command_kind, request, response) = journal
        .connection
        .query_row(
            "SELECT command_kind, request, response FROM command_receipts \
             WHERE command_id = ?1",
            [CLOSE],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    (version, kind, envelope, command_kind, request, response)
}
