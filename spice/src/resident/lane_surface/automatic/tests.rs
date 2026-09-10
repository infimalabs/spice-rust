use super::super::launch::test_support::{AdvisoryRtkFixture, assert_advisory_health};
use super::*;

#[test]
fn automatic_wake_commits_advisory_rtk_for_capability_free_readback_without_reprobe() {
    let fixture = AdvisoryRtkFixture::new();
    let bus = Bus::start(journal::Journal::in_memory().expect("fixture journal opens"))
        .expect("fixture bus starts");
    seed_vacant_lane(&bus, &fixture, "drive");
    let capabilities = canvas::axum::Capabilities::default();
    let discovery = canvas::Discovery {
        endpoint: "http://127.0.0.1:1".to_owned(),
        capability: fixture.capability,
        epoch: fixture.epoch,
    };
    let runtime = Runtime::new(&capabilities, &discovery, fixture.reporter());
    let mut seats = Seats::new();

    let result = wake_automatic(
        &bus,
        &fixture.paths,
        fixture.epoch,
        &mut seats,
        &runtime,
        fixture.lane,
        fixture.occupant,
        None,
        None,
    )
    .expect("automatic advisory-RTK launch commits");

    match result {
        AutomaticWake::Started => {}
        AutomaticWake::Unstarted(outcome) => {
            panic!("automatic advisory-RTK launch did not start: {outcome:?}")
        }
    }
    fixture.assert_occupant_started_with_rtk();
    fixture.assert_one_configured_probe_from_launch_worktree();
    fixture.assert_rust_native_history_path_prepared();
    let residency = current_residency(&bus, fixture.epoch).expect("committed residency reads");
    let answer = answer(residency.body(), None, &seats).expect("capability-free lane readback");
    let row = answer.lanes.first().expect("automatic lane projects");
    assert_advisory_health(row.rtk.as_ref().expect("readback retains RTK health"));
    let encoded = serde_json::to_string(&answer).expect("lane readback encodes");
    assert!(
        !encoded.contains(&fixture.capability.to_string()),
        "{encoded}"
    );
    fixture.assert_one_configured_probe_from_launch_worktree();

    stop_lane(&mut seats, fixture.lane);
    bus.verify().expect("automatic RTK authority verifies");
    bus.shutdown().expect("fixture bus stops");
}

#[test]
fn explicit_wake_starts_an_already_seated_drain_lane() {
    let fixture = AdvisoryRtkFixture::new();
    let bus = Bus::start(journal::Journal::in_memory().expect("fixture journal opens"))
        .expect("fixture bus starts");
    seed_vacant_lane(&bus, &fixture, "drain");
    let capabilities = canvas::axum::Capabilities::default();
    let discovery = canvas::Discovery {
        endpoint: "http://127.0.0.1:1".to_owned(),
        capability: fixture.capability,
        epoch: fixture.epoch,
    };
    let runtime = Runtime::new(&capabilities, &discovery, fixture.reporter());
    let request = LaneState {
        lane: fixture.lane,
        action: LaneAction::Wake,
        worktree: None,
    };
    let command = admitted_operator_wake(&fixture, &request);
    let mut seats = Seats::new();

    let outcome = wake(
        &bus,
        &fixture.paths,
        fixture.epoch,
        &mut seats,
        &runtime,
        &command,
        &request,
    );

    assert!(matches!(outcome, Outcome::Reply(_)), "{outcome:?}");
    let residency = current_residency(&bus, fixture.epoch).expect("committed residency reads");
    let row = residency
        .body()
        .lanes
        .first()
        .expect("explicit lane remains seated");
    assert_eq!(row.life, "drain");
    let occupant = row.occupant.expect("explicit wake records its occupant");
    assert_eq!(row.lineage.last(), Some(&occupant));
    assert!(seats.occupied(fixture.lane));
    stop_lane(&mut seats, fixture.lane);
    bus.verify().expect("explicit wake authority verifies");
    bus.shutdown().expect("fixture bus stops");
}

#[test]
fn automatic_wake_refuses_an_already_seated_drain_lane() {
    let fixture = AdvisoryRtkFixture::new();
    let bus = Bus::start(journal::Journal::in_memory().expect("fixture journal opens"))
        .expect("fixture bus starts");
    seed_vacant_lane(&bus, &fixture, "drain");
    let capabilities = canvas::axum::Capabilities::default();
    let discovery = canvas::Discovery {
        endpoint: "http://127.0.0.1:1".to_owned(),
        capability: fixture.capability,
        epoch: fixture.epoch,
    };
    let runtime = Runtime::new(&capabilities, &discovery, fixture.reporter());

    let outcome = wake_automatic(
        &bus,
        &fixture.paths,
        fixture.epoch,
        &mut Seats::new(),
        &runtime,
        fixture.lane,
        fixture.occupant,
        None,
        None,
    );

    match outcome {
        Err(Outcome::Refusal(refusal)) => {
            assert_eq!(refusal.reason(), "lifecycle::wake-life-ended");
        }
        Err(Outcome::Fault(fault)) => panic!("automatic wake faulted: {fault:?}"),
        Err(Outcome::Reply(_)) | Ok(_) => panic!("automatic wake started a Drain lane"),
    }
    bus.verify().expect("automatic refusal verifies");
    bus.shutdown().expect("fixture bus stops");
}

#[test]
fn automatic_wake_never_overlaps_an_authoritatively_occupied_lane() {
    let fixture = AdvisoryRtkFixture::new();
    let bus = Bus::start(journal::Journal::in_memory().expect("fixture journal opens"))
        .expect("fixture bus starts");
    seed_vacant_lane(&bus, &fixture, "drive");
    let observed = current_residency(&bus, fixture.epoch).expect("vacant residency reads");
    let mut occupied = observed.body().clone();
    let row = occupied.lanes.first_mut().expect("fixture lane exists");
    row.occupant = Some(fixture.occupant);
    row.lineage.push(fixture.occupant);
    row.disposition = OCCUPIED.to_owned();
    let mut write = write(Actor::Spice, fixture.epoch, Producer::ResidentRecognition)
        .expect("occupancy write starts");
    guard_residency(&mut write, &observed);
    let started = write
        .fact(
            wire::body::LaneStarted {
                lane: fixture.lane,
                occupant: fixture.occupant,
                epoch: fixture.epoch,
                group: 42,
            },
            FactCause::Existing(observed.position()),
        )
        .expect("fixture start fact is valid");
    write.residency_head(started, move |_| occupied);
    commit(&bus, write).expect("occupied residency commits");
    let capabilities = canvas::axum::Capabilities::default();
    let discovery = canvas::Discovery {
        endpoint: "http://127.0.0.1:1".to_owned(),
        capability: fixture.capability,
        epoch: fixture.epoch,
    };
    let runtime = Runtime::new(&capabilities, &discovery, fixture.reporter());

    let outcome = wake_automatic(
        &bus,
        &fixture.paths,
        fixture.epoch,
        &mut Seats::new(),
        &runtime,
        fixture.lane,
        fixture.occupant,
        None,
        None,
    );

    match outcome {
        Err(Outcome::Refusal(refusal)) => {
            assert_eq!(refusal.reason(), "lifecycle::wake-capacity-moved");
        }
        Err(Outcome::Fault(fault)) => panic!("occupied wake faulted: {fault:?}"),
        Err(Outcome::Reply(_)) | Ok(_) => panic!("occupied lane launched a successor"),
    }
    assert!(
        !fixture.root.join("rtk-probes").exists(),
        "successor launch reached the vendor preparation path"
    );
    bus.verify().expect("occupied wake refusal verifies");
    bus.shutdown().expect("fixture bus stops");
}

fn admitted_operator_wake(
    fixture: &AdvisoryRtkFixture,
    request: &LaneState,
) -> wire::AdmittedCommand {
    Command {
        v: wire::WireVersion::CURRENT,
        id: fixture.capability,
        epoch: fixture.epoch,
        kind: wire::CommandKind::LaneState,
        expected: Vec::new(),
        body: serde_json::to_value(request).expect("lane intent encodes"),
    }
    .admit(&wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor: Actor::Operator,
        epoch: fixture.epoch,
    })
    .expect("operator wake intent admits")
}

fn seed_vacant_lane(bus: &Bus, fixture: &AdvisoryRtkFixture, life: &str) {
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
    let root = fixture.root.display().to_string();
    let vendor = fixture.vendor.clone();
    let epoch = fixture.epoch;
    let lane = fixture.lane;
    let capability = fixture.capability;
    let life = life.to_owned();
    write.residency_head(seated, move |_| wire::ResidencyHead {
        epoch,
        state: ResidentState::Seated,
        topology_revision: 1,
        lanes: vec![wire::LaneResidency {
            lane,
            lineage_id: lane,
            name: None,
            worktree: root,
            life,
            vendor,
            occupant: None,
            lineage: Vec::new(),
            capabilities: vec![capability],
            session: None,
            disposition: VACANT.to_owned(),
        }],
        subscriptions: vec![wire::LineageSubscription {
            lineage_id: lane,
            policy: wire::intent::LineageLife::Drive,
            manual_projects: Vec::new(),
            auto_projects: Vec::new(),
        }],
        renewals: Vec::new(),
    });
    commit(bus, write).expect("fixture residency commits");
}

#[test]
fn startup_ready_promotes_expiry_and_preserves_a_one_hour_reclaim_horizon() {
    let epoch = "018f82d4-6f59-7d06-92e3-bce9131163aa"
        .parse()
        .expect("fixture epoch");
    let prior_epoch = "018f82d4-6f59-7d06-92e3-bce9131163ab"
        .parse()
        .expect("fixture prior epoch");
    let start: wire::Instant = "2026-08-30T08:00:00.000Z".parse().expect("fixture start");
    let provisional = wire::Instant::new(
        start
            .get()
            .checked_add(jiff::SignedDuration::from_secs(
                crate::board_command::AUTOMATIC_PROVISIONAL_SECONDS,
            ))
            .expect("provisional deadline shifts"),
    )
    .expect("provisional deadline");
    let ready: wire::Instant = "2026-08-30T08:00:10.000Z"
        .parse()
        .expect("fixture ready time");
    let initial_maximum = wire::Instant::new(
        start
            .get()
            .checked_add(jiff::SignedDuration::from_secs(
                crate::board_command::AUTOMATIC_MAXIMUM_SECONDS,
            ))
            .expect("maximum deadline shifts"),
    )
    .expect("maximum deadline");
    let promoted = promoted_lease(
        &wire::LeaseHead {
            start,
            renewal: start,
            maximum: initial_maximum,
            expiry: provisional,
            epoch: prior_epoch,
        },
        epoch,
        ready,
        false,
    )
    .expect("startup promotion succeeds");

    assert_eq!(promoted.start, start);
    assert_eq!(promoted.renewal, ready);
    assert_eq!(promoted.epoch, epoch);
    assert_eq!(
        promoted.expiry,
        wire::Instant::new(
            ready
                .get()
                .checked_add(jiff::SignedDuration::from_secs(
                    crate::board_command::AUTOMATIC_ROLLING_SECONDS,
                ))
                .expect("rolling deadline shifts")
        )
        .expect("rolling deadline")
    );
    assert_eq!(
        promoted.maximum,
        wire::Instant::new(
            ready
                .get()
                .checked_add(jiff::SignedDuration::from_secs(
                    crate::board_command::AUTOMATIC_MAXIMUM_SECONDS,
                ))
                .expect("maximum deadline shifts")
        )
        .expect("maximum deadline")
    );
    assert!(promoted.expiry < promoted.maximum);
}

#[test]
fn startup_ready_cannot_revive_an_expired_provisional_lease() {
    let epoch = "018f82d4-6f59-7d06-92e3-bce9131163aa"
        .parse()
        .expect("fixture epoch");
    let start: wire::Instant = "2026-08-30T08:00:00.000Z".parse().expect("fixture start");
    let expiry: wire::Instant = "2026-08-30T08:01:00.000Z".parse().expect("fixture expiry");
    let observed = wire::LeaseHead {
        start,
        renewal: start,
        maximum: expiry,
        expiry,
        epoch,
    };

    let refusal = promoted_lease(&observed, epoch, expiry, false)
        .expect_err("an expired provisional lease cannot be promoted");

    assert!(format!("{refusal:?}").contains("lifecycle::claim-promotion-expired"));
}

#[test]
fn thaw_replaces_the_indefinite_horizon_with_ordinary_automatic_bounds() {
    let epoch = "018f82d4-6f59-7d06-92e3-bce9131163aa"
        .parse()
        .expect("fixture epoch");
    let start: wire::Instant = "2026-08-30T08:00:00.000Z".parse().expect("fixture start");
    let ready: wire::Instant = "2026-08-30T08:05:00.000Z".parse().expect("fixture ready");
    let horizon = spice::claim_lease::indefinite_horizon();
    let thawed = promoted_lease(
        &wire::LeaseHead {
            start,
            renewal: start,
            expiry: horizon,
            maximum: horizon,
            epoch,
        },
        epoch,
        ready,
        true,
    )
    .expect("iced claim thaws");

    assert_eq!(thawed.start, start);
    assert_eq!(thawed.renewal, ready);
    assert!(!spice::claim_lease::is_indefinite(&thawed));
    assert_eq!(
        thawed.expiry,
        wire::Instant::new(
            ready
                .get()
                .checked_add(jiff::SignedDuration::from_secs(
                    crate::board_command::AUTOMATIC_ROLLING_SECONDS,
                ))
                .expect("rolling deadline shifts")
        )
        .expect("rolling deadline")
    );
    assert_eq!(
        thawed.maximum,
        wire::Instant::new(
            ready
                .get()
                .checked_add(jiff::SignedDuration::from_secs(
                    crate::board_command::AUTOMATIC_MAXIMUM_SECONDS,
                ))
                .expect("maximum deadline shifts")
        )
        .expect("maximum deadline")
    );
}
