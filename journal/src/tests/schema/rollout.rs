use super::*;

#[test]
fn schema_fifteen_rollout_unblocks_a_requirement_without_rewriting_task_authority() {
    let scratch = Scratch::new();
    let path = scratch.path().join("schema-fifteen-rollout.sqlite3");
    let journal = authority_file_fixture(&path);
    let requirements = wire::EnvironmentRequirements::one(
        wire::EnvironmentRequirement::journal_schema(SCHEMA_VERSION)
            .expect("current schema is positive"),
    );
    let added = fact(
        1,
        None,
        FactKind::TaskAdded,
        json!({
            "task": "1kKRRsDP",
            "title": "Journal fixture",
            "requirements": requirements,
            "project": "journal",
            "tags": [],
            "depends": [],
            "flow": ["todo", "review"],
            "phase": "todo",
            "revision": 1
        }),
    );
    journal
        .connection
        .execute(
            "UPDATE fact_envelopes SET envelope = ?1 WHERE seq = 1",
            params![wire::encode(&added).expect("requirement-bearing fact encodes")],
        )
        .expect("schema-fifteen task fact carries its requirement");
    let mut head = task_head();
    head.requirements = requirements.clone();
    journal
        .connection
        .execute(
            "UPDATE task_heads SET body = ?1 WHERE stamp = '1kKRRsDP'",
            params![wire::encode(&head).expect("requirement-bearing head encodes")],
        )
        .expect("schema-fifteen task head carries its requirement");
    journal
        .connection
        .execute_batch(
            "UPDATE journal_schema SET version = 15 WHERE singleton = 1;
             PRAGMA user_version = 15;",
        )
        .expect("fixture becomes the exact requirement-aware predecessor schema");
    let fact_before: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE seq = 1",
            [],
            |row| row.get(0),
        )
        .expect("predecessor fact reads");
    let head_before: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT body FROM task_heads WHERE stamp = '1kKRRsDP'",
            [],
            |row| row.get(0),
        )
        .expect("predecessor head reads");
    let predecessor =
        wire::Environment::journal_schema(15).expect("predecessor schema is positive");
    assert!(!requirements.are_satisfied_by(&predecessor));
    drop(journal);

    let journal = Journal::open(&path).expect("successor migrates schema-fifteen authority");
    let migration = journal
        .migration()
        .expect("rollout crossing is observed once");
    assert_eq!((migration.from(), migration.to()), (15, SCHEMA_VERSION));
    let successor =
        wire::Environment::journal_schema(SCHEMA_VERSION).expect("successor schema is positive");
    assert!(requirements.are_satisfied_by(&successor));
    let fact_after: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE seq = 1",
            [],
            |row| row.get(0),
        )
        .expect("successor fact reads");
    let head_after: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT body FROM task_heads WHERE stamp = '1kKRRsDP'",
            [],
            |row| row.get(0),
        )
        .expect("successor head reads");
    assert_eq!(
        fact_after, fact_before,
        "migration does not rewrite the task fact"
    );
    assert_eq!(
        head_after, head_before,
        "migration does not rewrite the task head"
    );
    journal
        .verify()
        .expect("migrated requirement authority verifies");
    drop(journal);

    let reopened = Journal::open(&path).expect("successor authority reopens");
    assert_eq!(
        reopened.migration(),
        None,
        "rollout migration never duplicates"
    );
}
