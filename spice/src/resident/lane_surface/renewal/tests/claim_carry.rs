use super::*;

const TASK: &str = "1kQpV9Gt";
const SUCCESSOR_SESSION: &str = "018f8888-8888-7444-8888-999999999999";

#[test]
fn renewal_carries_one_claim_through_the_same_org_slot() {
    let fixture = AdvisoryRtkFixture::new();
    fixture.set_next_session(SUCCESSOR_SESSION);
    let seeded = seed_claimed_org(&fixture);
    let discovery = publish_surface(&fixture);
    let repository = Repository::discover(&fixture.root).expect("fixture repository resolves");
    let surface = super::super::super::super::load_startup_surface(&repository)
        .expect("fixture startup policy resolves");
    let capabilities = canvas::axum::Capabilities::default();
    let lane_runtime = Runtime::new(&capabilities, &discovery, fixture.reporter());
    let mut lifecycle = super::super::super::super::lifecycle::Runtime::new(&surface.config.lane);
    let mut seats = Seats::new();

    reconcile(
        &mut lifecycle,
        &seeded.bus,
        &repository,
        &fixture,
        &mut seats,
        &lane_runtime,
        &surface,
    );
    let successor = assert_carried(&seeded, &fixture);
    assert_eq!(fixture.launches().len(), 1);
    assert_successor_observations(&seeded, &fixture, &surface, &mut seats, successor);

    reconcile(
        &mut lifecycle,
        &seeded.bus,
        &repository,
        &fixture,
        &mut seats,
        &lane_runtime,
        &surface,
    );
    let mut restarted = super::super::super::super::lifecycle::Runtime::new(&surface.config.lane);
    reconcile(
        &mut restarted,
        &seeded.bus,
        &repository,
        &fixture,
        &mut seats,
        &lane_runtime,
        &surface,
    );
    assert_eq!(fixture.launches().len(), 1);
    assert_eq!(claim_carries(&seeded.bus).len(), 1);
    assert_eq!(
        fact_bodies(&seeded.bus, wire::FactKind::LaneStarted).len(),
        1
    );
    assert_eq!(
        current_residency(&seeded.bus, fixture.epoch)
            .expect("stable successor residency reads")
            .body()
            .lanes[1]
            .occupant,
        Some(successor),
    );
    assert_successor_command(&seeded, &fixture, &surface, &seats, successor);
    assert_eq!(fixture.launches().len(), 1);
    assert_eq!(claim_carries(&seeded.bus).len(), 1);
    assert_eq!(
        fact_bodies(&seeded.bus, wire::FactKind::LaneStarted).len(),
        1,
    );

    stop_lane(&mut seats, fixture.lane);
    super::super::super::super::steering::remove_lane_surface(
        &fixture.paths,
        fixture.root.to_str().expect("fixture root is UTF-8"),
        fixture.lane,
    );
    drop(discovery);
    seeded.bus.verify().expect("claim carry authority verifies");
    seeded.bus.shutdown().expect("claim carry bus stops");
}

fn assert_successor_observations(
    seeded: &SeededOrg,
    fixture: &AdvisoryRtkFixture,
    surface: &super::super::super::super::StartupSurface,
    seats: &mut Seats,
    successor: CanonicalUuid,
) {
    let lane = fixture.lane;
    let pending = seeded
        .bus
        .read(move |read| crate::steering_read::pending(read, Some(lane)))
        .expect("successor steering reads");
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].body().control.as_deref(),
        Some(context_control(&handoff(fixture)).as_str()),
    );
    assert_ne!(
        pending[0].body().control.as_deref(),
        Some(handoff_control(&handoff(fixture)).as_str()),
        "the predecessor handoff control is retired",
    );

    let source = canvas::stream::Source::new(seeded.bus.reader(), fixture_uuid(107), 8);
    let publisher = source.message_publisher();
    let mut messages = source.subscribe_messages();
    let tastes = taste::Bag::new(Vec::new()).expect("empty taste bag");
    let surfaces = super::super::super::super::message_surface::CaptureSurfaces::new(
        &fixture.paths,
        &tastes,
        &surface.config.steering,
        &publisher,
    );
    let mut state = super::super::super::super::message_surface::MessageState::default();
    let mut diagnostic = Vec::new();
    for (occupant, text) in [
        (fixture.occupant, "stale predecessor message"),
        (successor, "successor message"),
    ] {
        super::super::super::super::message_surface::accept_capture_record(
            &seeded.bus,
            fixture.epoch,
            seats,
            &surfaces,
            None,
            &prose_record(fixture, occupant, text),
            super::super::super::super::message_surface::CaptureViews::new(
                &mut state,
                &mut diagnostic,
            ),
        )
        .expect("post-renewal capture record is classified");
    }
    let message = messages.try_recv().expect("successor message publishes");
    assert_eq!(message.author, successor);
    assert_eq!(message.text, "successor message");
    assert!(
        messages.try_recv().is_err(),
        "the predecessor published no message"
    );

    let residency =
        current_residency(&seeded.bus, fixture.epoch).expect("successor metrics authority reads");
    let answer = super::super::super::reply::answer(residency.body(), Some(fixture.lane), seats)
        .expect("successor metrics projection succeeds");
    let [row] = answer.lanes.as_slice() else {
        panic!("selected successor has one metrics row: {:?}", answer.lanes)
    };
    assert_eq!(row.occupant, Some(successor));
    assert_eq!(row.session.as_deref(), Some(SUCCESSOR_SESSION));
    assert_eq!(row.liveness, wire::intent::LaneLiveness::Running);
}

fn assert_successor_command(
    seeded: &SeededOrg,
    fixture: &AdvisoryRtkFixture,
    surface: &super::super::super::super::StartupSurface,
    seats: &Seats,
    successor: CanonicalUuid,
) {
    let again = request(fixture, true);
    assert!(matches!(
        set(
            &seeded.bus,
            &fixture.paths,
            fixture.epoch,
            seats,
            &surface.config.steering,
            &command(fixture, 55, &again),
            &again,
        ),
        Outcome::Reply(_)
    ));
    let current =
        current_residency(&seeded.bus, fixture.epoch).expect("successor renewal command reads");
    let wire::LaneRenewalState::Requested { predecessor, .. } = &current.body().renewals[0].state
    else {
        panic!("second renewal is requested")
    };
    assert_eq!(predecessor.occupant, successor);
    assert_eq!(predecessor.session, SUCCESSOR_SESSION);
    assert_eq!(predecessor.thread, SUCCESSOR_SESSION);
    let lane = fixture.lane;
    let controls = seeded
        .bus
        .read(move |read| crate::steering_read::pending(read, Some(lane)))
        .expect("successor controls read");
    assert!(controls.iter().any(|head| {
        head.body().control.as_deref()
            == Some(handoff_control(&current.body().renewals[0]).as_str())
    }));
    assert!(controls.iter().all(|head| {
        head.body().control.as_deref() != Some(handoff_control(&handoff(fixture)).as_str())
    }));
}

fn prose_record(
    fixture: &AdvisoryRtkFixture,
    occupant: CanonicalUuid,
    text: &str,
) -> lane::capture::CaptureRecord {
    let locus = listen::Locus::new(1, 0, text.len() as u64);
    lane::capture::CaptureRecord {
        lane: fixture.lane,
        occupant,
        stream: vendor::RecordStream::Stdout,
        boundary: sill::RecordBoundary::Complete,
        locus,
        declaration: None,
        decoded: listen::Decoded::Events {
            context: listen::RecordContext::default(),
            events: vec![listen::Event::Prose(listen::Prose {
                role: listen::ProseRole::Assistant,
                text: text.to_owned(),
                phase: None,
                media: Vec::new(),
                locus,
            })],
        },
    }
}

fn reconcile(
    lifecycle: &mut super::super::super::super::lifecycle::Runtime,
    bus: &Bus,
    repository: &Repository,
    fixture: &AdvisoryRtkFixture,
    seats: &mut Seats,
    lane_runtime: &Runtime<'_>,
    surface: &super::super::super::super::StartupSurface,
) {
    lifecycle
        .reconcile(
            bus,
            repository,
            &fixture.paths,
            fixture.epoch,
            seats,
            lane_runtime,
            &surface.config,
            &mut Vec::new(),
        )
        .expect("renewal reconciliation succeeds");
}

struct SeededOrg {
    bus: Bus,
    task: wire::TaskStamp,
    phase: wire::Phase,
    expiry: wire::Instant,
    maximum: wire::Instant,
    lanes: Vec<wire::LaneResidency>,
    subscription: wire::LineageSubscription,
}

fn seed_claimed_org(fixture: &AdvisoryRtkFixture) -> SeededOrg {
    let task: wire::TaskStamp = TASK.parse().expect("fixture task");
    let phase: wire::Phase = "todo".parse().expect("fixture phase");
    let now = wire::Instant::new(jiff::Timestamp::now()).expect("fixture clock is canonical");
    let start = shifted(now, -60);
    let expiry = shifted(now, 60 * 60);
    let maximum = shifted(now, 2 * 60 * 60);
    let lanes = org_rows(fixture);
    let subscription = wire::LineageSubscription {
        lineage_id: fixture.lane,
        policy: wire::intent::LineageLife::Drive,
    };
    let renewal = handoff(fixture);
    let bus = Bus::start(journal::Journal::in_memory().expect("fixture journal opens"))
        .expect("fixture bus starts");
    let mut proposal =
        write(Actor::Spice, fixture.epoch, Producer::CommandDoor).expect("fixture write starts");
    let seated = proposal
        .fact(
            wire::body::UpSeated {
                epoch: fixture.epoch,
            },
            FactCause::Root,
        )
        .expect("fixture resident seats");
    let next_lanes = lanes.clone();
    let next_subscription = subscription.clone();
    let epoch = fixture.epoch;
    proposal.residency_head(seated, move |_| wire::ResidencyHead {
        epoch,
        state: wire::ResidentState::Seated,
        topology_revision: 1,
        lanes: next_lanes,
        subscriptions: vec![next_subscription],
        renewals: vec![renewal],
    });
    commit(&bus, proposal).expect("claimed organization authority commits");
    let mut claimed = lanes[1].clone();
    claimed.occupant = Some(fixture.occupant);
    super::super::super::propose::tests::seed_task_claim(
        &bus,
        &task,
        &claimed,
        fixture.epoch,
        start,
        expiry,
        maximum,
    );
    SeededOrg {
        bus,
        task,
        phase,
        expiry,
        maximum,
        lanes,
        subscription,
    }
}

fn org_rows(fixture: &AdvisoryRtkFixture) -> Vec<wire::LaneResidency> {
    let west_lane = fixture_uuid(101);
    let west_occupant = fixture_uuid(102);
    let east_lane = fixture_uuid(103);
    let east_occupant = fixture_uuid(104);
    vec![
        sibling_row(fixture, west_lane, west_occupant, fixture_uuid(105), "west"),
        wire::LaneResidency {
            lane: fixture.lane,
            lineage_id: fixture.lane,
            name: Some("renewed".to_owned()),
            worktree: fixture.root.display().to_string(),
            life: "drive".to_owned(),
            vendor: fixture.vendor.clone(),
            occupant: None,
            lineage: vec![fixture.occupant],
            capabilities: vec![fixture.capability],
            session: Some(predecessor_session(fixture)),
            disposition: VACANT.to_owned(),
        },
        sibling_row(fixture, east_lane, east_occupant, fixture_uuid(106), "east"),
    ]
}

fn sibling_row(
    fixture: &AdvisoryRtkFixture,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    capability: CanonicalUuid,
    name: &str,
) -> wire::LaneResidency {
    wire::LaneResidency {
        lane,
        lineage_id: fixture.lane,
        name: Some(name.to_owned()),
        worktree: format!("{}-{name}", fixture.root.display()),
        life: "drive".to_owned(),
        vendor: fixture.vendor.clone(),
        occupant: Some(occupant),
        lineage: vec![occupant],
        capabilities: vec![capability],
        session: None,
        disposition: OCCUPIED.to_owned(),
    }
}

fn predecessor_session(fixture: &AdvisoryRtkFixture) -> wire::LaneSession {
    wire::LaneSession {
        lane: fixture.lane,
        occupant: fixture.occupant,
        worktree: fixture.root.display().to_string(),
        vendor: fixture.vendor.clone(),
        launch: None,
        epoch: fixture.epoch,
        stream: "stdout".to_owned(),
        identity: SESSION.to_owned(),
        locus: wire::TapeLocus {
            member: 0,
            offset: 0,
            length: 1,
        },
    }
}

fn handoff(fixture: &AdvisoryRtkFixture) -> wire::LaneRenewalRecord {
    wire::LaneRenewalRecord {
        lineage_id: fixture.lane,
        lane: fixture.lane,
        revision: 1,
        state: wire::LaneRenewalState::HandoffPending {
            predecessor: wire::RenewalIdentity {
                occupant: fixture.occupant,
                session: SESSION.to_owned(),
                thread: SESSION.to_owned(),
            },
            org_slot: 1,
        },
    }
}

fn assert_carried(seeded: &SeededOrg, fixture: &AdvisoryRtkFixture) -> CanonicalUuid {
    let snapshot = seeded.bus.task_snapshot().expect("carried task heads read");
    let heads = snapshot.tasks().first().expect("fixture task remains");
    let claim = heads.claim().expect("claim remains active");
    let lease = heads.lease().expect("lease remains present");
    assert_eq!(heads.statement().body().born, seeded.task);
    assert_eq!(heads.workflow().body().phase, seeded.phase);
    assert_eq!(claim.body().cell.phase, seeded.phase);
    assert_eq!(claim.body().actor, Actor::Lane(fixture.lane));
    assert_eq!(claim.body().lane, fixture.lane);
    assert_eq!(claim.body().epoch, fixture.epoch);
    assert_eq!(claim.body().disposition, wire::ClaimDisposition::Active);
    assert_eq!(lease.body().expiry, seeded.expiry);
    assert_eq!(lease.body().maximum, seeded.maximum);
    assert_eq!(lease.body().epoch, fixture.epoch);
    let successor = claim.body().occupant;
    assert_ne!(successor, fixture.occupant);

    let residency = snapshot.residency().expect("successor residency reads");
    assert_eq!(residency.body().lanes.len(), seeded.lanes.len());
    assert_eq!(
        residency
            .body()
            .lanes
            .iter()
            .map(|row| row.lane)
            .collect::<Vec<_>>(),
        seeded.lanes.iter().map(|row| row.lane).collect::<Vec<_>>(),
    );
    assert_eq!(residency.body().lanes[0], seeded.lanes[0]);
    assert_eq!(residency.body().lanes[2], seeded.lanes[2]);
    assert_eq!(residency.body().lanes[1].occupant, Some(successor));
    assert_eq!(
        residency.body().lanes[1].lineage,
        [fixture.occupant, successor]
    );
    assert_eq!(
        residency.body().subscriptions.as_slice(),
        std::slice::from_ref(&seeded.subscription),
    );
    assert!(matches!(
        &residency.body().renewals[0].state,
        wire::LaneRenewalState::SuccessorStarted {
            predecessor,
            successor: next,
            org_slot: 1,
        } if predecessor.occupant == fixture.occupant && next.occupant == successor
    ));

    let carries = claim_carries(&seeded.bus);
    assert_eq!(carries.len(), 1);
    assert_eq!(
        carries[0].get("task").and_then(serde_json::Value::as_str),
        Some(TASK)
    );
    assert_eq!(
        carries[0]
            .get("ancestor")
            .and_then(serde_json::Value::as_str),
        Some(fixture.occupant.to_string().as_str()),
    );
    assert_eq!(
        carries[0]
            .get("occupant")
            .and_then(serde_json::Value::as_str),
        Some(successor.to_string().as_str()),
    );
    let starts = fact_bodies(&seeded.bus, wire::FactKind::LaneStarted);
    assert_eq!(starts.len(), 1);
    assert_eq!(
        starts[0]
            .get("occupant")
            .and_then(serde_json::Value::as_str),
        Some(successor.to_string().as_str()),
    );
    successor
}

fn claim_carries(bus: &Bus) -> Vec<serde_json::Map<String, serde_json::Value>> {
    fact_bodies(bus, wire::FactKind::ClaimCarried)
}

fn fact_bodies(bus: &Bus, kind: wire::FactKind) -> Vec<serde_json::Map<String, serde_json::Value>> {
    bus.fact_snapshot()
        .expect("claim carry facts read")
        .facts()
        .iter()
        .filter(|fact| fact.fact.kind() == kind)
        .map(|fact| fact.fact.body().clone())
        .collect()
}

fn shifted(at: wire::Instant, seconds: i64) -> wire::Instant {
    wire::Instant::new(
        at.get()
            .checked_add(jiff::SignedDuration::from_secs(seconds))
            .expect("fixture instant remains in range"),
    )
    .expect("shifted fixture instant is canonical")
}
