//! Timer runtime tests keep scheduling and its full state-transition matrix reviewable.

use board::Entry;
use journal::{FactCause, Journal, JournalWrite, PendingFact, TaskHeads};
use steer::Standing as _;
use wire::body::GitBoundary;
use wire::{
    Accepted, Actor, ClaimHead, Command, CommandKind, LaneResidency, LeaseHead, Origin, Outcome,
    Phase, Producer, Reply, ReplyPosition, ResidencyHead, TaskHead, WireVersion, Witness,
    WorkflowHead, body,
};

use super::*;

mod deferral_fixture;
use deferral_fixture::*;
mod lane_held_claim;
mod trust_ice;

struct Fixture {
    bus: Bus,
    authority: Authority,
}

struct Authority {
    task: TaskHandle,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    epoch: CanonicalUuid,
    worktree: String,
    start: Instant,
    expiry: Instant,
    maximum: Instant,
}

struct SeedFacts {
    added: PendingFact,
    won: PendingFact,
    seated: PendingFact,
}

impl Authority {
    fn born(&self) -> wire::TaskStamp {
        self.task.born().expect("fixture task carries a stamp")
    }
}

#[test]
fn automatic_claim_heartbeat_rolls_one_hundred_seconds_and_rearms_lapse() {
    let fixture = automatic_fixture();
    let initial_expiry = fixture.authority.expiry;
    let mut timers = start_timers().expect("timer runtime starts");
    timers
        .synchronize(&fixture.bus)
        .expect("automatic heartbeat arms");
    assert_eq!(timers.renewals.len(), 1);
    assert!(timers.directives.is_empty());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("fixture async runtime builds");
    let event = runtime
        .block_on(timers.recv())
        .expect("due automatic heartbeat fires");
    assert_eq!(event.kind(), TimerKind::ClaimRenew);
    assert_eq!(timers.consume(&fixture.bus, event), Ok(Fired::ClaimRenew));
    timers
        .synchronize(&fixture.bus)
        .expect("renewed heartbeat re-arms");
    let lease = fixture
        .bus
        .read({
            let stamp = fixture.authority.born();
            move |read| Ok(read.lease_head(&stamp)?.expect("lease head exists"))
        })
        .expect("renewed lease reads");
    assert!(lease.body().renewal > fixture.authority.start);
    assert!(lease.body().expiry > initial_expiry);
    assert_eq!(
        lease.body().expiry,
        shifted(
            lease.body().renewal,
            crate::board_command::AUTOMATIC_ROLLING_SECONDS,
        )
    );
    assert_eq!(
        lease.body().maximum,
        shifted(
            lease.body().renewal,
            crate::board_command::AUTOMATIC_MAXIMUM_SECONDS,
        )
    );
    assert!(lease.body().expiry < lease.body().maximum);
    assert_eq!(
        timers
            .claims
            .get(&fixture.authority.task)
            .expect("renewed lapse timer exists")
            .authority
            .expiry,
        lease.body().expiry,
    );
    assert_eq!(
        timers
            .renewals
            .get(&fixture.authority.task)
            .expect("next heartbeat exists")
            .authority
            .deadline,
        shifted(
            lease.body().renewal,
            crate::board_command::AUTOMATIC_HEARTBEAT_SECONDS,
        ),
    );
    assert_eq!(
        fixture
            .bus
            .fact_snapshot()
            .expect("renewal facts read")
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == wire::FactKind::ClaimRenewed)
            .count(),
        1,
    );

    drop(timers);
    fixture.bus.shutdown().expect("fixture bus stops");
}

#[test]
fn automatic_claim_heartbeat_refuses_moved_occupant_authority() {
    let fixture = automatic_fixture();
    let mut timers = start_timers().expect("timer runtime starts");
    timers
        .synchronize(&fixture.bus)
        .expect("automatic heartbeat arms");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("fixture async runtime builds");
    let event = runtime
        .block_on(timers.recv())
        .expect("due automatic heartbeat fires");
    move_residency(&fixture, fixture.authority.epoch, uuid(24));

    assert_eq!(timers.consume(&fixture.bus, event), Ok(Fired::Stale));
    timers
        .synchronize(&fixture.bus)
        .expect("moved occupant disarms heartbeat");
    assert!(timers.renewals.is_empty());
    assert!(
        fixture
            .bus
            .fact_snapshot()
            .expect("facts read")
            .facts()
            .iter()
            .all(|fact| fact.fact.kind() != wire::FactKind::ClaimRenewed)
    );

    drop(timers);
    fixture.bus.shutdown().expect("fixture bus stops");
}

#[test]
fn automatic_claim_heartbeat_refuses_a_moved_lease_head() {
    let fixture = automatic_fixture();
    let mut timers = start_timers().expect("timer runtime starts");
    timers
        .synchronize(&fixture.bus)
        .expect("automatic heartbeat arms");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("fixture async runtime builds");
    let event = runtime
        .block_on(timers.recv())
        .expect("due automatic heartbeat fires");
    renew_claim(&fixture, fixture.authority.expiry);

    assert_eq!(timers.consume(&fixture.bus, event), Ok(Fired::Stale));
    assert_eq!(
        fixture
            .bus
            .fact_snapshot()
            .expect("facts read")
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == wire::FactKind::ClaimRenewed)
            .count(),
        1,
        "only the competing lease-head movement committed"
    );

    drop(timers);
    fixture.bus.shutdown().expect("fixture bus stops");
}

#[test]
fn automatic_claim_release_cancels_heartbeat_without_a_renewal() {
    let fixture = automatic_fixture();
    let mut timers = start_timers().expect("timer runtime starts");
    timers
        .synchronize(&fixture.bus)
        .expect("automatic heartbeat arms");
    assert_eq!(timers.renewals.len(), 1);

    free_claim(&fixture);
    timers
        .synchronize(&fixture.bus)
        .expect("released automatic claim disarms heartbeat");
    assert!(timers.renewals.is_empty());
    assert!(
        fixture
            .bus
            .fact_snapshot()
            .expect("facts read")
            .facts()
            .iter()
            .all(|fact| fact.fact.kind() != wire::FactKind::ClaimRenewed)
    );

    drop(timers);
    fixture.bus.shutdown().expect("fixture bus stops");
}

#[test]
fn expired_automatic_claim_lapses_without_arming_a_renewal_loop() {
    let fixture = automatic_fixture_with_age(61);
    let mut timers = start_timers().expect("timer runtime starts");
    timers
        .synchronize(&fixture.bus)
        .expect("expired automatic claim folds");
    assert!(timers.renewals.is_empty());
    assert_eq!(timers.claims.len(), 1);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("fixture async runtime builds");
    let event = runtime
        .block_on(timers.recv())
        .expect("expired claim lapse fires");

    assert_eq!(event.kind(), TimerKind::ClaimLapse);
    let Ok(Fired::ClaimLapse(Some(lapsed))) = timers.consume(&fixture.bus, event) else {
        panic!("the expired claim lapse commits");
    };
    assert_eq!(
        lapsed_heads(&fixture)
            .claim()
            .expect("lapsed claim head remains")
            .body()
            .disposition,
        ClaimDisposition::Lapsed,
    );

    // The holding lane hears the lapse at whoever occupies it now: a restart
    // that re-seated the lane does not strand the notice on the old occupant,
    // and a vacant lane hears nothing.
    let seated = fixture
        .bus
        .residency_head()
        .expect("residency reads")
        .expect("fixture residency exists")
        .body()
        .clone();
    let successor = new_uuid();
    let mut reseated = seated.clone();
    let mut vacant = seated;
    for (residency, occupant) in [(&mut reseated, Some(successor)), (&mut vacant, None)] {
        residency
            .lanes
            .iter_mut()
            .find(|durable| durable.lane == fixture.authority.lane)
            .expect("the holding lane is durable")
            .occupant = occupant;
    }
    let (lane, notice) = lapsed
        .notice(&reseated)
        .expect("a re-seated holding lane hears its lapse");
    assert_eq!(lane, fixture.authority.lane);
    assert_eq!(notice.occupant, successor);
    assert_eq!(notice.task, fixture.authority.task.to_string());
    assert_eq!(notice.expiry, fixture.authority.expiry);
    assert_eq!(lapsed.notice(&vacant), None);

    drop(timers);
    fixture.bus.shutdown().expect("fixture bus stops");
}

#[test]
fn manual_claims_never_create_resident_warning_protocol() {
    let fixture = fixture_with_expiry(60);
    let initial_fact_count = fixture
        .bus
        .fact_snapshot()
        .expect("initial facts read")
        .facts()
        .len();
    let mut timers = start_timers().expect("timer runtime starts");

    timers
        .synchronize(&fixture.bus)
        .expect("manual claim lapse arms without warning protocol");

    assert_eq!(timers.claims.len(), 1);
    assert_eq!(timers.renewals.len(), 1, "the live lease rolls instead");
    assert!(timers.directives.is_empty());
    let facts = fixture.bus.fact_snapshot().expect("facts read");
    assert_eq!(facts.facts().len(), initial_fact_count);
    assert!(
        facts
            .facts()
            .iter()
            .all(|envelope| envelope.fact.kind() != wire::FactKind::ClaimLeaseWarned)
    );
    assert!(
        fixture
            .bus
            .read({
                let lane = fixture.authority.lane;
                move |read| crate::steering_read::pending(read, Some(lane))
            })
            .expect("lane inbox reads")
            .is_empty()
    );

    renew_claim(&fixture, shifted(fixture.authority.expiry, 60));
    timers
        .synchronize(&fixture.bus)
        .expect("renewed manual claim remains outside warning protocol");
    assert!(timers.directives.is_empty());
    assert!(
        fixture
            .bus
            .fact_snapshot()
            .expect("renewed facts read")
            .facts()
            .iter()
            .all(|envelope| envelope.fact.kind() != wire::FactKind::ClaimLeaseWarned)
    );

    drop(timers);
    fixture.bus.shutdown().expect("fixture bus stops");
}

#[test]
fn expired_commit_bearing_claim_lapses_and_returns_to_allocation() {
    let fixture = fixture();
    let commits = fixture
        .bus
        .subscribe()
        .expect("lapse commit subscription registers");
    let mut timers = start_timers().expect("timer runtime starts");
    timers
        .synchronize(&fixture.bus)
        .expect("expired claim arms");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("fixture async runtime builds");
    let fired = runtime
        .block_on(timers.recv())
        .expect("expired claim timer fires");
    timers
        .consume(&fixture.bus, fired)
        .expect("expired claim lapses");
    timers
        .synchronize(&fixture.bus)
        .expect("lapsed claim disarms");

    let committed = commits.recv().expect("lapse broadcasts after durability");
    assert_eq!(committed.first(), committed.last());
    let heads = lapsed_heads(&fixture);
    assert_lapsed_and_eligible(&fixture, &heads);
    capture_lapsed_task(
        &fixture,
        &heads,
        fixture.authority.epoch,
        fixture.authority.occupant,
    );
    assert_commit_history(&fixture.bus);
    drop(timers);
    fixture.bus.shutdown().expect("fixture bus stops");
}

#[test]
fn expired_claim_lapses_after_resident_epoch_and_occupant_move() {
    let fixture = fixture();
    let successor = uuid(32);
    let successor_epoch = uuid(33);
    move_residency(&fixture, successor_epoch, successor);
    let mut timers = start_timers().expect("timer runtime starts");
    timers
        .synchronize(&fixture.bus)
        .expect("stale expired claim still arms");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("fixture async runtime builds");
    let fired = runtime
        .block_on(timers.recv())
        .expect("expired stale claim timer fires");
    timers
        .consume(&fixture.bus, fired)
        .expect("expired stale claim lapses");

    let heads = lapsed_heads(&fixture);
    assert_lapsed_and_eligible(&fixture, &heads);
    capture_lapsed_task(&fixture, &heads, successor_epoch, successor);
    drop(timers);
    fixture.bus.shutdown().expect("fixture bus stops");
}

#[test]
fn task_wake_is_an_explicit_timer_event_for_the_selected_lane() {
    let bus = Bus::start(Journal::in_memory().expect("fixture journal opens"))
        .expect("fixture bus starts");
    let mut timers = start_timers().expect("timer runtime starts");
    let now = Instant::new(jiff::Timestamp::now()).expect("fixture clock is canonical");
    let lane = uuid(41);
    timers
        .synchronize_wake(Some(super::super::lifecycle::WakeDeadline {
            lane,
            at: shifted(now, -1),
            kind: super::super::lifecycle::WakeKind::Task,
        }))
        .expect("past task wake arms");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("fixture async runtime builds");
    let event = runtime
        .block_on(timers.recv())
        .expect("task wake timer fires");
    assert_eq!(event.kind(), TimerKind::TaskWake);
    assert_eq!(event.owner(), lane.to_string());
    assert_eq!(
        timers.consume(&bus, event).expect("task wake consumes"),
        Fired::TaskWake(lane)
    );
    drop(timers);
    bus.shutdown().expect("fixture bus stops");
}

#[test]
fn due_task_deferral_wakes_the_task_into_allocator_eligibility() {
    let bus = Bus::start(Journal::in_memory().expect("fixture journal opens"))
        .expect("fixture bus starts");
    let task = deferred_task("timer.defer", "1kKRRsDQ");
    let phase = "todo".parse::<Phase>().expect("fixture phase");
    // Waiting for a future deadline is the bus timer's contract; this one is
    // already due, so the runtime fires and consumes it at once.
    let deadline = shifted(
        Instant::new(jiff::Timestamp::now()).expect("fixture clock is canonical"),
        -1,
    );
    seed_timer_task(
        &bus,
        &task,
        "timer.defer",
        wire::TaskFiling::Filed,
        Some(deadline),
        Vec::new(),
    );
    let mut timers = start_timers().expect("timer runtime starts");
    timers.synchronize(&bus).expect("due task deferral arms");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("fixture async runtime builds");
    let event = runtime
        .block_on(timers.recv())
        .expect("due task deferral fires");
    assert_eq!(event.kind(), TimerKind::TaskWake);
    assert_eq!(event.owner(), task.to_string());
    assert_eq!(
        timers.consume(&bus, event).expect("task deferral consumes"),
        Fired::DeferredTaskWoken(task.clone()),
    );
    let heads = timer_task_heads(&bus, &task);
    assert_eq!(heads.workflow().body().phase, phase);
    assert_eq!(heads.workflow().body().deferral, None);
    assert!(heads.statement().body().dependencies.is_empty());
    let (statement, workflow, claim, lease) = heads.into_bodies();
    let entry = Entry::new(statement, workflow, claim, lease).expect("woken task remains valid");
    assert!(
        board::eligible_entry(
            &board::resolved_tasks(std::slice::from_ref(&entry)),
            &entry,
            &Actor::Lane(uuid(49)),
            &wire::Environment::journal_schema(journal::SCHEMA_VERSION).expect("positive schema"),
        ),
        "the elapsed deferral becomes allocator-eligible",
    );
    assert_eq!(task_woken_count(&bus), 1);
    drop(timers);
    bus.shutdown().expect("fixture bus stops");
}

#[test]
fn startup_catches_up_two_overdue_tasks_but_leaves_future_and_parked_work_waiting() {
    let bus = Bus::start(Journal::in_memory().expect("fixture journal opens"))
        .expect("fixture bus starts");
    let now = Instant::new(jiff::Timestamp::now()).expect("fixture clock is canonical");
    let anchor = deferred_task("timer.defer", "1kKRRsDR");
    let first = deferred_task("timer.defer", "1kKRRsDS");
    let second = deferred_task("timer.defer", "1kKRRsDT");
    let future = deferred_task("timer.defer", "1kKRRsDV");
    let parked = wire::TaskHandle::for_filing(
        "timer.defer",
        &"1kKRRsDW".parse().expect("fixture task stamp"),
        wire::TaskFiling::Draft,
    );
    seed_timer_task(
        &bus,
        &anchor,
        "timer.defer",
        wire::TaskFiling::Filed,
        None,
        Vec::new(),
    );
    let dependency = vec![anchor.born().expect("anchor has a stamp")];
    seed_timer_task(
        &bus,
        &first,
        "timer.defer",
        wire::TaskFiling::Filed,
        Some(shifted(now, -2)),
        dependency.clone(),
    );
    seed_timer_task(
        &bus,
        &second,
        "timer.defer",
        wire::TaskFiling::Filed,
        Some(shifted(now, -1)),
        dependency.clone(),
    );
    seed_timer_task(
        &bus,
        &future,
        "timer.defer",
        wire::TaskFiling::Filed,
        Some(shifted(now, 3600)),
        dependency.clone(),
    );
    seed_timer_task(
        &bus,
        &parked,
        "timer.defer",
        wire::TaskFiling::Draft,
        Some(shifted(now, -1)),
        dependency.clone(),
    );

    let mut timers = start_timers().expect("timer runtime starts");
    timers.synchronize(&bus).expect("startup deferrals arm");
    assert_eq!(timers.deferrals.len(), 3, "Draft filing is not armed");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("fixture async runtime builds");
    let mut woken = std::collections::BTreeSet::new();
    for _ in 0..2 {
        let event = runtime
            .block_on(timers.recv())
            .expect("overdue task wake fires");
        let Fired::DeferredTaskWoken(task) =
            timers.consume(&bus, event).expect("catch-up consumes")
        else {
            panic!("overdue deferral did not wake")
        };
        woken.insert(task);
    }
    assert_eq!(
        woken,
        std::collections::BTreeSet::from([first.clone(), second.clone()])
    );
    timers
        .synchronize(&bus)
        .expect("post-catch-up state synchronizes");
    assert_eq!(timers.deferrals.keys().collect::<Vec<_>>(), [&future]);
    for task in [&first, &second, &future, &parked] {
        let heads = timer_task_heads(&bus, task);
        assert_eq!(heads.workflow().body().phase.as_str(), "todo");
        assert_eq!(heads.statement().body().dependencies, dependency);
    }
    assert_eq!(
        timer_task_heads(&bus, &first).workflow().body().deferral,
        None
    );
    assert_eq!(
        timer_task_heads(&bus, &second).workflow().body().deferral,
        None
    );
    assert_eq!(
        timer_task_heads(&bus, &future).workflow().body().deferral,
        Some(shifted(now, 3600)),
    );
    assert_eq!(
        timer_task_heads(&bus, &parked).workflow().body().deferral,
        Some(shifted(now, -1)),
    );
    assert_eq!(task_woken_count(&bus), 2);

    drop(timers);
    let mut restarted = start_timers().expect("replacement timer starts");
    restarted
        .synchronize(&bus)
        .expect("replacement observes completed catch-up");
    assert_eq!(restarted.deferrals.keys().collect::<Vec<_>>(), [&future]);
    assert_eq!(task_woken_count(&bus), 2, "restart adds no duplicate wake");
    drop(restarted);
    bus.shutdown().expect("fixture bus stops");
}

#[test]
fn duplicate_overdue_catch_up_commits_one_wake_transition() {
    let bus = Bus::start(Journal::in_memory().expect("fixture journal opens"))
        .expect("fixture bus starts");
    let task = deferred_task("timer.defer", "1kKRRsDY");
    let deadline = shifted(
        Instant::new(jiff::Timestamp::now()).expect("fixture clock is canonical"),
        -1,
    );
    seed_timer_task(
        &bus,
        &task,
        "timer.defer",
        wire::TaskFiling::Filed,
        Some(deadline),
        Vec::new(),
    );
    let mut first = start_timers().expect("first timer starts");
    let mut second = start_timers().expect("second timer starts");
    first.synchronize(&bus).expect("first catch-up arms");
    second.synchronize(&bus).expect("second catch-up arms");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("fixture async runtime builds");
    let first_event = runtime
        .block_on(first.recv())
        .expect("first catch-up fires");
    let second_event = runtime
        .block_on(second.recv())
        .expect("second catch-up fires");
    assert_eq!(
        first
            .consume(&bus, first_event)
            .expect("first catch-up consumes"),
        Fired::DeferredTaskWoken(task.clone()),
    );
    assert_eq!(
        second
            .consume(&bus, second_event)
            .expect("duplicate catch-up settles"),
        Fired::Stale,
    );
    assert_eq!(
        timer_task_heads(&bus, &task).workflow().body().deferral,
        None
    );
    assert_eq!(task_woken_count(&bus), 1);

    drop(first);
    drop(second);
    bus.shutdown().expect("fixture bus stops");
}

#[test]
fn lane_resume_is_an_explicit_timer_event_for_the_held_lane() {
    let bus = Bus::start(Journal::in_memory().expect("fixture journal opens"))
        .expect("fixture bus starts");
    let mut timers = start_timers().expect("timer runtime starts");
    let now = Instant::new(jiff::Timestamp::now()).expect("fixture clock is canonical");
    let lane = uuid(42);
    timers
        .synchronize_wake(Some(super::super::lifecycle::WakeDeadline {
            lane,
            at: shifted(now, -1),
            kind: super::super::lifecycle::WakeKind::Hold,
        }))
        .expect("past lane resume arms");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("fixture async runtime builds");
    let event = runtime
        .block_on(timers.recv())
        .expect("lane resume timer fires");
    assert_eq!(event.kind(), TimerKind::LaneResume);
    assert_eq!(
        timers.consume(&bus, event).expect("lane resume consumes"),
        Fired::LaneResume(lane)
    );
    drop(timers);
    bus.shutdown().expect("fixture bus stops");
}

#[test]
fn an_expired_silence_horizon_moves_one_stable_directive_and_rearms_the_next() {
    let (bus, directive, key) = steering_fixture();
    let mut timers = start_timers().expect("timer runtime starts");
    timers.synchronize(&bus).expect("steering horizon arms");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("fixture async runtime builds");
    let fired = runtime
        .block_on(timers.recv())
        .expect("past reminder horizon fires");
    assert_eq!(fired.kind(), TimerKind::SteerEscalate);
    timers
        .consume(&bus, fired)
        .expect("reminder transition commits");
    timers.synchronize(&bus).expect("next horizon re-arms");

    let snapshot = bus.fact_snapshot().expect("steering facts read");
    let ledger = steer::Ledger::from_snapshot(&snapshot).expect("steering replays");
    let standing = ledger
        .directive(directive)
        .expect("stable directive remains");
    assert_eq!(standing.key(), &key);
    assert_eq!(standing.stage(), wire::body::SteeringStage::Reminded);
    assert_eq!(standing.escalations().len(), 1);
    assert_eq!(ledger.directives().len(), 1);
    let armed = timers
        .directives
        .get(&directive)
        .expect("escalated horizon is armed next");
    assert!(matches!(
        armed.authority.body.transition,
        steering::DeadlineTransition::Escalate {
            from: wire::body::SteeringStage::Reminded,
            to: wire::body::SteeringStage::Escalated,
        }
    ));
    drop(timers);
    bus.shutdown().expect("fixture bus stops");
}

#[test]
fn retirement_winning_the_race_makes_a_fired_escalation_append_nothing() {
    let (bus, directive, key) = steering_fixture();
    let mut timers = start_timers().expect("timer runtime starts");
    timers.synchronize(&bus).expect("steering horizon arms");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("fixture async runtime builds");
    let fired = runtime
        .block_on(timers.recv())
        .expect("past reminder horizon fires");

    let at = Instant::new(jiff::Timestamp::now()).expect("fixture clock");
    let current = bus
        .read(move |read| read.steering_head(directive))
        .expect("indexed directive reads")
        .expect("indexed directive exists");
    let mut retirement = JournalWrite::new(
        Actor::Lane(lane_uuid()),
        format!("resident:{}", epoch_uuid())
            .parse()
            .expect("fixture origin"),
        at,
        Producer::AckScanner,
    );
    let body = body::SteerAcked {
        directive,
        key: key.to_string(),
        lane: lane_uuid(),
        epoch: epoch_uuid(),
        locus: body::TranscriptLocus {
            source: Some(epoch_uuid()),
            member: 1,
            offset: 2,
            length: 3,
            event: 0,
            line: 1,
            column: 1,
        },
    };
    let pending = retirement
        .fact(body.clone(), FactCause::Root)
        .expect("retirement fact builds");
    let next = steer::Ledger::transition_head(current.body(), steer::Steering::Acked(&body), at)
        .expect("retirement head builds");
    retirement.steering_head(pending, move |_| next);
    assert!(matches!(
        bus.commit(retirement).expect("retirement submits"),
        bus::CommitOutcome::Committed(_)
    ));

    timers
        .consume(&bus, fired)
        .expect("stale fired transition settles harmlessly");
    timers.synchronize(&bus).expect("retired timer disarms");
    let snapshot = bus.fact_snapshot().expect("steering facts read");
    assert_eq!(
        snapshot
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == wire::FactKind::SteerEscalated)
            .count(),
        0
    );
    let ledger = steer::Ledger::from_snapshot(&snapshot).expect("steering replays");
    assert_eq!(ledger.disposition(&key), Some(wire::Disposition::Acked));
    assert!(!timers.directives.contains_key(&directive));
    drop(timers);
    bus.shutdown().expect("fixture bus stops");
}

#[test]
fn overdue_startup_catches_up_all_missing_stages_in_order_on_one_identity() {
    let overdue = horizons().last().copied().expect("a cadence with rungs") + 1;
    let (bus, directive, key) = steering_fixture_with_age(overdue);
    let mut timers = start_timers().expect("timer runtime starts");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("fixture async runtime builds");
    for expected in [
        wire::body::SteeringStage::Reminded,
        wire::body::SteeringStage::Escalated,
        wire::body::SteeringStage::Overdue,
    ] {
        timers.synchronize(&bus).expect("next overdue horizon arms");
        let fired = runtime
            .block_on(timers.recv())
            .expect("overdue horizon fires");
        timers
            .consume(&bus, fired)
            .expect("ordered transition commits");
        let snapshot = bus.fact_snapshot().expect("steering facts read");
        let ledger = steer::Ledger::from_snapshot(&snapshot).expect("steering replays");
        let standing = ledger
            .directive(directive)
            .expect("stable directive remains");
        assert_eq!(standing.key(), &key);
        assert_eq!(standing.stage(), expected);
    }
    timers
        .synchronize(&bus)
        .expect("expiry re-arms after overdue");
    let snapshot = bus.fact_snapshot().expect("steering facts read");
    let ledger = steer::Ledger::from_snapshot(&snapshot).expect("steering replays");
    let standing = ledger
        .directive(directive)
        .expect("stable directive remains");
    assert_eq!(standing.escalations().len(), 3);
    assert_eq!(ledger.directives().len(), 1);
    assert!(matches!(
        timers
            .directives
            .get(&directive)
            .expect("expiry is armed")
            .authority
            .body
            .transition,
        steering::DeadlineTransition::Lapse
    ));
    drop(timers);
    bus.shutdown().expect("fixture bus stops");
}

/// The horizons the shipped cadence names, in seconds since publication.
///
/// Ages taken from the cadence keep these tests immediate whatever the
/// cadence says: a directive published before a rung is already due, so the
/// runtime fires it at once instead of sleeping out the horizon.
fn horizons() -> Vec<i64> {
    steer::Config::default()
        .resend()
        .iter()
        .map(|rung| i64::try_from(rung.get().as_secs()).expect("a horizon in range"))
        .collect()
}

fn steering_fixture() -> (Bus, CanonicalUuid, steer::Key) {
    steering_fixture_with_age(horizons()[0] + 1)
}

fn steering_fixture_with_age(age_seconds: i64) -> (Bus, CanonicalUuid, steer::Key) {
    let now = Instant::new(jiff::Timestamp::now()).expect("fixture clock");
    let published = shifted(now, -age_seconds);
    let expires = shifted(now, 3600);
    let directive = uuid(80);
    let key: steer::Key = "1kJK6N3R".parse().expect("fixture key");
    let origin: wire::Origin = format!("resident:{}", epoch_uuid())
        .parse()
        .expect("fixture origin");
    let mut write = JournalWrite::new(
        Actor::Operator,
        origin.clone(),
        published,
        Producer::CommandDoor,
    );
    let body = body::SteerSent {
        from: None,
        room: None,
        directive,
        fanout: None,
        key: key.to_string(),
        lane: lane_uuid(),
        text: "hold the stable line".to_owned(),
        control: None,
        attachments: Vec::new(),
        expires,
    };
    let pending = write
        .fact(body.clone(), FactCause::Root)
        .expect("steering publication builds");
    write.steering_head(pending, move |seq| {
        steer::Ledger::sent_head(&body, origin, published, seq).expect("steering head builds")
    });
    let mut journal = Journal::in_memory().expect("fixture journal opens");
    journal.apply(write).expect("steering publication seeds");
    (
        Bus::start(journal).expect("fixture bus starts"),
        directive,
        key,
    )
}

fn lane_uuid() -> CanonicalUuid {
    uuid(81)
}

fn epoch_uuid() -> CanonicalUuid {
    uuid(82)
}

fn fixture() -> Fixture {
    fixture_with_expiry(-1)
}

fn fixture_with_expiry(expiry_seconds: i64) -> Fixture {
    fixture_with_expiry_at(expiry_seconds, format!("/fixture/{}", uuid(20)))
}

fn fixture_with_expiry_at(expiry_seconds: i64, worktree: String) -> Fixture {
    let now = Instant::new(jiff::Timestamp::now()).expect("fixture clock is canonical");
    let authority = Authority {
        task: task(),
        lane: uuid(20),
        occupant: uuid(21),
        epoch: uuid(22),
        worktree,
        start: shifted(now, -2),
        expiry: shifted(now, expiry_seconds),
        maximum: shifted(now, 600),
    };
    let mut journal = Journal::in_memory().expect("fixture journal opens");
    journal
        .apply(seed_write(&authority))
        .expect("fixture authority seeds");
    Fixture {
        bus: Bus::start(journal).expect("fixture bus starts"),
        authority,
    }
}

fn automatic_fixture() -> Fixture {
    automatic_fixture_with_age(crate::board_command::AUTOMATIC_HEARTBEAT_SECONDS + 1)
}

fn automatic_fixture_with_age(age_seconds: i64) -> Fixture {
    let now = Instant::new(jiff::Timestamp::now()).expect("fixture clock is canonical");
    let start = shifted(now, -age_seconds);
    let expiry = shifted(start, crate::board_command::AUTOMATIC_PROVISIONAL_SECONDS);
    let maximum = shifted(start, crate::board_command::AUTOMATIC_MAXIMUM_SECONDS);
    let authority = Authority {
        task: task(),
        lane: uuid(20),
        occupant: uuid(21),
        epoch: uuid(22),
        worktree: format!("/fixture/{}", uuid(20)),
        start,
        expiry,
        maximum,
    };
    let mut journal = Journal::in_memory().expect("fixture journal opens");
    journal
        .apply(seed_write(&authority))
        .expect("fixture automatic authority seeds");
    Fixture {
        bus: Bus::start(journal).expect("fixture bus starts"),
        authority,
    }
}

fn renew_claim(fixture: &Fixture, expiry: Instant) {
    let stamp = fixture.authority.born();
    let lease = fixture
        .bus
        .read({
            let stamp = stamp.clone();
            move |read| Ok(read.lease_head(&stamp)?.expect("lease head exists"))
        })
        .expect("lease head reads");
    let at = Instant::new(jiff::Timestamp::now()).expect("fixture clock is canonical");
    let mut write = JournalWrite::new(
        Actor::Spice,
        format!("task:{}", fixture.authority.task)
            .parse()
            .expect("fixture origin"),
        at,
        Producer::BusTimer,
    );
    let body = body::ClaimRenewed {
        task: stamp.clone(),
        epoch: fixture.authority.epoch,
        until: expiry,
        maximum: lease.body().maximum,
    };
    let renewed = write
        .fact(body, FactCause::Root)
        .expect("renewal fact builds");
    let start = lease.body().start;
    let maximum = lease.body().maximum;
    let epoch = fixture.authority.epoch;
    write.lease_head(stamp, renewed, move |_| LeaseHead {
        start,
        renewal: at,
        expiry,
        maximum,
        epoch,
    });
    assert!(matches!(
        fixture.bus.commit(write).expect("renewal commits"),
        bus::CommitOutcome::Committed(_)
    ));
}

fn free_claim(fixture: &Fixture) {
    let stamp = fixture.authority.born();
    let claim = fixture
        .bus
        .read({
            let stamp = stamp.clone();
            move |read| Ok(read.claim_head(&stamp)?.expect("claim head exists"))
        })
        .expect("claim head reads");
    let at = Instant::new(jiff::Timestamp::now()).expect("fixture clock is canonical");
    let actor = Actor::Lane(fixture.authority.lane);
    let mut write = JournalWrite::new(
        actor.clone(),
        format!("task:{}", fixture.authority.task)
            .parse()
            .expect("fixture origin"),
        at,
        Producer::CommandDoor,
    );
    let freed = write
        .fact(
            body::ClaimFreed {
                task: stamp.clone(),
                actor,
                parked: None,
            },
            FactCause::Root,
        )
        .expect("free fact builds");
    let mut next = claim.body().clone();
    next.disposition = ClaimDisposition::Freed;
    write.claim_head(stamp, freed, move |fact| ClaimHead { fact, ..next });
    assert!(matches!(
        fixture.bus.commit(write).expect("release commits"),
        bus::CommitOutcome::Committed(_)
    ));
}

fn seed_write(authority: &Authority) -> JournalWrite {
    seed_write_with_lanes(authority, vec![lane_residency(authority)])
}

fn seed_write_with_lanes(authority: &Authority, lanes: Vec<LaneResidency>) -> JournalWrite {
    let phase = "todo".parse::<Phase>().expect("fixture phase");
    let origin = format!("task:{}", authority.task)
        .parse::<Origin>()
        .expect("fixture origin");
    let mut write = JournalWrite::new(
        Actor::Lane(authority.lane),
        origin.clone(),
        authority.start,
        Producer::CommandDoor,
    );
    let added = write
        .fact(
            body::TaskAdded {
                preparation: None,
                intent: None,
                task: authority.born(),
                title: "Capture commit after lapse".to_owned(),
                pitch: "A lapsed capture still needs its commit.".to_owned(),
                description: None,
                acceptance: Vec::new(),
                requirements: wire::EnvironmentRequirements::default(),
                project: "timer.claim".to_owned(),
                filing: wire::TaskFiling::Filed,
                priority: None,
                taste: None,
                document_slug: None,
                depends: Vec::new(),
                flow: vec![phase.clone()],
                phase: phase.clone(),
                creation: body::TaskCreationContext::unavailable(
                    body::TaskCreationUnavailable::SpiceActor,
                ),
                revision: 1,
            },
            FactCause::Root,
        )
        .expect("task fact builds");
    let won = write
        .fact(
            body::ClaimWon {
                task: authority.born(),
                actor: Actor::Lane(authority.lane),
                lane: authority.lane,
                occupant: authority.occupant,
                epoch: authority.epoch,
                cell: wire::ClaimCell {
                    project: "timer.claim".to_owned(),
                    phase: phase.clone(),
                },
                until: authority.expiry,
                git: Some(GitBoundary {
                    head: "local-commit".to_owned(),
                    integrated: "local-commit".to_owned(),
                    upstream: Some("origin/main".to_owned()),
                    upstream_head: Some("baseline".to_owned()),
                    local_commits: 1,
                }),
                recovery: None,
                source: body::SourceBoundary::Proven { heads: Vec::new() },
            },
            FactCause::Pending(added),
        )
        .expect("claim fact builds");
    let seated = write
        .fact(
            body::UpSeated {
                epoch: authority.epoch,
            },
            FactCause::Pending(won),
        )
        .expect("resident fact builds");
    attach_heads(
        &mut write,
        authority,
        phase,
        SeedFacts { added, won, seated },
        lanes,
    );
    write
}

fn attach_heads(
    write: &mut JournalWrite,
    authority: &Authority,
    phase: Phase,
    facts: SeedFacts,
    lanes: Vec<LaneResidency>,
) {
    let SeedFacts { added, won, seated } = facts;
    let statement_task = authority.born();
    let statement_origin = wire::TaskProvenance::Task(authority.born());
    let statement_phase = phase.clone();
    write.task_head(added, move |position| TaskHead {
        preparation: None,
        intent: None,
        born: statement_task,
        queue_origin: position,
        title: "Capture commit after lapse".to_owned(),
        pitch: "A lapsed capture still needs its commit.".to_owned(),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        project: "timer.claim".to_owned(),
        filing: wire::TaskFiling::Filed,
        priority: None,
        taste: None,
        document_slug: None,
        dependencies: Vec::new(),
        provenance: statement_origin,
        creation: body::TaskCreationContext::unavailable(body::TaskCreationUnavailable::SpiceActor),
        revision: position,
    });
    let workflow_task = authority.born();
    write.workflow_head(workflow_task, added, move |_| WorkflowHead {
        flow: vec![statement_phase.clone()],
        phase: statement_phase,
        status: wire::WorkflowStatus::pending(),
        deferral: None,
        generation: 0,
    });
    let claim_task = authority.born();
    let lane = authority.lane;
    let occupant = authority.occupant;
    let epoch = authority.epoch;
    write.claim_head(claim_task, won, move |position| ClaimHead {
        actor: Actor::Lane(lane),
        fact: position,
        claimed: position,
        cell: wire::ClaimCell {
            project: "timer.claim".to_owned(),
            phase,
        },
        lane,
        occupant,
        epoch,
        recovery: None,
        disposition: ClaimDisposition::Active,
        acknowledged: 0,
    });
    let lease_task = authority.born();
    let start = authority.start;
    let expiry = authority.expiry;
    let maximum = authority.maximum;
    write.lease_head(lease_task, won, move |_| LeaseHead {
        start,
        renewal: start,
        expiry,
        maximum,
        epoch,
    });
    let subscriptions = lanes
        .iter()
        .map(|lane| wire::TeamSubscription {
            team: lane.team,
            formed_at: None,
            policy: wire::intent::TeamLife::Drive,
            fast_mode: false,
        })
        .collect();
    write.residency_head(seated, move |_| ResidencyHead {
        epoch,
        state: ResidentState::Seated,
        topology_revision: 1,
        lanes,
        subscriptions,
        renewals: Vec::new(),
    });
}

fn lane_residency(authority: &Authority) -> LaneResidency {
    LaneResidency {
        lane: authority.lane,
        team: authority.lane,
        name: None,
        worktree: authority.worktree.clone(),
        life: "drive".to_owned(),
        vendor: "omp".parse().expect("fixture vendor"),
        model: None,
        effort: None,
        fast_mode: false,
        occupant: Some(authority.occupant),
        lineage: vec![authority.occupant],
        capabilities: vec![authority.occupant],
        session: None,
        disposition: "occupied".to_owned(),
    }
}

fn lapsed_heads(fixture: &Fixture) -> TaskHeads {
    fixture
        .bus
        .task_snapshot()
        .expect("lapsed heads read")
        .tasks()
        .iter()
        .find(|heads| heads.statement().body().born == fixture.authority.born())
        .expect("fixture task remains")
        .clone()
}
fn assert_lapsed_and_eligible(fixture: &Fixture, heads: &TaskHeads) {
    assert_eq!(
        heads
            .claim()
            .expect("claim head remains")
            .body()
            .disposition,
        ClaimDisposition::Lapsed
    );
    let (statement, workflow, claim, lease) = heads.clone().into_bodies();
    let entry = Entry::new(statement, workflow, claim, lease).expect("lapsed task remains valid");
    assert!(
        board::eligible_entry(
            &board::resolved_tasks(std::slice::from_ref(&entry)),
            &entry,
            &Actor::Lane(fixture.authority.lane),
            &wire::Environment::journal_schema(journal::SCHEMA_VERSION).expect("positive schema"),
        ),
        "the commit-bearing task is eligible for stale-claim adoption"
    );
}
fn capture_lapsed_task(
    fixture: &Fixture,
    heads: &TaskHeads,
    epoch: CanonicalUuid,
    occupant: CanonicalUuid,
) {
    let capture = "capture".parse::<wire::Name>().expect("capture rule");
    let capture_command = Command {
        v: WireVersion::CURRENT,
        id: uuid(31),
        epoch,
        kind: CommandKind::ClaimWin,
        expected: vec![
            Witness::Task {
                task: fixture.authority.born(),
                head: heads.statement().position(),
            },
            Witness::Workflow {
                task: fixture.authority.born(),
                head: heads.workflow().position(),
                phase: heads.workflow().body().phase.clone(),
            },
            Witness::Claim {
                task: fixture.authority.born(),
                head: Some(heads.claim().expect("lapsed claim head").position()),
            },
            Witness::Eligibility {
                rule: capture.clone(),
            },
            Witness::Residency {
                lane: fixture.authority.lane,
                occupant: Some(occupant),
                epoch,
            },
        ],
        body: serde_json::to_value(wire::intent::ClaimWin {
            rule: capture,
            recovery: None,
        })
        .expect("capture intent encodes"),
    }
    .admit(
        &Accepted {
            wire: WireVersion::CURRENT,
            actor: Actor::Lane(fixture.authority.lane),
            epoch,
            release: "1.2.3".parse().expect("fixture release"),
        },
        &wire::principal::PrincipalRoles::default(),
    )
    .expect("capture command admits");
    let capture = crate::board_command::dispatch_with_repository(
        &fixture.bus,
        &capture_command,
        None,
        &board::NextConfig::default(),
        None,
    )
    .expect("board owns capture command");
    assert!(
        matches!(
            capture,
            Outcome::Reply(Reply {
                position: ReplyPosition::Committed(_),
                ..
            })
        ),
        "commit capture did not acquire the lapsed task: {capture:?}"
    );
}

fn move_residency(fixture: &Fixture, epoch: CanonicalUuid, occupant: CanonicalUuid) {
    let observed = fixture
        .bus
        .residency_head()
        .expect("residency reads")
        .expect("residency exists");
    let mut next = observed.body().clone();
    next.epoch = epoch;
    next.lanes[0].occupant = Some(occupant);
    next.lanes[0].lineage.push(occupant);
    let mut write = JournalWrite::new(
        Actor::Spice,
        format!("resident:{epoch}").parse().expect("fixture origin"),
        Instant::new(jiff::Timestamp::now()).expect("fixture clock is canonical"),
        Producer::CommandDoor,
    );
    let seated = write
        .fact(body::UpSeated { epoch }, FactCause::Root)
        .expect("successor resident fact builds");
    write.residency_head(seated, move |_| next);
    assert!(matches!(
        fixture
            .bus
            .commit(write)
            .expect("successor resident commits"),
        bus::CommitOutcome::Committed(_)
    ));
}

fn assert_commit_history(bus: &Bus) {
    let facts = bus.fact_snapshot().expect("lapse facts read");
    assert!(
        facts
            .facts()
            .iter()
            .any(|fact| fact.fact.kind() == wire::FactKind::ClaimLapsed)
    );
    assert_eq!(
        facts
            .facts()
            .iter()
            .find(|fact| fact.fact.kind() == wire::FactKind::ClaimWon)
            .expect("winning claim fact remains")
            .fact
            .body()["git"]["local_commits"],
        1
    );
    assert_eq!(
        facts
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == wire::FactKind::ClaimWon)
            .count(),
        2,
        "capture writes a new claim after the timer-owned lapse"
    );
}

/// A timer runtime whose lanes keep no jobs.
fn start_timers() -> CommandResult<Runtime> {
    Runtime::start(
        steer::Config::default(),
        PathBuf::from("/nonexistent/spice-timer-supervisors"),
    )
}

fn task() -> TaskHandle {
    TaskHandle::for_project(
        "timer.claim",
        &"1kKRRsDP".parse().expect("fixture task stamp"),
    )
}

fn uuid(last: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131164{last:02}")
        .parse()
        .expect("fixture UUID")
}

fn shifted(at: Instant, seconds: i64) -> Instant {
    Instant::new(
        at.get()
            .checked_add(jiff::SignedDuration::from_secs(seconds))
            .expect("fixture instant is in range"),
    )
    .expect("shifted fixture instant is canonical")
}
