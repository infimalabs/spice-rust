use super::*;
use crate::resident::lane_surface::{AdvisoryRtkFixture, seed_task_claim};
use bus::CommitOutcome;
use journal::{Journal, JournalWrite};
use wire::{LaneResidency, Origin, Producer, ResidentState};

#[test]
fn trailing_generations_project_private_claimant_notices_without_steering() {
    let fixture = AdvisoryRtkFixture::new();
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
            bus.cursor().expect("cursor reads"),
            Duration::from_secs(3_600),
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
fn reclaim_clears_prior_generations_and_a_later_edit_gets_a_new_identity() {
    let fixture = AdvisoryRtkFixture::new();
    let (bus, task) = claimed_fixture(&fixture);
    note(&bus, &task, Actor::Operator);
    note(&bus, &task, Actor::Operator);
    let before = bus.task_snapshot().expect("changed snapshot reads");
    let prior = notices(&bus, &before).expect("prior notices project");
    assert_eq!(prior.len(), 2);

    reclaim(&bus, &task);
    let reclaimed = bus.task_snapshot().expect("reclaimed snapshot reads");
    assert!(
        notices(&bus, &reclaimed)
            .expect("reclaimed notices project")
            .is_empty(),
        "successful reclaim retires every acknowledged reminder",
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
fn retired_steering_notices_are_migrated_once_before_runtime_startup() {
    let fixture = AdvisoryRtkFixture::new();
    let (bus, _) = claimed_fixture(&fixture);
    let control = "task-generation:1kKRRsDQ:33:4".to_owned();
    let (mut proposal, published) =
        crate::resident::steering::internal_write(fixture.epoch).expect("internal write builds");
    crate::resident::steering::append_internal_directive(
        &bus,
        &mut proposal,
        fixture.epoch,
        crate::resident::steering::InternalDirective {
            directive: crate::resident::new_uuid(),
            lane: fixture.lane,
            text: "retired keyed reminder".to_owned(),
            control,
            lifetime: Duration::from_secs(3_600),
            cause: FactCause::Root,
            published,
        },
    )
    .expect("legacy directive builds");
    committed(bus.commit(proposal).expect("legacy directive commits"));

    assert_eq!(
        bus.read(move |read| crate::steering_read::pending(read, Some(fixture.lane)))
            .expect("steering index reads")
            .len(),
        1,
        "predecessor evidence starts live",
    );
    crate::resident::steering::migrate_task_generation_notices(&bus, fixture.epoch)
        .expect("startup migration succeeds");
    crate::resident::steering::migrate_task_generation_notices(&bus, fixture.epoch)
        .expect("completed migration is inert");
    assert!(
        bus.read(move |read| crate::steering_read::pending(read, Some(fixture.lane)))
            .expect("steering index reads")
            .is_empty(),
        "legacy reminder is durably canceled",
    );
    finish(bus);
}

fn claimed_fixture(fixture: &AdvisoryRtkFixture) -> (Bus, TaskStamp) {
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
        occupant: Some(fixture.occupant),
        lineage: vec![fixture.occupant],
        capabilities: vec![fixture.capability],
        session: None,
        disposition: "occupied".to_owned(),
    };
    let origin: Origin = format!("resident:{}", fixture.epoch)
        .parse()
        .expect("resident origin");
    let now = now();
    let mut write = JournalWrite::new(Actor::Spice, origin, now, Producer::CommandDoor);
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
    seed_task_claim(
        &bus,
        &task,
        &row,
        fixture.epoch,
        shifted(now, -60),
        shifted(now, 3_600),
        shifted(now, 7_200),
    );
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

fn reclaim(bus: &Bus, task: &TaskStamp) {
    let snapshot = bus.task_snapshot().expect("pre-reclaim snapshot reads");
    let heads = &snapshot.tasks()[0];
    let claim = heads.claim().expect("claim head").body().clone();
    let lease = heads.lease().expect("lease head").body().clone();
    let acknowledged = heads.workflow().body().generation;
    let mut write = JournalWrite::new(
        claim.actor.clone(),
        task_origin(task),
        now(),
        Producer::BusTimer,
    );
    let renewed = write
        .fact(
            wire::body::ClaimRenewed {
                task: task.clone(),
                epoch: claim.epoch,
                until: lease.expiry,
                maximum: lease.maximum,
            },
            FactCause::Root,
        )
        .expect("claim renewal fact builds");
    write.lease_head(task.clone(), renewed, move |_| lease);
    write.claim_head(task.clone(), renewed, move |fact| wire::ClaimHead {
        fact,
        acknowledged,
        ..claim
    });
    committed(bus.commit(write).expect("reclaim commits"));
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
