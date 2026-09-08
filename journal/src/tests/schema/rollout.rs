use super::*;

fn store_trust_requirements(journal: &Journal, requirements: &wire::EnvironmentRequirements) {
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
            params![wire::encode(&added).expect("trust requirement fact encodes")],
        )
        .expect("predecessor fixture stores the unchanged successor wire bytes");
    let mut head = task_head();
    head.requirements = requirements.clone();
    journal
        .connection
        .execute(
            "UPDATE task_heads SET body = ?1 WHERE stamp = '1kKRRsDP'",
            params![wire::encode(&head).expect("trust requirement head encodes")],
        )
        .expect("predecessor fixture stores the unchanged successor head bytes");
}

fn requirement_carriers(journal: &Journal) -> (Vec<u8>, Vec<u8>) {
    let fact = journal
        .connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE seq = 1",
            [],
            |row| row.get(0),
        )
        .expect("task fact bytes read");
    let head = journal
        .connection
        .query_row(
            "SELECT body FROM task_heads WHERE stamp = '1kKRRsDP'",
            [],
            |row| row.get(0),
        )
        .expect("task head bytes read");
    (fact, head)
}

#[test]
fn schema_seventeen_rolls_out_trust_requirements_without_rewriting_authority() {
    let scratch = Scratch::new();
    let path = scratch
        .path()
        .join("schema-seventeen-trust-requirement.sqlite3");
    let journal = authority_file_fixture(&path);
    let digest: wire::Digest = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        .parse()
        .expect("fixture digest is canonical");
    let requirements =
        wire::EnvironmentRequirements::one(wire::EnvironmentRequirement::trust_grant(digest));
    store_trust_requirements(&journal, &requirements);
    journal
        .connection
        .execute_batch(
            "UPDATE journal_schema SET version = 17 WHERE singleton = 1;
             PRAGMA user_version = 17;",
        )
        .expect("fixture becomes the exact predecessor schema");
    let (fact_before, head_before) = requirement_carriers(&journal);
    let mut census_before = table_census(&journal.connection);
    census_before.remove("journal_schema");
    drop(journal);

    assert!(matches!(
        JournalReader::open(&path),
        Err(OpenError::Migration(MigrationError::UnmigratedRead {
            found: 17,
            current: SCHEMA_VERSION,
        }))
    ));

    let journal = Journal::open(&path).expect("successor migrates schema-seventeen authority");
    let migration = journal
        .migration()
        .expect("trust rollout crossing is observed");
    assert_eq!((migration.from(), migration.to()), (17, SCHEMA_VERSION));
    let (fact_after, head_after) = requirement_carriers(&journal);
    assert_eq!(fact_after, fact_before, "task fact bytes remain immutable");
    assert_eq!(head_after, head_before, "task head bytes remain immutable");
    let mut census_after = table_census(&journal.connection);
    census_after.remove("journal_schema");
    assert_eq!(
        census_after, census_before,
        "rollout is a no-data migration"
    );
    let current = journal
        .read(|read| read.task_head(&"1kKRRsDP".parse().expect("fixture stamp")))
        .expect("current head reads")
        .expect("current head exists");
    assert_eq!(current.body().requirements, requirements);
    journal
        .verify()
        .expect("trust requirement authority verifies");
}

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
