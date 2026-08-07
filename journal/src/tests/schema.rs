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
    journal
        .connection
        .execute_batch(
            "DROP TABLE transcript_source_heads;
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

    let journal = Journal::open(&path).expect("a schema-1 authority opens");

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
    // Compared one table at a time so a failure prints the table that lost a
    // row rather than every blob in the authority.
    let mut expected = before;
    expected.remove("journal_schema");
    assert_eq!(
        after.keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>(),
        "the step must add a table and drop none"
    );
    for (table, rows) in expected {
        assert_eq!(
            after[&table], rows,
            "{table} must survive byte for byte: an additive step migrates the \
             authority, a step that recreated the table would lose it"
        );
    }

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
            current: 2
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

    let forward = schema_fixture(3, "finalized", None, None, None);
    let error = Journal::from_connection(forward).expect_err("forward schema refuses open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::ForwardSchema {
            found: 3,
            current: 2
        })
    ));
    assert!(error.to_string().contains("schema 3"));
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
        unmigratable.contains("this build steps forward from schema 1"),
        "the refusal must name the versions that do have a path: {unmigratable}"
    );
    assert!(
        !unmigratable.contains("empty authority"),
        "an unmigratable version must not be answered by starting over: {unmigratable}"
    );

    let disagreement = schema_fixture(2, "finalized", None, None, None);
    disagreement
        .pragma_update(None, "user_version", 0)
        .expect("make pragma disagree with schema row");
    let error =
        Journal::from_connection(disagreement).expect_err("version disagreement refuses open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::VersionDisagreement { row: 2, pragma: 0 })
    ));
    assert!(
        error
            .to_string()
            .contains("row names version 2, but SQLite user_version names 0")
    );
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
            3,
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
            3,
            "finalized",
            None,
            None,
            None,
            MigrationError::ForwardSchema {
                found: 3,
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
