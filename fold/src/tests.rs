use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{Connection, params};
use serde_json::json;

use crate::store::STORE_SCHEMA_VERSION;
use crate::{
    Availability, Beyond, ConfigSource, Cursor, FactPlane, FoldId, HeadSource, Horizon, INVENTORY,
    RebuildCause, Reducer, Reduction, Refusal, RingId, Source, Store, StoreError, TapeSource,
};

static NEXT_STORE: AtomicU64 = AtomicU64::new(0);

#[test]
fn inventory_accounts_for_every_fold_fact_head_and_ring() {
    let definitions = INVENTORY.definitions();
    let folds = definitions
        .iter()
        .map(|definition| definition.fold)
        .collect::<BTreeSet<_>>();
    assert_eq!(folds, FoldId::ALL.iter().copied().collect());
    assert_eq!(definitions.len(), folds.len(), "fold rows are unique");
    for definition in definitions {
        assert_eq!(
            definition.recovery_command().as_deref(),
            Some(format!("spice journal replay {}", definition.fold).as_str())
        );
    }

    assert_source_sets(definitions);
    assert_fold_order(definitions);
}

fn assert_source_sets(definitions: &[crate::Definition]) {
    let sources = definitions
        .iter()
        .flat_map(|definition| definition.sources)
        .copied()
        .collect::<Vec<_>>();

    assert_every_variant_consumed(FactPlane::ALL, &sources, |source| match source {
        Source::Facts(plane) => Some(plane),
        _ => None,
    });
    assert_every_variant_consumed(HeadSource::ALL, &sources, |source| match source {
        Source::Head(head) => Some(head),
        _ => None,
    });
    assert_every_variant_consumed(RingId::ALL, &sources, |source| match source {
        Source::Ring(ring) => Some(ring),
        _ => None,
    });
    assert_every_variant_consumed(ConfigSource::ALL, &sources, |source| match source {
        Source::Config(config) => Some(config),
        _ => None,
    });
    assert_every_variant_consumed(TapeSource::ALL, &sources, |source| match source {
        Source::Tape(tape) => Some(tape),
        _ => None,
    });

    let rings = sources
        .iter()
        .filter_map(|source| match source {
            Source::Ring(ring) => Some(*ring),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let declared = INVENTORY
        .rings()
        .iter()
        .map(|definition| definition.ring)
        .collect::<BTreeSet<_>>();
    assert_eq!(rings, declared);
}

/// Every variant a closed vocabulary declares must be named by some fold.
///
/// The sweep reads the generated `ALL` rather than a list retyped here, so a
/// variant added to the enum arrives in this check on its own and fails by
/// name until a fold consumes it. A variant no fold names is dead vocabulary.
fn assert_every_variant_consumed<T: Copy + Eq + std::fmt::Debug>(
    declared: &[T],
    sources: &[Source],
    named: impl Fn(Source) -> Option<T>,
) {
    for variant in declared {
        assert!(
            sources
                .iter()
                .copied()
                .filter_map(&named)
                .any(|found| found == *variant),
            "{variant:?} is declared but no fold names it as a source",
        );
    }
}

fn assert_fold_order(definitions: &[crate::Definition]) {
    let positions = definitions
        .iter()
        .enumerate()
        .map(|(position, definition)| (definition.fold, position))
        .collect::<BTreeMap<_, _>>();
    for (position, definition) in definitions.iter().enumerate() {
        for source in definition.sources {
            if let Source::Fold(source) = source {
                assert!(
                    positions[source] < position,
                    "{} must follow source fold {source}",
                    definition.fold,
                );
            }
        }
    }
}

#[test]
fn horizon_policy_marks_or_waits_beyond_completeness() {
    let horizon = Horizon::at(seq(2));
    assert_eq!(
        horizon.availability(seq(2), Beyond::Wait),
        Availability::Complete
    );
    assert_eq!(
        horizon.availability(seq(3), Beyond::Wait),
        Availability::Wait
    );
    assert_eq!(
        horizon.availability(seq(3), Beyond::Answer),
        Availability::Marked(horizon)
    );
}

#[test]
fn rebuild_reads_journal_without_mutating_authority() {
    let journal = journal::Journal::in_memory().expect("empty authority");
    let before = journal.verify().expect("authority verifies before rebuild");
    let mut store = Store::in_memory().expect("fold store");
    let outcome = store
        .read_or_rebuild(&journal, &KindReducer::new(FoldId::SteerInbox, 1))
        .expect("missing fold rebuilds");
    let after = journal.verify().expect("authority verifies after rebuild");

    assert_eq!(outcome.rebuilt, Some(RebuildCause::Missing));
    assert_eq!(outcome.materialization.cursor, Cursor::empty());
    assert_eq!(before, after);
}

#[test]
fn stale_rebuild_advances_and_never_moves_the_cursor_backward() {
    let mut store = Store::in_memory().expect("fold store");
    let reducer = KindReducer::new(FoldId::CanvasFlow, 1);
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
    let reducer = KindReducer::new(FoldId::CanvasVerdicts, 1);
    let mut store = Store::open(&path).expect("fold store");
    let first = store
        .read_or_rebuild_snapshot(&snapshot(1), &reducer)
        .expect("first generation");
    drop(store);

    let connection = Connection::open(&path).expect("open fixture directly");
    connection
        .execute(
            "UPDATE fold_generations SET payload = X'00' WHERE fold = ?1",
            params![FoldId::CanvasVerdicts.as_str()],
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
    drop(store);
    fs::remove_file(path).expect("remove fixture store");
}

#[test]
fn missing_head_rebuilds_past_an_orphaned_generation() {
    let path = store_path("missing-head");
    let reducer = KindReducer::new(FoldId::CanvasMagnitude, 1);
    let mut store = Store::open(&path).expect("fold store");
    store
        .read_or_rebuild_snapshot(&snapshot(1), &reducer)
        .expect("first generation");
    drop(store);

    let connection = Connection::open(&path).expect("open fixture directly");
    connection
        .execute(
            "DELETE FROM fold_heads WHERE fold = ?1",
            params![FoldId::CanvasMagnitude.as_str()],
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
    let v1 = KindReducer::new(FoldId::ReleaseHistory, 1);
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
        .read_or_rebuild_snapshot(&facts, &KindReducer::new(FoldId::ReleaseHistory, 2))
        .expect("version change replaces generation")
        .materialization;
    assert_eq!(replacement.generation, 2);
    assert_eq!(held.generation, 1);
    assert_eq!(held.payload_bytes(), replacement.payload_bytes());
    assert_eq!(
        left.read(FoldId::ReleaseHistory)
            .expect("read current")
            .expect("generation exists"),
        replacement
    );
}

#[test]
fn a_replacement_may_never_retreat_the_applied_cursor() {
    let mut store = Store::in_memory().expect("fold store");
    let ahead = store
        .read_or_rebuild_snapshot(&snapshot(2), &KindReducer::new(FoldId::TasteActive, 1))
        .expect("first generation");
    assert_eq!(ahead.materialization.cursor, Cursor::at(seq(2)));

    // A new reducer version is stale against any snapshot, so this rebuilds
    // rather than serving the current generation, and the replacement it
    // derives sits behind what the fold already applied.
    let retreat =
        store.read_or_rebuild_snapshot(&snapshot(1), &KindReducer::new(FoldId::TasteActive, 2));
    assert_eq!(
        refusal(retreat),
        Refusal::CursorMovedBackward {
            fold: FoldId::TasteActive,
            current: Cursor::at(seq(2)),
            next: Cursor::at(seq(1)),
        }
    );
    assert_eq!(
        store
            .read(FoldId::TasteActive)
            .expect("read current")
            .expect("the refused publication left the old generation whole"),
        ahead.materialization
    );
}

#[test]
fn a_corrupt_generation_is_the_one_cause_allowed_to_retreat() {
    let path = store_path("retreat");
    let reducer = KindReducer::new(FoldId::TasteActive, 1);
    let mut store = Store::open(&path).expect("fold store");
    store
        .read_or_rebuild_snapshot(&snapshot(2), &reducer)
        .expect("first generation");
    drop(store);

    let connection = Connection::open(&path).expect("open fixture directly");
    connection
        .execute(
            "UPDATE fold_generations SET payload = X'00' WHERE fold = ?1",
            params![FoldId::TasteActive.as_str()],
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
fn a_bounded_fold_publishes_an_answer_that_trails_the_facts_it_applied() {
    let mut store = Store::in_memory().expect("fold store");
    let reducer = DeclaredReducer {
        fold: FoldId::LaneCanary,
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
    let stored = store
        .read(FoldId::LaneCanary)
        .expect("read current")
        .expect("generation exists");
    assert_eq!(stored.cursor, Cursor::at(seq(3)));
    assert_eq!(stored.horizon, Horizon::at(seq(2)));

    // Readers meet the gap through the horizon, not by rebuilding forever.
    assert_eq!(
        stored.horizon.availability(seq(2), Beyond::Answer),
        Availability::Complete
    );
    assert_eq!(
        stored.horizon.availability(seq(3), Beyond::Answer),
        Availability::Marked(Horizon::at(seq(2)))
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
                fold: FoldId::SteerInbox,
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
            store
                .read_or_rebuild_snapshot(&snapshot(1), &KindReducer::new(FoldId::ConsentQueue, 0))
        ),
        Refusal::ZeroReducerVersion {
            fold: FoldId::ConsentQueue,
        }
    );

    // A position SQLite's signed column cannot hold.
    assert_eq!(
        refusal(store.read_or_rebuild_snapshot(
            &snapshot(0),
            &DeclaredReducer {
                fold: FoldId::CanvasFlow,
                version: 1,
                cursor: Cursor::at(seq(u64::MAX)),
                horizon: Horizon::empty(),
            },
        )),
        Refusal::PositionOutOfRange {
            fold: FoldId::CanvasFlow,
            value: u64::MAX,
        }
    );
}

#[test]
fn a_fold_out_of_generation_numbers_refuses_rather_than_wrapping() {
    let path = store_path("exhausted");
    let mut store = Store::open(&path).expect("fold store");
    store
        .read_or_rebuild_snapshot(&snapshot(1), &KindReducer::new(FoldId::ReleaseHistory, 1))
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
            fold = FoldId::ReleaseHistory.as_str(),
        ))
        .expect("age the fold to the last generation");
    drop(connection);

    let mut store = Store::open(&path).expect("reopen fold store");
    assert_eq!(
        refusal(
            store.read_or_rebuild_snapshot(
                &snapshot(1),
                &KindReducer::new(FoldId::ReleaseHistory, 2),
            )
        ),
        Refusal::GenerationExhausted {
            fold: FoldId::ReleaseHistory,
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
        .read_or_rebuild_snapshot(&snapshot(1), &KindReducer::new(FoldId::CanvasVerdicts, 1))
        .expect("first generation")
        .materialization;

    let reader = Store::open(&path).expect("a second connection to the same store");
    assert_eq!(
        reader
            .read(FoldId::CanvasVerdicts)
            .expect("reader sees the first generation")
            .expect("generation exists"),
        first
    );

    let second = writer
        .read_or_rebuild_snapshot(&snapshot(2), &KindReducer::new(FoldId::CanvasVerdicts, 1))
        .expect("replacement generation")
        .materialization;
    assert_eq!(second.generation, 2);

    // The reader crosses the publish and lands on exactly one whole generation:
    // never the predecessor that was deleted, never a head without a row.
    let observed = reader
        .read(FoldId::CanvasVerdicts)
        .expect("reader crosses the publish")
        .expect("a head always points at a row that exists");
    assert_eq!(observed, second);
    assert_eq!(
        reader
            .read(FoldId::CanvasVerdicts)
            .expect("a corrupt or dangling head would surface here"),
        Some(second.clone())
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
            params![FoldId::CanvasVerdicts.as_str()],
            |row| row.get(0),
        )
        .expect("count generations");
    assert_eq!(rows, 1, "predecessors are deleted, not accumulated");

    // Deleting the pointed-at row is refused, so no publication can ever leave
    // the head naming a generation that is gone.
    let orphan = connection.execute(
        "DELETE FROM fold_generations WHERE fold = ?1 AND generation = ?2",
        params![
            FoldId::CanvasVerdicts.as_str(),
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

impl KindReducer {
    const fn new(fold: FoldId, version: u32) -> Self {
        Self { fold, version }
    }
}

impl Reducer for KindReducer {
    fn fold(&self) -> FoldId {
        self.fold
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
            origin: "task:FOLD-a1".parse().expect("fixture origin"),
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
        task: "FOLD-a1".parse().expect("fixture task handle"),
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

    fn version(&self) -> u32 {
        self.version
    }

    fn rebuild(&self, _facts: &journal::FactSnapshot) -> Result<Reduction, String> {
        Reduction::bounded(self.cursor, self.horizon, json!({"declared": true}))
            .map_err(|refusal| format!("{refusal:?}"))
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
