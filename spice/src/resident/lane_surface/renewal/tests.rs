use std::os::unix::process::ExitStatusExt as _;
use std::process::ExitStatus;
use std::time::Duration;

use lane::occupant::{DetachedProcess, DetachedWait};
use sill::{WaitError, WaitOutcome};

use super::super::launch::test_support::{AdvisoryRtkFixture, SESSION};
use super::*;

mod claim_carry;

struct RunningProcess;
struct ExitedProcess;

impl DetachedProcess for RunningProcess {
    fn group(&self) -> u32 {
        77
    }

    fn runtime_remaining(&self) -> Duration {
        Duration::from_secs(60)
    }

    fn termination_grace(&self) -> Duration {
        Duration::ZERO
    }

    fn reap_bound(&self) -> Duration {
        Duration::ZERO
    }

    fn wait_for(self: Box<Self>, _duration: Duration) -> Result<DetachedWait, WaitError> {
        Ok(DetachedWait::Running(self))
    }

    fn terminate(&self) -> Result<(), WaitError> {
        Ok(())
    }

    fn kill(&self) -> Result<(), WaitError> {
        Ok(())
    }

    fn wait(self: Box<Self>) -> Result<WaitOutcome, WaitError> {
        panic!("the command-only fixture never waits for its process")
    }
}

impl DetachedProcess for ExitedProcess {
    fn group(&self) -> u32 {
        78
    }

    fn runtime_remaining(&self) -> Duration {
        Duration::ZERO
    }

    fn termination_grace(&self) -> Duration {
        Duration::ZERO
    }

    fn reap_bound(&self) -> Duration {
        Duration::ZERO
    }

    fn wait_for(self: Box<Self>, _duration: Duration) -> Result<DetachedWait, WaitError> {
        Ok(DetachedWait::Exited(success()))
    }

    fn terminate(&self) -> Result<(), WaitError> {
        Ok(())
    }

    fn kill(&self) -> Result<(), WaitError> {
        Ok(())
    }

    fn wait(self: Box<Self>) -> Result<WaitOutcome, WaitError> {
        Ok(success())
    }
}

#[test]
fn request_duplicate_and_cancel_keep_one_pre_handoff_transition() {
    let fixture = AdvisoryRtkFixture::new();
    let bus = seeded(&fixture, None);
    let discovery = publish_surface(&fixture);
    let mut seats = Seats::new();
    seats
        .seat_detached(fixture.lane, fixture.occupant, Box::new(RunningProcess))
        .expect("fixture predecessor seats");
    let config = steer::Config::default();
    let renewal_request = request(&fixture, true);

    assert_invalid_requests(&fixture, &bus, &seats, &config, renewal_request);
    assert_first_request(&fixture, &bus, &seats, &config, renewal_request);
    assert_duplicate_and_retired(&fixture, &bus, &seats, &config, renewal_request);
    assert_cancellation(&fixture, &bus, &seats, &config);

    seats.release_all();
    super::super::super::steering::remove_lane_surface(
        &fixture.paths,
        fixture.root.to_str().expect("fixture root is UTF-8"),
        fixture.lane,
    );
    drop(discovery);
    bus.verify().expect("renewal command authority verifies");
    bus.shutdown().expect("renewal command bus stops");
}

#[test]
fn handoff_pending_launches_one_fresh_successor_with_context() {
    let fixture = AdvisoryRtkFixture::new();
    let bus = seeded(&fixture, Some(requested(&fixture, true)));
    let discovery = publish_surface(&fixture);
    let repository = Repository::discover(&fixture.root).expect("fixture repository resolves");
    let surface = super::super::super::load_startup_surface(&repository)
        .expect("fixture startup policy resolves");
    let capabilities = canvas::axum::Capabilities::default();
    let lane_runtime = Runtime::new(&capabilities, &discovery, fixture.reporter());
    let mut lifecycle = super::super::super::lifecycle::Runtime::new(&surface.config.lane);
    let mut seats = Seats::new();

    lifecycle
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
        .expect("handoff-pending renewal reconciles");
    assert!(seats.occupied(fixture.lane));
    let head = current_residency(&bus, fixture.epoch).expect("successor head reads");
    let successor = head.body().lanes[0]
        .occupant
        .expect("successor occupies the retained lane");
    assert_ne!(successor, fixture.occupant);
    assert!(matches!(
        &head.body().renewals[0].state,
        wire::LaneRenewalState::SuccessorStarted {
            predecessor,
            successor: next,
            org_slot: 0,
        } if predecessor.occupant == fixture.occupant && next.occupant == successor
    ));
    assert_eq!(controls(&bus, fixture.lane), [CONTEXT_CONTROL]);
    let launches = fixture.launches();
    assert_eq!(launches.len(), 1);
    assert!(
        !launches[0]
            .split_whitespace()
            .any(|word| word == "--resume"),
        "renewal successor must be a fresh vendor thread: {launches:?}"
    );

    lifecycle
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
        .expect("repeated reconciliation is idle");
    assert_eq!(fixture.launches().len(), 1);

    assert_second_renewal(&fixture, &bus, &seats, &surface.config.steering, successor);

    stop_lane(&mut seats, fixture.lane);
    super::super::super::steering::remove_lane_surface(
        &fixture.paths,
        fixture.root.to_str().expect("fixture root is UTF-8"),
        fixture.lane,
    );
    bus.verify().expect("successor authority verifies");
    bus.shutdown().expect("successor bus stops");
}

#[test]
fn predecessor_exit_advances_handoff_and_retires_its_control_before_successor_launch() {
    let fixture = AdvisoryRtkFixture::new();
    let bus = seeded(&fixture, None);
    let discovery = publish_surface(&fixture);
    let repository = Repository::discover(&fixture.root).expect("fixture repository resolves");
    let surface = super::super::super::load_startup_surface(&repository)
        .expect("fixture startup policy resolves");
    let capabilities = canvas::axum::Capabilities::default();
    let lane_runtime = Runtime::new(&capabilities, &discovery, fixture.reporter());
    let mut lifecycle = super::super::super::lifecycle::Runtime::new(&surface.config.lane);
    let mut seats = Seats::new();
    seats
        .seat_detached(fixture.lane, fixture.occupant, Box::new(ExitedProcess))
        .expect("fixture predecessor seats");
    let renewal_request = request(&fixture, true);
    assert!(matches!(
        set(
            &bus,
            &fixture.paths,
            fixture.epoch,
            &seats,
            &surface.config.steering,
            &command(&fixture, 20, &renewal_request),
            &renewal_request,
        ),
        Outcome::Reply(_)
    ));
    assert_eq!(controls(&bus, fixture.lane), [HANDOFF_CONTROL]);

    occupant_exited(
        &bus,
        fixture.epoch,
        &mut seats,
        lane::capture::CaptureExit {
            lane: fixture.lane,
            occupant: fixture.occupant,
        },
    )
    .expect("predecessor exit commits handoff");
    let pending = current_residency(&bus, fixture.epoch).expect("pending handoff reads");
    assert!(matches!(
        pending.body().renewals[0].state,
        wire::LaneRenewalState::HandoffPending { .. }
    ));
    assert_eq!(pending.body().lanes[0].occupant, None);

    lifecycle
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
        .expect("successor reconciliation completes");
    assert_eq!(controls(&bus, fixture.lane), [CONTEXT_CONTROL]);
    assert_eq!(fixture.launches().len(), 1);

    stop_lane(&mut seats, fixture.lane);
    super::super::super::steering::remove_lane_surface(
        &fixture.paths,
        fixture.root.to_str().expect("fixture root is UTF-8"),
        fixture.lane,
    );
    bus.verify().expect("complete handoff authority verifies");
    bus.shutdown().expect("complete handoff bus stops");
}

#[test]
fn launch_refusal_is_durable_and_an_explicit_retry_starts_one_successor() {
    let fixture = AdvisoryRtkFixture::new();
    let bus = seeded(&fixture, Some(requested(&fixture, true)));
    let discovery = publish_surface(&fixture);
    let repository = Repository::discover(&fixture.root).expect("fixture repository resolves");
    let surface = super::super::super::load_startup_surface(&repository)
        .expect("fixture startup policy resolves");
    let original = refuse_launch_config(&fixture);
    let capabilities = canvas::axum::Capabilities::default();
    let lane_runtime = Runtime::new(&capabilities, &discovery, fixture.reporter());
    let mut lifecycle = super::super::super::lifecycle::Runtime::new(&surface.config.lane);
    let mut seats = Seats::new();

    lifecycle
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
        .expect("launch refusal records rather than escaping reconciliation");
    let head = current_residency(&bus, fixture.epoch).expect("refused renewal reads");
    assert!(matches!(
        &head.body().renewals[0].state,
        wire::LaneRenewalState::LaunchRefused { reason, .. }
            if reason.contains("supervisor-launch-failed")
    ));
    assert!(!seats.occupied(fixture.lane));
    assert_eq!(controls(&bus, fixture.lane), [CONTEXT_CONTROL]);

    std::fs::write(fixture.root.join("spice-rust.toml"), original)
        .expect("valid launch configuration restores");
    let retry = request(&fixture, true);
    assert!(matches!(
        set(
            &bus,
            &fixture.paths,
            fixture.epoch,
            &seats,
            &surface.config.steering,
            &command(&fixture, 30, &retry),
            &retry,
        ),
        Outcome::Reply(_)
    ));
    lifecycle
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
        .expect("repaired successor launches");
    assert!(matches!(
        current_residency(&bus, fixture.epoch)
            .expect("repaired renewal reads")
            .body()
            .renewals[0]
            .state,
        wire::LaneRenewalState::SuccessorStarted { .. }
    ));
    assert_eq!(fixture.launches().len(), 1);

    stop_lane(&mut seats, fixture.lane);
    super::super::super::steering::remove_lane_surface(
        &fixture.paths,
        fixture.root.to_str().expect("fixture root is UTF-8"),
        fixture.lane,
    );
    bus.verify().expect("launch refusal authority verifies");
    bus.shutdown().expect("launch refusal bus stops");
}

#[test]
fn resident_restart_preserves_the_original_predecessor_across_a_resumed_process() {
    let fixture = AdvisoryRtkFixture::new();
    let bus = seeded(&fixture, Some(requested(&fixture, false)));
    let restored = fixture_uuid(40);
    let session = wire::LaneSession {
        lane: fixture.lane,
        occupant: restored,
        worktree: fixture.root.display().to_string(),
        vendor: fixture.vendor.clone(),
        launch: None,
        epoch: fixture.epoch,
        stream: "stdout".to_owned(),
        identity: SESSION.to_owned(),
        locus: wire::TapeLocus {
            member: 1,
            offset: 4,
            length: 8,
        },
    };
    let proven = ProvenOccupant {
        occupant: restored,
        group: 88,
        session,
        git: "restart launch advanced".to_owned(),
    };
    let durable = current_residency(&bus, fixture.epoch)
        .expect("durable predecessor reads")
        .body()
        .lanes[0]
        .clone();

    super::super::propose::commit_restored(&bus, fixture.epoch, &durable, &proven)
        .expect("restart replacement commits");
    let current = current_residency(&bus, fixture.epoch).expect("restored renewal reads");
    assert_eq!(current.body().lanes[0].occupant, Some(restored));
    assert!(matches!(
        &current.body().renewals[0].state,
        wire::LaneRenewalState::Requested {
            predecessor,
            org_slot: 0,
        } if predecessor.occupant == fixture.occupant
            && predecessor.session == SESSION
    ));
    assert!(requested_exit(current.body(), fixture.lane, restored,));

    bus.verify().expect("restart renewal authority verifies");
    bus.shutdown().expect("restart renewal bus stops");
}

#[test]
fn predecessor_supervisor_failure_advances_requested_renewal_to_handoff() {
    let fixture = AdvisoryRtkFixture::new();
    let bus = seeded(&fixture, Some(requested(&fixture, false)));

    supervisor_failed(
        &bus,
        fixture.epoch,
        fixture.lane,
        fixture.occupant,
        "detached supervisor ended",
    )
    .expect("terminal predecessor failure commits");
    let current = current_residency(&bus, fixture.epoch).expect("failed predecessor reads");
    assert_eq!(current.body().lanes[0].occupant, None);
    assert_eq!(current.body().lanes[0].disposition, DEGRADED);
    assert!(matches!(
        current.body().renewals[0].state,
        wire::LaneRenewalState::HandoffPending { .. }
    ));

    bus.verify().expect("failed predecessor authority verifies");
    bus.shutdown().expect("failed predecessor bus stops");
}

fn assert_invalid_requests(
    fixture: &AdvisoryRtkFixture,
    bus: &Bus,
    seats: &Seats,
    config: &steer::Config,
    renewal_request: wire::intent::LaneRenewal,
) {
    let mut stale = renewal_request;
    stale.topology_revision = 0;
    assert!(matches!(
        set(
            bus,
            &fixture.paths,
            fixture.epoch,
            seats,
            config,
            &command(fixture, 8, &stale),
            &stale,
        ),
        Outcome::Refusal(refusal) if refusal.reason() == "lane::renewal-topology-moved"
    ));
    let mut moved = renewal_request;
    moved.lineage_id = fixture_uuid(99);
    assert!(matches!(
        set(
            bus,
            &fixture.paths,
            fixture.epoch,
            seats,
            config,
            &command(fixture, 9, &moved),
            &moved,
        ),
        Outcome::Refusal(refusal) if refusal.reason() == "lane::renewal-lineage-moved"
    ));
    assert!(controls(bus, fixture.lane).is_empty());
}

fn assert_first_request(
    fixture: &AdvisoryRtkFixture,
    bus: &Bus,
    seats: &Seats,
    config: &steer::Config,
    renewal_request: wire::intent::LaneRenewal,
) {
    let first = set(
        bus,
        &fixture.paths,
        fixture.epoch,
        seats,
        config,
        &command(fixture, 10, &renewal_request),
        &renewal_request,
    );
    assert!(matches!(first, Outcome::Reply(_)), "{first:?}");
    let head = current_residency(bus, fixture.epoch).expect("requested head reads");
    assert!(matches!(
        head.body().renewals[0].state,
        wire::LaneRenewalState::Requested { org_slot: 0, .. }
    ));
    assert_eq!(controls(bus, fixture.lane), [HANDOFF_CONTROL]);
}

fn assert_duplicate_and_retired(
    fixture: &AdvisoryRtkFixture,
    bus: &Bus,
    seats: &Seats,
    config: &steer::Config,
    renewal_request: wire::intent::LaneRenewal,
) {
    let duplicate = set(
        bus,
        &fixture.paths,
        fixture.epoch,
        seats,
        config,
        &command(fixture, 11, &renewal_request),
        &renewal_request,
    );
    assert!(matches!(
        duplicate,
        Outcome::Refusal(refusal) if refusal.reason() == "lane::renewal-already-active"
    ));
    assert_eq!(controls(bus, fixture.lane), [HANDOFF_CONTROL]);
    let requested = current_residency(bus, fixture.epoch)
        .expect("requested renewal reads")
        .body()
        .renewals[0]
        .clone();
    cancel_handoff(
        bus,
        &fixture.paths,
        fixture.epoch,
        handoff_directive(&requested),
        fixture.lane,
        &handoff_control(&requested),
    )
    .expect("fixture retires the predecessor request as though acknowledged");
    ensure_handoff(bus, &fixture.paths, fixture.epoch, config, fixture.lane)
        .expect("reconciliation observes durable handoff publication");
    assert!(
        controls(bus, fixture.lane).is_empty(),
        "an acknowledged handoff request must not be recreated"
    );
}

fn assert_cancellation(
    fixture: &AdvisoryRtkFixture,
    bus: &Bus,
    seats: &Seats,
    config: &steer::Config,
) {
    let clear = request(fixture, false);
    let canceled = set(
        bus,
        &fixture.paths,
        fixture.epoch,
        seats,
        config,
        &command(fixture, 12, &clear),
        &clear,
    );
    assert!(matches!(canceled, Outcome::Reply(_)), "{canceled:?}");
    assert!(
        current_residency(bus, fixture.epoch)
            .expect("cleared head reads")
            .body()
            .renewals
            .is_empty()
    );
    assert!(controls(bus, fixture.lane).is_empty());
}

fn assert_second_renewal(
    fixture: &AdvisoryRtkFixture,
    bus: &Bus,
    seats: &Seats,
    steering: &steer::Config,
    successor: CanonicalUuid,
) {
    let again = request(fixture, true);
    assert!(matches!(
        set(
            bus,
            &fixture.paths,
            fixture.epoch,
            seats,
            steering,
            &command(fixture, 15, &again),
            &again,
        ),
        Outcome::Reply(_)
    ));
    let renewed_again = current_residency(bus, fixture.epoch)
        .expect("second renewal request reads")
        .body()
        .renewals[0]
        .clone();
    assert!(matches!(
        renewed_again.state,
        wire::LaneRenewalState::Requested { predecessor, .. }
            if predecessor.occupant == successor
    ));
    assert_eq!(
        controls(bus, fixture.lane)
            .into_iter()
            .filter(|control| control == HANDOFF_CONTROL)
            .count(),
        1
    );
}

fn refuse_launch_config(fixture: &AdvisoryRtkFixture) -> String {
    let path = fixture.root.join("spice-rust.toml");
    let original = std::fs::read_to_string(&path).expect("fixture configuration reads");
    let refused = original.replace(
        &format!(
            "{:?}",
            fixture.root.join("fake-claude").display().to_string()
        ),
        &format!(
            "{:?}",
            fixture.root.join("missing-claude").display().to_string()
        ),
    );
    std::fs::write(path, refused).expect("refused launch configuration writes");
    original
}

fn request(fixture: &AdvisoryRtkFixture, requested: bool) -> wire::intent::LaneRenewal {
    wire::intent::LaneRenewal {
        topology_revision: 1,
        lineage_id: fixture.lane,
        lane: fixture.lane,
        requested,
    }
}

fn command(
    fixture: &AdvisoryRtkFixture,
    id: u64,
    request: &wire::intent::LaneRenewal,
) -> wire::AdmittedCommand {
    Command {
        v: wire::WireVersion::CURRENT,
        id: fixture_uuid(id),
        epoch: fixture.epoch,
        kind: wire::CommandKind::LaneRenewal,
        expected: Vec::new(),
        body: serde_json::to_value(request).expect("renewal request encodes"),
    }
    .admit(&wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor: Actor::Operator,
        epoch: fixture.epoch,
    })
    .expect("operator renewal command admits")
}

fn requested(fixture: &AdvisoryRtkFixture, handoff_pending: bool) -> wire::LaneRenewalRecord {
    let predecessor = wire::RenewalIdentity {
        occupant: fixture.occupant,
        session: SESSION.to_owned(),
        thread: SESSION.to_owned(),
    };
    wire::LaneRenewalRecord {
        lineage_id: fixture.lane,
        lane: fixture.lane,
        revision: 1,
        state: if handoff_pending {
            wire::LaneRenewalState::HandoffPending {
                predecessor,
                org_slot: 0,
            }
        } else {
            wire::LaneRenewalState::Requested {
                predecessor,
                org_slot: 0,
            }
        },
    }
}

fn seeded(fixture: &AdvisoryRtkFixture, renewal: Option<wire::LaneRenewalRecord>) -> Bus {
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
    let occupied = renewal
        .as_ref()
        .is_none_or(|record| matches!(record.state, wire::LaneRenewalState::Requested { .. }));
    let session = wire::LaneSession {
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
    };
    let lane = fixture.lane;
    let occupant = fixture.occupant;
    let capability = fixture.capability;
    let worktree = fixture.root.display().to_string();
    let vendor = fixture.vendor.clone();
    let epoch = fixture.epoch;
    proposal.residency_head(seated, move |_| wire::ResidencyHead {
        epoch,
        state: wire::ResidentState::Seated,
        topology_revision: 1,
        lanes: vec![wire::LaneResidency {
            lane,
            lineage_id: lane,
            name: None,
            worktree,
            life: "drive".to_owned(),
            vendor,
            occupant: occupied.then_some(occupant),
            lineage: vec![occupant],
            capabilities: vec![capability],
            session: Some(session),
            disposition: if occupied { OCCUPIED } else { VACANT }.to_owned(),
        }],
        subscriptions: vec![wire::LineageSubscription {
            lineage_id: lane,
            policy: wire::intent::LineageLife::Drive,
        }],
        renewals: renewal.into_iter().collect(),
    });
    commit(&bus, proposal).expect("fixture residency commits");
    bus
}

fn publish_surface(fixture: &AdvisoryRtkFixture) -> canvas::Discovery {
    let discovery = canvas::Discovery {
        endpoint: "http://127.0.0.1:1".to_owned(),
        capability: fixture.capability,
        epoch: fixture.epoch,
    };
    super::super::super::steering::publish_lane_surface(
        &fixture.paths,
        &discovery,
        fixture.root.to_str().expect("fixture root is UTF-8"),
        fixture.lane,
        fixture.capability,
    )
    .expect("fixture lane surface publishes");
    discovery
}

fn controls(bus: &Bus, lane: CanonicalUuid) -> Vec<String> {
    bus.read(move |read| crate::steering_read::pending(read, Some(lane)))
        .expect("pending controls read")
        .iter()
        .filter_map(|head| {
            head.body()
                .control
                .as_deref()
                .map(|control| control.split(':').next().unwrap_or(control).to_owned())
        })
        .collect()
}

fn fixture_uuid(value: u64) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-{value:012x}")
        .parse()
        .expect("fixture UUID")
}

fn success() -> WaitOutcome {
    WaitOutcome::externally_reaped_completion(sill::Completion::Exited(ExitStatus::from_raw(0)))
}
