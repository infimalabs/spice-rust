use super::*;

use crate::resident::lane_surface::{AdvisoryRtkFixture, seed_task_claim};
use bus::CommitOutcome;
use journal::{Journal, JournalWrite};
use wire::{ClaimDisposition, LaneResidency, Origin, Producer, ResidentState, TaskHandle};

#[test]
fn finding_feedback_reaches_the_live_author_once_across_restart() {
    let fixture = AdvisoryRtkFixture::new();
    let authority = fixture.root.join("authority.sqlite3");
    let (bus, task) = reviewed_fixture(&fixture, &authority, true, wire::body::Verdict::Finding);
    let snapshot = bus.task_snapshot().expect("reviewed snapshot reads");

    assert!(
        reconcile(
            &bus,
            &fixture.paths,
            fixture.epoch,
            &snapshot,
            None,
            Duration::from_secs(3_600),
        )
        .expect("review feedback reconciles")
    );
    let facts = bus.fact_snapshot().expect("feedback facts read");
    let ledger = steer::Ledger::from_snapshot(&facts).expect("feedback steering replays");
    let pending = ledger.pending(fixture.lane);
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].text(),
        feedback_text(
            &TaskHandle::for_project("task.claim", &task),
            "missing current coverage\n\nexercise the live refusal",
        ),
    );

    bus.shutdown().expect("feedback bus stops");
    let reopened = Bus::start(Journal::open(&authority).expect("feedback journal reopens"))
        .expect("feedback bus restarts");
    let snapshot = reopened.task_snapshot().expect("restarted snapshot reads");
    assert!(
        !reconcile(
            &reopened,
            &fixture.paths,
            fixture.epoch,
            &snapshot,
            None,
            Duration::from_secs(3_600),
        )
        .expect("replayed review feedback stays deduplicated")
    );
    let facts = reopened
        .fact_snapshot()
        .expect("restarted feedback facts read");
    assert_eq!(
        steer::Ledger::from_snapshot(&facts)
            .expect("restarted feedback steering replays")
            .pending(fixture.lane)
            .len(),
        1,
    );
    reopened.shutdown().expect("restarted feedback bus stops");
}

#[test]
fn a_stopped_author_receives_no_review_feedback() {
    let fixture = AdvisoryRtkFixture::new();
    let authority = fixture.root.join("inactive-authority.sqlite3");
    let (bus, _) = reviewed_fixture(&fixture, &authority, false, wire::body::Verdict::Finding);
    let snapshot = bus.task_snapshot().expect("inactive snapshot reads");
    assert!(
        !reconcile(
            &bus,
            &fixture.paths,
            fixture.epoch,
            &snapshot,
            None,
            Duration::from_secs(3_600),
        )
        .expect("stopped author is inert")
    );
    assert!(
        steer::Ledger::from_snapshot(&bus.fact_snapshot().expect("inactive facts read"))
            .expect("inactive steering replays")
            .directives()
            .is_empty(),
    );
    bus.shutdown().expect("inactive feedback bus stops");
}

#[test]
fn a_clean_review_creates_no_feedback() {
    let fixture = AdvisoryRtkFixture::new();
    let authority = fixture.root.join("clean-authority.sqlite3");
    let (bus, _) = reviewed_fixture(&fixture, &authority, true, wire::body::Verdict::Clean);
    let snapshot = bus.task_snapshot().expect("clean snapshot reads");
    assert!(
        !reconcile(
            &bus,
            &fixture.paths,
            fixture.epoch,
            &snapshot,
            None,
            Duration::from_secs(3_600),
        )
        .expect("clean review is inert")
    );
    assert!(
        steer::Ledger::from_snapshot(&bus.fact_snapshot().expect("clean facts read"))
            .expect("clean steering replays")
            .directives()
            .is_empty(),
    );
    bus.shutdown().expect("clean feedback bus stops");
}

#[test]
fn every_finding_after_the_cursor_is_answered_from_one_fold_of_the_history() {
    let fixture = AdvisoryRtkFixture::new();
    let authority = fixture.root.join("twice-authority.sqlite3");
    let (bus, task) = reviewed_fixture(&fixture, &authority, true, wire::body::Verdict::Finding);
    // The author takes the task back, completes it again, and is reviewed again.
    claim_review(&bus, fixture.epoch, &task, fixture.lane, fixture.occupant);
    complete_author_phase(&bus, &task);
    claim_review(&bus, fixture.epoch, &task, uuid(20), uuid(21));
    complete_review(&bus, &task, uuid(20), wire::body::Verdict::Finding);
    let snapshot = bus.task_snapshot().expect("twice-reviewed snapshot reads");
    let first = snapshot
        .facts()
        .iter()
        .find(|envelope| envelope.fact.kind() == wire::FactKind::TaskReviewed)
        .map(|envelope| envelope.seq)
        .expect("the first review");
    let pending = |bus: &Bus| {
        steer::Ledger::from_snapshot(&bus.fact_snapshot().expect("feedback facts read"))
            .expect("feedback steering replays")
            .pending(fixture.lane)
            .len()
    };

    // Only the finding past the cursor is judged.
    assert!(
        reconcile(
            &bus,
            &fixture.paths,
            fixture.epoch,
            &snapshot,
            Some(first),
            Duration::from_secs(3_600),
        )
        .expect("the later finding reconciles")
    );
    assert_eq!(pending(&bus), 1);

    // From the start, the earlier finding is judged by its own phase's authors.
    assert!(
        reconcile(
            &bus,
            &fixture.paths,
            fixture.epoch,
            &snapshot,
            None,
            Duration::from_secs(3_600),
        )
        .expect("the earlier finding reconciles")
    );
    assert_eq!(pending(&bus), 2);
    bus.shutdown().expect("twice-reviewed bus stops");
}

fn reviewed_fixture(
    fixture: &AdvisoryRtkFixture,
    authority: &std::path::Path,
    author_live: bool,
    verdict: wire::body::Verdict,
) -> (Bus, TaskStamp) {
    let bus = Bus::start(Journal::open(authority).expect("feedback journal opens"))
        .expect("feedback bus starts");
    let reviewer_lane = uuid(20);
    let reviewer = uuid(21);
    seat_reviewers(&bus, fixture, reviewer_lane, reviewer, author_live);
    let task: TaskStamp = "1kKRRsDR".parse().expect("feedback task stamp");
    let now = now();
    let author = author_row(fixture, true);
    seed_task_claim(
        &bus,
        &task,
        &author,
        fixture.epoch,
        shifted(now, -60),
        shifted(now, 3_600),
        shifted(now, 7_200),
    );
    complete_author_phase(&bus, &task);
    claim_review(&bus, fixture.epoch, &task, reviewer_lane, reviewer);
    complete_review(&bus, &task, reviewer_lane, verdict);
    (bus, task)
}

fn seat_reviewers(
    bus: &Bus,
    fixture: &AdvisoryRtkFixture,
    reviewer_lane: CanonicalUuid,
    reviewer: CanonicalUuid,
    author_live: bool,
) {
    let rows = vec![
        author_row(fixture, author_live),
        LaneResidency {
            lane: reviewer_lane,
            team: reviewer_lane,
            name: Some("reviewer".to_owned()),
            worktree: fixture.root.display().to_string(),
            life: "drive".to_owned(),
            vendor: fixture.vendor.clone(),
            model: None,
            effort: None,
            fast_mode: None,
            occupant: Some(reviewer),
            lineage: vec![reviewer],
            capabilities: vec![uuid(22)],
            session: Some(session(fixture, reviewer_lane, reviewer)),
            disposition: "occupied".to_owned(),
        },
    ];
    let origin: Origin = format!("resident:{}", fixture.epoch)
        .parse()
        .expect("resident origin");
    let mut write = JournalWrite::new(Actor::Spice, origin, now(), Producer::CommandDoor);
    let seated = write
        .fact(
            wire::body::UpSeated {
                epoch: fixture.epoch,
            },
            FactCause::Root,
        )
        .expect("resident seating fact builds");
    let epoch = fixture.epoch;
    write.residency_head(seated, move |_| wire::ResidencyHead {
        epoch,
        state: ResidentState::Seated,
        topology_revision: 1,
        lanes: rows,
        subscriptions: Vec::new(),
        renewals: Vec::new(),
    });
    committed(bus.commit(write).expect("resident seating commits"));
}

fn author_row(fixture: &AdvisoryRtkFixture, live: bool) -> LaneResidency {
    LaneResidency {
        lane: fixture.lane,
        team: fixture.lane,
        name: Some("author".to_owned()),
        worktree: fixture.root.display().to_string(),
        life: "drive".to_owned(),
        vendor: fixture.vendor.clone(),
        model: None,
        effort: None,
        fast_mode: None,
        occupant: live.then_some(fixture.occupant),
        lineage: vec![fixture.occupant],
        capabilities: vec![fixture.capability],
        session: live.then(|| session(fixture, fixture.lane, fixture.occupant)),
        disposition: if live { "occupied" } else { "vacant" }.to_owned(),
    }
}

fn session(
    fixture: &AdvisoryRtkFixture,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> wire::LaneSession {
    wire::LaneSession {
        lane,
        occupant,
        worktree: fixture.root.display().to_string(),
        vendor: fixture.vendor.clone(),
        launch: None,
        epoch: fixture.epoch,
        stream: "session".to_owned(),
        identity: occupant.to_string(),
        locus: wire::TapeLocus {
            member: 0,
            offset: 0,
            length: 1,
        },
    }
}

fn complete_author_phase(bus: &Bus, task: &TaskStamp) {
    let snapshot = bus.task_snapshot().expect("author snapshot reads");
    let heads = &snapshot.tasks()[0];
    let claim = heads.claim().expect("author claim").body().clone();
    let mut write = JournalWrite::new(
        claim.actor.clone(),
        format!("task:{task}").parse().expect("task origin"),
        now(),
        Producer::CommandDoor,
    );
    let done = write
        .fact(
            wire::body::TaskDone {
                task: task.clone(),
                validation: "author validation".to_owned(),
                terminal: None,
                git: None,
                source: wire::body::SourceBoundary::unrecorded(),
            },
            FactCause::Root,
        )
        .expect("task completion builds");
    let mut workflow = heads.workflow().body().clone();
    workflow.flow = vec!["todo".parse().unwrap(), "review".parse().unwrap()];
    workflow.phase = "review".parse().expect("review phase");
    workflow.review = "unassigned".to_owned();
    write.workflow_head(task.clone(), done, move |_| workflow);
    write.claim_head(task.clone(), done, move |fact| wire::ClaimHead {
        fact,
        disposition: ClaimDisposition::Freed,
        ..claim
    });
    committed(bus.commit(write).expect("author completion commits"));
}

fn claim_review(
    bus: &Bus,
    epoch: CanonicalUuid,
    task: &TaskStamp,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) {
    let snapshot = bus.task_snapshot().expect("review claim snapshot reads");
    let heads = &snapshot.tasks()[0];
    let actor = Actor::Lane(lane);
    let at = now();
    let expiry = shifted(at, 3_600);
    let mut write = JournalWrite::new(
        actor.clone(),
        format!("task:{task}").parse().expect("task origin"),
        at,
        Producer::CommandDoor,
    );
    let won = write
        .fact(
            wire::body::ClaimWon {
                task: task.clone(),
                actor: actor.clone(),
                lane,
                occupant,
                epoch,
                cell: Some(wire::ClaimCell {
                    project: "canvas.message".to_owned(),
                    phase: "review".parse().expect("review phase"),
                }),
                until: expiry,
                git: None,
                recovery: None,
                source: wire::body::SourceBoundary::unrecorded(),
            },
            FactCause::Root,
        )
        .expect("review claim builds");
    let mut workflow = heads.workflow().body().clone();
    workflow.assignment = Some(actor.clone());
    workflow.review = "assigned".to_owned();
    write.workflow_head(task.clone(), won, move |_| workflow);
    let phase = "review".parse().expect("review phase");
    write.claim_head(task.clone(), won, move |fact| wire::ClaimHead {
        actor,
        fact,
        claimed: fact,
        cell: wire::ClaimCell {
            project: "canvas.message".to_owned(),
            phase,
        },
        lane,
        occupant,
        epoch,
        recovery: None,
        disposition: ClaimDisposition::Active,
        acknowledged: 0,
    });
    write.lease_head(task.clone(), won, move |_| wire::LeaseHead {
        start: at,
        renewal: at,
        expiry,
        maximum: expiry,
        epoch,
    });
    committed(bus.commit(write).expect("review claim commits"));
}

fn complete_review(bus: &Bus, task: &TaskStamp, lane: CanonicalUuid, verdict: wire::body::Verdict) {
    let snapshot = bus.task_snapshot().expect("review snapshot reads");
    let heads = &snapshot.tasks()[0];
    let claim = heads.claim().expect("review claim").body().clone();
    let actor = Actor::Lane(lane);
    let mut write = JournalWrite::new(
        actor.clone(),
        format!("task:{task}").parse().expect("task origin"),
        now(),
        Producer::CommandDoor,
    );
    let reviewed = write
        .fact(
            wire::body::TaskReviewed {
                task: task.clone(),
                reviewer: actor,
                verdict,
                finding: (verdict == wire::body::Verdict::Finding)
                    .then(|| "missing current coverage\n\nexercise the live refusal".to_owned()),
                note: None,
                followups: Vec::new(),
                git: None,
                source: wire::body::SourceBoundary::unrecorded(),
            },
            FactCause::Root,
        )
        .expect("review completion builds");
    let mut workflow = heads.workflow().body().clone();
    match verdict {
        wire::body::Verdict::Clean => {
            workflow.status = "completed".to_owned();
            workflow.review = "clean".to_owned();
        }
        wire::body::Verdict::Finding => {
            workflow.status = "pending".to_owned();
            workflow.review = "unassigned".to_owned();
            workflow.completion = None;
        }
    }
    workflow.assignment = None;
    write.workflow_head(task.clone(), reviewed, move |_| workflow);
    write.claim_head(task.clone(), reviewed, move |fact| wire::ClaimHead {
        fact,
        disposition: ClaimDisposition::Freed,
        ..claim
    });
    committed(bus.commit(write).expect("review completion commits"));
}

fn uuid(value: u64) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-{value:012x}")
        .parse()
        .expect("fixture UUID")
}

fn now() -> wire::Instant {
    wire::Instant::new(jiff::Timestamp::now()).expect("fixture instant")
}

fn shifted(at: wire::Instant, seconds: i64) -> wire::Instant {
    wire::Instant::new(
        at.get()
            .checked_add(jiff::SignedDuration::from_secs(seconds))
            .expect("fixture instant shifts"),
    )
    .expect("shifted fixture instant")
}

fn committed(outcome: CommitOutcome) {
    assert!(matches!(outcome, CommitOutcome::Committed(_)));
}
