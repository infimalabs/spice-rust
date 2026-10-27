//! A review assignment never gates who may claim the row.
//!
//! The assignment lives exactly as long as the reviewing claim: free and lapse
//! both release it, and while that claim is live, possession alone keeps every
//! other lane out until one steals it. A pin retained beside a claim that no
//! longer possesses the row therefore admits every lane, by allocation,
//! capture, or steal.

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

/// One review row assigned to `reviewer()`, whose claim is in `disposition`.
fn review_row(disposition: ClaimDisposition) -> Entry {
    let statement = TaskHead {
        preparation: None,
        intent: None,
        born: stamp("BOARD-r1"),
        queue_origin: seq(1),
        title: "Review the assigned row".to_owned(),
        pitch: "An assigned review still needs a reviewer.".to_owned(),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        project: "board.kernel".to_owned(),
        filing: wire::TaskFiling::Filed,
        priority: None,
        taste: None,
        document_slug: None,
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
        status: wire::WorkflowStatus::assigned(reviewer()),
        deferral: None,
        generation: 0,
    };
    let claim = ClaimHead {
        actor: reviewer(),
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
    Entry::new(statement, workflow, Some(claim), Some(lease)).expect("review row")
}

/// Whether `actor` may take `row` by allocation, capture, and steal.
fn routes(row: &Entry, actor: &Actor) -> [bool; 3] {
    let satisfied = board::resolved_tasks(std::slice::from_ref(row));
    [
        board::select(std::slice::from_ref(row), actor, &drain(), &environment()).is_some(),
        board::capturable_entry(&satisfied, row, actor, &environment()),
        board::stealable_entry(&satisfied, row, actor, &environment()),
    ]
}

#[test]
fn a_retained_review_assignment_admits_every_lane_once_its_claim_ends() {
    for disposition in [ClaimDisposition::Freed, ClaimDisposition::Lapsed] {
        let row = review_row(disposition);
        for actor in [newcomer(), reviewer()] {
            assert_eq!(
                routes(&row, &actor),
                [true; 3],
                "{actor} after a {disposition:?} review claim"
            );
        }
    }
}

#[test]
fn a_live_review_claim_yields_only_to_a_steal() {
    let row = review_row(ClaimDisposition::Active);
    assert_eq!(routes(&row, &newcomer()), [false, false, true]);
}
