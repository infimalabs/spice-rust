use super::*;

static NEXT_FIXTURE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct Fixture {
    root: std::path::PathBuf,
    bus: Bus,
    old_epoch: CanonicalUuid,
    new_epoch: CanonicalUuid,
    lane: CanonicalUuid,
    ancestor: CanonicalUuid,
    successor: CanonicalUuid,
    task: wire::TaskHandle,
    at: wire::Instant,
    expiry: wire::Instant,
    maximum: wire::Instant,
}

fn uuid(value: u64) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-{value:012x}")
        .parse()
        .expect("fixture UUID")
}

fn fixture() -> Fixture {
    let fixture = NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root =
        std::env::temp_dir().join(format!("spice-lane-carry-{}-{fixture}", std::process::id()));
    std::fs::create_dir_all(&root).expect("fixture directory");
    let journal =
        journal::Journal::open(root.join("authority.sqlite3")).expect("fixture journal opens");
    Fixture {
        root,
        bus: Bus::start(journal).expect("fixture bus starts"),
        old_epoch: uuid(1),
        new_epoch: uuid(2),
        lane: uuid(3),
        ancestor: uuid(4),
        successor: uuid(5),
        task: "CARRY-active01".parse().expect("fixture task"),
        at: "2026-08-01T06:00:00.000Z".parse().expect("fixture instant"),
        expiry: "2030-08-01T07:00:00.000Z".parse().expect("fixture expiry"),
        maximum: "2030-08-01T08:00:00.000Z".parse().expect("fixture maximum"),
    }
}

fn seed_residency(fixture: &Fixture, seed: &mut JournalWrite) {
    let seated = seed
        .fact(
            wire::body::UpSeated {
                epoch: fixture.old_epoch,
            },
            FactCause::Root,
        )
        .expect("resident fact");
    let lane = fixture.lane;
    let ancestor = fixture.ancestor;
    let epoch = fixture.old_epoch;
    seed.residency_head(seated, move |_| wire::ResidencyHead {
        epoch,
        state: wire::ResidentState::Seated,
        lanes: vec![wire::LaneResidency {
            lane,
            worktree: "/worktrees/predecessor".into(),
            life: "drive".into(),
            vendor: "claude".parse().expect("fixture vendor"),
            occupant: Some(ancestor),
            lineage: vec![ancestor],
            capabilities: vec![uuid(6)],
            session: None,
            disposition: OCCUPIED.into(),
        }],
    });
}

fn seed_task(fixture: &Fixture, seed: &mut JournalWrite) -> PendingFact {
    let added = seed
        .fact(
            wire::body::TaskAdded {
                task: fixture.task.clone(),
                uuid: uuid(7),
                title: "Carry active work".into(),
                project: "rim.recovery".into(),
                tags: Vec::new(),
                depends: Vec::new(),
                flow: vec!["todo".parse().expect("fixture phase")],
                phase: "todo".parse().expect("fixture phase"),
                revision: 1,
            },
            FactCause::Root,
        )
        .expect("task fact");
    let task = fixture.task.clone();
    seed.task_head(added, move |position| wire::TaskHead {
        handle: task,
        uuid: uuid(7),
        title: "Carry active work".into(),
        project: "rim.recovery".into(),
        tags: Vec::new(),
        dependencies: Vec::new(),
        provenance: "resident:018f82d4-6f59-7d06-92e3-000000000001"
            .parse()
            .expect("fixture origin"),
        revision: position,
    });
    let task = fixture.task.clone();
    seed.workflow_head(task, added, move |_| wire::WorkflowHead {
        flow: vec!["todo".parse().expect("fixture phase")],
        phase: "todo".parse().expect("fixture phase"),
        status: "pending".into(),
        review: "unassigned".into(),
        assignment: None,
        deferral: None,
        completion: None,
    });
    added
}

fn seed_claim(fixture: &Fixture, seed: &mut JournalWrite, added: PendingFact) {
    let won = seed
        .fact(
            wire::body::ClaimWon {
                task: fixture.task.clone(),
                actor: Actor::Operator,
                lane: fixture.lane,
                occupant: fixture.ancestor,
                epoch: fixture.old_epoch,
                until: fixture.expiry,
                git: None,
            },
            FactCause::Pending(added),
        )
        .expect("claim fact");
    let task = fixture.task.clone();
    let lane = fixture.lane;
    let occupant = fixture.ancestor;
    let epoch = fixture.old_epoch;
    seed.claim_head(task, won, move |fact| wire::ClaimHead {
        actor: Actor::Operator,
        fact,
        lane,
        occupant,
        epoch,
        disposition: wire::ClaimDisposition::Active,
    });
    let task = fixture.task.clone();
    let at = fixture.at;
    let maximum = fixture.maximum;
    let expiry = fixture.expiry;
    seed.lease_head(task, won, move |_| wire::LeaseHead {
        start: at,
        renewal: at,
        maximum,
        expiry,
        epoch,
    });
}

fn seed(fixture: &Fixture) {
    let origin: wire::Origin = format!("resident:{}", fixture.old_epoch)
        .parse()
        .expect("fixture origin");
    let mut seed = JournalWrite::new(Actor::Operator, origin, fixture.at, Producer::CommandDoor);
    seed_residency(fixture, &mut seed);
    let added = seed_task(fixture, &mut seed);
    seed_claim(fixture, &mut seed, added);
    assert!(matches!(
        fixture.bus.commit(seed).expect("seed reaches writer"),
        bus::CommitOutcome::Committed(_)
    ));
}

fn reseat(fixture: &Fixture) {
    let old = fixture
        .bus
        .residency_head()
        .expect("residency reads")
        .expect("residency exists");
    let mut next = old.body().clone();
    next.epoch = fixture.new_epoch;
    let origin: wire::Origin = format!("resident:{}", fixture.new_epoch)
        .parse()
        .expect("fixture origin");
    let mut write = JournalWrite::new(Actor::Spice, origin, fixture.at, Producer::CommandDoor);
    let fact = write
        .fact(
            wire::body::UpSeated {
                epoch: fixture.new_epoch,
            },
            FactCause::Root,
        )
        .expect("new generation fact");
    write.residency_head(fact, move |_| next);
    assert!(matches!(
        fixture.bus.commit(write).expect("reseat reaches writer"),
        bus::CommitOutcome::Committed(_)
    ));
}

fn carry(fixture: &Fixture) {
    let carried = observe_carried_claim(&fixture.bus, fixture.lane, fixture.ancestor, fixture.at)
        .expect("claim observation")
        .expect("operator claim is lane-bound");
    let observed = current_residency(&fixture.bus, fixture.new_epoch).expect("current residency");
    let row = observed.body().lanes[0].clone();
    let context = ImportContext {
        observed: observed.clone(),
        row,
        worktree: "/worktrees/successor".into(),
        ancestor: fixture.ancestor,
        resume: vendor::Resume::Fresh,
        carried: Some(carried.clone()),
    };
    let mut next = observed.body().clone();
    next.lanes[0].occupant = Some(fixture.successor);
    next.lanes[0].lineage.push(fixture.successor);
    let mut write = super::write(
        Actor::Spice,
        fixture.new_epoch,
        Producer::ResidentRecognition,
    )
    .expect("carry proposal");
    guard_residency(&mut write, &observed);
    guard_carried_claim(&mut write, &carried);
    let renewed = write
        .fact(
            wire::body::LaneRenewed {
                lane: fixture.lane,
                occupant: fixture.successor,
                ancestor: fixture.ancestor,
                epoch: fixture.new_epoch,
                group: 42,
            },
            FactCause::Existing(observed.position()),
        )
        .expect("renewal fact");
    let fact = append_claim_carry(
        &mut write,
        &carried,
        &context,
        fixture.successor,
        fixture.new_epoch,
        renewed,
    )
    .expect("claim carry appends");
    write.residency_head(fact, move |_| next);
    commit(&fixture.bus, write).expect("carry commits atomically");
}

fn assert_carried(fixture: &Fixture) {
    let snapshot = fixture.bus.task_snapshot().expect("carried heads read");
    let heads = snapshot.tasks().first().expect("fixture task remains");
    let claim = heads.claim().expect("claim remains active");
    let lease = heads.lease().expect("lease remains present");
    let residency = snapshot.residency().expect("residency remains present");
    assert_eq!(claim.position(), lease.position());
    assert_eq!(claim.position(), residency.position());
    assert_eq!(claim.body().actor, Actor::Operator);
    assert_eq!(claim.body().occupant, fixture.successor);
    assert_eq!(claim.body().epoch, fixture.new_epoch);
    assert_eq!(lease.body().epoch, fixture.new_epoch);
    assert_eq!(lease.body().expiry, fixture.expiry);
    assert_eq!(lease.body().maximum, fixture.maximum);
    assert!(
        fixture
            .bus
            .fact_snapshot()
            .expect("facts read")
            .facts()
            .iter()
            .any(|fact| fact.fact.kind() == wire::FactKind::ClaimCarried)
    );
    fixture.bus.verify().expect("carried authority verifies");
}

#[test]
fn successor_carry_moves_operator_claim_and_lease_authority_together() {
    let fixture = fixture();
    seed(&fixture);
    reseat(&fixture);
    carry(&fixture);
    assert_carried(&fixture);
    fixture.bus.shutdown().expect("fixture bus stops");
    std::fs::remove_dir_all(fixture.root).expect("fixture cleanup");
}

#[test]
fn import_refuses_an_active_head_whose_lease_is_no_longer_live() {
    let fixture = fixture();
    seed(&fixture);

    assert!(matches!(
        observe_carried_claim(
            &fixture.bus,
            fixture.lane,
            fixture.ancestor,
            fixture.expiry,
        ),
        Err(Outcome::Refusal(Refusal { ref reason, .. }))
            if reason == "lane::claim-lease-stale"
    ));

    fixture.bus.shutdown().expect("fixture bus stops");
    std::fs::remove_dir_all(fixture.root).expect("fixture cleanup");
}
