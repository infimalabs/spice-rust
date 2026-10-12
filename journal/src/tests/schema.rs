//! Schema identity, admission, and the read-only probe.
//!
//! Every test here judges a database this build did not necessarily write: an
//! empty file, a half-migrated one, a forward one, or one that is not SQLite at
//! all. What they share is that admission must answer by name and change
//! nothing, which is why they sit apart from the tests that assume an admitted
//! authority and go on to read it.

use super::*;

#[test]
fn current_schema_is_the_only_executable_journal_shape() {
    let mut journal = Journal::in_memory().expect("empty journal creates the current schema");
    let migration = journal
        .migration()
        .expect("empty journal records its baseline");
    assert_eq!((migration.from(), migration.to()), (0, SCHEMA_VERSION));
    assert_eq!(
        journal.schema_version().expect("schema version"),
        SCHEMA_VERSION
    );
    assert_eq!(
        crate::migrate::admit(&mut journal.connection).expect("baseline is idempotent"),
        None
    );
    assert_eq!(authority_tables(&journal), expected_tables());
    let verified = journal.verify().expect("empty baseline verifies");
    assert_eq!(
        (
            verified.facts,
            verified.receipts,
            verified.heads,
            verified.threads
        ),
        (0, 0, 0, 0)
    );
}

#[test]
fn predecessor_authority_crosses_once_to_the_current_shape_without_rewriting_rows() {
    for predecessor in [33, 34] {
        let scratch = Scratch::new();
        let path = scratch.path().join("authority.sqlite3");
        let journal = authority_file_fixture(&path);
        journal
            .connection
            .execute_batch(&format!(
                "UPDATE journal_schema SET version = {predecessor} WHERE singleton = 1;
             PRAGMA user_version = {predecessor};",
            ))
            .expect("restore predecessor identity");
        let rows = |journal: &Journal| -> (Vec<u8>, Vec<u8>) {
            let fact = journal
                .connection
                .query_row(
                    "SELECT envelope FROM fact_envelopes WHERE kind = 'task::added'",
                    [],
                    |row| row.get(0),
                )
                .expect("read retained task fact");
            let head = journal
                .connection
                .query_row("SELECT body FROM task_heads", [], |row| row.get(0))
                .expect("read retained task head");
            (fact, head)
        };
        let before = rows(&journal);
        drop(journal);

        let journal = Journal::open(&path).expect("predecessor authority crosses forward");
        let migration = journal.migration().expect("the crossing is reported");
        assert_eq!(
            (migration.from(), migration.to()),
            (predecessor, SCHEMA_VERSION)
        );
        assert_eq!(
            journal.schema_version().expect("schema reads"),
            SCHEMA_VERSION
        );
        assert_eq!(rows(&journal), before, "retained authority bytes stay put");
        let task = "1kKRRsDP".parse().expect("fixture task stamp");
        let head = journal
            .read(|read| read.task_head(&task))
            .expect("retained task head reads")
            .expect("retained task exists");
        assert_eq!(head.body().intent, None, "no intent is inferred");
        assert_eq!(head.body().preparation, None, "no preparation is inferred");
        journal.verify().expect("crossed authority verifies");
        drop(journal);

        let reopened = Journal::open(&path).expect("current authority reopens");
        assert_eq!(reopened.migration(), None, "the crossing runs exactly once");
    }
}
#[test]
fn schema_admission_requires_trusted_schema_to_be_disabled_before_inspection() {
    let mut connection = Connection::open_in_memory().expect("open empty fixture");
    let trusted: bool = connection
        .query_row("PRAGMA trusted_schema", [], |row| row.get(0))
        .expect("default trust posture reads");
    assert!(trusted);

    let error = crate::migrate::admit(&mut connection)
        .expect_err("unhardened connection refuses schema inspection");

    assert!(matches!(error, OpenError::TrustedSchemaEnabled));
    let objects: u32 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .expect("empty fixture remains inspectable");
    assert_eq!(objects, 0, "refusal initialized no schema objects");

    connection
        .pragma_update(None, "trusted_schema", false)
        .expect("disable trusted schema");
    let migration = crate::migrate::admit(&mut connection)
        .expect("hardened empty connection initializes")
        .expect("initialization is recorded");
    assert_eq!((migration.from(), migration.to()), (0, SCHEMA_VERSION));
}

#[test]
fn every_retired_schema_refuses_and_names_its_last_migration_owner() {
    let owners = [
        (1, "122d2a842"),
        (2, "IDENTIT-1kLGl2S5"),
        (3, "IDENTIT-1kLGl2S5"),
        (4, "b6acb226d"),
        (5, "d0c14dcc4"),
        (6, "SESSION-1kLLCyqw"),
        (7, "MINING-1kLNTy1C"),
        (8, "MINING-1kLNTy1C"),
        (9, "87636d7af"),
        (10, "1kRNj6X7"),
        (11, "257200978"),
        (12, "6ad57c713"),
        (13, "d811711a3"),
        (14, "ENVIRON-dRyzKgHJw"),
        (15, "SCHEMAR-dRyzNKsdp"),
        (16, "b9a45a6ec"),
        (17, "37cc1297f"),
        (18, "ALLOCAT-dS8cvNplX"),
        (19, "BOUNDS-dSHsMK0kz"),
        (20, "eab8365a6"),
        (21, "d431530eb"),
        (22, "c64c48cac"),
        (23, "TASKS-dSPJyjNSH"),
        (24, "TASKS-dSPW61MXd"),
        (25, "fb96c12a7"),
        (26, "966d33bef"),
        (27, "916081f8a"),
        (28, "02715f9e1"),
        (29, "SPEECH-dTQBpHHJq"),
        (30, "a4abe071f"),
        (31, "55ad00080"),
        (32, "a15ed13f4"),
    ];

    for (version, owner) in owners {
        let error =
            Journal::from_connection(schema_fixture(version, "finalized", None, None, None))
                .expect_err("retired schema refuses write open");
        assert!(matches!(
            error,
            OpenError::Migration(MigrationError::UnknownVersion { found }) if found == version
        ));
        let refusal = error.to_string();
        assert!(
            refusal.contains(&format!(
                "schema {version} was last migrated by release {owner}"
            )),
            "retired schema {version} did not name {owner}: {refusal}"
        );
        assert!(
            refusal.contains(&format!("reach schema {SCHEMA_VERSION}")),
            "retired schema {version} did not name the current cutoff: {refusal}"
        );
    }
}
#[test]
fn retired_file_refuses_before_wal_or_any_storage_mutation() {
    let scratch = Scratch::new();
    let path = scratch.path().join("retired.sqlite3");
    drop(schema_file_fixture(
        &path,
        11,
        "finalized",
        None,
        None,
        None,
    ));
    let connection = Connection::open(&path).expect("retired fixture opens directly");
    let mode: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .expect("retired fixture mode reads");
    assert_eq!(mode, "delete");
    drop(connection);
    let before_file = FileState::read(&path);
    let before_entries = directory_entries(scratch.path());
    let before_directory = directory_bytes(scratch.path());

    let error = Journal::open(&path).expect_err("retired file refuses write open");

    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::UnknownVersion { found: 11 })
    ));
    assert_eq!(FileState::read(&path), before_file);
    assert_eq!(directory_entries(scratch.path()), before_entries);
    assert_eq!(directory_bytes(scratch.path()), before_directory);
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

    let missing = schema_fixture(1, "finalized", None, None, None);
    missing
        .execute("UPDATE journal_schema SET singleton = 2", [])
        .expect("move schema row away from singleton one");
    let error = Journal::from_connection(missing).expect_err("missing singleton refuses open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::MissingSingleton)
    ));

    let unknown = schema_fixture(0, "finalized", None, None, None);
    let error = Journal::from_connection(unknown).expect_err("unknown version refuses open");
    assert!(matches!(
        error,
        OpenError::Migration(MigrationError::UnknownVersion { found: 0 })
    ));

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
        ),
        (
            "newer.sqlite3",
            SCHEMA_VERSION + 1,
            "finalized",
            None,
            None,
            None,
        ),
        ("predecessor.sqlite3", 33, "finalized", None, None, None),
        ("older.sqlite3", 0, "finalized", None, None, None),
    ];

    for (name, version, state, from, to, digest) in cases {
        let path = scratch.path().join(name);
        drop(schema_file_fixture(&path, version, state, from, to, digest));
        let before = FileState::read(&path);

        let error = JournalReader::open(&path).expect_err("schema state refuses read-only open");

        assert!(matches!(error, OpenError::Migration(_)));
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
