use super::super::lane_surface::AdvisoryRtkFixture;
use super::*;

#[test]
fn resident_startup_leaves_a_vacant_drain_org_idle_without_work() {
    let fixture = AdvisoryRtkFixture::new();
    let bus = Bus::start(journal::Journal::in_memory().expect("fixture journal opens"))
        .expect("fixture bus starts");
    seed_vacant_org(&bus, &fixture, wire::intent::TeamLife::Drain);
    let repository = Repository::discover(&fixture.root).expect("fixture repository resolves");
    let surface =
        super::super::load_startup_surface(&repository).expect("fixture startup policy resolves");
    let capabilities = canvas::axum::Capabilities::default();
    let discovery = canvas::Discovery {
        endpoint: "http://127.0.0.1:1".to_owned(),
        capability: fixture.capability,
        epoch: fixture.epoch,
    };
    let lane_runtime = fixture.lane_runtime(&bus, &capabilities, &discovery);
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
    assert!(!seats.occupied(fixture.lane));
    assert!(diagnostic.is_empty());
    assert!(runtime.backoff.timed.is_empty());
    assert!(runtime.backoff.parked.is_empty());
    assert!(
        bus.task_snapshot()
            .expect("idle Drain snapshot reads")
            .tasks()
            .is_empty(),
        "idle reconciliation does not manufacture an automatic claim"
    );
    bus.verify().expect("Drain startup authority verifies");
    bus.shutdown().expect("fixture bus stops");
}

#[test]
fn changing_org_policy_to_drain_waits_until_work_arrives() {
    let fixture = AdvisoryRtkFixture::new();
    initialize_git(&fixture.root);
    let sibling: CanonicalUuid = "018f82d4-6f59-7d06-92e3-000000000005"
        .parse()
        .expect("fixture sibling lane");
    let bus = Bus::start(journal::Journal::in_memory().expect("fixture journal opens"))
        .expect("fixture bus starts");
    seed_vacant_org_members(
        &bus,
        &fixture,
        wire::intent::TeamLife::Drive,
        &[fixture.lane, sibling],
    );
    let repository = Repository::discover(&fixture.root).expect("fixture repository resolves");
    let surface =
        super::super::load_startup_surface(&repository).expect("fixture startup policy resolves");
    let capabilities = canvas::axum::Capabilities::default();
    let discovery = canvas::Discovery {
        endpoint: "http://127.0.0.1:1".to_owned(),
        capability: fixture.capability,
        epoch: fixture.epoch,
    };
    let lane_runtime = fixture.lane_runtime(&bus, &capabilities, &discovery);
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
    assert!(!seats.occupied(sibling));

    let (request, command) = drain_configuration(&fixture);
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
            &fixture.paths,
            fixture.epoch,
            &seats,
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
    assert!(
        !seats.occupied(fixture.lane),
        "changing policy without work remains idle"
    );

    assert_fresh_drain_settle_then_launch(
        &mut runtime,
        &bus,
        &repository,
        &fixture,
        &mut seats,
        &lane_runtime,
        &surface.config,
        sibling,
    );
    bus.verify().expect("policy update authority verifies");
    bus.shutdown().expect("fixture bus stops");
}

#[allow(
    clippy::too_many_arguments,
    reason = "the deterministic startup phase retains one complete resident effect boundary"
)]
fn assert_fresh_drain_settle_then_launch(
    runtime: &mut Runtime,
    bus: &Bus,
    repository: &Repository,
    fixture: &AdvisoryRtkFixture,
    seats: &mut Seats,
    lane_runtime: &lane_surface::Runtime<'_>,
    config: &SurfaceConfig,
    sibling: CanonicalUuid,
) {
    let (task, ready_at) = seed_selectable_task(bus);
    let before = bus
        .task_snapshot()
        .expect("pre-settle snapshot reads")
        .cursor();
    let mut diagnostic = Vec::new();
    let deadline = runtime
        .reconcile_at(
            ready_at,
            bus,
            repository,
            &fixture.paths,
            fixture.epoch,
            seats,
            lane_runtime,
            config,
            &mut diagnostic,
        )
        .expect("newly available Drain work reconciles");
    assert_eq!(
        deadline,
        Some(WakeDeadline {
            lane: fixture.lane,
            at: after(ready_at, SETTLE_SECONDS).expect("first Drain deadline computes"),
            kind: WakeKind::Task,
        })
    );
    assert!(!seats.occupied(fixture.lane));
    assert!(!seats.occupied(sibling));
    assert_eq!(active_claim_count(bus), 0);
    assert!(diagnostic.is_empty());
    assert_eq!(
        bus.task_snapshot()
            .expect("post-settle snapshot reads")
            .cursor(),
        before,
        "pre-deadline reconciliation writes no task or lifecycle fact"
    );
    assert!(
        bus.task_snapshot()
            .expect("unclaimed task snapshot reads")
            .tasks()
            .iter()
            .find(|heads| heads.statement().body().born == task)
            .is_some_and(|heads| heads.claim().is_none()),
        "the selected task remains organically claimable during the settle interval"
    );

    let mut launch_diagnostic = Vec::new();
    runtime
        .reconcile_at(
            after(ready_at, SETTLE_SECONDS).expect("settled first Drain instant computes"),
            bus,
            repository,
            &fixture.paths,
            fixture.epoch,
            seats,
            lane_runtime,
            config,
            &mut launch_diagnostic,
        )
        .expect("settled first Drain work reconciles");
    assert_drain_launch(bus, &task, fixture.lane, sibling, seats, &launch_diagnostic);
    stop(fixture.lane, seats, "updated Drain member");
}

fn drain_configuration(
    fixture: &AdvisoryRtkFixture,
) -> (wire::intent::TeamConfigure, wire::AdmittedCommand) {
    let request = wire::intent::TeamConfigure {
        topology_revision: 1,
        team: fixture.lane,
        member: None,
        life: Some(wire::intent::TeamLife::Drain),
    };
    let command = wire::Command {
        v: wire::WireVersion::CURRENT,
        id: fixture.occupant,
        epoch: fixture.epoch,
        kind: wire::CommandKind::TeamConfigure,
        expected: Vec::new(),
        body: serde_json::to_value(&request).expect("configuration encodes"),
    }
    .admit(&wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor: Actor::Operator,
        epoch: fixture.epoch,
    })
    .expect("configuration admits");
    (request, command)
}

fn active_claim_count(bus: &Bus) -> usize {
    bus.task_snapshot()
        .expect("contention snapshot reads")
        .tasks()
        .iter()
        .filter(|heads| {
            heads
                .claim()
                .is_some_and(|claim| claim.body().disposition == ClaimDisposition::Active)
        })
        .count()
}

fn assert_drain_launch(
    bus: &Bus,
    task: &wire::TaskStamp,
    lane: CanonicalUuid,
    sibling: CanonicalUuid,
    seats: &Seats,
    diagnostic: &[u8],
) {
    assert!(
        seats.occupied(lane),
        "Drain seat did not start after task arrival: {}",
        String::from_utf8_lossy(diagnostic)
    );
    assert!(!seats.occupied(sibling));
    assert_automatic_claim(bus, task, lane, seats);
    assert!(
        String::from_utf8_lossy(diagnostic).contains("lifecycle started trigger=drain-queue"),
        "Drain launch diagnostic did not follow the automatic claim: {}",
        String::from_utf8_lossy(diagnostic)
    );
}

fn seed_selectable_task(bus: &Bus) -> (wire::TaskStamp, Instant) {
    let task = wire::TaskStamp::for_micros(1_900_000_000_000).expect("fixture task stamp");
    let handle = wire::TaskHandle::for_project("task.lifecycle", &task);
    let origin = format!("task:{handle}")
        .parse::<wire::Origin>()
        .expect("fixture task origin");
    let at = Instant::new(jiff::Timestamp::now()).expect("fixture current instant");
    let phase: wire::Phase = "todo".parse().expect("fixture phase");
    let flow = ["plan", "todo", "review"]
        .map(|value| value.parse().expect("fixture flow phase"))
        .to_vec();
    let mut proposal =
        journal::JournalWrite::new(Actor::Operator, origin.clone(), at, Producer::CommandDoor);
    let added = proposal
        .fact(
            wire::body::TaskAdded {
                task: task.clone(),
                title: "Exercise drain lifecycle demand".to_owned(),
                description: None,
                acceptance: Vec::new(),
                requirements: wire::EnvironmentRequirements::default(),
                project: "task.lifecycle".to_owned(),
                tags: Vec::new(),
                depends: Vec::new(),
                flow: flow.clone(),
                phase: phase.clone(),
                creation: wire::body::TaskCreationContext::unavailable(
                    wire::body::TaskCreationUnavailable::OperatorActor,
                ),
                revision: 1,
            },
            FactCause::Root,
        )
        .expect("fixture task fact builds");
    let statement_task = task.clone();
    proposal.task_head(added, move |position| wire::TaskHead {
        born: statement_task,
        queue_origin: position,
        title: "Exercise drain lifecycle demand".to_owned(),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        project: "task.lifecycle".to_owned(),
        tags: Vec::new(),
        dependencies: Vec::new(),
        provenance: origin,
        creation: wire::body::TaskCreationContext::unavailable(
            wire::body::TaskCreationUnavailable::OperatorActor,
        ),
        revision: position,
    });
    let workflow_task = task.clone();
    proposal.workflow_head(workflow_task, added, move |_| wire::WorkflowHead {
        flow,
        phase,
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
    });
    commit(bus, proposal).expect("fixture task commits");
    (task, at)
}

fn assert_automatic_claim(bus: &Bus, task: &wire::TaskStamp, lane: CanonicalUuid, seats: &Seats) {
    let snapshot = bus.task_snapshot().expect("claimed snapshot reads");
    let heads = snapshot
        .tasks()
        .iter()
        .find(|heads| heads.statement().body().born == *task)
        .expect("claimed task remains present");
    let claim = heads.claim().expect("Drain wake committed a claim").body();
    assert_eq!(claim.actor, Actor::Lane(lane));
    assert_eq!(claim.lane, lane);
    assert_eq!(claim.disposition, ClaimDisposition::Active);
    assert_eq!(
        seats
            .started(lane)
            .expect("claimed lane is supervised")
            .occupant(),
        claim.occupant,
    );
    assert!(crate::board_command::is_automatic_claim(
        claim,
        snapshot.facts()
    ));
}

fn initialize_git(root: &std::path::Path) {
    std::fs::remove_dir_all(root.join(".git")).expect("placeholder Git directory removes");
    crate::test_repository::initialize(root);
    let repository = Repository::discover(root).expect("initialized fixture repository resolves");
    let skill = spice::install::installed_skill(&repository);
    std::fs::create_dir_all(skill.parent().expect("installed skill parent"))
        .expect("installed skill parent creates");
    std::fs::write(&skill, ::std::include_bytes!("../../../agent/SKILL.md"))
        .expect("current installed skill writes");
    crate::test_repository::stage_all(root);
    crate::test_repository::commit(root, "fixture opening");
}

fn seed_vacant_org(bus: &Bus, fixture: &AdvisoryRtkFixture, policy: wire::intent::TeamLife) {
    seed_vacant_org_members(bus, fixture, policy, &[fixture.lane]);
}

fn seed_vacant_org_members(
    bus: &Bus,
    fixture: &AdvisoryRtkFixture,
    policy: wire::intent::TeamLife,
    lanes: &[CanonicalUuid],
) {
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
    let capability = fixture.capability;
    let team = fixture.lane;
    let lanes = lanes
        .iter()
        .map(|lane| wire::LaneResidency {
            lane: *lane,
            team,
            name: None,
            worktree: root.clone(),
            life: "drive".to_owned(),
            vendor: vendor.clone(),
            model: None,
            effort: None,
            occupant: None,
            lineage: Vec::new(),
            capabilities: vec![capability],
            session: None,
            disposition: "vacant".to_owned(),
        })
        .collect::<Vec<_>>();
    proposal.residency_head(seated, move |_| wire::ResidencyHead {
        epoch,
        state: wire::ResidentState::Seated,
        topology_revision: 1,
        lanes,
        subscriptions: vec![wire::TeamSubscription { team, policy }],
        renewals: Vec::new(),
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
