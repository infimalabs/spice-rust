//! Starts whose session proof completes after other commits landed.

use super::super::super::propose::tests::seed_task_claim;
use super::*;

const OTHER_LANE: u64 = 40;
const OTHER_OCCUPANT: u64 = 41;
const SUCCESSOR: u64 = 42;
const DIRECTIVE: u64 = 43;
const LATE_DIRECTIVE: u64 = 44;

struct Window {
    fixture: AdvisoryRtkFixture,
    bus: Bus,
    capabilities: serve::axum::Capabilities,
    discovery: serve::Discovery,
    seats: Seats,
}

impl Window {
    fn new() -> Self {
        let fixture = AdvisoryRtkFixture::new();
        let bus = Bus::start(journal::Journal::in_memory().expect("fixture journal opens"))
            .expect("fixture bus starts");
        seed_two_lanes(&bus, &fixture);
        let discovery = serve::Discovery {
            endpoint: "http://127.0.0.1:1".to_owned(),
            capability: fixture.capability,
            epoch: fixture.epoch,
        };
        super::super::super::super::steering::publish_lane_surface(
            &fixture.paths,
            &discovery,
            fixture.lane,
            fixture.capability,
        )
        .expect("fixture lane surface publishes");
        Self {
            fixture,
            bus,
            capabilities: serve::axum::Capabilities::default(),
            discovery,
            seats: Seats::new(),
        }
    }

    fn begin(
        &self,
        decision: Option<&bus::TaskSnapshot>,
        reserved: Option<&crate::board_command::AutomaticClaim>,
        inbox: bool,
    ) -> AutomaticLaunch {
        let runtime = self
            .fixture
            .lane_runtime(&self.bus, &self.capabilities, &self.discovery);
        match begin_automatic_wake(
            &self.bus,
            &self.fixture.paths,
            self.fixture.epoch,
            &self.seats,
            &runtime,
            &steer::Config::default(),
            self.fixture.lane,
            uuid(SUCCESSOR),
            decision,
            reserved,
            inbox,
        ) {
            Ok(BeginAutomaticWake::Launch(launch)) => launch,
            Ok(BeginAutomaticWake::Unstarted(outcome)) => {
                panic!("launch did not spawn: {outcome:?}")
            }
            Err(outcome) => panic!("automatic wake refused admission: {outcome:?}"),
        }
    }

    fn finish(&mut self, launch: AutomaticLaunch) -> CommandResult<AutomaticWake> {
        let (launch, pending) = launch.into_parts();
        let established = launch.establish();
        finish_automatic_wake(
            &self.bus,
            &self.fixture.paths,
            self.fixture.epoch,
            &mut self.seats,
            pending,
            established,
        )
    }

    /// Another lane starts and an unrelated task is claimed while proof runs.
    fn land_unrelated_commits(&self) {
        let observed = current_residency(&self.bus, self.fixture.epoch).expect("residency reads");
        let mut next = observed.body().clone();
        let other = next
            .lanes
            .iter_mut()
            .find(|row| row.lane == uuid(OTHER_LANE))
            .expect("other lane exists");
        other.occupant = Some(uuid(OTHER_OCCUPANT));
        other.lineage.push(uuid(OTHER_OCCUPANT));
        other.disposition = OCCUPIED.to_owned();
        let durable = other.clone();
        let mut write = write(
            Actor::Spice,
            self.fixture.epoch,
            Producer::ResidentRecognition,
        )
        .expect("other start write opens");
        guard_residency(&mut write, &observed);
        let started = write
            .fact(
                wire::body::LaneStarted {
                    lane: uuid(OTHER_LANE),
                    occupant: uuid(OTHER_OCCUPANT),
                    epoch: self.fixture.epoch,
                    group: 7,
                    fresh_session: None,
                },
                FactCause::Existing(observed.position()),
            )
            .expect("other start fact is valid");
        write.residency_head(started, move |_| next);
        commit(&self.bus, write).expect("other lane start commits");
        let unrelated: wire::TaskStamp = "1kQpV9Gw".parse().expect("unrelated task");
        claim_task(&self.bus, &unrelated, &durable, self.fixture.epoch);
    }

    fn row(&self) -> wire::LaneResidency {
        current_residency(&self.bus, self.fixture.epoch)
            .expect("residency reads")
            .body()
            .lanes
            .iter()
            .find(|row| row.lane == self.fixture.lane)
            .cloned()
            .expect("fixture lane exists")
    }

    fn assert_started(&self, outcome: CommandResult<AutomaticWake>) {
        match outcome {
            Ok(AutomaticWake::Started) => {}
            Ok(AutomaticWake::Unstarted(failure)) => panic!("proof failed: {failure:?}"),
            Err(refusal) => panic!("proven occupant was refused: {refusal:?}"),
        }
        assert_eq!(self.row().occupant, Some(uuid(SUCCESSOR)));
        assert!(self.seats.occupied(self.fixture.lane));
        let head = current_residency(&self.bus, self.fixture.epoch).expect("residency reads");
        let other = head
            .body()
            .lanes
            .iter()
            .find(|row| row.lane == uuid(OTHER_LANE))
            .expect("other lane exists");
        assert_eq!(other.occupant, Some(uuid(OTHER_OCCUPANT)));
    }

    fn assert_refused(&self, outcome: CommandResult<AutomaticWake>, reason: &str) {
        match outcome {
            Err(Outcome::Refusal(refusal)) => assert_eq!(refusal.reason(), reason),
            Err(other) => panic!("expected {reason}, found {other:?}"),
            Ok(AutomaticWake::Started) => panic!("expected {reason}, but the occupant started"),
            Ok(AutomaticWake::Unstarted(failure)) => panic!("expected {reason}, found {failure:?}"),
        }
        assert_eq!(self.row().occupant, None);
        assert!(!self.seats.occupied(self.fixture.lane));
    }

    fn close(mut self) {
        self.seats.release_all();
        super::super::super::super::steering::remove_lane_surface(
            &self.fixture.paths,
            self.fixture.lane,
        );
        self.bus.verify().expect("proof window authority verifies");
        self.bus.shutdown().expect("fixture bus stops");
    }
}

#[test]
fn an_inbox_wake_starts_after_unrelated_commits_but_not_after_its_steering_is_withdrawn() {
    let mut window = Window::new();
    send_steering(&window, DIRECTIVE, "1kInbox1");
    let snapshot = window.bus.task_snapshot().expect("decision snapshot reads");
    let launch = window.begin(Some(&snapshot), None, true);
    window.land_unrelated_commits();
    let outcome = window.finish(launch);
    window.assert_started(outcome);
    window.close();

    let mut window = Window::new();
    send_steering(&window, DIRECTIVE, "1kInbox1");
    let snapshot = window.bus.task_snapshot().expect("decision snapshot reads");
    let launch = window.begin(Some(&snapshot), None, true);
    cancel_steering(&window, DIRECTIVE);
    let outcome = window.finish(launch);
    window.assert_refused(outcome, "lifecycle::wake-inbox-moved");
    window.close();
}

#[test]
fn a_held_claim_wake_carries_its_claim_after_unrelated_commits_but_not_after_it_moves() {
    let task: wire::TaskStamp = "1kQpV9Gt".parse().expect("held task");
    let mut window = Window::new();
    claim_task(
        &window.bus,
        &task,
        &predecessor_row(&window),
        window.fixture.epoch,
    );
    let snapshot = window.bus.task_snapshot().expect("decision snapshot reads");
    let launch = window.begin(Some(&snapshot), None, false);
    window.land_unrelated_commits();
    let outcome = window.finish(launch);
    window.assert_started(outcome);
    let claim = window
        .bus
        .task_snapshot()
        .expect("carried claim reads")
        .tasks()
        .iter()
        .find(|heads| heads.statement().body().born == task)
        .and_then(|heads| heads.claim().cloned())
        .expect("held claim survives");
    assert_eq!(claim.body().occupant, uuid(SUCCESSOR));
    window.close();

    let mut window = Window::new();
    claim_task(
        &window.bus,
        &task,
        &predecessor_row(&window),
        window.fixture.epoch,
    );
    let snapshot = window.bus.task_snapshot().expect("decision snapshot reads");
    let launch = window.begin(Some(&snapshot), None, false);
    renew_claim(&window.bus, &task);
    let outcome = window.finish(launch);
    window.assert_refused(outcome, "lifecycle::held-claim-head-moved");
    window.close();
}

#[test]
fn an_available_work_wake_starts_after_unrelated_commits_but_not_after_its_lane_moves() {
    let task: wire::TaskStamp = "1kQpV9Gt".parse().expect("reserved task");
    for retuned in [false, true] {
        let mut window = Window::new();
        let mut reserved_row = window.row();
        reserved_row.occupant = Some(uuid(SUCCESSOR));
        claim_task(&window.bus, &task, &reserved_row, window.fixture.epoch);
        let reserved = crate::board_command::AutomaticClaim {
            task: task.clone(),
            lane: window.fixture.lane,
            occupant: uuid(SUCCESSOR),
        };
        let launch = window.begin(None, Some(&reserved), false);
        window.land_unrelated_commits();
        if retuned {
            retune_lane(&window);
            let outcome = window.finish(launch);
            window.assert_refused(outcome, "resident::head-moved");
        } else {
            let outcome = window.finish(launch);
            window.assert_started(outcome);
        }
        window.close();
    }
}

#[test]
fn an_operator_wake_commits_after_unrelated_commits() {
    let mut window = Window::new();
    let request = LaneState {
        lane: window.fixture.lane,
        action: wire::intent::LaneAction::Wake,
        worktree: None,
    };
    let runtime = window
        .fixture
        .lane_runtime(&window.bus, &window.capabilities, &window.discovery);
    let launch = begin_wake(
        &window.bus,
        &window.fixture.paths,
        window.fixture.epoch,
        &window.seats,
        &runtime,
        &steer::Config::default(),
        &request,
    )
    .unwrap_or_else(|outcome| panic!("operator wake refused admission: {outcome:?}"));
    window.land_unrelated_commits();
    let occupant = launch.pending.occupant;
    let (launch, pending) = launch.into_parts();
    let outcome = finish_wake(
        &window.bus,
        &window.fixture.paths,
        window.fixture.epoch,
        &mut window.seats,
        &operator_wake(&window.fixture, &request),
        pending,
        launch.establish(),
    );
    assert!(matches!(outcome, Outcome::Reply(_)), "{outcome:?}");
    assert_eq!(window.row().occupant, Some(occupant));
    window.close();
}

#[test]
fn an_operator_wake_carries_the_claim_and_its_successor_lease_keeps_renewing() {
    let task: wire::TaskStamp = "1kQpV9Gt".parse().expect("held task");
    let mut window = Window::new();
    let minted_expiry = claim_task_due(
        &window.bus,
        &task,
        &predecessor_row(&window),
        window.fixture.epoch,
    );
    let before = window.bus.task_snapshot().expect("predecessor claim reads");
    let before = before
        .tasks()
        .iter()
        .find(|heads| heads.statement().body().born == task)
        .expect("predecessor task exists");
    let predecessor = before.claim().expect("predecessor claim exists").clone();
    let predecessor_phase = before.workflow().body().phase.clone();
    let request = LaneState {
        lane: window.fixture.lane,
        action: wire::intent::LaneAction::Wake,
        worktree: None,
    };
    let runtime = window
        .fixture
        .lane_runtime(&window.bus, &window.capabilities, &window.discovery);
    let launch = begin_wake(
        &window.bus,
        &window.fixture.paths,
        window.fixture.epoch,
        &window.seats,
        &runtime,
        &steer::Config::default(),
        &request,
    )
    .unwrap_or_else(|outcome| panic!("operator wake refused admission: {outcome:?}"));
    let successor = launch.pending.occupant;
    let (launch, pending) = launch.into_parts();
    let wake_started =
        wire::Instant::new(jiff::Timestamp::now()).expect("wake instant is canonical");
    let outcome = finish_wake(
        &window.bus,
        &window.fixture.paths,
        window.fixture.epoch,
        &mut window.seats,
        &operator_wake(&window.fixture, &request),
        pending,
        launch.establish(),
    );
    assert!(matches!(outcome, Outcome::Reply(_)), "{outcome:?}");

    let carried_snapshot = window.bus.task_snapshot().expect("carried claim reads");
    let residency = carried_snapshot
        .residency()
        .expect("carried residency exists");
    let carried = carried_snapshot
        .tasks()
        .iter()
        .find(|heads| heads.statement().body().born == task)
        .expect("carried task exists");
    let claim = carried.claim().expect("carried claim exists");
    let lease = carried.lease().expect("carried lease exists");
    assert_eq!(claim.body().occupant, successor);
    assert_eq!(claim.body().epoch, window.fixture.epoch);
    assert_eq!(lease.body().epoch, window.fixture.epoch);
    assert!(lease.body().renewal >= wake_started);
    let rolling_floor = wire::Instant::new(
        wake_started
            .get()
            .checked_add(jiff::SignedDuration::from_secs(
                crate::board_command::AUTOMATIC_ROLLING_SECONDS,
            ))
            .expect("rolling deadline shifts"),
    )
    .expect("rolling deadline is canonical");
    assert!(lease.body().expiry >= rolling_floor);
    assert!(lease.body().expiry > minted_expiry);
    let carried_expiry = lease.body().expiry;
    assert_eq!(claim.body().claimed, predecessor.body().claimed);
    assert_eq!(claim.body().cell, predecessor.body().cell);
    assert_eq!(claim.body().acknowledged, predecessor.body().acknowledged);
    assert_eq!(carried.workflow().body().phase, predecessor_phase);
    assert_eq!(claim.position(), lease.position());
    assert_eq!(claim.position(), residency.position());
    let facts = window.bus.fact_snapshot().expect("carried facts read");
    let carried_fact = facts
        .facts()
        .iter()
        .find(|fact| fact.seq == claim.position())
        .expect("claim head names its carried fact");
    assert_eq!(carried_fact.fact.kind(), wire::FactKind::ClaimCarried);

    assert_successor_renews(&window, &task, successor, carried_expiry);
    window.close();
}

#[test]
fn a_failed_inbox_start_parks_only_the_steering_it_was_woken_for() {
    let window = Window::new();
    send_steering(&window, DIRECTIVE, "1kInbox1");
    let woken = super::super::super::super::steering::woken_inbox(&window.bus, window.fixture.lane)
        .expect("woken steering reads");
    window.land_unrelated_commits();
    send_steering(&window, LATE_DIRECTIVE, "1kInbox2");
    let failure = Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(
            "lane::session-authority-missing".to_owned(),
            None,
            None,
            None,
        )
        .with_detail("fixture proof failure"),
    ));

    let parked = super::super::super::super::steering::dead_letter_pending(
        &window.bus,
        &window.fixture.paths,
        window.fixture.epoch,
        window.fixture.lane,
        &woken,
        &failure,
    )
    .expect("woken steering parks despite unrelated commits");

    assert_eq!(
        parked
            .iter()
            .map(|parked| parked.directive)
            .collect::<Vec<_>>(),
        [uuid(DIRECTIVE)]
    );
    let pending = window
        .bus
        .read({
            let lane = window.fixture.lane;
            move |read| crate::steering_read::pending(read, Some(lane))
        })
        .expect("pending steering reads");
    assert_eq!(
        pending
            .iter()
            .map(|head| head.body().directive)
            .collect::<Vec<_>>(),
        [uuid(LATE_DIRECTIVE)],
        "steering that arrived during proof stays pending for the next wake"
    );
    window.close();
}

fn seed_two_lanes(bus: &Bus, fixture: &AdvisoryRtkFixture) {
    let mut write =
        write(Actor::Spice, fixture.epoch, Producer::CommandDoor).expect("fixture write starts");
    let seated = write
        .fact(
            wire::body::UpSeated {
                epoch: fixture.epoch,
            },
            FactCause::Root,
        )
        .expect("fixture resident seats");
    let row = |lane: CanonicalUuid, capabilities| wire::LaneResidency {
        lane,
        team: lane,
        name: None,
        worktree: fixture.root.display().to_string(),
        life: "drive".to_owned(),
        vendor: fixture.vendor.clone(),
        model: None,
        effort: None,
        fast_mode: false,
        occupant: None,
        lineage: Vec::new(),
        capabilities,
        session: None,
        disposition: VACANT.to_owned(),
    };
    let subscription = |team| wire::TeamSubscription {
        team,
        policy: wire::intent::TeamLife::Drive,
        fast_mode: false,
    };
    let head = wire::ResidencyHead {
        epoch: fixture.epoch,
        state: ResidentState::Seated,
        topology_revision: 1,
        lanes: vec![
            row(fixture.lane, vec![fixture.capability]),
            row(uuid(OTHER_LANE), Vec::new()),
        ],
        subscriptions: vec![subscription(fixture.lane), subscription(uuid(OTHER_LANE))],
        renewals: Vec::new(),
    };
    write.residency_head(seated, move |_| head);
    commit(bus, write).expect("fixture residency commits");
}

fn predecessor_row(window: &Window) -> wire::LaneResidency {
    let mut row = window.row();
    row.occupant = Some(window.fixture.occupant);
    row
}

fn claim_task(bus: &Bus, task: &wire::TaskStamp, row: &wire::LaneResidency, epoch: CanonicalUuid) {
    let now = wire::Instant::new(jiff::Timestamp::now()).expect("fixture clock is canonical");
    let shifted = |seconds| {
        wire::Instant::new(
            now.get()
                .checked_add(jiff::SignedDuration::from_secs(seconds))
                .expect("fixture instant shifts"),
        )
        .expect("fixture instant")
    };
    seed_task_claim(
        bus,
        task,
        row,
        epoch,
        shifted(-60),
        shifted(3600),
        shifted(7200),
    );
}

fn claim_task_due(
    bus: &Bus,
    task: &wire::TaskStamp,
    row: &wire::LaneResidency,
    epoch: CanonicalUuid,
) -> wire::Instant {
    let now = wire::Instant::new(jiff::Timestamp::now()).expect("fixture clock is canonical");
    let shifted = |seconds| {
        wire::Instant::new(
            now.get()
                .checked_add(jiff::SignedDuration::from_secs(seconds))
                .expect("fixture instant shifts"),
        )
        .expect("fixture instant")
    };
    let expiry = shifted(60);
    seed_task_claim(bus, task, row, epoch, shifted(-60), expiry, shifted(3600));
    expiry
}

fn assert_successor_renews(
    window: &Window,
    task: &wire::TaskStamp,
    successor: CanonicalUuid,
    carried_expiry: wire::Instant,
) {
    let projects = board::ProjectPolicy::new(["task".to_owned()], Vec::new(), 2, 4)
        .expect("fixture project policy");
    let mut timers = crate::resident::timers::Runtime::start(projects, steer::Config::default())
        .expect("timer runtime starts");
    timers
        .synchronize(&window.bus)
        .expect("successor lease arms renewal");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("fixture async runtime builds");
    let event = runtime
        .block_on(timers.recv())
        .expect("successor lease renewal fires");
    assert_eq!(event.kind(), bus::timer::TimerKind::ClaimRenew);
    assert!(matches!(
        timers.consume(&window.bus, event),
        Ok(crate::resident::timers::Fired::ClaimRenew)
    ));
    let renewed = window.bus.task_snapshot().expect("renewed claim reads");
    let renewed = renewed
        .tasks()
        .iter()
        .find(|heads| heads.statement().body().born == *task)
        .expect("renewed task exists");
    assert_eq!(
        renewed
            .claim()
            .expect("renewed claim exists")
            .body()
            .occupant,
        successor
    );
    assert!(
        renewed.lease().expect("renewed lease exists").body().expiry > carried_expiry,
        "the successor extends the promoted lease"
    );
}

fn renew_claim(bus: &Bus, task: &wire::TaskStamp) {
    let snapshot = bus.task_snapshot().expect("held claim reads");
    let heads = snapshot
        .tasks()
        .iter()
        .find(|heads| &heads.statement().body().born == task)
        .expect("held task exists");
    let claim = heads.claim().expect("claim head").body().clone();
    let lease = heads.lease().expect("lease head").body().clone();
    let origin: wire::Origin = format!("task:{task}").parse().expect("task origin");
    let at = wire::Instant::new(jiff::Timestamp::now()).expect("renewal instant");
    let mut write = JournalWrite::new(claim.actor.clone(), origin, at, Producer::BusTimer);
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
        ..claim
    });
    commit(bus, write).expect("claim renewal commits");
}

fn retune_lane(window: &Window) {
    let observed = current_residency(&window.bus, window.fixture.epoch).expect("residency reads");
    let mut next = observed.body().clone();
    let row = next
        .lanes
        .iter_mut()
        .find(|row| row.lane == window.fixture.lane)
        .expect("fixture lane exists");
    row.model = Some("fixture-model".to_owned());
    let mut write = write(
        Actor::Operator,
        window.fixture.epoch,
        Producer::ResidentRecognition,
    )
    .expect("retune write opens");
    guard_residency(&mut write, &observed);
    let retuned = write
        .fact(
            wire::body::LaneRetuned {
                lane: window.fixture.lane,
                from: None,
                model: Some("fixture-model".to_owned()),
            },
            FactCause::Existing(observed.position()),
        )
        .expect("retune fact is valid");
    write.residency_head(retuned, move |_| next);
    commit(&window.bus, write).expect("lane retune commits");
}

fn send_steering(window: &Window, directive: u64, key: &str) {
    let epoch = window.fixture.epoch;
    let residency = current_residency(&window.bus, epoch).expect("residency reads");
    let at = wire::Instant::new(jiff::Timestamp::now()).expect("steering instant");
    let origin: wire::Origin = format!("resident:{epoch}")
        .parse()
        .expect("steering origin");
    let mut sent = JournalWrite::new(Actor::Operator, origin.clone(), at, Producer::CommandDoor);
    let body = wire::body::SteerSent {
        from: None,
        directive: uuid(directive),
        fanout: None,
        key: key.to_owned(),
        lane: window.fixture.lane,
        text: "deliver me".to_owned(),
        control: None,
        attachments: Vec::new(),
        expires: "2099-01-01T00:00:00.000Z".parse().expect("fixture expiry"),
    };
    let pending = sent
        .fact(body.clone(), FactCause::Existing(residency.position()))
        .expect("steering fact is valid");
    sent.steering_head(pending, move |seq| {
        steer::Ledger::sent_head(&body, origin, at, seq).expect("steering head is valid")
    });
    commit(&window.bus, sent).expect("pending steering commits");
}

fn cancel_steering(window: &Window, directive: u64) {
    let directive = uuid(directive);
    let head = window
        .bus
        .read(move |read| read.steering_head(directive))
        .expect("steering head reads")
        .expect("directive exists");
    let at = wire::Instant::new(jiff::Timestamp::now()).expect("cancel instant");
    let origin: wire::Origin = format!("resident:{}", window.fixture.epoch)
        .parse()
        .expect("cancel origin");
    let mut write = JournalWrite::new(Actor::Operator, origin, at, Producer::CommandDoor);
    let body = wire::body::SteerCanceled {
        key: head.body().key.clone(),
        lane: window.fixture.lane,
    };
    let canceled = write
        .fact(body.clone(), FactCause::Existing(head.body().created))
        .expect("cancel fact is valid");
    let next = steer::Ledger::transition_head(head.body(), steer::Steering::Canceled(&body), at)
        .expect("cancel transition is valid");
    write.steering_head(canceled, move |_| next);
    commit(&window.bus, write).expect("steering cancel commits");
}

fn operator_wake(fixture: &AdvisoryRtkFixture, request: &LaneState) -> wire::AdmittedCommand {
    wire::Command {
        v: wire::WireVersion::CURRENT,
        id: uuid(45),
        epoch: fixture.epoch,
        kind: wire::CommandKind::LaneState,
        expected: Vec::new(),
        body: serde_json::to_value(request).expect("lane state request encodes"),
    }
    .admit(&wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor: Actor::Operator,
        epoch: fixture.epoch,
    })
    .expect("operator wake command admits")
}

fn uuid(value: u64) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-{value:012x}")
        .parse()
        .expect("fixture UUID")
}
