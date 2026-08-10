//! Schema-8 stored spellings cross once into the current public vocabulary.

use super::*;

use wire::body::TasteJudged;

#[test]
fn schema_eight_moves_task_taste_and_receipt_fields_in_one_transaction() {
    let scratch = Scratch::new();
    let path = scratch.path().join("public-vocabulary.sqlite3");
    let journal = authority_file_fixture(&path);

    insert_fact(
        &journal,
        fact(
            3,
            None,
            FactKind::TasteJudged,
            json!({
                "lane": "018f82d4-6f59-7d06-92e3-bce9131163d1",
                "occupant": "018f82d4-6f59-7d06-92e3-bce9131163d2",
                "compaction": 1,
                "taste": "one path",
                "content": "a".repeat(64),
                "bag": "b".repeat(64),
                "trigger": "fallback",
                "vendor": "Keep one path.",
                "source": "A fallback appeared.",
                "locus": {
                    "member": 1,
                    "offset": 2,
                    "length": 3,
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
        .execute(
            "INSERT INTO thread_index(seq, origin, cause, root, depth) \
             VALUES (3, 'task:1kKRRsDP', NULL, 3, 0)",
            [],
        )
        .expect("index the planted taste fact");

    rewrite_object_key(
        &journal.connection,
        "SELECT body FROM task_heads WHERE stamp = '1kKRRsDP'",
        "UPDATE task_heads SET body = ?1 WHERE stamp = '1kKRRsDP'",
        "born",
        "incepted",
    );
    rewrite_nested_fact_key(&journal.connection, 3, "vendor", "driver");
    plant_legacy_receipt_fields(&journal.connection);
    journal
        .connection
        .execute_batch(
            "UPDATE cursor_head SET seq = 3 WHERE singleton = 1;
             UPDATE steering_index SET indexed_seq = 3 WHERE singleton = 1;
             UPDATE journal_schema SET version = 8 WHERE singleton = 1;
             PRAGMA user_version = 8;",
        )
        .expect("present the current authority as exact schema 8");
    drop(journal);

    let journal = Journal::open(&path).expect("schema-8 authority migrates");
    assert_eq!(
        journal
            .migration()
            .map(|migration| (migration.from(), migration.to())),
        Some((8, SCHEMA_VERSION))
    );

    let task: TaskHead = wire::decode(&read_blob(
        &journal.connection,
        "SELECT body FROM task_heads WHERE stamp = '1kKRRsDP'",
    ))
    .expect("migrated task head is current wire");
    assert_eq!(task.born.as_str(), "1kKRRsDP");

    let snapshot = journal.fact_snapshot().expect("migrated facts read");
    let taste = snapshot
        .facts()
        .iter()
        .find_map(|envelope| envelope.fact.read::<TasteJudged>())
        .expect("migrated taste fact is current wire");
    assert_eq!(taste.vendor, "Keep one path.");

    let response: Value = serde_json::from_slice(&read_blob(
        &journal.connection,
        "SELECT response FROM command_receipts",
    ))
    .expect("migrated receipt response is JSON");
    let body = response
        .pointer("/outcome/body")
        .and_then(Value::as_object)
        .expect("receipt reply body");
    assert_eq!(body.get("born"), Some(&json!("1kKRRsDP")));
    assert_eq!(body.get("vendor"), Some(&json!("Keep one path.")));
    assert!(!body.contains_key("incepted"));
    assert!(!body.contains_key("driver"));

    journal.verify().expect("migrated authority verifies");
}

fn rewrite_object_key(
    connection: &Connection,
    select: &str,
    update: &str,
    current: &str,
    legacy: &str,
) {
    let mut document: Value =
        serde_json::from_slice(&read_blob(connection, select)).expect("stored body is JSON");
    let object = document.as_object_mut().expect("stored body is an object");
    let value = object.remove(current).expect("current key exists");
    assert!(object.insert(legacy.to_owned(), value).is_none());
    connection
        .execute(
            update,
            params![serde_json::to_vec(&document).expect("legacy body encodes")],
        )
        .expect("store schema-8 body");
}

fn rewrite_nested_fact_key(connection: &Connection, seq: i64, current: &str, legacy: &str) {
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE seq = ?1",
            params![seq],
            |row| row.get(0),
        )
        .expect("read fact envelope");
    let mut document: Value = serde_json::from_slice(&bytes).expect("fact envelope is JSON");
    let fact = document
        .get_mut("fact")
        .and_then(Value::as_object_mut)
        .expect("fact body is an object");
    let value = fact.remove(current).expect("current fact key exists");
    assert!(fact.insert(legacy.to_owned(), value).is_none());
    connection
        .execute(
            "UPDATE fact_envelopes SET envelope = ?1 WHERE seq = ?2",
            params![
                serde_json::to_vec(&document).expect("legacy fact encodes"),
                seq
            ],
        )
        .expect("store schema-8 fact");
}

fn plant_legacy_receipt_fields(connection: &Connection) {
    let mut response: Value = serde_json::from_slice(&read_blob(
        connection,
        "SELECT response FROM command_receipts",
    ))
    .expect("receipt response is JSON");
    let body = response
        .pointer_mut("/outcome/body")
        .and_then(Value::as_object_mut)
        .expect("receipt reply body");
    body.insert("incepted".to_owned(), json!("1kKRRsDP"));
    body.insert("driver".to_owned(), json!("Keep one path."));
    connection
        .execute(
            "UPDATE command_receipts SET response = ?1",
            params![serde_json::to_vec(&response).expect("legacy receipt encodes")],
        )
        .expect("store schema-8 receipt");
}

fn read_blob(connection: &Connection, sql: &str) -> Vec<u8> {
    connection
        .query_row(sql, [], |row| row.get(0))
        .expect("read stored blob")
}
