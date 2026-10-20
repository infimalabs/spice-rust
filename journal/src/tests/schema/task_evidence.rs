//! Retained required question-result evidence crosses out of task statements,
//! its derived index, and live receipt authority exactly once.

use rusqlite::OptionalExtension;

use super::*;

const EDIT_COMMAND: &str = "018f82d4-6f59-7d06-92e3-bce913116311";
const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce9131163c3";
const RESULT: &str = "1kKRRsDR4";

/// The retained edit fact and statement head, each in its canonical current
/// encoding.
fn evidence_rows(journal: &Journal) -> (Vec<u8>, Vec<u8>) {
    let row = |sql: &str| {
        journal
            .connection
            .query_row(sql, [], |row| row.get::<_, Vec<u8>>(0))
            .expect("evidence-bearing row reads")
    };
    (
        row("SELECT envelope FROM fact_envelopes WHERE seq = 3"),
        row("SELECT body FROM task_heads WHERE stamp = '1kKRRsDP'"),
    )
}

fn receipt_request(journal: &Journal, table: &str) -> Option<Vec<u8>> {
    journal
        .connection
        .query_row(
            &format!("SELECT request FROM {table} WHERE command_id = ?1"),
            [EDIT_COMMAND],
            |row| row.get(0),
        )
        .optional()
        .expect("receipt table reads")
}

/// A schema-86 authority whose statement was edited to require `edit`, with
/// the canonical rows before evidence was planted and the planted request.
fn retained_evidence_fixture(
    path: &Path,
    edit: serde_json::Value,
) -> ((Vec<u8>, Vec<u8>), Vec<u8>) {
    let journal = authority_file_fixture(path);
    insert_fact(
        &journal,
        fact(
            3,
            Some(2),
            FactKind::TaskEdited,
            json!({"task": "1kKRRsDP", "revision": 3}),
        ),
    );
    journal
        .connection
        .execute_batch(
            "INSERT INTO thread_index(seq, origin, cause, root, depth)
             VALUES (3, 'task:1kKRRsDP', 2, 1, 2);
             UPDATE cursor_head SET seq = 3, wake = 3 WHERE singleton = 1;
             UPDATE steering_index SET indexed_seq = 3 WHERE singleton = 1;",
        )
        .expect("edit fact thread is planted");
    let canonical = evidence_rows(&journal);
    journal
        .connection
        .execute(
            "UPDATE fact_envelopes SET envelope = CAST(json_set(CAST(envelope AS TEXT), \
             '$.fact.evidence', json(?1)) AS BLOB) WHERE seq = 3",
            [edit.to_string()],
        )
        .expect("edit evidence is planted");
    journal
        .connection
        .execute(
            "UPDATE task_heads SET body = CAST(json_set(CAST(body AS TEXT), \
             '$.evidence', json(?1)) AS BLOB) WHERE stamp = '1kKRRsDP'",
            [json!([RESULT]).to_string()],
        )
        .expect("statement evidence is planted");
    let witnesses: Vec<Witness> =
        serde_json::from_value(json!([{"kind": "task", "task": "1kKRRsDP", "head": 1}]))
            .expect("task witness");
    let command = Command {
        v: WireVersion::CURRENT,
        id: uuid(EDIT_COMMAND),
        epoch: uuid(EPOCH),
        kind: CommandKind::TaskEdit,
        expected: witnesses.clone(),
        body: json!({"task": "JOURNAL-1kKRRsDP", "evidence": edit}),
    };
    let request = command
        .canonical_json()
        .expect("canonical request")
        .into_bytes();
    let response = Response {
        v: WireVersion::CURRENT,
        id: command.id,
        epoch: command.epoch,
        outcome: Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(3),
                last: seq(3),
            }),
            body: json!({"task": "JOURNAL-1kKRRsDP", "status": "edited", "next": "spice task done"}),
        }),
    };
    journal
        .connection
        .execute(
            "INSERT INTO command_receipts(command_id, actor, command_kind, request, \
             evaluated_witnesses, response, first_seq, last_seq, snapshot_seq, epoch) \
             VALUES (?1, 'operator', 'task::edit', ?2, ?3, ?4, 3, 3, NULL, ?5)",
            params![
                EDIT_COMMAND,
                request,
                wire::encode(&witnesses).expect("encode witnesses"),
                wire::encode(&response).expect("encode response"),
                EPOCH,
            ],
        )
        .expect("evidence edit receipt is planted");
    restore_predecessor(&journal, 86);
    journal
        .connection
        .execute(
            "INSERT INTO task_evidence(task, result, head_seq) VALUES ('1kKRRsDP', ?1, 3)",
            [RESULT],
        )
        .expect("derived evidence row is planted");
    drop(journal);
    (canonical, request)
}

#[test]
fn writable_open_retires_required_evidence_from_statements_index_and_receipts_once() {
    let scratch = Scratch::new();
    let path = scratch.path().join("retained-evidence.sqlite3");
    let (canonical, request) = retained_evidence_fixture(
        &path,
        json!({"action": "replace", "result": RESULT, "with": "1kKRRsDR5"}),
    );

    let journal = Journal::open(&path).expect("retained evidence crosses on writable open");
    assert_eq!(
        journal
            .migration()
            .map(|migration| (migration.from(), migration.to())),
        Some((86, SCHEMA_VERSION)),
    );
    assert_eq!(
        evidence_rows(&journal),
        canonical,
        "the edit and statement keep every other field in their canonical encoding",
    );
    assert_eq!(receipt_request(&journal, "command_receipts"), None);
    assert_eq!(
        receipt_request(&journal, "historical_command_receipts"),
        Some(request),
        "the receipt moves into historical authority byte for byte",
    );
    assert_eq!(
        authority_tables(&journal),
        expected_tables(),
        "the derived evidence index is gone",
    );
    journal.verify().expect("crossed authority verifies");
    assert!(
        journal
            .connection
            .execute(
                "INSERT INTO command_receipts SELECT * FROM historical_command_receipts \
                 WHERE command_id = ?1",
                [EDIT_COMMAND],
            )
            .is_err(),
        "the retired command identity can never execute again",
    );
    drop(journal);

    let reopened = Journal::open(&path).expect("crossed authority reopens");
    assert_eq!(reopened.migration(), None, "the crossing runs exactly once");
    assert_eq!(evidence_rows(&reopened), canonical);
}

#[test]
fn an_unknown_evidence_change_refuses_naming_its_release_without_mutation() {
    let scratch = Scratch::new();
    let path = scratch.path().join("foreign-evidence.sqlite3");
    retained_evidence_fixture(&path, json!({"action": "accept", "result": RESULT}));
    let before = fs::read(&path).expect("authority bytes read");

    let error = Journal::open(&path).expect_err("an unknown evidence change refuses");
    assert!(
        matches!(
            &error,
            OpenError::Migration(MigrationError::RetainedRow {
                table: "fact_envelopes",
                row,
                ..
            }) if row == "3"
        ),
        "{error}"
    );
    assert!(error.to_string().contains("963a54c37"), "{error}");
    assert_eq!(fs::read(&path).expect("authority bytes reread"), before);
}
