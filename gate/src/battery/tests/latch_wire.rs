//! The durable ledger reaching every bound a whole battery decides.
//!
//! `RepositoryExecutor::capture` reads the live ledger once and splits the
//! inventory it holds two ways: `CoreSource` decides repository documents
//! against it, and `MetricSource` decides file shape and routine complexity
//! against it. Both legs enforce one law — a latched key is held to its base
//! limit, because the breach that latched it already spent the elastic headroom
//! the ratchet lends — and each leg decides that law in its own module and is
//! proved there. What is proved here is the wire: that the inventory an arm
//! decides against is the one read from the live ledger, rather than an empty
//! list the run would never notice it was holding.
//!
//! Three consumers read that inventory, and a fixture following one of them
//! leaves the others free to be handed nothing. The document consumer and the
//! routine consumer are proved here; the file-shape one is proved beside the arm
//! that owns it, in `file_shape`. That split keeps one law to one fixture, and
//! it leaves this module holding the two ends the other cannot reach: the
//! document leg is the only reader of the `CoreSource` half of the split, and
//! the routine leg is decided in a call of its own, so a run that reached file
//! shape with the live ledger and routines with an empty slice passes every
//! fixture but this one.
//!
//! Every fixture writes two keys measuring exactly the same quantity and seeds
//! a prior breach for one of them. Same bytes, same rule, same actor, same run —
//! the only difference between the two rows is the ledger entry. An arm handed
//! no inventory measures them identically, which is what makes the difference
//! between the two rows the assertion rather than either row alone.

use super::*;
use crate::metrics::{
    EvidenceState as MetricState, LatchBinding, MetricId, MetricKey, StructuralArmEvidence,
};
use crate::state::{
    LatchEntry, LatchKind, LatchObservation, RenameMap, StateKey, current_state_path,
    predecessor_state_sources,
};

/// The document a prior run already latched.
const HELD_DOCUMENT: &str = "app/held.md";

/// Its twin, identical in every way the bound reads except the ledger.
const FREE_DOCUMENT: &str = "app/free.md";

/// A length above the base character cap and below every elastic one.
///
/// The length must sit strictly between the two limits or the fixture stops
/// measuring the latch: above both, each document breaches and the latch only
/// moves a number; below both, neither breaches and the latch moves nothing.
/// Between them the *standing* flips, so an arm that lost the inventory admits
/// a document this one refuses.
///
/// The tracked policy this fixture copies caps a document at this depth at
/// [`DOCUMENT_BASE_CHARACTERS`], lets a ratchet reach 30000, and varies that by
/// 5 percent in either direction, so no seed a path and actor can draw puts the
/// elastic limit below 28500. This length clears the base by 4000 and the
/// lowest reachable elastic limit by 4500, rather than being tuned to the one
/// seed this fixture's actor happens to draw.
const DOCUMENT_CHARACTERS: usize = 24_000;

/// The unvaried base cap a latched document at this depth is pulled back to.
///
/// Spelled out rather than left to the comparison against the twin's limit,
/// because that comparison is also satisfied by a run that varied the base or
/// picked some third limit below the elastic one. The contract is that a latch
/// spends the ratchet's headroom exactly, and only an equality says so. The
/// elastic limit has no such number to name — it is seeded per path and per
/// actor — which is why the row that keeps it is asserted by relation instead.
const DOCUMENT_BASE_CHARACTERS: u64 = 20_000;

/// The one file both fixture routines are declared in.
///
/// One file rather than two, because a routine bound resolves its actor inputs
/// from the file the routine lives in, and the elastic limit is seeded from
/// that path and the acting actor. Declaring both routines in one file
/// therefore hands them the same seed, which lets this fixture assert their
/// elastic limits are equal outright rather than merely ordered.
const ROUTINE_SOURCE: &str = "src/routines.rs";

/// The routine a prior run already latched.
const HELD_ROUTINE: &str = "held_routine";

/// Its twin, generated from the same body.
const FREE_ROUTINE: &str = "free_routine";

/// Body lines each fixture routine is generated with.
const ROUTINE_BODY_LINES: usize = 98;

/// The length both fixture routines measure.
///
/// A signature, [`ROUTINE_BODY_LINES`] statements, and a closing brace. Chosen
/// on the rule [`DOCUMENT_CHARACTERS`] follows: the tracked policy caps
/// `routine_length` at [`ROUTINE_BASE_LENGTH`], lets a ratchet reach 120, and
/// varies that by 5 percent in either direction, so no seed can put the elastic
/// limit below 114. This clears the base by 20 lines and stays 14 under that
/// floor, rather than being tuned to the seed this fixture happens to draw.
const ROUTINE_LINES: u64 = 100;

/// The unvaried base cap a latched routine is pulled back to.
const ROUTINE_BASE_LENGTH: u64 = 80;

/// The clock the seeding run is taken under.
///
/// Fixed rather than read from the wall, because a ledger written under one
/// clock and read under another would make the fixture depend on how long the
/// battery below it took to run.
const SEED_CLOCK: f64 = 1_000.0;

/// One fixture routine of exactly [`ROUTINE_LINES`] lines.
fn routine(name: &str) -> String {
    let body = "    let _ = 1;\n".repeat(ROUTINE_BODY_LINES);
    format!("pub fn {name}() {{\n{body}}}\n")
}

/// The bound decision the report publishes for one measured quantity.
fn published_bound<'report>(
    report: &'report BatteryReport,
    path: &str,
    metric: MetricId,
) -> &'report BoundEvidence {
    let name = metric.as_str();
    let mut matching =
        report.context().bounds().iter().filter(|bound| {
            bound.actor_inputs().candidate_path() == path && bound.metric() == name
        });
    let bound = matching
        .next()
        .unwrap_or_else(|| panic!("the battery published no {name} bound for {path:?}"));
    assert!(
        matching.next().is_none(),
        "{path:?} was decided more than once for {name} in one run"
    );
    bound
}

/// One arm's result within one complete battery report.
fn arm_result(report: &BatteryReport, gate: GateId) -> &BatteryArmResult {
    report
        .results()
        .iter()
        .find(|result| result.gate_id() == gate)
        .unwrap_or_else(|| panic!("the battery ran the {gate:?} arm"))
}

/// One structural family's evidence, whichever arm carries it.
fn structural_evidence(report: &BatteryReport, gate: GateId) -> &StructuralArmEvidence {
    let ArmEvidence::Metrics { report } = arm_result(report, gate).evidence() else {
        panic!("the {gate:?} arm projects structural metric evidence")
    };
    report
}

/// One measured row and the bound it was judged against.
///
/// Read from the arm rather than from the published context because the routine
/// decisions the complexity arm makes are not published there; the row is the
/// only place a routine's active limit is legible today.
fn measured_row<'report>(
    report: &'report BatteryReport,
    gate: GateId,
    key: &MetricKey,
    metric: MetricId,
) -> (u64, &'report LatchBinding) {
    let name = metric.as_str();
    let mut matching = structural_evidence(report, gate)
        .metrics()
        .iter()
        .filter(|row| row.metric() == metric && row.key() == key);
    let row = matching
        .next()
        .unwrap_or_else(|| panic!("{key:?} carries no {name} row"));
    assert!(
        matching.next().is_none(),
        "{key:?} carries more than one {name} row"
    );
    let MetricState::Measured { measurement } = row.state() else {
        panic!(
            "{key:?} was expected to measure {name}, but carried {:?}",
            row.state()
        )
    };
    let latch = measurement.latch().unwrap_or_else(|| {
        panic!("{key:?} measured {name} without the bound it was judged against")
    });
    (measurement.value(), latch)
}

/// A ledger entry naming a whole file.
fn path_entry(kind: LatchKind, path: &str) -> LatchEntry {
    LatchEntry::new(kind, StateKey::path(path).expect("path key"))
        .expect("a file latch is keyed by path")
}

/// A ledger entry naming one routine within a file.
fn function_entry(kind: LatchKind, path: &str, function: &str) -> LatchEntry {
    LatchEntry::new(
        kind,
        StateKey::function(path, function).expect("function key"),
    )
    .expect("a routine latch is keyed by path and function")
}

/// Record a prior breach for one key in the ledger the battery will read.
///
/// Written through the transaction the production writer uses rather than as
/// bytes, so a fixture cannot seed a ledger shape the loader would refuse. The
/// seeding bound is the fixture's own: the ledger stores entries, not the
/// bounds they were decided under, so all this observation has to do is breach.
fn seed_latch(repository: &Path, entry: &LatchEntry) {
    let path = entry.key().repo_path();
    let (worktree_git, common_git) = crate::tree::git_directories(repository)
        .expect("the fixture repository has a Git directory");
    let sources = predecessor_state_sources(&worktree_git, &common_git);
    let destination = current_state_path(&common_git);
    let mut state = CurrentState::load(&destination, &sources).expect("load the fixture ledger");
    state
        .begin_run(&RenameMap::default(), [path], SEED_CLOCK)
        .expect("begin the seeding run");
    state
        .reconcile([LatchObservation::new(
            entry.clone(),
            BoundRule::new("fixture-seed", "fixture-metric", 10, 15, false, false)
                .expect("seeding bound"),
            20,
            ActorInputs::new(path)
                .expect("seeding actor inputs")
                .with_active_actor(FIXTURE_BATTERY_ACTOR),
            0,
        )
        .expect("seeding observation")])
        .expect("reconcile the prior breach");
    state
        .finish()
        .expect("finish the seeding run")
        .accept()
        .expect("persist the seeded ledger");

    // Read the ledger back through the loader the battery uses. A seed that
    // silently landed nowhere would leave every assertion below measuring two
    // unlatched keys and agreeing with itself about it.
    let reloaded = CurrentState::load(&destination, &sources).expect("reload the seeded ledger");
    assert!(
        reloaded.before().latches().contains(entry),
        "the seeding run left {entry:?} unlatched, so the fixture would prove nothing"
    );
}

/// One battery run over two identical routines, one of them already latched.
///
/// Both routines are declared in a single file so they resolve their actor
/// inputs from one path and are seeded identically; see [`ROUTINE_SOURCE`].
fn latched_routine_report(fixture: &Fixture) -> BatteryReport {
    let repository = production_repository(&fixture.root);
    fs::write(
        repository.join(ROUTINE_SOURCE),
        format!("{}{}", routine(HELD_ROUTINE), routine(FREE_ROUTINE)),
    )
    .expect("fixture repository routines");
    // Staged rather than merely written, because the selection is the index.
    fixture_git(&repository, &["add", ROUTINE_SOURCE]);
    seed_latch(
        &repository,
        &function_entry(LatchKind::ComplexityLength, ROUTINE_SOURCE, HELD_ROUTINE),
    );
    battery_report(&repository)
}

/// Every complexity measurement one run judged to be over its active bound.
///
/// Collected across the whole family rather than looked up by key, because the
/// question a caller asks of it is which rows broke, and a lookup can only
/// confirm the ones it already named.
fn complexity_breaches(report: &BatteryReport) -> Vec<(MetricKey, MetricId)> {
    structural_evidence(report, GateId::Complexity)
        .metrics()
        .iter()
        .filter_map(|row| {
            let MetricState::Measured { measurement } = row.state() else {
                return None;
            };
            let latch = measurement.latch()?;
            latch
                .over_limit()
                .then(|| (row.key().clone(), row.metric()))
        })
        .collect()
}

/// A latched document loses the headroom its identical twin keeps.
///
/// Both documents are the same length, so `measured` is asserted equal before
/// anything else: it is what rules out a fixture that measured two different
/// files and read their difference as the latch. The limits are then compared
/// against each other rather than against spelled numbers, because the elastic
/// one is seeded per path and per actor and has no single value to name.
///
/// The arm's standing closes it. A bound that reported the breach and admitted
/// the tree anyway would satisfy every row-level assertion above, and a run
/// that failed for some unrelated reason would satisfy the standing alone, so
/// the reason is required to name the document that broke and to leave its twin
/// out of it.
#[test]
fn a_latched_document_is_held_to_its_base_limit_while_its_twin_keeps_the_elastic_one() {
    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    let document = "d".repeat(DOCUMENT_CHARACTERS);
    for path in [HELD_DOCUMENT, FREE_DOCUMENT] {
        fs::write(repository.join(path), &document).expect("fixture repository document");
    }
    // Staged rather than merely written, because the selection is the index.
    fixture_git(&repository, &["add", HELD_DOCUMENT, FREE_DOCUMENT]);
    seed_latch(
        &repository,
        &path_entry(LatchKind::RepoDocChars, HELD_DOCUMENT),
    );

    let report = battery_report(&repository);

    let held = published_bound(
        &report,
        HELD_DOCUMENT,
        MetricId::RepositoryDocumentCharacters,
    );
    let free = published_bound(
        &report,
        FREE_DOCUMENT,
        MetricId::RepositoryDocumentCharacters,
    );
    let characters = u64::try_from(DOCUMENT_CHARACTERS)
        .expect("a fixture document shorter than the address space");
    assert_eq!(
        (held.measured(), free.measured()),
        (characters, characters),
        "both documents were written to one length, so a difference between the \
         rows below is the ledger rather than the file"
    );
    assert_eq!(
        held.latch_transition(),
        "held",
        "the ledger entry the battery read reaches the decision as a held latch"
    );
    assert_eq!(
        free.latch_transition(),
        "clear",
        "and the document no ledger entry names is decided without one"
    );
    assert_eq!(
        held.active_limit(),
        DOCUMENT_BASE_CHARACTERS,
        "the latch pulled the document back to the base cap exactly, not to a varied one"
    );
    assert!(
        held.active_limit() < free.active_limit(),
        "the latch is what removed the headroom: held {} against free {} over the \
         same {characters} characters",
        held.active_limit(),
        free.active_limit()
    );
    assert!(
        held.measured() > held.active_limit(),
        "the held document breaks the base limit it was pulled back to: {characters} \
         characters against {}",
        held.active_limit()
    );
    assert!(
        free.measured() <= free.active_limit(),
        "while the elastic limit still admits the same {characters} characters: {}",
        free.active_limit()
    );

    let arm = arm_result(&report, GateId::RepoDocs);
    assert_eq!(
        arm.state(),
        BatteryState::Failed,
        "the arm fails through the document its own ledger held: {}",
        arm.reason()
    );
    assert!(
        arm.reason().contains(HELD_DOCUMENT) && !arm.reason().contains(FREE_DOCUMENT),
        "the refusal names the document that broke and not the one that fit: {}",
        arm.reason()
    );
}

/// A latched routine loses the length headroom its twin in the same file keeps.
///
/// The complexity leg decides its bound from the same inventory through a call
/// of its own, so neither the document fixture above nor the file-shape one in
/// `file_shape` reaches it: a run that handed the routine decision an empty
/// slice satisfies both of them completely.
///
/// Both routines are declared in one file, which is what lets the two elastic
/// limits be asserted equal rather than merely ordered. That equality is the
/// discriminator: nothing about these two routines differs except the ledger
/// entry, so an active limit that moved was moved by the latch.
#[test]
fn a_latched_routine_is_held_to_its_base_length_while_its_twin_keeps_the_elastic_one() {
    let fixture = Fixture::new();
    let report = latched_routine_report(&fixture);

    let key = |function: &str| {
        MetricKey::function(ROUTINE_SOURCE, function).expect("a fixture routine keys")
    };
    let (held_length, held) = measured_row(
        &report,
        GateId::Complexity,
        &key(HELD_ROUTINE),
        MetricId::RoutineLength,
    );
    let (free_length, free) = measured_row(
        &report,
        GateId::Complexity,
        &key(FREE_ROUTINE),
        MetricId::RoutineLength,
    );
    assert_eq!(
        (held_length, free_length),
        (ROUTINE_LINES, ROUTINE_LINES),
        "both routines were generated from one body, so a difference between the \
         rows below is the ledger rather than the source"
    );
    assert_eq!(
        held.transition(),
        "held",
        "the ledger entry the battery read reaches the routine decision as a held latch"
    );
    assert_eq!(
        free.transition(),
        "clear",
        "and the routine no ledger entry names is decided without one"
    );
    assert_eq!(
        held.ratchet_limit(),
        free.ratchet_limit(),
        "both routines are declared in one file and decided under one actor, so they \
         draw the same elastic limit and only the ledger can separate them"
    );
    assert_eq!(
        held.active_limit(),
        ROUTINE_BASE_LENGTH,
        "the latch pulled the routine back to the base cap exactly, not to a varied one"
    );
    assert_eq!(
        free.active_limit(),
        free.ratchet_limit(),
        "while the twin keeps the elastic limit the same file and actor produced"
    );
    assert!(
        held.over_limit(),
        "the held routine breaks the base limit it was pulled back to: {held_length} \
         lines against {}",
        held.active_limit()
    );
    assert!(
        !free.over_limit(),
        "while the elastic limit still admits the same {free_length} lines: {}",
        free.active_limit()
    );
}

/// A routine latch moves the one metric it names, and the arm follows that row.
///
/// The same run as the fixture above, asked the two questions that bound the
/// latch rather than measure it. A ledger keyed by routine alone rather than by
/// routine and metric would hold the cyclomatic row of the very same function,
/// and would satisfy every length assertion above while quietly demoting a
/// measurement no prior breach ever touched.
///
/// The standing is closed through the rows because this arm summarises its
/// refusal instead of naming what caused it: a run that failed for an unrelated
/// reason reaches the same state, and would carry a breach the inventory
/// asserted here does not admit.
#[test]
fn a_routine_latch_binds_one_metric_and_the_arm_fails_through_that_row() {
    let fixture = Fixture::new();
    let report = latched_routine_report(&fixture);
    let held = MetricKey::function(ROUTINE_SOURCE, HELD_ROUTINE).expect("a fixture routine keys");

    let (_, held_ccn) = measured_row(&report, GateId::Complexity, &held, MetricId::RoutineCcn);
    assert_eq!(
        held_ccn.transition(),
        "clear",
        "the length latch is keyed by kind: the same routine's cyclomatic bound keeps its headroom"
    );
    assert!(
        held_ccn.active_limit() > held_ccn.base_limit(),
        "so that bound still holds the elastic headroom: base {} against active {}",
        held_ccn.base_limit(),
        held_ccn.active_limit()
    );

    assert_eq!(
        complexity_breaches(&report),
        vec![(held, MetricId::RoutineLength)],
        "the latched routine's length is the only complexity measurement over its bound"
    );
    let arm = arm_result(&report, GateId::Complexity);
    assert_eq!(
        arm.state(),
        BatteryState::Failed,
        "so the arm fails through the routine its own ledger held: {}",
        arm.reason()
    );
}
