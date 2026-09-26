use super::*;
use crate::resident::CommandEffects;
use crate::resident::lane_surface::{AdvisoryRtkFixture, seed_task_claim};
use bus::CommitOutcome;
use journal::{Journal, JournalWrite};
use wire::intent::{CommandAnswer, TaskGenerationNotice};
use wire::{Command, CommandKind, Disposition, LaneResidency, Origin, Producer, ResidentState};

#[test]
fn a_control_is_the_whole_notice_identity() {
    let authority = NoticeAuthority {
        task: "1kKRRsDP".parse().expect("task stamp"),
        claim: Seq::new(33).expect("claim"),
        generation: 2,
    };
    let control = authority.control();
    assert_eq!(
        notice_authority(Some(&control)),
        Ok(Some(authority.clone()))
    );
    assert_eq!(notice_authority(Some("lane-renewal-context")), Ok(None));
    for malformed in [
        "task-generation:1kKRRsDP:33",
        "task-generation:1kKRRsDP:33:0",
    ] {
        assert!(notice_authority(Some(malformed)).is_err(), "{malformed}");
    }
    let successor = NoticeAuthority {
        claim: Seq::new(34).expect("claim"),
        ..authority.clone()
    };
    assert_ne!(
        identity(&control),
        identity(&successor.control()),
        "a successor claim never inherits its predecessor's notice"
    );
}

#[test]
fn every_foreign_change_since_the_acknowledgement_publishes_one_reclaim_notice() {
    let fixture = AdvisoryRtkFixture::new();
    let (bus, task) = claimed_fixture(&fixture);
    note(&bus, &task, Actor::Lane(fixture.lane));
    note(&bus, &task, Actor::Operator);
    note(&bus, &task, Actor::Spice);
    let before = bus.task_snapshot().expect("noted snapshot reads").tasks()[0].clone();

    let cursor = bus.cursor().expect("cursor reads");
    assert!(
        pass(&fixture, &bus, cursor),
        "a pass after the change still delivers it"
    );
    assert!(
        !pass(&fixture, &bus, None),
        "a repeated pass publishes nothing new"
    );

    let notices = notices(&fixture, &bus);
    assert_eq!(
        notices
            .iter()
            .map(|notice| notice.generation)
            .collect::<Vec<_>>(),
        [2, 3],
        "the claimant's own change at generation 1 kept its claim current"
    );
    let handle = TaskHandle::for_project("task.claim", &task);
    for notice in &notices {
        assert_eq!(notice.task, task);
        assert_eq!(notice.handle, handle);
        assert_eq!(
            notice.claim,
            before.claim().expect("claim head").body().claimed
        );
        assert!(
            notice
                .text
                .contains(&format!("spice task reclaim {handle}"))
        );
    }
    assert!(notices[0].text.contains("by operator"));
    let after = bus.task_snapshot().expect("post-notice snapshot reads");
    assert_eq!(after.tasks()[0].claim(), before.claim());
    assert_eq!(after.tasks()[0].lease(), before.lease());
    finish(bus);
}

#[test]
fn a_reclaim_retires_exactly_the_notices_it_acknowledges() {
    let fixture = AdvisoryRtkFixture::new();
    let (bus, task) = claimed_fixture(&fixture);
    note(&bus, &task, Actor::Operator);
    note(&bus, &task, Actor::Operator);
    assert!(pass(&fixture, &bus, None));
    let published = notices(&fixture, &bus);
    assert_eq!(published.len(), 2);

    reclaim(&bus, &task);
    note(&bus, &task, Actor::Operator);
    assert!(pass(&fixture, &bus, None));

    assert_eq!(
        notices(&fixture, &bus)
            .iter()
            .map(|notice| notice.generation)
            .collect::<Vec<_>>(),
        [3],
        "the reclaim acknowledged generation 2; the later change is new"
    );
    for notice in &published {
        assert_eq!(disposition(&bus, notice.directive), Disposition::Canceled);
    }
    finish(bus);
}

#[test]
fn expiry_does_not_retire_a_notice_its_claim_still_trails() {
    let fixture = AdvisoryRtkFixture::new();
    let (bus, task) = claimed_fixture(&fixture);
    note(&bus, &task, Actor::Operator);
    assert!(pass(&fixture, &bus, None));
    let [notice] = notices(&fixture, &bus).try_into().expect("one notice");
    let lapse = crate::resident::steering::SteeringDeadline {
        directive: notice.directive,
        key: notice.key.parse().expect("notice key"),
        lane: fixture.lane,
        expires: notice.expires,
        transition: crate::resident::steering::DeadlineTransition::Lapse,
    };
    assert!(
        crate::resident::steering::apply_deadline(&bus, &lapse, notice.expires)
            .expect("the notice lapses")
    );
    assert!(notices(&fixture, &bus).is_empty());

    assert!(pass(&fixture, &bus, None));
    let [recovered] = notices(&fixture, &bus).try_into().expect("one notice");
    assert_eq!(recovered.key, notice.key, "the notice keeps its identity");
    assert_eq!(disposition(&bus, notice.directive), Disposition::Live);
    assert!(!pass(&fixture, &bus, None));
    finish(bus);
}

#[test]
fn a_notice_stays_bound_to_the_claim_that_trailed() {
    let fixture = AdvisoryRtkFixture::new();
    let (bus, task) = claimed_fixture(&fixture);
    note(&bus, &task, Actor::Operator);
    carry_claim(&bus, &task);
    assert!(pass(&fixture, &bus, None));
    let [carried] = notices(&fixture, &bus).try_into().expect("one notice");

    end_claim(&bus, &task);
    note(&bus, &task, Actor::Operator);
    assert!(pass(&fixture, &bus, None));
    assert!(notices(&fixture, &bus).is_empty());
    assert_eq!(disposition(&bus, carried.directive), Disposition::Canceled);
    finish(bus);
}

#[test]
fn answering_a_notice_key_cannot_retire_it() {
    let fixture = AdvisoryRtkFixture::new();
    let (bus, task) = claimed_fixture(&fixture);
    note(&bus, &task, Actor::Operator);
    assert!(pass(&fixture, &bus, None));
    let [notice] = notices(&fixture, &bus).try_into().expect("one notice");

    let dispose = wire::intent::SteerDispose {
        token: notice.key.clone(),
        disposition: Disposition::Acked,
    };
    let command = Command {
        v: wire::WireVersion::CURRENT,
        id: CanonicalUuid::new(uuid::Uuid::from_u128(0xAC4)),
        epoch: fixture.epoch,
        kind: CommandKind::SteerDispose,
        expected: Vec::new(),
        body: serde_json::to_value(&dispose).expect("ack request encodes"),
    }
    .admit(&wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor: Actor::Lane(fixture.lane),
        epoch: fixture.epoch,
    })
    .expect("lane ack admits");
    let outcome = crate::resident::steering::dispose(
        &bus,
        &fixture.paths,
        fixture.epoch,
        &command,
        &dispose,
        &mut CommandEffects::default(),
    );

    let Outcome::<CommandAnswer>::Refusal(refusal) = outcome else {
        panic!("a notice key is not an acknowledgment: {outcome:?}");
    };
    assert_eq!(refusal.reason(), "steer::reclaim-required");
    assert_eq!(disposition(&bus, notice.directive), Disposition::Live);
    finish(bus);
}

fn pass(fixture: &AdvisoryRtkFixture, bus: &Bus, cursor: Option<Seq>) -> bool {
    let snapshot = bus.task_snapshot().expect("snapshot reads");
    reconcile(
        bus,
        &fixture.paths,
        fixture.epoch,
        &snapshot,
        cursor,
        Duration::from_secs(3_600),
    )
    .expect("generation notices reconcile")
}

fn notices(fixture: &AdvisoryRtkFixture, bus: &Bus) -> Vec<TaskGenerationNotice> {
    let facts = bus.fact_snapshot().expect("steering facts read");
    let ledger = steer::Ledger::from_snapshot(&facts).expect("steering facts replay");
    let snapshot = bus.task_snapshot().expect("snapshot reads");
    let (authored, notices) =
        crate::resident::steering::pending_presentations(&ledger, fixture.lane, snapshot.tasks())
            .expect("pending presentation partitions");
    assert!(
        authored.is_empty(),
        "no notice presents as authored steering"
    );
    notices
}

fn disposition(bus: &Bus, directive: CanonicalUuid) -> Disposition {
    bus.read(move |read| read.steering_head(directive))
        .expect("steering index reads")
        .expect("the notice is indexed")
        .body()
        .disposition
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

/// One task change from `actor`, in its own transaction.
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

/// Renew the claim as `spice task reclaim` does, acknowledging the task.
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

fn carry_claim(bus: &Bus, task: &TaskStamp) {
    let snapshot = bus.task_snapshot().expect("pre-carry snapshot reads");
    let claim = snapshot.tasks()[0]
        .claim()
        .expect("claim head")
        .body()
        .clone();
    let mut write = JournalWrite::new(
        Actor::Spice,
        task_origin(task),
        now(),
        Producer::ResidentRecognition,
    );
    let carried = write
        .fact(
            wire::body::ClaimCarried {
                task: task.clone(),
                lane: claim.lane,
                occupant: claim.occupant,
                ancestor: claim.occupant,
                epoch: claim.epoch,
                source: wire::body::SourceBoundary::unrecorded(),
            },
            FactCause::Existing(claim.claimed),
        )
        .expect("claim carry fact builds");
    write.claim_head(task.clone(), carried, move |position| wire::ClaimHead {
        fact: position,
        ..claim
    });
    committed(bus.commit(write).expect("claim carry commits"));
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
