use super::*;

const TASK: &str = "1kQpV9Gt";

#[test]
fn renewal_carries_one_claim_through_the_same_team_slot() {
    let fixture = AdvisoryRtkFixture::new();
    let seeded = seed_claimed_team(&fixture);
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

struct SeededTeam {
    bus: Bus,
    task: wire::TaskStamp,
    phase: wire::Phase,
    expiry: wire::Instant,
    maximum: wire::Instant,
    lanes: Vec<wire::LaneResidency>,
    subscription: wire::LineageSubscription,
}

fn seed_claimed_team(fixture: &AdvisoryRtkFixture) -> SeededTeam {
    let task: wire::TaskStamp = TASK.parse().expect("fixture task");
    let phase: wire::Phase = "todo".parse().expect("fixture phase");
    let now = wire::Instant::new(jiff::Timestamp::now()).expect("fixture clock is canonical");
    let start = shifted(now, -60);
    let expiry = shifted(now, 60 * 60);
    let maximum = shifted(now, 2 * 60 * 60);
    let lanes = team_rows(fixture);
    let subscription = wire::LineageSubscription {
        lineage_id: fixture.lane,
        policy: wire::intent::LineageLife::Drive,
        manual_projects: Vec::new(),
        auto_projects: Vec::new(),
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
    commit(&bus, proposal).expect("claimed team authority commits");
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
    SeededTeam {
        bus,
        task,
        phase,
        expiry,
        maximum,
        lanes,
        subscription,
    }
}

fn team_rows(fixture: &AdvisoryRtkFixture) -> Vec<wire::LaneResidency> {
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
            team_slot: 1,
        },
    }
}

fn assert_carried(seeded: &SeededTeam, fixture: &AdvisoryRtkFixture) -> CanonicalUuid {
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
            team_slot: 1,
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
