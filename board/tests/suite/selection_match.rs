//! The claimant's match score is the last in-band key before the handle: it
//! separates rows that tie on every ladder rung above it and nothing else.

use board::Entry;
use wire::{
    Actor, CanonicalUuid, Instant, Phase, Seq, TaskHead, TaskPriority, TaskStamp, WorkflowHead,
};

fn stamp(text: &str) -> TaskStamp {
    let value = text.bytes().fold(0_i64, |value, byte| {
        (value * 131 + i64::from(byte)) % 10_000_000_000_000
    });
    TaskStamp::for_micros(value).expect("fixture name fits the task stamp codec")
}

fn phase(text: &str) -> Phase {
    text.parse().expect("phase")
}

fn environment() -> wire::Environment {
    wire::Environment::journal_schema(journal::SCHEMA_VERSION)
        .expect("the compiled journal schema is positive")
}

fn projects() -> board::ProjectPolicy {
    board::ProjectPolicy::new(
        Vec::new(),
        dialect::PROJECT_MIN_DEPTH as u64,
        dialect::PROJECT_MAX_DEPTH as u64,
    )
    .expect("fixture project policy")
}

fn all_projects() -> wire::TeamSubscription {
    wire::TeamSubscription {
        team: "019fbbde-5105-75a3-a76a-722afde1e408"
            .parse::<CanonicalUuid>()
            .expect("canonical UUID"),
        formed_at: None,
        policy: wire::intent::TeamLife::Drain,
        fast_mode: false,
    }
}

fn entry(name: &str, project: &str, queue_origin: u64, priority: Option<TaskPriority>) -> Entry {
    let head = TaskHead {
        preparation: None,
        intent: None,
        born: stamp(name),
        queue_origin: Seq::new(queue_origin).expect("non-zero sequence"),
        title: format!("Implement {name}"),
        pitch: "Selection must follow the ladder.".to_owned(),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        project: project.to_owned(),
        filing: wire::TaskFiling::Filed,
        priority,
        taste: None,
        document_slug: None,
        taste_proposal: None,
        dependencies: Vec::new(),
        provenance: wire::TaskProvenance::Task(stamp(name)),
        creation: wire::body::TaskCreationContext::unavailable(
            wire::body::TaskCreationUnavailable::SpiceActor,
        ),
        revision: Seq::new(1).expect("non-zero sequence"),
    };
    let workflow = WorkflowHead {
        flow: vec![phase("todo"), phase("review")],
        phase: phase("todo"),
        status: wire::WorkflowStatus::pending(),
        deferral: None,
        generation: 0,
    };
    Entry::new(head, workflow, None, None, &projects()).expect("placed candidate")
}

fn now() -> Instant {
    "2026-08-01T06:00:00.000Z"
        .parse()
        .expect("canonical instant")
}

/// Select with a score that prefers the named handle and ranks every other row zero.
fn select<'a>(
    entries: &'a [Entry],
    recent: Option<&wire::ClaimCell>,
    band: u64,
    favored: Option<&str>,
) -> &'a Entry {
    let subscription = all_projects();
    let environment = environment();
    let score = |entry: &Entry| f64::from(favored == Some(entry.handle().as_str()));
    let match_score: Option<&dyn Fn(&Entry) -> f64> = favored.is_some().then_some(&score);
    board::select_with_satisfied_at(
        entries,
        board::Claimant {
            actor: &Actor::Spice,
            subscription: &subscription,
            recent_cell: recent,
            environment: &environment,
            match_score,
        },
        band,
        &board::resolved_tasks(entries),
        now(),
    )
    .expect("one candidate is selectable")
    .entry()
}

#[test]
fn among_in_band_rows_tied_on_locality_the_higher_score_wins_in_either_slice_order() {
    let handle_first = entry("BOARD-a1", "board.alpha", 1, None);
    let handle_last = entry("BOARD-z1", "board.alpha", 1, None);
    let favored = handle_last.handle().as_str().to_owned();

    for entries in [
        [handle_first.clone(), handle_last.clone()],
        [handle_last.clone(), handle_first.clone()],
    ] {
        let selected = select(&entries, None, 2, Some(&favored));
        assert_eq!(selected.born(), handle_last.born());
    }
}

#[test]
fn a_lower_movement_row_beats_a_higher_scored_one() {
    let recent = wire::ClaimCell {
        project: "board.alpha".to_owned(),
        phase: phase("todo"),
    };
    let local = entry("BOARD-z1", "board.alpha", 1, None);
    let moved = entry("BOARD-a1", "board.beta", 1, None);
    let favored = moved.handle().as_str().to_owned();
    let entries = [moved.clone(), local.clone()];

    let selected = select(&entries, Some(&recent), 2, Some(&favored));

    assert_eq!(selected.born(), local.born());
}

#[test]
fn priority_and_out_of_band_age_decide_before_any_score() {
    let urgent = entry("BOARD-z1", "board.alpha", 1, Some(TaskPriority::High));
    let favored_ordinary = entry("BOARD-a1", "board.alpha", 1, None);
    let favored = favored_ordinary.handle().as_str().to_owned();
    let entries = [favored_ordinary.clone(), urgent.clone()];
    assert_eq!(
        select(&entries, None, 2, Some(&favored)).born(),
        urgent.born()
    );

    let oldest = entry("BOARD-z2", "board.alpha", 1, None);
    let favored_young = entry("BOARD-a2", "board.alpha", 40, None);
    let favored = favored_young.handle().as_str().to_owned();
    let entries = [favored_young, oldest.clone()];
    assert_eq!(
        select(&entries, None, 2, Some(&favored)).born(),
        oldest.born()
    );
}

#[test]
fn an_absent_score_selects_exactly_what_the_handle_ladder_selects() {
    let handle_first = entry("BOARD-a1", "board.alpha", 1, None);
    let handle_last = entry("BOARD-z1", "board.alpha", 1, None);
    let entries = [handle_last.clone(), handle_first.clone()];

    let scored = select(&entries, None, 2, Some("never-a-handle"));
    let unscored = select(&entries, None, 2, None);

    assert_eq!(scored.born(), handle_first.born());
    assert_eq!(unscored.born(), handle_first.born());
}
