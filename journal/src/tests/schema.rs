//! Schema identity, admission, and the read-only probe.
//!
//! Every test here judges a database this build did not necessarily write: an
//! empty file, a half-migrated one, a forward one, or one that is not SQLite at
//! all. What they share is that admission must answer by name and change
//! nothing, which is why they sit apart from the tests that assume an admitted
//! authority and go on to read it.

use super::*;

#[test]
fn empty_database_reaches_the_same_finalized_baseline_once() {
    let mut journal = Journal::in_memory().expect("empty journal migrates");
    let migration = journal.migration().expect("empty journal migrated");
    assert_eq!((migration.from(), migration.to()), (0, SCHEMA_VERSION));
    assert_eq!(
        journal.schema_version().expect("schema version"),
        SCHEMA_VERSION
    );

    assert_eq!(
        crate::migrate::baseline(&mut journal.connection).expect("baseline is idempotent"),
        None
    );
    assert_eq!(
        journal.schema_version().expect("schema version"),
        SCHEMA_VERSION
    );
    assert_eq!(authority_tables(&journal), expected_tables());

    let verified = journal.verify().expect("empty baseline verifies");
    assert_eq!(verified.facts, 0);
    assert_eq!(verified.receipts, 0);
    assert_eq!(verified.heads, 0);
    assert_eq!(verified.threads, 0);
}

#[test]
fn schema_nineteen_adds_the_bounded_startup_indexes() {
    let scratch = Scratch::new();
    let path = scratch.path().join("schema-nineteen.sqlite3");
    let journal = authority_file_fixture(&path);
    journal
        .connection
        .execute_batch(
            "DROP INDEX fact_envelopes_kind_seq;
             DROP INDEX fact_envelopes_origin_kind_seq;
             UPDATE journal_schema SET version = 19 WHERE singleton = 1;
             PRAGMA user_version = 19;",
        )
        .expect("restore the pre-index schema");
    drop(journal);

    let journal = Journal::open(&path).expect("schema-nineteen authority migrates");
    assert_eq!(
        journal
            .migration()
            .map(|migration| (migration.from(), migration.to())),
        Some((19, SCHEMA_VERSION))
    );
    for index in ["fact_envelopes_kind_seq", "fact_envelopes_origin_kind_seq"] {
        let exists: bool = journal
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1)",
                [index],
                |row| row.get(0),
            )
            .expect("read migrated index");
        assert!(exists, "migration omitted {index}");
    }
}

#[test]
fn schema_fourteen_crosses_the_requirement_wire_boundary_once_without_rewriting_tasks() {
    let scratch = Scratch::new();
    let path = scratch.path().join("schema-fourteen.sqlite3");
    let journal = authority_file_fixture(&path);
    journal
        .connection
        .execute_batch(
            "UPDATE journal_schema SET version = 14 WHERE singleton = 1;
             PRAGMA user_version = 14;",
        )
        .expect("downgrade the requirement-unaware fixture identity");
    let fact_before: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE kind = 'task::added'",
            [],
            |row| row.get(0),
        )
        .expect("read retained task fact");
    let head_before: Vec<u8> = journal
        .connection
        .query_row("SELECT body FROM task_heads", [], |row| row.get(0))
        .expect("read retained task head");
    let mut census_before = table_census(&journal.connection);
    census_before.remove("journal_schema");
    drop(journal);

    let journal = Journal::open(&path).expect("schema-fourteen authority migrates");
    let migration = journal.migration().expect("wire boundary advances");
    assert_eq!((migration.from(), migration.to()), (14, SCHEMA_VERSION));
    assert_eq!(
        version_records(&journal.connection),
        (SCHEMA_VERSION, SCHEMA_VERSION)
    );
    let fact_after: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE kind = 'task::added'",
            [],
            |row| row.get(0),
        )
        .expect("read migrated task fact");
    let head_after: Vec<u8> = journal
        .connection
        .query_row("SELECT body FROM task_heads", [], |row| row.get(0))
        .expect("read migrated task head");
    assert_eq!(
        fact_after, fact_before,
        "immutable fact bytes stay retained"
    );
    assert_eq!(head_after, head_before, "legacy head bytes stay retained");

    let snapshot = journal.fact_snapshot().expect("retained facts decode");
    let added = snapshot
        .facts()
        .iter()
        .find_map(|envelope| envelope.fact.read::<wire::body::TaskAdded>())
        .expect("retained task-added fact");
    assert!(added.requirements.is_empty());
    let task = "1kKRRsDP".parse().expect("fixture task stamp");
    let head = journal
        .read(|read| read.task_head(&task))
        .expect("retained task head reads")
        .expect("retained task exists");
    assert!(head.body().requirements.is_empty());

    let mut census_after = table_census(&journal.connection);
    census_after.remove("journal_schema");
    assert_eq!(census_after, census_before, "only schema identity advances");
    journal.verify().expect("migrated authority verifies");
    drop(journal);

    let reopened = Journal::open(&path).expect("current authority reopens");
    assert_eq!(reopened.migration(), None, "schema step runs exactly once");
}

mod rollout;

#[test]
fn schema_nine_steps_through_wire_only_expansions_without_rewriting_authority() {
    let scratch = Scratch::new();
    let path = scratch.path().join("schema-nine.sqlite3");
    let journal = authority_file_fixture(&path);
    downgrade_steering_index_to_millis(&journal.connection);
    journal
        .connection
        .execute_batch(
            "UPDATE journal_schema SET version = 9 WHERE singleton = 1;
             PRAGMA user_version = 9;",
        )
        .expect("downgrade source-unaware fixture identity");
    let mut before = table_census(&journal.connection);
    before.remove("journal_schema");
    before.remove("steering_heads");
    remove_creation_context_carriers(&mut before);
    drop(journal);

    let journal = Journal::open(&path).expect("schema-nine authority migrates");
    let migration = journal.migration().expect("wire-only steps run");
    assert_eq!((migration.from(), migration.to()), (9, SCHEMA_VERSION));
    assert_eq!(
        version_records(&journal.connection),
        (SCHEMA_VERSION, SCHEMA_VERSION)
    );
    let mut after = table_census(&journal.connection);
    after.remove("journal_schema");
    after.remove("steering_heads");
    remove_creation_context_carriers(&mut after);
    assert_eq!(
        after, before,
        "the migration chain rewrites no unrelated authority row"
    );
    assert_fixture_creation_context_migrated(&journal);
    journal
        .verify()
        .expect("migrated authority remains coherent");
}

#[test]
fn schema_ten_steps_to_fanout_aware_steering_without_rewriting_authority() {
    let scratch = Scratch::new();
    let path = scratch.path().join("schema-ten.sqlite3");
    let journal = authority_file_fixture(&path);
    downgrade_steering_index_to_millis(&journal.connection);
    journal
        .connection
        .execute_batch(
            "UPDATE journal_schema SET version = 10 WHERE singleton = 1;
             PRAGMA user_version = 10;",
        )
        .expect("downgrade fanout-unaware fixture identity");
    let mut before = table_census(&journal.connection);
    before.remove("journal_schema");
    before.remove("steering_heads");
    remove_creation_context_carriers(&mut before);
    drop(journal);

    let journal = Journal::open(&path).expect("schema-ten authority migrates");
    let migration = journal.migration().expect("fanout-aware step runs");
    assert_eq!((migration.from(), migration.to()), (10, SCHEMA_VERSION));
    assert_eq!(
        version_records(&journal.connection),
        (SCHEMA_VERSION, SCHEMA_VERSION)
    );
    let mut after = table_census(&journal.connection);
    after.remove("journal_schema");
    after.remove("steering_heads");
    remove_creation_context_carriers(&mut after);
    assert_eq!(
        after, before,
        "the migration chain rewrites no unrelated authority row"
    );
    assert_fixture_creation_context_migrated(&journal);
    journal
        .verify()
        .expect("fanout-aware authority remains coherent");
}

#[test]
fn schema_eleven_steps_to_worktree_aware_trust_without_guessing_legacy_identity() {
    let scratch = Scratch::new();
    let path = scratch.path().join("schema-eleven.sqlite3");
    let journal = authority_file_fixture(&path);
    downgrade_steering_index_to_millis(&journal.connection);
    journal
        .connection
        .execute_batch(
            "UPDATE journal_schema SET version = 11 WHERE singleton = 1;
             PRAGMA user_version = 11;",
        )
        .expect("downgrade worktree-unaware fixture identity");
    let mut before = table_census(&journal.connection);
    before.remove("journal_schema");
    before.remove("steering_heads");
    remove_creation_context_carriers(&mut before);
    drop(journal);

    let journal = Journal::open(&path).expect("schema-eleven authority migrates");
    let migration = journal.migration().expect("trust wire step runs");
    assert_eq!((migration.from(), migration.to()), (11, SCHEMA_VERSION));
    let bytes: Vec<u8> = journal
        .connection
        .query_row("SELECT body FROM trust_heads LIMIT 1", [], |row| row.get(0))
        .expect("legacy trust head reads");
    let head: TrustHead = wire::decode(&bytes).expect("legacy trust head has current shape");
    assert_eq!(head.worktree, None, "migration never guesses a producer");
    let mut after = table_census(&journal.connection);
    after.remove("journal_schema");
    after.remove("steering_heads");
    remove_creation_context_carriers(&mut after);
    assert_eq!(
        after, before,
        "the migration chain rewrites no unrelated authority row"
    );
    assert_fixture_creation_context_migrated(&journal);
    journal
        .verify()
        .expect("worktree-aware authority remains coherent");
}

/// Every row of every table, so a migration cannot quietly recreate one.
///
/// Reading the columns back as SQLite values rather than counting rows is the
/// point: a step that dropped and rebuilt a table would keep the count and lose
/// the contents, and a census of counts would call that survival.
fn table_census(connection: &Connection) -> BTreeMap<String, Vec<Vec<rusqlite::types::Value>>> {
    let mut names = connection
        .prepare(
            "SELECT name FROM sqlite_schema WHERE type = 'table' \
             AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .expect("query census tables");
    let tables: Vec<String> = names
        .query_map([], |row| row.get::<_, String>(0))
        .expect("map census table rows")
        .map(|row| row.expect("read census table name"))
        .collect();
    drop(names);

    tables
        .into_iter()
        .map(|table| {
            let mut statement = connection
                .prepare(&format!("SELECT * FROM \"{table}\""))
                .expect("select census rows");
            let columns = statement.column_count();
            let rows = statement
                .query_map([], |row| {
                    (0..columns).map(|column| row.get(column)).collect()
                })
                .expect("map census rows")
                .map(|row| row.expect("read census row"))
                .collect();
            (table, rows)
        })
        .collect()
}

fn remove_creation_context_carriers(
    census: &mut BTreeMap<String, Vec<Vec<rusqlite::types::Value>>>,
) {
    for table in ["fact_envelopes", "task_heads"] {
        assert!(
            census.remove(table).is_some(),
            "authority census contains {table}"
        );
    }
}

fn assert_fixture_creation_context_migrated(journal: &Journal) {
    let expected = wire::body::TaskCreationContext::unavailable(
        wire::body::TaskCreationUnavailable::SpiceActor,
    );
    let snapshot = journal.fact_snapshot().expect("migrated facts read");
    let added = snapshot
        .facts()
        .iter()
        .find_map(|envelope| envelope.fact.read::<wire::body::TaskAdded>())
        .expect("fixture task-added fact remains present");
    assert_eq!(added.creation, expected, "creation fact context migrates");

    let task = "1kKRRsDP".parse().expect("fixture task stamp");
    let head = journal
        .read(|read| read.task_head(&task))
        .expect("migrated task head reads")
        .expect("fixture task head remains present");
    assert_eq!(
        head.body().creation,
        expected,
        "creation head context migrates"
    );
}

/// An authority at schema 1, carrying the rows a schema-1 build would have.
///
/// Derived by downgrading the current baseline rather than freezing a copy of
/// the old DDL here, because commit 7453545b is the entire 1-to-2 delta: it
/// added `transcript_source_heads` and advanced the two version records, and
/// touched nothing else. Building the fixture this way keeps it honest as the
/// baseline grows, and the dropped table takes the one seeded head with it --
/// which is correct, since schema 1 had nowhere to put it.
fn schema_one_authority(path: &Path) {
    let journal = authority_file_fixture(path);
    drop_steering_index(&journal.connection);
    downgrade_task_identity_to_five(&journal.connection);
    journal
        .connection
        .execute_batch(
            "DELETE FROM lease_heads;
             DELETE FROM claim_heads;
             DROP TABLE transcript_source_heads;
             UPDATE journal_schema SET version = 1 WHERE singleton = 1;
             PRAGMA user_version = 1;",
        )
        .expect("downgrade the fixture to schema 1");
    drop(journal);
}

#[test]
fn a_schema_one_authority_is_stepped_forward_without_losing_a_row() {
    let scratch = Scratch::new();
    let path = scratch.path().join("schema-one.sqlite3");
    schema_one_authority(&path);
    let before = {
        let reader = Connection::open(&path).expect("read the schema-1 fixture back");
        assert!(
            !before_has_table(&reader, "transcript_source_heads"),
            "the fixture must actually be at schema 1, or the step under test never runs"
        );
        table_census(&reader)
    };

    let mut journal = Journal::open(&path).expect("a schema-1 authority opens");

    // The two version records are asserted before anything that reads them
    // back, because admission refuses a disagreement between them: leaving that
    // refusal to speak first would shadow whichever record actually stalled.
    let migration = journal
        .migration()
        .expect("the open stepped the schema forward");
    assert_eq!((migration.from(), migration.to()), (1, SCHEMA_VERSION));
    let mut after = table_census(&journal.connection);
    assert_eq!(
        after.remove("journal_schema").expect("schema row survives"),
        vec![vec![
            rusqlite::types::Value::Integer(1),
            rusqlite::types::Value::Integer(i64::from(SCHEMA_VERSION)),
            rusqlite::types::Value::Text("finalized".to_owned()),
            rusqlite::types::Value::Null,
            rusqlite::types::Value::Null,
            rusqlite::types::Value::Null,
        ]],
        "the schema row must name the current version and stay finalized"
    );
    let pragma: u32 = journal
        .connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("read the migrated pragma");
    assert_eq!(
        pragma, SCHEMA_VERSION,
        "the pragma must advance with the row; a disagreement refuses the next open"
    );
    assert_eq!(
        journal.schema_version().expect("migrated schema version"),
        SCHEMA_VERSION
    );

    assert_eq!(authority_tables(&journal), expected_tables());
    assert_eq!(
        after.remove("transcript_source_heads"),
        Some(Vec::new()),
        "the step creates the new table empty; a schema-1 authority has no head to carry"
    );
    for table in [
        "steering_heads",
        "steering_misses",
        "steering_index",
        "claim_index",
    ] {
        assert!(
            after.remove(table).is_some(),
            "the migration chain must add indexed authority table {table}"
        );
    }
    // Compared one table at a time so a failure prints the unrelated table
    // that lost a row. Task identity carriers are deliberately rewritten by
    // schema 4 and have their own byte- and rollback-level migration cases.
    let mut expected = before;
    expected.remove("journal_schema");
    assert_eq!(
        after.keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>(),
        "the step must add a table and drop none"
    );
    for (table, rows) in expected {
        if [
            "fact_envelopes",
            "thread_index",
            "command_receipts",
            "task_heads",
            "workflow_heads",
            "claim_heads",
            "lease_heads",
        ]
        .contains(&table.as_str())
        {
            continue;
        }
        assert_eq!(
            after[&table], rows,
            "unrelated table {table} must survive byte for byte across the chained migration"
        );
    }

    admit_empty_steering_fold(&mut journal);
    let verified = journal.verify().expect("the migrated authority verifies");
    assert_eq!(verified.facts, 2);
}

/// Whether the fixture already carries the table the step under test creates.
fn before_has_table(connection: &Connection, table: &str) -> bool {
    connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            params![table],
            |row| row.get::<_, u32>(0),
        )
        .expect("probe the fixture for a table")
        > 0
}

/// The two records a migration advances together, read as one pair.
///
/// Read together because the failure this guards against is a disagreement
/// between them, and asserting one without the other cannot see it.
fn version_records(connection: &Connection) -> (u32, u32) {
    (
        connection
            .query_row(
                "SELECT version FROM journal_schema WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .expect("read the stored schema version"),
        connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("read the pragma version"),
    )
}

/// A migration abandoned after both version records are written strands nothing.
///
/// `advance` writes the step and both version records onto a transaction it does
/// not own, so the whole promise rests on the caller committing or not. This
/// test becomes that caller and then fails, which is the one thing the suite
/// never did: the existing coverage opens a journal and lets it commit, so every
/// write in `advance` could be leaking out of the transaction and every test
/// would still pass.
///
/// The failure is injected after `advance` returns rather than inside a step,
/// and that placement is the point. A step that fails takes nothing with it
/// because the version records never ran, so it cannot tell whether those
/// records are inside the transaction. Failing here, with both records already
/// written, is the only moment that can.
///
/// The three assertions taken before the rollback are what keep the three after
/// it from passing vacuously: an `advance` that wrote nothing at all would leave
/// the authority at version 1 too.
#[test]
fn a_migration_that_fails_after_writing_both_version_records_commits_nothing() {
    let scratch = Scratch::new();
    let path = scratch.path().join("schema-one-abandoned.sqlite3");
    schema_one_authority(&path);
    let before = {
        let reader = Connection::open(&path).expect("read the schema-1 fixture back");
        assert!(
            !before_has_table(&reader, "transcript_source_heads"),
            "the fixture must actually be at schema 1, or the step under test never runs"
        );
        assert_eq!(
            version_records(&reader),
            (1, 1),
            "the fixture must start at schema 1 by both records"
        );
        table_census(&reader)
    };

    {
        let mut connection = Connection::open(&path).expect("open the authority to migrate it");
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Exclusive)
            .expect("take the exclusive transaction a write open takes");
        let migration = crate::migrate::advance(&transaction)
            .expect("the migration itself succeeds; this test fails the open around it")
            .expect("a schema-1 authority has a step to take");

        assert_eq!((migration.from(), migration.to()), (1, SCHEMA_VERSION));
        assert!(
            before_has_table(&transaction, "transcript_source_heads"),
            "the step must have applied, or the rollback below has nothing to undo"
        );
        assert_eq!(
            version_records(&transaction),
            (SCHEMA_VERSION, SCHEMA_VERSION),
            "both records must read forward inside the transaction, or this test \
             proves only that a migration which did nothing changed nothing"
        );

        // The open fails here: after the step, after both version records, and
        // before anything commits. Dropping the transaction unclaimed is what a
        // failed open does, and it must take all three writes with it.
        transaction
            .execute_batch("SELECT no_such_function();")
            .expect_err("the injected failure must actually fail");
    }

    let after = Connection::open(&path).expect("read the authority back after the failed open");
    assert_eq!(
        version_records(&after),
        (1, 1),
        "a failed migration must leave the authority at the version it started \
         from; a record that survived the rollback strands it between two"
    );
    assert!(
        !before_has_table(&after, "transcript_source_heads"),
        "the step must roll back with the records it was supposed to travel with"
    );
    assert_eq!(
        table_census(&after),
        before,
        "every table must read back exactly as it did before the failed migration"
    );
}

#[test]
fn a_read_only_open_names_the_write_open_that_would_migrate() {
    let scratch = Scratch::new();
    let path = scratch.path().join("schema-one-readonly.sqlite3");
    schema_one_authority(&path);
    let before = FileState::read(&path);

    let error = JournalReader::open(&path).expect_err("a read-only open never migrates");

    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::UnmigratedRead {
            found: 1,
            current: SCHEMA_VERSION
        })
    ));
    assert!(
        error
            .to_string()
            .contains("open this authority for writing"),
        "a migratable version must name the open that migrates it, not another release: {error}"
    );
    assert_eq!(
        FileState::read(&path),
        before,
        "the refusal must leave the authority at schema 1"
    );
}

#[test]
fn prepared_and_forward_schema_states_are_refused_by_name() {
    let prepared = schema_fixture(1, "prepared", Some(0), Some(1), Some("plan:baseline"));
    let error = Journal::from_connection(prepared).expect_err("prepared migration refuses open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::Unfinalized {
            from: Some(0),
            to: Some(1)
        })
    ));
    assert!(error.to_string().contains("prepared and unfinalized"));

    let forward = schema_fixture(SCHEMA_VERSION + 1, "finalized", None, None, None);
    let error = Journal::from_connection(forward).expect_err("forward schema refuses open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::ForwardSchema {
            found,
            current: SCHEMA_VERSION
        })
        if found == SCHEMA_VERSION + 1
    ));
    assert!(
        error
            .to_string()
            .contains(&format!("schema {}", SCHEMA_VERSION + 1))
    );
}

#[test]
fn unknown_states_and_occupied_databases_are_refused_by_name() {
    // These two refusals exist for databases this build did not write, so the
    // baseline table's own CHECK constraints cannot stand in for them: a
    // foreign file reaches `finalized_version` carrying whatever it likes.
    let unknown = schema_fixture(1, "migrating", None, None, None);
    let error = Journal::from_connection(unknown).expect_err("unknown state refuses open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::UnknownState { .. })
    ));
    assert!(error.to_string().contains("\"migrating\" is unknown"));

    let occupied = Connection::open_in_memory().expect("open occupied fixture");
    occupied
        .execute_batch("CREATE TABLE somebody_elses_table(id INTEGER PRIMARY KEY) STRICT;")
        .expect("occupy the fixture database");
    let error =
        Journal::from_connection(occupied).expect_err("an occupied database refuses baselining");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::UnversionedAuthority { object_count: 1 })
    ));
    assert!(error.to_string().contains("empty authority"));
}

#[test]
fn malformed_schema_identity_is_refused_by_name() {
    let duplicate = schema_fixture(1, "finalized", None, None, None);
    duplicate
        .execute(
            "INSERT INTO journal_schema(singleton, version, state) VALUES (2, 1, 'finalized')",
            [],
        )
        .expect("insert duplicate schema row");
    let error = Journal::from_connection(duplicate).expect_err("two schema rows refuse open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::SchemaRowCount { count: 2 })
    ));
    assert!(error.to_string().contains("exactly one row, found 2"));

    let missing = schema_fixture(1, "finalized", None, None, None);
    missing
        .execute("UPDATE journal_schema SET singleton = 2", [])
        .expect("move schema row away from singleton one");
    let error = Journal::from_connection(missing).expect_err("missing singleton refuses open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::MissingSingleton)
    ));
    assert!(error.to_string().contains("singleton row 1 is missing"));

    let unknown = schema_fixture(0, "finalized", None, None, None);
    let error = Journal::from_connection(unknown).expect_err("unknown version refuses open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::UnknownVersion { found: 0 })
    ));
    // A version with no step is the only one this refusal still covers, and the
    // remedy it names must be reachable: discarding the authority was never one,
    // and it stopped being the honest answer entirely once steps existed.
    let unmigratable = error.to_string();
    assert!(unmigratable.contains("schema 0 has no migration path"));
    assert!(
        unmigratable.contains("this build steps forward from schema 1, 2"),
        "the refusal must name the versions that do have a path: {unmigratable}"
    );
    assert!(
        !unmigratable.contains("empty authority"),
        "an unmigratable version must not be answered by starting over: {unmigratable}"
    );

    let disagreement = schema_fixture(SCHEMA_VERSION, "finalized", None, None, None);
    disagreement
        .pragma_update(None, "user_version", 0)
        .expect("make pragma disagree with schema row");
    let error =
        Journal::from_connection(disagreement).expect_err("version disagreement refuses open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::VersionDisagreement {
            row: SCHEMA_VERSION,
            pragma: 0
        })
    ));
    assert!(error.to_string().contains(&format!(
        "row names version {SCHEMA_VERSION}, but SQLite user_version names 0"
    )));
}

#[test]
fn read_only_handle_reports_schema_and_cursor_without_mutation() {
    let scratch = Scratch::new();
    let path = scratch.path().join("journal.sqlite3");
    drop(authority_file_fixture(&path));
    let before = FileState::read(&path);

    let reader = JournalReader::open(&path).expect("finalized authority opens read-only");

    assert_eq!(
        reader.schema_version().expect("read schema version"),
        SCHEMA_VERSION
    );
    assert_eq!(reader.cursor().expect("read cursor"), Seq::new(2));
    let query_only: bool = reader
        .connection
        .query_row("PRAGMA query_only", [], |row| row.get(0))
        .expect("query-only posture");
    assert!(query_only);
    let write = reader
        .connection
        .execute("UPDATE cursor_head SET wake = seq WHERE singleton = 1", [])
        .expect_err("SQLite rejects a write through the reader");
    assert_eq!(
        write.sqlite_error_code(),
        Some(rusqlite::ErrorCode::ReadOnly)
    );
    drop(reader);

    assert_eq!(FileState::read(&path), before);

    let empty = scratch.path().join("empty.sqlite3");
    drop(Journal::open(&empty).expect("create empty finalized authority"));
    let before = FileState::read(&empty);

    let reader = JournalReader::open(&empty).expect("empty finalized authority opens read-only");

    assert_eq!(reader.cursor().expect("read empty cursor"), None);
    drop(reader);
    assert_eq!(FileState::read(&empty), before);
}

#[test]
fn read_only_admission_refuses_schema_states_without_mutation() {
    let scratch = Scratch::new();
    let cases = [
        (
            "prepared.sqlite3",
            1,
            "prepared",
            Some(0),
            Some(1),
            Some("plan:baseline"),
            "prepared and unfinalized",
        ),
        (
            "newer.sqlite3",
            SCHEMA_VERSION + 1,
            "finalized",
            None,
            None,
            None,
            "newer than this build",
        ),
        (
            "older.sqlite3",
            0,
            "finalized",
            None,
            None,
            None,
            "has no migration path",
        ),
    ];

    for (name, version, state, from, to, digest, expected) in cases {
        let path = scratch.path().join(name);
        drop(schema_file_fixture(&path, version, state, from, to, digest));
        let before = FileState::read(&path);

        let error = JournalReader::open(&path).expect_err("schema state refuses read-only open");

        assert!(
            error.to_string().contains(expected),
            "{name} did not name {expected:?}: {error}",
        );
        assert_eq!(FileState::read(&path), before, "{name} changed on refusal");
    }
}

#[test]
fn read_only_admission_never_creates_or_repairs_an_authority() {
    let scratch = Scratch::new();
    let absent = scratch.path().join("absent.sqlite3");
    let entries_before = directory_entries(scratch.path());

    let error = JournalReader::open(&absent).expect_err("absent authority refuses open");

    assert!(matches!(error, OpenError::Open { ref path, .. } if path == &absent));
    assert!(!absent.exists());
    assert_eq!(directory_entries(scratch.path()), entries_before);

    let malformed = scratch.path().join("malformed.sqlite3");
    fs::write(&malformed, b"not a SQLite authority").expect("write malformed fixture");
    let before = FileState::read(&malformed);

    let error = JournalReader::open(&malformed).expect_err("malformed authority refuses open");

    assert!(matches!(error, OpenError::Sqlite(_)));
    assert_eq!(FileState::read(&malformed), before);
}

#[test]
fn schema_probe_reads_finalized_identity_without_changing_the_store() {
    let scratch = Scratch::new();
    let path = scratch.path().join("authority.db");
    drop(Journal::open(&path).expect("create finalized authority"));
    let before = directory_bytes(scratch.path());

    assert_eq!(
        Journal::inspect_schema(&path).expect("inspect finalized authority"),
        SCHEMA_VERSION
    );
    assert_eq!(directory_bytes(scratch.path()), before);
}

#[test]
fn stored_schema_probe_observes_finalized_versions_without_admitting_them() {
    for version in [SCHEMA_VERSION - 1, SCHEMA_VERSION + 1] {
        let scratch = Scratch::new();
        let path = scratch.path().join(format!("schema-{version}.db"));
        drop(schema_file_fixture(
            &path,
            version,
            "finalized",
            None,
            None,
            None,
        ));
        let before = directory_bytes(scratch.path());

        assert_eq!(
            Journal::inspect_stored_schema(&path).expect("stored version observes"),
            version
        );
        assert_eq!(directory_bytes(scratch.path()), before);
    }
}

#[test]
fn stored_schema_probe_still_refuses_unfinalized_state_without_mutation() {
    let scratch = Scratch::new();
    let path = scratch.path().join("prepared.db");
    drop(schema_file_fixture(
        &path,
        SCHEMA_VERSION - 1,
        "prepared",
        Some(SCHEMA_VERSION - 1),
        Some(SCHEMA_VERSION),
        Some("plan:upgrade"),
    ));
    let before = directory_bytes(scratch.path());

    let error = Journal::inspect_stored_schema(&path).expect_err("prepared state refuses");

    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::Unfinalized { .. })
    ));
    assert_eq!(directory_bytes(scratch.path()), before);
}

#[test]
fn schema_probe_refuses_unfinalized_and_forward_state_without_changing_the_store() {
    for (name, version, state, from, to, digest, expected) in [
        (
            "prepared",
            1,
            "prepared",
            Some(0),
            Some(1),
            Some("plan:baseline"),
            MigrationError::Unfinalized {
                from: Some(0),
                to: Some(1),
            },
        ),
        (
            "forward",
            SCHEMA_VERSION + 1,
            "finalized",
            None,
            None,
            None,
            MigrationError::ForwardSchema {
                found: SCHEMA_VERSION + 1,
                current: SCHEMA_VERSION,
            },
        ),
    ] {
        let scratch = Scratch::new();
        let path = scratch.path().join(format!("{name}.db"));
        drop(schema_file_fixture(&path, version, state, from, to, digest));
        let before = directory_bytes(scratch.path());

        let error = Journal::inspect_schema(&path).expect_err("schema state refuses");
        assert!(
            matches!(error, OpenError::Migration(ref found) if *found == expected),
            "unexpected schema refusal: {error}"
        );
        assert_eq!(directory_bytes(scratch.path()), before);
    }
}

#[test]
fn schema_probe_distinguishes_absence_from_sqlite_corruption_and_creates_nothing() {
    let scratch = Scratch::new();
    let missing = scratch.path().join("missing.db");

    let error = Journal::inspect_schema(&missing).expect_err("missing authority refuses");
    assert!(matches!(error, OpenError::Open { ref path, .. } if path == &missing));
    assert!(directory_bytes(scratch.path()).is_empty());

    let empty = scratch.path().join("empty.db");
    drop(Connection::open(&empty).expect("create empty SQLite fixture"));
    let before = directory_bytes(scratch.path());
    let error = Journal::inspect_schema(&empty).expect_err("missing schema identity refuses");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::MissingSchema)
    ));
    assert_eq!(directory_bytes(scratch.path()), before);

    let malformed = scratch.path().join("malformed.db");
    fs::write(&malformed, b"not a SQLite authority").expect("write malformed fixture");
    let before = directory_bytes(scratch.path());
    let error = Journal::inspect_schema(&malformed).expect_err("malformed SQLite refuses");
    assert!(matches!(error, OpenError::Sqlite(_)));
    assert_eq!(directory_bytes(scratch.path()), before);
}
