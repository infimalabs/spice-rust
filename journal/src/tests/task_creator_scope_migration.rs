//! Schema 18 to 19 creator-team allocation boundary.

use super::*;

#[test]
fn schema_eighteen_retires_project_filters_without_reclassifying_old_tasks() {
    let scratch = Scratch::new();
    let path = scratch.path().join("task-creator-scope.sqlite3");
    let journal = authority_file_fixture(&path);

    let stored: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT body FROM residency_head WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .expect("read current residency carrier");
    let mut legacy: Value = serde_json::from_slice(&stored).expect("decode residency carrier");
    let subscription = legacy["subscriptions"][0]
        .as_object_mut()
        .expect("fixture subscription is an object");
    subscription.insert("manual_projects".to_owned(), json!(["journal"]));
    subscription.insert("auto_projects".to_owned(), json!(["journal.claimed"]));
    journal
        .connection
        .execute(
            "UPDATE residency_head SET body = ?1 WHERE singleton = 1",
            [serde_json::to_vec(&legacy).expect("encode schema-eighteen residency")],
        )
        .expect("plant schema-eighteen residency");
    journal
        .connection
        .execute_batch(
            "UPDATE journal_schema SET version = 18 WHERE singleton = 1;
             PRAGMA user_version = 18;",
        )
        .expect("downgrade creator-scope fixture to schema 18");
    drop(journal);

    let journal = Journal::open(&path).expect("schema-eighteen creator scope migrates");
    assert_eq!(
        journal
            .migration()
            .map(|migration| (migration.from(), migration.to())),
        Some((18, SCHEMA_VERSION)),
    );
    let migrated: ResidencyHead = journal
        .connection
        .query_row(
            "SELECT body FROM residency_head WHERE singleton = 1",
            [],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .map(|bytes| wire::decode(&bytes).expect("migrated residency is current wire"))
        .expect("read migrated residency");
    assert_eq!(
        migrated.subscriptions[0].policy,
        wire::intent::TeamLife::Drive,
        "migration preserves the lifetime while removing its former project state",
    );
    journal.verify().expect("migrated authority verifies");
    drop(journal);

    let reopened = Journal::open(&path).expect("migrated authority reopens");
    assert_eq!(reopened.migration(), None, "migration runs exactly once");
}
