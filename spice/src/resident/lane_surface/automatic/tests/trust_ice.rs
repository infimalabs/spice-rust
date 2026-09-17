use super::*;

const TASK: &str = "1kQpV9Gt";

#[test]
fn trust_movement_refuses_a_stale_thaw_and_a_fresh_retry_restores_bounded_lease() {
    let fixture = AdvisoryRtkFixture::new();
    let bus = Bus::start(journal::Journal::in_memory().expect("fixture journal opens"))
        .expect("fixture bus starts");
    seed_vacant_lane(&bus, &fixture, "drive");
    let task: wire::TaskStamp = TASK.parse().expect("fixture task");
    let digest = digest();
    let start: wire::Instant = "2026-08-30T08:00:00.000Z".parse().expect("fixture start");
    seed_iced_claim(&bus, &fixture, &task, &digest, start);
    grant_trust(&bus, &digest);

    let before_snapshot = bus.task_snapshot().expect("thaw decision snapshot reads");
    let before = task_heads(&before_snapshot, &task);
    let before_claim = before.claim().expect("iced claim exists").clone();
    let before_lease = before.lease().expect("iced lease exists").clone();
    let before_residency = current_residency(&bus, fixture.epoch).expect("vacant residency reads");
    assert!(spice::claim_lease::is_indefinite(before_lease.body()));
    assert_eq!(before_residency.body().lanes[0].occupant, None);
    let capabilities = canvas::axum::Capabilities::default();
    let discovery = canvas::Discovery {
        endpoint: "http://127.0.0.1:1".to_owned(),
        capability: fixture.capability,
        epoch: fixture.epoch,
    };
    let runtime = fixture.lane_runtime(&bus, &capabilities, &discovery);
    let (admitted, proven, mut seats) = admitted_thaw(&bus, &fixture, &runtime);

    revoke_trust(&bus, &digest);
    assert_stale_thaw_refused(&bus, &fixture, &admitted, &proven, &mut seats);
    let after_refusal = bus.task_snapshot().expect("refused thaw snapshot reads");
    let refused_heads = task_heads(&after_refusal, &task);
    assert_eq!(refused_heads.claim(), Some(&before_claim));
    assert_eq!(refused_heads.lease(), Some(&before_lease));
    assert_eq!(
        current_residency(&bus, fixture.epoch).expect("refused residency reads"),
        before_residency,
    );

    grant_existing_trust(&bus, &digest);
    retry_thaw(&bus, &fixture, &runtime, &task, start, &mut seats);
    stop_lane(&mut seats, fixture.lane);
    bus.verify()
        .expect("stale and fresh thaw authority verifies");
    bus.shutdown().expect("fixture bus stops");
}

fn admitted_thaw(
    bus: &Bus,
    fixture: &AdvisoryRtkFixture,
    runtime: &Runtime<'_>,
) -> (WakeAdmission, ProvenOccupant, Seats) {
    let request = WakeRequest {
        lane: fixture.lane,
        occupant: fixture.occupant,
        decision: None,
        reserved: None,
        renewal: None,
    };
    let admitted =
        admit_automatic_wake(bus, &fixture.paths, fixture.epoch, &Seats::new(), &request)
            .expect("thaw authority admits");
    assert!(admitted.claim.as_ref().is_some_and(|claim| claim.thaw));
    let launch = crate::resident::supervisor::launch(
        &fixture.paths,
        OccupantLaunchRequest {
            worktree: &admitted.row.worktree,
            vendor: &admitted.row.vendor,
            epoch: fixture.epoch,
            lane: fixture.lane,
            occupant: fixture.occupant,
            resume: admitted.resume.clone(),
            fast_mode: admitted.fast_mode,
            model: admitted.row.model.as_deref(),
            effort: admitted.row.effort.as_deref(),
        },
        Arc::clone(&runtime.capture),
    )
    .expect("stale thaw launch proves");
    let proven = ProvenOccupant {
        occupant: launch.occupant,
        group: launch.group,
        session: launch.session,
        git: launch.git,
    };
    let mut seats = Seats::new();
    seats
        .seat_detached(fixture.lane, fixture.occupant, launch.process)
        .expect("stale thaw process seats");
    (admitted, proven, seats)
}

fn assert_stale_thaw_refused(
    bus: &Bus,
    fixture: &AdvisoryRtkFixture,
    admitted: &WakeAdmission,
    proven: &ProvenOccupant,
    seats: &mut Seats,
) {
    let refused = commit_automatic_wake(
        bus,
        fixture.epoch,
        &admitted.observed,
        &admitted.row,
        proven,
        WakeCommitAuthority {
            expected_cursor: admitted.expected_cursor,
            claim: admitted.claim.as_ref(),
            renewal: None,
        },
    )
    .expect_err("trust movement refuses the stale seating transaction");
    match refused {
        Outcome::Refusal(refusal) => {
            assert_eq!(refusal.reason(), "lifecycle::held-claim-trust-moved");
        }
        Outcome::Fault(fault) => panic!("stale thaw faulted: {fault:?}"),
        Outcome::Reply(_) => panic!("stale thaw returned a reply"),
    }
    stop_lane(seats, fixture.lane);
}

fn retry_thaw(
    bus: &Bus,
    fixture: &AdvisoryRtkFixture,
    runtime: &Runtime<'_>,
    task: &wire::TaskStamp,
    start: wire::Instant,
    seats: &mut Seats,
) {
    let fresh = bus.task_snapshot().expect("fresh thaw snapshot reads");
    assert!(
        !task_heads(&fresh, task)
            .statement()
            .body()
            .requirements
            .has_unsatisfied_trust_grant(&fresh.environment())
    );
    let started = wake_automatic(
        bus,
        &fixture.paths,
        fixture.epoch,
        seats,
        runtime,
        fixture.lane,
        fixture.occupant,
        Some(&fresh),
        None,
    )
    .expect("fresh thaw commits");
    assert!(matches!(started, AutomaticWake::Started));
    let thawed = bus.task_snapshot().expect("thawed snapshot reads");
    let thawed_lease = task_heads(&thawed, task)
        .lease()
        .expect("thawed lease exists")
        .body();
    assert_eq!(thawed_lease.start, start);
    assert!(!spice::claim_lease::is_indefinite(thawed_lease));
    assert!(thawed_lease.start < thawed_lease.renewal);
    assert!(thawed_lease.renewal < thawed_lease.expiry);
    assert!(thawed_lease.expiry < thawed_lease.maximum);
}

fn seed_iced_claim(
    bus: &Bus,
    fixture: &AdvisoryRtkFixture,
    task: &wire::TaskStamp,
    digest: &wire::Digest,
    start: wire::Instant,
) {
    let residency = current_residency(bus, fixture.epoch).expect("seed residency reads");
    let mut durable = residency.body().lanes[0].clone();
    durable.occupant = Some(fixture.capability);
    let horizon = spice::claim_lease::indefinite_horizon();
    seed_task_claim(bus, task, &durable, fixture.epoch, start, horizon, horizon);
    let snapshot = bus.task_snapshot().expect("seed claim snapshot reads");
    let heads = task_heads(&snapshot, task);
    let mut statement = heads.statement().body().clone();
    statement.requirements = wire::EnvironmentRequirements::one(
        wire::EnvironmentRequirement::trust_grant(digest.clone()),
    );
    let mut write = JournalWrite::new(
        Actor::Lane(fixture.lane),
        format!("task:{task}").parse().expect("fixture task origin"),
        now(),
        Producer::CommandDoor,
    );
    let edited = write
        .fact(
            wire::body::TaskEdited {
                task: task.clone(),
                revision: 2,
                notice: None,
            },
            FactCause::Root,
        )
        .expect("trust requirement edit builds");
    write.task_head(edited, move |position| wire::TaskHead {
        revision: position,
        ..statement
    });
    committed(bus.commit(write).expect("trust requirement commits"));
}

fn grant_trust(bus: &Bus, digest: &wire::Digest) {
    let mut write = trust_write();
    let pended = write
        .fact(
            wire::body::TrustPended {
                surface: "spice".to_owned(),
                digest: digest.clone(),
                worktree: Some("/producer/worktree".to_owned()),
                previous: None,
            },
            FactCause::Root,
        )
        .expect("trust pend builds");
    let granted = write
        .fact(
            wire::body::TrustGranted {
                surface: "spice".to_owned(),
                digest: digest.clone(),
            },
            FactCause::Pending(pended),
        )
        .expect("trust grant builds");
    let digest = digest.clone();
    write.trust_head(granted, move |_| wire::TrustHead {
        digest,
        state: wire::TrustState::Granted,
        worktree: Some("/producer/worktree".to_owned()),
    });
    committed(bus.commit(write).expect("trust grant commits"));
}

fn revoke_trust(bus: &Bus, digest: &wire::Digest) {
    move_trust(bus, digest, wire::TrustState::Revoked);
}

fn grant_existing_trust(bus: &Bus, digest: &wire::Digest) {
    move_trust(bus, digest, wire::TrustState::Granted);
}

fn move_trust(bus: &Bus, digest: &wire::Digest, state: wire::TrustState) {
    let current = bus
        .trust_head(digest)
        .expect("trust head reads")
        .expect("trust head exists");
    let mut write = trust_write();
    let moved = match state {
        wire::TrustState::Granted => write.fact(
            wire::body::TrustGranted {
                surface: "spice".to_owned(),
                digest: digest.clone(),
            },
            FactCause::Existing(current.position()),
        ),
        wire::TrustState::Revoked => write.fact(
            wire::body::TrustRevoked {
                surface: "spice".to_owned(),
                digest: digest.clone(),
            },
            FactCause::Existing(current.position()),
        ),
        wire::TrustState::Pended => unreachable!("fixture moves only terminal trust states"),
    }
    .expect("trust movement builds");
    let digest = digest.clone();
    let worktree = current.body().worktree.clone();
    write.trust_head(moved, move |_| wire::TrustHead {
        digest,
        state,
        worktree,
    });
    committed(bus.commit(write).expect("trust movement commits"));
}

fn trust_write() -> JournalWrite {
    JournalWrite::new(
        Actor::Operator,
        "repo:genesis".parse().expect("fixture origin"),
        now(),
        Producer::CommandDoor,
    )
}

fn task_heads<'a>(
    snapshot: &'a bus::TaskSnapshot,
    task: &wire::TaskStamp,
) -> &'a journal::TaskHeads {
    snapshot
        .tasks()
        .iter()
        .find(|heads| heads.statement().body().born == *task)
        .expect("fixture task heads exist")
}

fn committed(outcome: bus::CommitOutcome) {
    assert!(matches!(outcome, bus::CommitOutcome::Committed(_)));
}

fn now() -> wire::Instant {
    wire::Instant::new(jiff::Timestamp::now()).expect("fixture instant")
}

fn digest() -> wire::Digest {
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        .parse()
        .expect("fixture digest")
}
