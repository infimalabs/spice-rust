//! The durable ledger reaching the bound a whole battery publishes.
//!
//! The repository-document arm holds a latched document to its base limit,
//! because the breach that latched it already spent the elastic headroom the
//! ratchet lends. That rule is decided inside `CoreSource`, and it is proved
//! there. What is proved here is the wire: that the latch inventory the battery
//! reads out of the live ledger is the one the arm decides against, rather than
//! an empty list the run would never notice it was holding.
//!
//! The fixture writes two documents of exactly the same length and seeds a
//! prior breach for one of them. Same bytes, same rule, same actor, same run —
//! the only difference between the two rows is the ledger entry. An arm handed
//! no inventory measures them identically, which is what makes the difference
//! between the two rows the assertion rather than either row alone.

use super::*;
use crate::state::{
    LatchEntry, LatchKind, LatchObservation, RenameMap, StateKey, current_state_path,
    predecessor_state_sources,
};

/// The document a prior run already latched.
const HELD: &str = "app/held.md";

/// Its twin, identical in every way the bound reads except the ledger.
const FREE: &str = "app/free.md";

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

/// The clock the seeding run is taken under.
///
/// Fixed rather than read from the wall, because a ledger written under one
/// clock and read under another would make the fixture depend on how long the
/// battery below it took to run.
const SEED_CLOCK: f64 = 1_000.0;

/// The bound decision the report publishes for one selected document.
fn document_bound<'report>(report: &'report BatteryReport, path: &str) -> &'report BoundEvidence {
    let mut matching = report
        .context()
        .bounds()
        .iter()
        .filter(|bound| bound.actor_inputs().candidate_path() == path);
    let bound = matching
        .next()
        .unwrap_or_else(|| panic!("the battery published no bound decision for {path:?}"));
    assert!(
        matching.next().is_none(),
        "{path:?} was decided more than once in one run"
    );
    bound
}

/// The repository-document arm's result within one complete battery report.
fn document_arm(report: &BatteryReport) -> &BatteryArmResult {
    report
        .results()
        .iter()
        .find(|result| result.gate_id() == GateId::RepoDocs)
        .expect("the battery ran the repository-document arm")
}

/// Record a prior breach for one document in the ledger the battery will read.
///
/// Written through the transaction the production writer uses rather than as
/// bytes, so a fixture cannot seed a ledger shape the loader would refuse. The
/// seeding bound is the fixture's own: the ledger stores entries, not the
/// bounds they were decided under, so all this observation has to do is breach.
fn seed_document_latch(repository: &Path, path: &str) {
    let (worktree_git, common_git) = crate::tree::git_directories(repository)
        .expect("the fixture repository has a Git directory");
    let sources = predecessor_state_sources(&worktree_git, &common_git);
    let destination = current_state_path(&common_git);
    let mut state = CurrentState::load(&destination, &sources).expect("load the fixture ledger");
    state
        .begin_run(&RenameMap::default(), [path], SEED_CLOCK)
        .expect("begin the seeding run");
    let entry = LatchEntry::new(
        LatchKind::RepoDocChars,
        StateKey::path(path).expect("path key"),
    )
    .expect("a document latch is keyed by path");
    state
        .reconcile([LatchObservation::new(
            entry.clone(),
            BoundRule::new("fixture-seed", "repo-doc-chars", 10, 15, false, false)
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
    // unlatched documents and agreeing with itself about it.
    let reloaded = CurrentState::load(&destination, &sources).expect("reload the seeded ledger");
    assert!(
        reloaded.before().latches().contains(&entry),
        "the seeding run left {path:?} unlatched, so the fixture would prove nothing"
    );
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
    for path in [HELD, FREE] {
        fs::write(repository.join(path), &document).expect("fixture repository document");
    }
    // Staged rather than merely written, because the selection is the index.
    fixture_git(&repository, &["add", HELD, FREE]);
    seed_document_latch(&repository, HELD);

    let report = battery_report(&repository);

    let held = document_bound(&report, HELD);
    let free = document_bound(&report, FREE);
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

    let arm = document_arm(&report);
    assert_eq!(
        arm.state(),
        BatteryState::Failed,
        "the arm fails through the document its own ledger held: {}",
        arm.reason()
    );
    assert!(
        arm.reason().contains(HELD) && !arm.reason().contains(FREE),
        "the refusal names the document that broke and not the one that fit: {}",
        arm.reason()
    );
}
