use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{Connection, params};
use serde_json::json;

use crate::{
    Availability, Beyond, Cursor, FactPlane, FoldId, HeadSource, Horizon, INVENTORY, RebuildCause,
    Reducer, Reduction, RingId, Source, Store,
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

    for kind in wire::FactKind::ALL {
        assert!(
            definitions.iter().any(|definition| definition
                .sources
                .iter()
                .any(|source| source_contains(*source, *kind))),
            "fact kind {kind} has no consuming fold",
        );
    }
    assert_source_sets(definitions);
    assert_fold_order(definitions);
}

fn source_contains(source: Source, kind: wire::FactKind) -> bool {
    match source {
        Source::Fact(source) => source == kind,
        Source::Facts(plane) => plane.contains(kind),
        Source::Fold(_)
        | Source::Head(_)
        | Source::Ring(_)
        | Source::Config(_)
        | Source::Tape(_)
        | Source::CommandResults => false,
    }
}

fn assert_source_sets(definitions: &[crate::Definition]) {
    let heads = definitions
        .iter()
        .flat_map(|definition| definition.sources)
        .filter_map(|source| match source {
            Source::Head(head) => Some(*head),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(heads, HeadSource::ALL.iter().copied().collect());

    let rings = definitions
        .iter()
        .flat_map(|definition| definition.sources)
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
    assert_eq!(rings, RingId::ALL.iter().copied().collect());
    assert_eq!(rings, declared);
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
            fact: any_fact(),
        })
        .collect();
    journal::FactSnapshot::from_facts(facts).expect("fixture snapshot")
}

/// One opaque fact, built the only way a consumer can build one.
///
/// `AnyFact` keeps its fields private so that a decoded body is always one the
/// kind's registered DTO admitted. A fixture cannot assemble the pair directly
/// without asserting a validation that never ran, so it decodes like everyone
/// else.
fn any_fact() -> wire::AnyFact {
    wire::decode(br#"{"kind":"task::noted","task":"FOLD-a1","note":"fixture"}"#)
        .expect("fixture fact decodes")
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
fn every_fact_plane_is_represented_by_the_closed_vocabulary() {
    let represented = wire::FactKind::ALL
        .iter()
        .flat_map(|kind| {
            [
                FactPlane::Task,
                FactPlane::Claim,
                FactPlane::Document,
                FactPlane::Steering,
                FactPlane::Consent,
                FactPlane::Lane,
                FactPlane::Taste,
                FactPlane::Canary,
                FactPlane::Vendor,
                FactPlane::Trust,
                FactPlane::Commit,
                FactPlane::Gate,
                FactPlane::Journal,
                FactPlane::Resident,
                FactPlane::Proof,
                FactPlane::Release,
            ]
            .into_iter()
            .filter(|plane| plane.contains(*kind))
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(represented.len(), 16);
}
