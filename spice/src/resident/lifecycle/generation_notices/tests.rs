use super::*;
use crate::resident::lane_surface::{LaunchFixture, seed_task_claim};
use bus::CommitOutcome;
use journal::{Journal, JournalWrite};
use wire::{LaneResidency, Origin, Producer, ResidentState};

#[test]
fn trailing_generations_project_private_claimant_notices_without_steering() {
    let fixture = LaunchFixture::new();
    let (bus, task) = claimed_fixture(&fixture);
    note(&bus, &task, Actor::Lane(fixture.lane));
    note(&bus, &task, Actor::Operator);
    note(&bus, &task, Actor::Lane(fixture.lane));
    let snapshot = bus.task_snapshot().expect("noted snapshot reads");
    let claim = snapshot.tasks()[0].claim().expect("claim head").body();

    let projected = notices(&bus, &snapshot).expect("private notices project");
    assert_eq!(
        projected
            .iter()
            .map(|(_, notice)| notice.generation)
            .collect::<Vec<_>>(),
        [2, 3],
        "the holder's current change was acknowledged, but changes after it trailed were not",
    );
    let handle = TaskHandle::for_project("task.claim", &task).to_string();
    let lane_editor = Actor::Lane(fixture.lane).to_string();
    assert_eq!(
        projected
            .iter()
            .map(|(lane, notice)| (
                *lane,
                notice.occupant,
                notice.task.as_str(),
                notice.claim,
                notice.editor.as_str(),
            ))
            .collect::<Vec<_>>(),
        [
            (
                fixture.lane,
                fixture.occupant,
                handle.as_str(),
                claim.claimed.get(),
                "operator",
            ),
            (
                fixture.lane,
                fixture.occupant,
                handle.as_str(),
                claim.claimed.get(),
                lane_editor.as_str(),
            ),
        ],
    );
    assert_eq!(
        notices(&bus, &snapshot).expect("duplicate observation projects"),
        projected,
        "the occupant driver can deduplicate a stable authority identity",
    );
    assert!(
        !reconcile(
            &bus,
            &fixture.paths,
            fixture.epoch,
            &snapshot,
            super::super::review_feedback::FindingWindow {
                cursor: bus.cursor().expect("cursor reads"),
                held: &mut BTreeSet::new(),
                now: current_instant().expect("clock reads"),
                lifetime: Duration::from_secs(3_600),
            },
        )
        .expect("private notice reconciliation succeeds"),
        "private delivery does not mutate durable authority",
    );
    assert!(
        bus.read(move |read| crate::steering_read::pending(read, Some(fixture.lane)))
            .expect("steering index reads")
            .is_empty(),
        "task authority reminders never enter the steering inbox",
    );
    finish(bus);
}

#[test]
fn acknowledgement_clears_prior_generations_and_a_later_edit_gets_a_new_identity() {
    let fixture = LaunchFixture::new();
    let (bus, task) = claimed_fixture(&fixture);
    note(&bus, &task, Actor::Operator);
    note(&bus, &task, Actor::Operator);
    let before = bus.task_snapshot().expect("changed snapshot reads");
    let prior = notices(&bus, &before).expect("prior notices project");
    assert_eq!(prior.len(), 2);

    acknowledge(&bus, &task);
    let acknowledged = bus.task_snapshot().expect("acknowledged snapshot reads");
    assert!(
        notices(&bus, &acknowledged)
            .expect("acknowledged notices project")
            .is_empty(),
        "successful acknowledgement retires every prior reminder",
    );

    note(&bus, &task, Actor::Operator);
    let changed = bus.task_snapshot().expect("later snapshot reads");
    let [(_, later)] = notices(&bus, &changed)
        .expect("later notice projects")
        .try_into()
        .expect("one later notice");
    assert!(
        prior
            .iter()
            .all(|(_, notice)| notice.generation != later.generation),
        "a later generation has a new delivery identity",
    );

    end_claim(&bus, &task);
    let released = bus.task_snapshot().expect("released snapshot reads");
    assert!(
        notices(&bus, &released)
            .expect("released notices project")
            .is_empty(),
        "a released claim receives no authority reminder",
    );
    finish(bus);
}

#[test]
fn only_the_lane_whose_write_opened_a_hold_hears_of_it_once() {
    let fixture = LaunchFixture::new();
    let (bus, task) = claimed_fixture(&fixture);
    let lane = Actor::Lane(fixture.lane);
    let hold = wire::TasteHold {
        plan: None,
        matches: vec![wire::TasteMatch {
            field: wire::StatementField::Title,
            line: 1,
            taste: "taskwording".to_owned(),
            finding: None,
        }],
        rechecks: 0,
        opened: None,
    };

    // The operator's policy read opens the hold; the lane then rewires the
    // task's dependencies, carrying the hold through without reading policy.
    let cursor = bus.cursor().expect("cursor reads");
    restate(&bus, &task, Actor::Operator, Hold::Read(Some(hold.clone())));
    restate(&bus, &task, lane.clone(), Hold::CarriedThrough);
    let snapshot = bus.task_snapshot().expect("carried snapshot reads");
    assert!(
        taste_holds(&bus, &snapshot, cursor)
            .expect("carried holds project")
            .is_empty(),
        "a lane that only carried another writer's hold is not told it wrote it",
    );

    // Cleared, then reopened by the lane's own policy-reading write, then
    // carried through once more by that lane's dependency edit.
    restate(&bus, &task, Actor::Operator, Hold::Read(None));
    let cursor = bus.cursor().expect("cursor reads");
    let opening = restate(&bus, &task, lane.clone(), Hold::Read(Some(hold)));
    restate(&bus, &task, lane, Hold::CarriedThrough);
    let snapshot = bus.task_snapshot().expect("reopened snapshot reads");
    let [(told, notice)] = taste_holds(&bus, &snapshot, cursor)
        .expect("hold notices project")
        .try_into()
        .expect("the opening write's lane hears the hold once");
    assert_eq!((told, notice.occupant), (fixture.lane, fixture.occupant));
    assert_eq!(
        notice.position,
        opening.get(),
        "the notice names the write that opened the hold, not the later carrier",
    );
    assert_eq!(
        notice.task,
        TaskHandle::for_project("task.claim", &task).to_string()
    );
    assert_eq!(notice.matches, ["title line 1 taste taskwording"]);
    assert!(
        taste_holds(&bus, &snapshot, bus.cursor().expect("cursor reads"))
            .expect("observed holds project")
            .is_empty(),
        "a hold observed by an earlier pass is not announced again",
    );
    assert!(
        taste_holds(&bus, &snapshot, None)
            .expect("first pass projects")
            .is_empty(),
        "the first pass has no earlier observation to announce against",
    );
    finish(bus);
}

#[test]
fn resident_minted_claim_projects_one_occupant_scoped_assignment() {
    let fixture = LaunchFixture::new();
    let (bus, task) = automatic_claimed_fixture(&fixture);
    let snapshot = bus.task_snapshot().expect("automatic claim snapshot reads");
    let claim = snapshot.tasks()[0].claim().expect("claim head").body();
    let handle = TaskHandle::for_project("task.claim", &task).to_string();

    let projected = assignment_notices(&snapshot);
    assert_eq!(
        projected
            .iter()
            .map(|(lane, notice)| (*lane, notice.occupant, notice.task.as_str(), notice.claim))
            .collect::<Vec<_>>(),
        [(
            fixture.lane,
            fixture.occupant,
            handle.as_str(),
            claim.claimed.get()
        )],
    );
    assert_eq!(
        assignment_notices(&snapshot),
        projected,
        "stable authority can be retried until the occupant driver receives it",
    );

    let observed = bus.cursor().expect("cursor reads");
    end_claim(&bus, &task);
    let released = bus.task_snapshot().expect("released snapshot reads");
    assert!(
        assignment_notices(&released).is_empty(),
        "ended possession is not announced as an assignment",
    );
    assert_eq!(
        assignment_ends(&released, observed),
        [(
            fixture.lane,
            TaskAssignmentEnd {
                occupant: fixture.occupant,
                task: TaskHandle::for_project("task.claim", &task).to_string(),
                claim: claim.claimed.get(),
            },
        )],
        "an ending after the previous pass is told to the occupant that held the claim",
    );
    assert!(
        assignment_ends(&released, bus.cursor().expect("cursor reads")).is_empty(),
        "an ending the previous pass already observed is not told again",
    );
    assert!(
        assignment_ends(&released, None).is_empty(),
        "a first pass has no earlier observation to tell endings against",
    );
    finish(bus);
}

#[test]
fn retired_steering_notices_are_migrated_once_before_runtime_startup() {
    let fixture = LaunchFixture::new();
    let (bus, _) = claimed_fixture(&fixture);
    let (mut proposal, published) =
        crate::resident::steering::internal_write(fixture.epoch).expect("internal write builds");
    for control in [
        "task-generation:1kKRRsDQ:33:4".to_owned(),
        format!("lane-renewal-handoff:{}", fixture.occupant),
        "ordinary-control".to_owned(),
    ] {
        crate::resident::steering::append_internal_directive(
            &bus,
            &mut proposal,
            fixture.epoch,
            crate::resident::steering::InternalDirective {
                directive: crate::resident::new_uuid(),
                lane: fixture.lane,
                text: control.clone(),
                control,
                lifetime: Duration::from_secs(3_600),
                cause: FactCause::Root,
                published,
            },
        )
        .expect("retained directive builds");
    }
    committed(bus.commit(proposal).expect("retained directives commit"));
    assert_eq!(
        bus.read(move |read| crate::steering_read::pending(read, Some(fixture.lane)))
            .expect("steering index reads")
            .len(),
        3,
        "predecessor evidence starts live",
    );
    crate::resident::steering::migrate_private_notices(&bus, fixture.epoch)
        .expect("startup migration succeeds");
    let migrated = bus.cursor().expect("migrated cursor reads");
    crate::resident::steering::migrate_private_notices(&bus, fixture.epoch)
        .expect("completed migration is inert");
    assert_eq!(
        bus.cursor().expect("repeated migration cursor reads"),
        migrated
    );
    let pending = bus
        .read(move |read| crate::steering_read::pending(read, Some(fixture.lane)))
        .expect("steering index reads");
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].body().control.as_deref(),
        Some("ordinary-control")
    );
    assert_eq!(pending[0].body().text, "ordinary-control");
    finish(bus);
}

fn claimed_fixture(fixture: &LaunchFixture) -> (Bus, TaskStamp) {
    let at = now();
    claimed_fixture_with_deadlines(
        fixture,
        shifted(at, -60),
        shifted(at, 3_600),
        shifted(at, 7_200),
    )
}

fn automatic_claimed_fixture(fixture: &LaunchFixture) -> (Bus, TaskStamp) {
    let at = now();
    claimed_fixture_with_deadlines(fixture, at, shifted(at, 60), shifted(at, 3_600))
}

fn claimed_fixture_with_deadlines(
    fixture: &LaunchFixture,
    start: Instant,
    expiry: Instant,
    maximum: Instant,
) -> (Bus, TaskStamp) {
    let bus = Bus::start(Journal::in_memory().expect("fixture journal opens"))
        .expect("fixture bus starts");
    let row = LaneResidency {
        lane: fixture.lane,
        team: fixture.lane,
        name: Some("claimant".to_owned()),
        worktree: fixture.root.display().to_string(),
        life: "drive".to_owned(),
        vendor: fixture.vendor.clone(),
        model: None,
        effort: None,
        fast_mode: false,
        occupant: Some(fixture.occupant),
        lineage: vec![fixture.occupant],
        capabilities: vec![fixture.capability],
        session: None,
        disposition: "occupied".to_owned(),
    };
    let origin: Origin = format!("resident:{}", fixture.epoch)
        .parse()
        .expect("resident origin");
    let mut write = JournalWrite::new(Actor::Spice, origin, start, Producer::CommandDoor);
    let seated = write
        .fact(
            wire::body::UpSeated {
                epoch: fixture.epoch,
            },
            FactCause::Root,
        )
        .expect("resident seating fact builds");
    let epoch = fixture.epoch;
    let seated_row = row.clone();
    write.residency_head(seated, move |_| wire::ResidencyHead {
        epoch,
        state: ResidentState::Seated,
        topology_revision: 1,
        lanes: vec![seated_row],
        subscriptions: Vec::new(),
        renewals: Vec::new(),
    });
    committed(bus.commit(write).expect("resident seating commits"));
    let task: TaskStamp = "1kKRRsDQ".parse().expect("task stamp");
    seed_task_claim(&bus, &task, &row, fixture.epoch, start, expiry, maximum);
    (bus, task)
}

fn note(bus: &Bus, task: &TaskStamp, actor: Actor) {
    let mut write = JournalWrite::new(actor, task_origin(task), now(), Producer::CommandDoor);
    write
        .fact(
            wire::body::TaskNoted {
                task: task.clone(),
                note: "durable context".to_owned(),
            },
            FactCause::Root,
        )
        .expect("task note builds");
    committed(bus.commit(write).expect("task note commits"));
}

/// How one statement write treats the taste hold.
enum Hold {
    /// A policy read that leaves this hold, opening it if new, or clears it.
    Read(Option<wire::TasteHold>),
    /// A write that reads no policy, such as a dependency edit, and keeps the
    /// standing hold as it is.
    CarriedThrough,
}

/// Revise the statement as `actor`, returning the revision's fact.
fn restate(bus: &Bus, task: &TaskStamp, actor: Actor, hold: Hold) -> Seq {
    let snapshot = bus.task_snapshot().expect("pre-edit snapshot reads");
    let mut statement = snapshot.tasks()[0].statement().body().clone();
    let reads_policy = match hold {
        Hold::Read(taste) => {
            statement.taste = taste;
            true
        }
        Hold::CarriedThrough => false,
    };
    let revision = u32::try_from(statement.revision.get())
        .expect("fixture revision fits")
        .saturating_add(1);
    let mut write = JournalWrite::new(actor, task_origin(task), now(), Producer::CommandDoor);
    let edited = write
        .fact(
            wire::body::TaskEdited {
                task: task.clone(),
                revision,
                dependencies: (!reads_policy).then(|| statement.dependencies.clone()),
                intent: None,
            },
            FactCause::Root,
        )
        .expect("task edit builds");
    write.task_head(edited, move |position| {
        let mut statement = wire::TaskHead {
            revision: position,
            ..statement
        };
        if let Some(hold) = statement.taste.as_mut().filter(|_| reads_policy) {
            hold.record_opening(position);
        }
        statement
    });
    committed(bus.commit(write).expect("task edit commits"));
    bus.task_snapshot().expect("edited snapshot reads").tasks()[0]
        .statement()
        .position()
}

fn acknowledge(bus: &Bus, task: &TaskStamp) {
    let snapshot = bus
        .task_snapshot()
        .expect("pre-acknowledgement snapshot reads");
    let heads = &snapshot.tasks()[0];
    let claim = heads.claim().expect("claim head").body().clone();
    let generation = heads.workflow().body().generation;
    let mut write = JournalWrite::new(
        claim.actor.clone(),
        task_origin(task),
        now(),
        Producer::CommandDoor,
    );
    let acknowledged = write
        .fact(
            wire::body::ClaimAcknowledged {
                task: task.clone(),
                generation,
            },
            FactCause::Root,
        )
        .expect("claim acknowledgement fact builds");
    write.claim_head(task.clone(), acknowledged, move |fact| wire::ClaimHead {
        fact,
        acknowledged: generation,
        ..claim
    });
    committed(bus.commit(write).expect("acknowledgement commits"));
}

fn end_claim(bus: &Bus, task: &TaskStamp) {
    let snapshot = bus.task_snapshot().expect("pre-release snapshot reads");
    let claim = snapshot.tasks()[0]
        .claim()
        .expect("claim head")
        .body()
        .clone();
    let mut write = JournalWrite::new(
        claim.actor.clone(),
        task_origin(task),
        now(),
        Producer::CommandDoor,
    );
    let freed = write
        .fact(
            wire::body::ClaimFreed {
                task: task.clone(),
                actor: claim.actor.clone(),
                parked: None,
            },
            FactCause::Existing(claim.claimed),
        )
        .expect("claim release fact builds");
    write.claim_head(task.clone(), freed, move |position| wire::ClaimHead {
        fact: position,
        disposition: ClaimDisposition::Freed,
        ..claim
    });
    committed(bus.commit(write).expect("claim release commits"));
}

fn task_origin(task: &TaskStamp) -> Origin {
    format!("task:{task}").parse().expect("task origin")
}

fn finish(bus: Bus) {
    bus.verify().expect("notice authority verifies");
    bus.shutdown().expect("notice bus stops");
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
