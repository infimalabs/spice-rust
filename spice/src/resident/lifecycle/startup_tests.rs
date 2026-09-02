use super::super::lane_surface::AdvisoryRtkFixture;
use super::*;

#[test]
fn resident_startup_kicks_off_a_vacant_drain_team_without_waiting_for_work() {
    let fixture = AdvisoryRtkFixture::new();
    let bus = Bus::start(journal::Journal::in_memory().expect("fixture journal opens"))
        .expect("fixture bus starts");
    seed_vacant_team(&bus, &fixture, wire::intent::LineageLife::Drain);
    let repository = Repository::discover(&fixture.root).expect("fixture repository resolves");
    let surface =
        super::super::load_startup_surface(&repository).expect("fixture startup policy resolves");
    let capabilities = canvas::axum::Capabilities::default();
    let discovery = canvas::Discovery {
        endpoint: "http://127.0.0.1:1".to_owned(),
        capability: fixture.capability,
        epoch: fixture.epoch,
    };
    let lane_runtime = lane_surface::Runtime::new(&capabilities, &discovery, fixture.reporter());
    let mut runtime = Runtime::new(&surface.config.lane);
    let mut seats = Seats::new();
    let mut diagnostic = Vec::new();

    let deadline = runtime
        .reconcile(
            &bus,
            &repository,
            &fixture.paths,
            fixture.epoch,
            &mut seats,
            &lane_runtime,
            &surface.config,
            &mut diagnostic,
        )
        .expect("Drain startup reconciles");

    assert_eq!(deadline, None);
    assert!(seats.occupied(fixture.lane));
    assert!(
        String::from_utf8(diagnostic)
            .expect("diagnostic is UTF-8")
            .contains("lifecycle started trigger=drain-queue")
    );
    stop(fixture.lane, &mut seats, "started Drain member");
    bus.verify().expect("Drain startup authority verifies");
    bus.shutdown().expect("fixture bus stops");
}

#[test]
fn changing_team_policy_to_drain_reenables_and_starts_its_vacant_member() {
    let fixture = AdvisoryRtkFixture::new();
    let bus = Bus::start(journal::Journal::in_memory().expect("fixture journal opens"))
        .expect("fixture bus starts");
    seed_vacant_team(&bus, &fixture, wire::intent::LineageLife::Drive);
    let repository = Repository::discover(&fixture.root).expect("fixture repository resolves");
    let surface =
        super::super::load_startup_surface(&repository).expect("fixture startup policy resolves");
    let capabilities = canvas::axum::Capabilities::default();
    let discovery = canvas::Discovery {
        endpoint: "http://127.0.0.1:1".to_owned(),
        capability: fixture.capability,
        epoch: fixture.epoch,
    };
    let lane_runtime = lane_surface::Runtime::new(&capabilities, &discovery, fixture.reporter());
    let mut runtime = Runtime::new(&surface.config.lane);
    let mut seats = Seats::new();
    assert_eq!(
        runtime
            .reconcile(
                &bus,
                &repository,
                &fixture.paths,
                fixture.epoch,
                &mut seats,
                &lane_runtime,
                &surface.config,
                &mut Vec::new(),
            )
            .expect("Drive startup reconciles without eligible work"),
        None,
    );
    assert!(!seats.occupied(fixture.lane));

    let request = wire::intent::LineageConfigure {
        topology_revision: 1,
        lineage_id: fixture.lane,
        member: None,
        life: Some(wire::intent::LineageLife::Drain),
        manual_projects: None,
    };
    let command = wire::Command {
        v: wire::WireVersion::CURRENT,
        id: fixture.occupant,
        epoch: fixture.epoch,
        kind: wire::CommandKind::LineageConfigure,
        expected: Vec::new(),
        body: serde_json::to_value(&request).expect("configuration encodes"),
    }
    .admit(&wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor: Actor::Operator,
        epoch: fixture.epoch,
    })
    .expect("configuration admits");
    runtime
        .backoff
        .parked
        .insert(fixture.lane, Trigger::DrainQueue);
    assert!(runtime.observe_topology(&command));
    assert!(
        runtime.backoff.parked.is_empty(),
        "team policy change re-enables members parked under prior authority"
    );
    assert!(matches!(
        lane_surface::configure(
            &bus,
            fixture.epoch,
            &seats,
            &surface.config.projects,
            &command,
            &request,
        ),
        Outcome::Reply(_)
    ));

    runtime
        .reconcile(
            &bus,
            &repository,
            &fixture.paths,
            fixture.epoch,
            &mut seats,
            &lane_runtime,
            &surface.config,
            &mut Vec::new(),
        )
        .expect("updated Drain team reconciles");
    assert!(seats.occupied(fixture.lane));
    stop(fixture.lane, &mut seats, "updated Drain member");
    bus.verify().expect("policy update authority verifies");
    bus.shutdown().expect("fixture bus stops");
}

fn seed_vacant_team(bus: &Bus, fixture: &AdvisoryRtkFixture, policy: wire::intent::LineageLife) {
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
    let root = fixture.root.display().to_string();
    let vendor = fixture.vendor.clone();
    let epoch = fixture.epoch;
    let lane = fixture.lane;
    let capability = fixture.capability;
    proposal.residency_head(seated, move |_| wire::ResidencyHead {
        epoch,
        state: wire::ResidentState::Seated,
        topology_revision: 1,
        lanes: vec![wire::LaneResidency {
            lane,
            lineage_id: lane,
            name: None,
            worktree: root,
            life: "drive".to_owned(),
            vendor,
            occupant: None,
            lineage: Vec::new(),
            capabilities: vec![capability],
            session: None,
            disposition: "vacant".to_owned(),
        }],
        subscriptions: vec![wire::LineageSubscription {
            lineage_id: lane,
            policy,
            manual_projects: Vec::new(),
            auto_projects: Vec::new(),
        }],
    });
    commit(bus, proposal).expect("fixture residency commits");
}

fn stop(lane: CanonicalUuid, seats: &mut Seats, label: &str) {
    seats
        .vacate(lane)
        .unwrap_or_else(|| panic!("{label} remains supervised"))
        .wait()
        .unwrap_or_else(|error| panic!("{label} exits: {error}"));
}
