//! A review assignment dies with the lapsed claim that carried it.
//!
//! The row this pins is the live one the defect was filed from: a workflow head
//! still assigning the review to one reviewer beside a claim head recording
//! that reviewer's possession as lapsed. Nobody holds the row, yet without this
//! law the allocator filters it out for every actor but the one that left.

use board::Entry;
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, ClaimHead, Instant, LeaseHead, Phase, Seq, TaskHead,
    TaskStamp, WorkflowHead,
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

fn uuid(text: &str) -> CanonicalUuid {
    text.parse().expect("canonical UUID")
}

fn instant(text: &str) -> Instant {
    text.parse().expect("canonical instant")
}

fn seq(value: u64) -> Seq {
    Seq::new(value).expect("non-zero sequence")
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

fn drain() -> wire::TeamSubscription {
    wire::TeamSubscription {
        team: uuid("019fbbde-5105-75a3-a76a-722afde1e408"),
        formed_at: None,
        policy: wire::intent::TeamLife::Drain,
        fast_mode: false,
    }
}

fn reviewer() -> Actor {
    Actor::Lane(uuid("019fbbde-5105-75a3-a76a-722afde1e4a1"))
}

fn newcomer() -> Actor {
    Actor::Lane(uuid("019fbbde-5105-75a3-a76a-722afde1e4b2"))
}

fn bystander() -> Actor {
    Actor::Lane(uuid("019fbbde-5105-75a3-a76a-722afde1e4c3"))
}

/// One review row assigned to `assignee`, with `holder`'s claim in `disposition`.
fn review_row(assignee: &Actor, holder: &Actor, disposition: ClaimDisposition) -> Entry {
    let statement = TaskHead {
        preparation: None,
        intent: None,
        born: stamp("BOARD-r1"),
        queue_origin: seq(1),
        title: "Review the lapsed assignment".to_owned(),
        pitch: "A lapsed assignment still needs its review.".to_owned(),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        project: "board.kernel".to_owned(),
        filing: wire::TaskFiling::Filed,
        priority: None,
        taste: None,
        document_slug: None,
        taste_proposal: None,
        dependencies: Vec::new(),
        provenance: wire::TaskProvenance::Task(stamp("BOARD-r1")),
        creation: wire::body::TaskCreationContext::unavailable(
            wire::body::TaskCreationUnavailable::SpiceActor,
        ),
        revision: seq(1),
    };
    let workflow = WorkflowHead {
        flow: vec![phase("todo"), phase("review")],
        phase: phase("review"),
        status: wire::WorkflowStatus::assigned(assignee.clone()),
        deferral: None,
        generation: 0,
    };
    let claim = ClaimHead {
        actor: holder.clone(),
        fact: seq(3),
        claimed: seq(3),
        cell: wire::ClaimCell {
            project: "board.kernel".to_owned(),
            phase: phase("review"),
        },
        lane: uuid("019fbbde-5105-75a3-a76a-722afde1e408"),
        occupant: uuid("019fbbde-5105-75a3-a76a-722afde1e409"),
        epoch: uuid("019fbbde-5105-75a3-a76a-722afde1e410"),
        recovery: None,
        disposition,
        acknowledged: 0,
    };
    let lease = LeaseHead {
        start: instant("2026-08-01T06:00:00.000Z"),
        renewal: instant("2026-08-01T06:00:00.000Z"),
        expiry: instant("2026-08-01T06:10:00.000Z"),
        maximum: instant("2026-08-01T06:30:00.000Z"),
        epoch: uuid("019fbbde-5105-75a3-a76a-722afde1e410"),
    };
    Entry::new(statement, workflow, Some(claim), Some(lease), &projects()).expect("review row")
}

fn eligible_for(entry: &Entry, actor: &Actor) -> bool {
    let satisfied = board::resolved_tasks(std::slice::from_ref(entry));
    board::eligible_entry(&satisfied, entry, actor, &environment())
}

fn selected_for(entry: &Entry, actor: &Actor) -> bool {
    board::select(
        std::slice::from_ref(entry),
        actor,
        &drain(),
        0,
        None,
        &environment(),
    )
    .is_some()
}

#[test]
fn a_review_whose_assignee_lapsed_is_handed_to_another_lane() {
    let row = review_row(&reviewer(), &reviewer(), ClaimDisposition::Lapsed);
    assert!(
        eligible_for(&row, &newcomer()),
        "a lapsed assignee no longer pins the review"
    );
    assert!(
        selected_for(&row, &newcomer()),
        "drain policy hands the lapsed review to the next lane"
    );
    assert!(
        eligible_for(&row, &reviewer()),
        "the assignee itself may still take its own lapsed review back"
    );
}

#[test]
fn a_review_whose_assignee_holds_or_freed_its_claim_stays_pinned() {
    for (disposition, reason) in [
        (
            ClaimDisposition::Active,
            "an active claim still possesses the row",
        ),
        (
            ClaimDisposition::Freed,
            "a pin that survives a free was not released by this lapse law",
        ),
    ] {
        let row = review_row(&reviewer(), &reviewer(), disposition);
        assert!(!eligible_for(&row, &newcomer()), "{reason}");
        assert!(!selected_for(&row, &newcomer()), "{reason}");
    }
}

#[test]
fn a_lapse_by_someone_other_than_the_assignee_does_not_unpin_the_review() {
    let row = review_row(&reviewer(), &bystander(), ClaimDisposition::Lapsed);
    assert!(
        !eligible_for(&row, &newcomer()),
        "the assignment outlives a lapse that was never the assignee's"
    );
    assert!(
        eligible_for(&row, &reviewer()),
        "the assignee is still the one lane the pin admits"
    );
}
