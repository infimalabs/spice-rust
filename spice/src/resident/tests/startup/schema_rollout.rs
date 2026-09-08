use rusqlite::Connection;

use super::*;

#[test]
fn successor_start_records_one_schema_crossing_on_the_preserved_authority() {
    let (root, paths) = resident_fixture("schema-rollout");
    let repository = Repository::discover(&root).expect("fixture repository resolves");
    let surface =
        crate::trust_command::Surface::load(&repository).expect("fixture trust surface loads");
    drop(journal::Journal::open(&paths.journal).expect("current authority initializes"));
    let connection = Connection::open(&paths.journal).expect("predecessor authority opens");
    connection
        .execute_batch(
            "UPDATE journal_schema SET version = 15 WHERE singleton = 1;
             PRAGMA user_version = 15;",
        )
        .expect("fixture becomes the requirement-aware predecessor schema");
    drop(connection);

    let journal = journal::Journal::open(&paths.journal).expect("successor migrates authority");
    let migration = journal
        .migration()
        .expect("successor observes one crossing");
    assert_eq!(
        (migration.from(), migration.to()),
        (15, journal::SCHEMA_VERSION)
    );
    let bus = Bus::start(journal).expect("successor bus starts");
    let first_epoch = test_uuid(61);
    commit_startup(&bus, &repository, &surface, first_epoch, Some(migration))
        .expect("successor startup commits");
    assert_eq!(migration_count(&bus), 1);
    assert_eq!(
        bus.residency_head()
            .expect("first residency reads")
            .expect("first residency exists")
            .body()
            .epoch,
        first_epoch,
    );
    bus.shutdown().expect("first successor bus stops");

    let journal = journal::Journal::open(&paths.journal).expect("authority reopens");
    assert_eq!(journal.migration(), None, "schema crossing is not repeated");
    let bus = Bus::start(journal).expect("replacement bus starts");
    let second_epoch = test_uuid(62);
    commit_startup(&bus, &repository, &surface, second_epoch, None)
        .expect("replacement startup commits");
    assert_eq!(
        migration_count(&bus),
        1,
        "restart duplicates no migration fact"
    );
    assert_eq!(
        bus.residency_head()
            .expect("replacement residency reads")
            .expect("replacement residency exists")
            .body()
            .epoch,
        second_epoch,
        "the replacement serves the same authority under a fresh epoch",
    );
    bus.verify().expect("replacement authority verifies");
    bus.shutdown().expect("replacement bus stops");
    fs::remove_dir_all(root).expect("schema rollout fixture cleans up");
}

fn migration_count(bus: &Bus) -> usize {
    bus.fact_snapshot()
        .expect("migration facts read")
        .facts()
        .iter()
        .filter(|envelope| envelope.fact.kind() == wire::FactKind::JournalMigrated)
        .count()
}
