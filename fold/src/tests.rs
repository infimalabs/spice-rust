use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{Connection, params};
use serde_json::json;

use crate::store::STORE_SCHEMA_VERSION;
use crate::{
    Availability, Beyond, Cursor, FactPlane, FoldId, Horizon, INVENTORY, Materialization,
    RebuildCause, Reducer, Reduction, Refusal, Source, Store, StoreError, TaskEffortReducer,
    task_effort_answer,
};

static NEXT_STORE: AtomicU64 = AtomicU64::new(0);

#[test]
fn inventory_contains_only_the_operational_typed_fold() {
    let definitions = INVENTORY.definitions();
    assert_eq!(FoldId::ALL, &[FoldId::TaskEffort]);
    assert_eq!(definitions.len(), 1);
    let definition = &definitions[0];
    assert_eq!(definition.fold, FoldId::TaskEffort);
    assert_eq!(definition.beyond, Beyond::Answer);
    assert_eq!(
        definition.sources,
        &[
            Source::Facts(FactPlane::Task),
            Source::Facts(FactPlane::Claim),
            Source::Facts(FactPlane::Lane),
            Source::Facts(FactPlane::Lineage),
            Source::Facts(FactPlane::Journal),
        ]
    );
    assert_eq!(
        definition.recovery_command(),
        "spice journal replay task::effort"
    );
}

#[test]
fn rebuild_reads_journal_without_mutating_authority() {
    let journal = journal::Journal::in_memory().expect("empty authority");
    let before = journal.verify().expect("authority verifies before rebuild");
    let mut store = Store::in_memory().expect("fold store");
    let outcome = store
        .read_or_rebuild(&journal, &KindReducer::new(FoldId::TaskEffort, 1))
        .expect("missing fold rebuilds");
    let after = journal.verify().expect("authority verifies after rebuild");

    assert_eq!(outcome.rebuilt, Some(RebuildCause::Missing));
    assert_eq!(outcome.materialization.cursor, Cursor::empty());
    assert_eq!(before, after);
}

#[test]
fn read_only_open_preserves_existing_store_bytes() {
    let path = store_path("read-only");
    let mut writer = Store::open(&path).expect("fold store");
    writer
        .read_or_rebuild_snapshot(&snapshot(1), &KindReducer::new(FoldId::TaskEffort, 1))
        .expect("first generation");
    drop(writer);
    let before = fs::read(&path).expect("read store bytes before inspection");

    let reader = Store::open_read_only(&path).expect("read-only fold store");
    assert!(
        reader
            .current(FoldId::TaskEffort)
            .expect("current generation")
            .is_some()
    );
    drop(reader);

    assert_eq!(
        fs::read(&path).expect("read store bytes after inspection"),
        before
    );
    fs::remove_file(path).expect("remove fixture store");
}

#[test]
fn stale_rebuild_advances_and_never_moves_the_cursor_backward() {
    let mut store = Store::in_memory().expect("fold store");
    let reducer = KindReducer::new(FoldId::TaskEffort, 1);
    let first = store
        .read_or_rebuild_snapshot(&snapshot(1), &reducer)
        .expect("first generation");
    let second = store
        .read_or_rebuild_snapshot(&snapshot(2), &reducer)
        .expect("stale generation rebuilds");
    let older_read = store
        .read_or_rebuild_snapshot(&snapshot(1), &reducer)
        .expect("older snapshot cannot lower current");

    assert_eq!(first.materialization.generation, 1);
    assert_eq!(second.rebuilt, Some(RebuildCause::Stale));
    assert_eq!(second.materialization.generation, 2);
    assert_eq!(second.materialization.cursor, Cursor::at(seq(2)));
    assert_eq!(older_read.rebuilt, None);
    assert_eq!(older_read.materialization, second.materialization);
}

#[test]
fn corrupt_materialization_rebuilds_in_one_new_generation() {
    let path = store_path("corrupt");
    let reducer = TaskEffortReducer;
    let mut store = Store::open(&path).expect("fold store");
    let first = store
        .read_or_rebuild_snapshot(&snapshot(1), &reducer)
        .expect("first generation");
    drop(store);

    let connection = Connection::open(&path).expect("open fixture directly");
    connection
        .execute(
            "UPDATE fold_generations SET payload = X'00' WHERE fold = ?1",
            params![FoldId::TaskEffort.as_str()],
        )
        .expect("corrupt payload");
    drop(connection);

    let mut store = Store::open(&path).expect("reopen fold store");
    let healed = store
        .read_or_rebuild_snapshot(&snapshot(1), &reducer)
        .expect("corrupt generation heals");
    assert_eq!(healed.rebuilt, Some(RebuildCause::Corrupt));
    assert_eq!(healed.materialization.generation, 2);
    assert_eq!(
        healed.materialization.payload(),
        first.materialization.payload()
    );
    assert_eq!(
        task_effort_answer(&healed.materialization),
        task_effort_answer(&first.materialization)
    );
    drop(store);
    fs::remove_file(path).expect("remove fixture store");
}

#[test]
fn deleting_the_disposable_store_rebuilds_the_same_typed_answer() {
    let path = store_path("deleted");
    let facts = snapshot(2);
    let mut store = Store::open(&path).expect("fold store");
    let first = store
        .read_or_rebuild_snapshot(&facts, &TaskEffortReducer)
        .expect("first typed generation")
        .materialization;
    drop(store);
    fs::remove_file(&path).expect("delete disposable materialization store");

    let mut rebuilt = Store::open(&path).expect("recreate fold store");
    let healed = rebuilt
        .read_or_rebuild_snapshot(&facts, &TaskEffortReducer)
        .expect("rebuild typed generation after deletion");
    assert_eq!(healed.rebuilt, Some(RebuildCause::Missing));
    assert_eq!(healed.materialization.cursor, first.cursor);
    assert_eq!(healed.materialization.horizon, first.horizon);
    assert_eq!(
        task_effort_answer(&healed.materialization),
        task_effort_answer(&first)
    );
    drop(rebuilt);
    fs::remove_file(path).expect("remove rebuilt fixture store");
}

#[test]
fn missing_head_rebuilds_past_an_orphaned_generation() {
    let path = store_path("missing-head");
    let reducer = KindReducer::new(FoldId::TaskEffort, 1);
    let mut store = Store::open(&path).expect("fold store");
    store
        .read_or_rebuild_snapshot(&snapshot(1), &reducer)
        .expect("first generation");
    drop(store);

    let connection = Connection::open(&path).expect("open fixture directly");
    connection
        .execute(
            "DELETE FROM fold_heads WHERE fold = ?1",
            params![FoldId::TaskEffort.as_str()],
        )
        .expect("remove current head");
    drop(connection);

    let mut store = Store::open(&path).expect("reopen fold store");
    let healed = store
        .read_or_rebuild_snapshot(&snapshot(1), &reducer)
        .expect("missing head heals");
    assert_eq!(healed.rebuilt, Some(RebuildCause::Missing));
    assert_eq!(healed.materialization.generation, 2);
    drop(store);
    fs::remove_file(path).expect("remove fixture store");
}

#[test]
fn replacement_keeps_old_readers_whole_and_payload_bytes_deterministic() {
    let facts = snapshot(2);
    let mut left = Store::in_memory().expect("left store");
    let mut right = Store::in_memory().expect("right store");
    let v1 = KindReducer::new(FoldId::TaskEffort, 1);
    let held = left
        .read_or_rebuild_snapshot(&facts, &v1)
        .expect("left first generation")
        .materialization;
    let same = right
        .read_or_rebuild_snapshot(&facts, &v1)
        .expect("right first generation")
        .materialization;
    assert_eq!(held.payload_bytes(), same.payload_bytes());

    let replacement = left
        .read_or_rebuild_snapshot(&facts, &KindReducer::new(FoldId::TaskEffort, 2))
        .expect("version change replaces generation")
        .materialization;
    assert_eq!(replacement.generation, 2);
    assert_eq!(held.generation, 1);
    assert_eq!(held.payload_bytes(), replacement.payload_bytes());
    assert_eq!(
        complete_read(left.read(FoldId::TaskEffort, seq(2))),
        replacement
    );
}

#[test]
fn a_replacement_may_never_retreat_the_applied_cursor() {
    let mut store = Store::in_memory().expect("fold store");
    let ahead = store
        .read_or_rebuild_snapshot(&snapshot(2), &KindReducer::new(FoldId::TaskEffort, 1))
        .expect("first generation");
    assert_eq!(ahead.materialization.cursor, Cursor::at(seq(2)));

    // A new reducer version is stale against any snapshot, so this rebuilds
    // rather than serving the current generation, and the replacement it
    // derives sits behind what the fold already applied.
    let retreat =
        store.read_or_rebuild_snapshot(&snapshot(1), &KindReducer::new(FoldId::TaskEffort, 2));
    assert_eq!(
        refusal(retreat),
        Refusal::CursorMovedBackward {
            fold: FoldId::TaskEffort,
            current: Cursor::at(seq(2)),
            next: Cursor::at(seq(1)),
        }
    );
    assert_eq!(
        complete_read(store.read(FoldId::TaskEffort, seq(2))),
        ahead.materialization
    );
}

#[test]
fn a_corrupt_generation_is_the_one_cause_allowed_to_retreat() {
    let path = store_path("retreat");
    let reducer = KindReducer::new(FoldId::TaskEffort, 1);
    let mut store = Store::open(&path).expect("fold store");
    store
        .read_or_rebuild_snapshot(&snapshot(2), &reducer)
        .expect("first generation");
    drop(store);

    let connection = Connection::open(&path).expect("open fixture directly");
    connection
        .execute(
            "UPDATE fold_generations SET payload = X'00' WHERE fold = ?1",
            params![FoldId::TaskEffort.as_str()],
        )
        .expect("corrupt payload");
    drop(connection);

    // Nothing trustworthy remains to retreat from, so a shorter snapshot heals.
    let mut store = Store::open(&path).expect("reopen fold store");
    let healed = store
        .read_or_rebuild_snapshot(&snapshot(1), &reducer)
        .expect("a corrupt generation heals to whatever authority now says");
    assert_eq!(healed.rebuilt, Some(RebuildCause::Corrupt));
    assert_eq!(healed.materialization.cursor, Cursor::at(seq(1)));
    drop(store);
    fs::remove_file(path).expect("remove fixture store");
}

#[test]
fn a_bounded_fold_marks_an_answer_that_trails_the_facts_it_applied() {
    let mut store = Store::in_memory().expect("fold store");
    let reducer = DeclaredReducer {
        fold: FoldId::TaskEffort,
        version: 1,
        cursor: Cursor::at(seq(3)),
        horizon: Horizon::at(seq(2)),
    };
    let published = store
        .read_or_rebuild_snapshot(&snapshot(3), &reducer)
        .expect("every fact applied, answer settled through an earlier position")
        .materialization;
    assert_eq!(published.cursor, Cursor::at(seq(3)));
    assert_eq!(published.horizon, Horizon::at(seq(2)));
    assert_ne!(published.cursor.position(), published.horizon.position());

    // The gap survives storage rather than being normalized away on read.
    let stored = complete_read(store.read(FoldId::TaskEffort, seq(2)));
    assert_eq!(stored.cursor, Cursor::at(seq(3)));
    assert_eq!(stored.horizon, Horizon::at(seq(2)));

    // Readers meet the gap through the inventory policy, not by rebuilding forever.
    assert_eq!(
        store
            .read(FoldId::TaskEffort, seq(3))
            .expect("read beyond the horizon")
            .expect("generation exists"),
        Availability::Marked {
            answer: published.clone(),
            horizon: Horizon::at(seq(2)),
        },
    );
    let reread = store
        .read_or_rebuild_snapshot(&snapshot(3), &reducer)
        .expect("second read");
    assert_eq!(reread.rebuilt, None, "a trailing horizon is not staleness");
    assert_eq!(reread.materialization, published);
}

#[test]
fn every_publication_refusal_is_reachable_and_named_by_value() {
    let mut store = Store::in_memory().expect("fold store");

    assert_eq!(
        refusal(store.read_or_rebuild_snapshot(&snapshot(1), &MismatchedReducer)),
        Refusal::ReducerSourcesMismatch {
            fold: FoldId::TaskEffort,
        }
    );

    // Declared out of order, before any store is involved.
    assert_eq!(
        Reduction::bounded(Cursor::at(seq(2)), Horizon::at(seq(3)), json!(null))
            .expect_err("completeness cannot outrun what was applied"),
        Refusal::HorizonPastCursor {
            cursor: Cursor::at(seq(2)),
            horizon: Horizon::at(seq(3)),
        }
    );

    // Facts handed over and left unapplied.
    assert_eq!(
        refusal(store.read_or_rebuild_snapshot(
            &snapshot(3),
            &DeclaredReducer {
                fold: FoldId::TaskEffort,
                version: 1,
                cursor: Cursor::at(seq(1)),
                horizon: Horizon::at(seq(1)),
            },
        )),
        Refusal::BehindSnapshot {
            applied: Cursor::at(seq(1)),
            required: Horizon::at(seq(3)),
        }
    );

    // Version zero is reserved and never reaches storage.
    assert_eq!(
        refusal(
            store.read_or_rebuild_snapshot(&snapshot(1), &KindReducer::new(FoldId::TaskEffort, 0))
        ),
        Refusal::ZeroReducerVersion {
            fold: FoldId::TaskEffort,
        }
    );

    // A position SQLite's signed column cannot hold.
    assert_eq!(
        refusal(store.read_or_rebuild_snapshot(
            &snapshot(0),
            &DeclaredReducer {
                fold: FoldId::TaskEffort,
                version: 1,
                cursor: Cursor::at(seq(u64::MAX)),
                horizon: Horizon::empty(),
            },
        )),
        Refusal::PositionOutOfRange {
            fold: FoldId::TaskEffort,
            value: u64::MAX,
        }
    );
}

#[test]
fn a_fold_out_of_generation_numbers_refuses_rather_than_wrapping() {
    let path = store_path("exhausted");
    let mut store = Store::open(&path).expect("fold store");
    store
        .read_or_rebuild_snapshot(&snapshot(1), &KindReducer::new(FoldId::TaskEffort, 1))
        .expect("first generation");
    drop(store);

    // Age the fold to the last generation number the column can hold, in the
    // order publication itself uses: write the successor, move the head, then
    // drop the predecessor. The head's foreign key holds at every step.
    let connection = Connection::open(&path).expect("open fixture directly");
    connection
        .execute_batch(&format!(
            "INSERT INTO fold_generations(fold, generation, reducer_version, cursor, horizon, payload)
                 SELECT fold, {max}, reducer_version, cursor, horizon, payload
                 FROM fold_generations WHERE fold = '{fold}';
             UPDATE fold_heads SET generation = {max} WHERE fold = '{fold}';
             DELETE FROM fold_generations WHERE fold = '{fold}' AND generation <> {max};",
            max = i64::MAX,
            fold = FoldId::TaskEffort.as_str(),
        ))
        .expect("age the fold to the last generation");
    drop(connection);

    let mut store = Store::open(&path).expect("reopen fold store");
    assert_eq!(
        refusal(
            store.read_or_rebuild_snapshot(&snapshot(1), &KindReducer::new(FoldId::TaskEffort, 2),)
        ),
        Refusal::GenerationExhausted {
            fold: FoldId::TaskEffort,
        }
    );
    drop(store);
    fs::remove_file(path).expect("remove fixture store");
}

#[test]
fn a_concurrent_reader_sees_one_whole_generation_across_a_publish() {
    let path = store_path("concurrent");
    let mut writer = Store::open(&path).expect("writer store");
    let first = writer
        .read_or_rebuild_snapshot(&snapshot(1), &KindReducer::new(FoldId::TaskEffort, 1))
        .expect("first generation")
        .materialization;

    let reader = Store::open(&path).expect("a second connection to the same store");
    assert_eq!(
        complete_read(reader.read(FoldId::TaskEffort, seq(1))),
        first
    );

    let second = writer
        .read_or_rebuild_snapshot(&snapshot(2), &KindReducer::new(FoldId::TaskEffort, 1))
        .expect("replacement generation")
        .materialization;
    assert_eq!(second.generation, 2);

    // The reader crosses the publish and lands on exactly one whole generation:
    // never the predecessor that was deleted, never a head without a row.
    let observed = complete_read(reader.read(FoldId::TaskEffort, seq(2)));
    assert_eq!(observed, second);
    assert_eq!(
        reader
            .read(FoldId::TaskEffort, seq(2))
            .expect("a corrupt or dangling head would surface here"),
        Some(Availability::Complete(second.clone()))
    );
    drop(reader);
    drop(writer);

    // Exactly one generation survives, and the head names it.
    let connection = Connection::open(&path).expect("open fixture directly");
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .expect("state the integrity this assertion rests on rather than inherit it");
    let rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM fold_generations WHERE fold = ?1",
            params![FoldId::TaskEffort.as_str()],
            |row| row.get(0),
        )
        .expect("count generations");
    assert_eq!(rows, 1, "predecessors are deleted, not accumulated");

    // Deleting the pointed-at row is refused, so no publication can ever leave
    // the head naming a generation that is gone.
    let orphan = connection.execute(
        "DELETE FROM fold_generations WHERE fold = ?1 AND generation = ?2",
        params![
            FoldId::TaskEffort.as_str(),
            i64::try_from(second.generation).expect("fixture generation fits"),
        ],
    );
    assert!(
        orphan.is_err(),
        "the head's foreign key must restrict deleting its own generation",
    );
    drop(connection);
    fs::remove_file(path).expect("remove fixture store");
}

struct KindReducer {
    fold: FoldId,
    version: u32,
}

struct MismatchedReducer;

impl Reducer for MismatchedReducer {
    fn fold(&self) -> FoldId {
        FoldId::TaskEffort
    }

    fn sources(&self) -> &'static [Source] {
        &[]
    }

    fn version(&self) -> u32 {
        1
    }

    fn rebuild(&self, _facts: &journal::FactSnapshot) -> Result<Reduction, String> {
        unreachable!("source admission runs before reduction")
    }
}

impl KindReducer {
    const fn new(fold: FoldId, version: u32) -> Self {
        Self { fold, version }
    }
}

impl Reducer for KindReducer {
    fn fold(&self) -> FoldId {
        self.fold
    }

    fn sources(&self) -> &'static [Source] {
        INVENTORY.definition(self.fold).sources
    }

    fn version(&self) -> u32 {
        self.version
    }

    fn rebuild(&self, facts: &journal::FactSnapshot) -> Result<Reduction, String> {
        let rows = facts
            .facts()
            .iter()
            .map(|fact| json!({"kind": fact.fact.kind().as_str(), "seq": fact.seq.get()}))
            .collect::<Vec<_>>();
        Ok(Reduction::complete(facts, json!({"rows": rows})))
    }
}

fn snapshot(count: u64) -> journal::FactSnapshot {
    let facts = (1..=count)
        .map(|position| wire::Envelope {
            v: wire::WireVersion::CURRENT,
            seq: seq(position),
            at: "2030-04-12T18:44:09.271Z"
                .parse::<wire::Instant>()
                .expect("fixture instant"),
            actor: wire::Actor::Spice,
            origin: "task:1kKRRsDP".parse().expect("fixture origin"),
            cause: (position > 1).then(|| seq(position - 1)),
            fact: opaque_note(),
        })
        .collect();
    journal::FactSnapshot::from_facts(facts).expect("fixture snapshot")
}

/// Read a typed fact back the way a journal verifier reads a stored row.
///
/// A fixture mints its body through the registered DTO rather than spelling
/// one out, so it can never carry fields the decoder would refuse.
fn opaque_note() -> wire::AnyFact {
    let fact = wire::Fact::new(wire::body::TaskNoted {
        task: "1kKRRsDP".parse().expect("fixture task stamp"),
        note: "fixture".to_owned(),
    });
    let bytes = wire::encode(&fact).expect("a typed fact encodes");
    wire::decode(&bytes).expect("a stored fact reads back without its body type")
}

/// A reducer that declares exactly the boundaries a fixture hands it.
///
/// [`KindReducer`] can only produce complete reductions, so it can never reach
/// the publication rules that exist for reducers whose answer trails the facts.
struct DeclaredReducer {
    fold: FoldId,
    version: u32,
    cursor: Cursor,
    horizon: Horizon,
}

impl Reducer for DeclaredReducer {
    fn fold(&self) -> FoldId {
        self.fold
    }

    fn sources(&self) -> &'static [Source] {
        INVENTORY.definition(self.fold).sources
    }

    fn version(&self) -> u32 {
        self.version
    }

    fn rebuild(&self, _facts: &journal::FactSnapshot) -> Result<Reduction, String> {
        Reduction::bounded(self.cursor, self.horizon, json!({"declared": true}))
            .map_err(|refusal| format!("{refusal:?}"))
    }
}

fn complete_read(
    outcome: Result<Option<Availability<Materialization>>, StoreError>,
) -> Materialization {
    match outcome {
        Ok(Some(Availability::Complete(materialization))) => materialization,
        other => panic!("expected a complete fold answer, got {other:?}"),
    }
}

/// The one refusal a store returned, or a panic naming what it did instead.
fn refusal<T: std::fmt::Debug>(outcome: Result<T, StoreError>) -> Refusal {
    match outcome {
        Err(StoreError::Refused(refusal)) => refusal,
        other => panic!("expected a publication refusal, got {other:?}"),
    }
}

fn seq(value: u64) -> wire::Seq {
    wire::Seq::new(value).expect("nonzero fixture seq")
}

fn store_path(label: &str) -> PathBuf {
    let unique = NEXT_STORE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "spice-fold-{label}-{}-{unique}.sqlite3",
        std::process::id()
    ))
}

#[test]
fn every_declared_fact_plane_claims_at_least_one_fact_kind() {
    for plane in FactPlane::ALL {
        assert!(
            wire::FactKind::ALL.iter().any(|kind| plane.contains(*kind)),
            "fact plane {plane:?} claims no fact kind",
        );
    }
}

/// A fresh store seeds whatever version the caller declares as current.
///
/// This is the falsifier for a version written into the SQL: a literal seeds
/// its own number, so opening a brand-new store under a bumped constant would
/// fail as a version disagreement on a store that has nothing to migrate.
#[test]
fn a_fresh_store_seeds_the_version_it_is_opened_at() {
    let connection = Connection::open_in_memory().expect("in-memory connection");

    crate::store::baseline(&connection, STORE_SCHEMA_VERSION + 1)
        .expect("a fresh store opens at a bumped version");

    assert_eq!(seeded_version(&connection), STORE_SCHEMA_VERSION + 1);
}

/// A store an older version wrote keeps its number and is refused, so the bump
/// above cannot be mistaken for silently rewriting existing data.
#[test]
fn a_store_written_by_another_version_is_refused_rather_than_reseeded() {
    let connection = Connection::open_in_memory().expect("in-memory connection");
    crate::store::baseline(&connection, STORE_SCHEMA_VERSION).expect("a fresh store opens");

    let refusal = crate::store::baseline(&connection, STORE_SCHEMA_VERSION + 1);

    assert!(matches!(
        refusal,
        Err(StoreError::SchemaVersion { found, current })
            if found == STORE_SCHEMA_VERSION && current == STORE_SCHEMA_VERSION + 1
    ));
    assert_eq!(seeded_version(&connection), STORE_SCHEMA_VERSION);
}

fn seeded_version(connection: &Connection) -> u32 {
    connection
        .query_row(
            "SELECT version FROM fold_schema WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .expect("the schema row is seeded")
}
