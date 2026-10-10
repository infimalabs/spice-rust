use super::*;

fn uuid(last: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131164{last:02}")
        .parse()
        .expect("fixture UUID")
}

fn shifted(at: wire::Instant, seconds: i64) -> wire::Instant {
    wire::Instant::new(
        at.get()
            .checked_add(jiff::SignedDuration::from_secs(seconds))
            .expect("fixture instant remains in range"),
    )
    .expect("shifted fixture instant is canonical")
}

#[test]
fn adoption_rebinds_claim_epoch_without_restarting_the_occupant() {
    let lane = uuid(20);
    let occupant = uuid(21);
    let old_epoch = uuid(22);
    let new_epoch = uuid(23);
    let task: wire::TaskStamp = "1kQpV9Gt".parse().expect("fixture task");
    let now = wire::Instant::new(jiff::Timestamp::now()).expect("fixture clock is canonical");
    let start = shifted(now, -60);
    let expiry = shifted(now, 60 * 60);
    let maximum = shifted(now, 2 * 60 * 60);
    let durable = wire::LaneResidency {
        lane,
        team: lane,
        name: Some("Adopted lane".to_owned()),
        worktree: "/worktrees/adopted".to_owned(),
        life: "drive".to_owned(),
        vendor: "omp".parse().expect("fixture vendor"),
        model: None,
        effort: None,
        fast_mode: false,
        occupant: Some(occupant),
        lineage: vec![occupant],
        capabilities: vec![uuid(24)],
        session: None,
        disposition: OCCUPIED.to_owned(),
    };
    let bus = Bus::start(journal::Journal::in_memory().expect("fixture journal opens"))
        .expect("fixture bus starts");
    let born = seed_birth(&bus, &durable, old_epoch, start);
    seed_restart_claim(
        &bus, &task, &durable, old_epoch, new_epoch, start, expiry, maximum,
    );
    let residency_position = bus
        .residency_head()
        .expect("residency reads")
        .expect("residency exists")
        .position();

    commit_adopted(&bus, new_epoch, &durable).expect("adopted claim commits");

    let snapshot = bus.task_snapshot().expect("adopted heads read");
    let heads = snapshot.tasks().first().expect("fixture task remains");
    let claim = heads.claim().expect("claim remains active");
    let lease = heads.lease().expect("lease remains present");
    let residency = snapshot.residency().expect("residency remains present");
    assert_eq!(claim.position(), lease.position());
    assert_ne!(claim.position(), residency.position());
    assert_eq!(residency.position(), residency_position);
    assert_eq!(claim.body().occupant, occupant);
    assert_eq!(claim.body().epoch, new_epoch);
    assert_eq!(lease.body().epoch, new_epoch);
    assert_eq!(residency.body().lanes[0], durable);
    let carried = bus
        .fact_snapshot()
        .expect("adopted facts read")
        .facts()
        .iter()
        .find(|fact| fact.fact.kind() == wire::FactKind::ClaimCarried)
        .expect("adoption records the authority carry")
        .fact
        .body()
        .clone();
    assert_eq!(carried.get("occupant"), carried.get("ancestor"));
    assert_eq!(
        snapshot.facts().occupant_address(lane, occupant),
        Ok(born),
        "adoption keeps the occupant's original birth address"
    );
    assert_eq!(
        bus.resolve_occupant(born)
            .expect("resolver answers")
            .map(|birth| (birth.lane, birth.occupant, birth.epoch)),
        Ok((lane, occupant, old_epoch))
    );
    bus.verify().expect("adopted authority verifies");
    bus.shutdown().expect("fixture bus stops");
}

#[test]
fn startup_seating_carries_claim_without_an_observable_epoch_gap() {
    let lane = uuid(30);
    let occupant = uuid(31);
    let old_epoch = uuid(32);
    let new_epoch = uuid(33);
    let task: wire::TaskStamp = "1kQpV9Gt".parse().expect("fixture task");
    let now = wire::Instant::new(jiff::Timestamp::now()).expect("fixture clock is canonical");
    let start = shifted(now, -60);
    let expiry = shifted(now, 60 * 60);
    let maximum = shifted(now, 2 * 60 * 60);
    let durable = wire::LaneResidency {
        lane,
        team: lane,
        name: Some("Atomic restart lane".to_owned()),
        worktree: "/worktrees/atomic-restart".to_owned(),
        life: "drive".to_owned(),
        vendor: "omp".parse().expect("fixture vendor"),
        model: None,
        effort: None,
        fast_mode: false,
        occupant: Some(occupant),
        lineage: vec![occupant],
        capabilities: vec![uuid(34)],
        session: None,
        disposition: OCCUPIED.to_owned(),
    };
    let bus = Bus::start(journal::Journal::in_memory().expect("fixture journal opens"))
        .expect("fixture bus starts");
    seed_task_claim(&bus, &task, &durable, old_epoch, start, expiry, maximum);
    let mut startup = JournalWrite::new(
        Actor::Spice,
        format!("resident:{new_epoch}")
            .parse()
            .expect("resident origin"),
        start,
        Producer::CommandDoor,
    );
    let seated = startup
        .fact(wire::body::UpSeated { epoch: new_epoch }, FactCause::Root)
        .expect("startup fact builds");
    let carried = super::super::restart_claim::append_startup_claims(
        &bus,
        std::slice::from_ref(&durable),
        new_epoch,
        &mut startup,
        seated,
    )
    .expect("startup claim appends");
    let row = durable.clone();
    startup.residency_head(carried, move |_| wire::ResidencyHead {
        epoch: new_epoch,
        state: wire::ResidentState::Seated,
        topology_revision: 1,
        lanes: vec![row],
        subscriptions: Vec::new(),
        renewals: Vec::new(),
    });
    assert!(matches!(
        bus.commit(startup).expect("atomic startup commits"),
        bus::CommitOutcome::Committed(_)
    ));

    let snapshot = bus.task_snapshot().expect("atomic heads read");
    let heads = snapshot.tasks().first().expect("fixture task remains");
    let claim = heads.claim().expect("claim remains active");
    let lease = heads.lease().expect("lease remains present");
    let residency = snapshot.residency().expect("residency remains present");
    assert_eq!(claim.position(), lease.position());
    assert_eq!(claim.position(), residency.position());
    assert_eq!(claim.body().occupant, occupant);
    assert_eq!(claim.body().epoch, new_epoch);
    assert_eq!(lease.body().epoch, new_epoch);
    assert_eq!(residency.body().epoch, new_epoch);
    bus.verify().expect("atomic restart authority verifies");
    bus.shutdown().expect("fixture bus stops");
}

/// Record the occupant's one launch birth beneath the generation it started in.
pub(in crate::resident) fn seed_birth(
    bus: &Bus,
    durable: &wire::LaneResidency,
    epoch: CanonicalUuid,
    at: wire::Instant,
) -> wire::OccupantAddress {
    let origin: wire::Origin = format!("resident:{epoch}")
        .parse()
        .expect("resident origin");
    let mut write = JournalWrite::new(Actor::Spice, origin, at, Producer::ResidentRecognition);
    write
        .fact(
            wire::body::LaneStarted {
                lane: durable.lane,
                occupant: durable.occupant.expect("durable occupant"),
                epoch,
                group: 7,
                fresh_session: Some(true),
            },
            FactCause::Root,
        )
        .expect("birth fact builds");
    match bus.commit(write).expect("fixture birth commits") {
        bus::CommitOutcome::Committed(committed) => wire::OccupantAddress::new(
            committed
                .cursor()
                .position()
                .expect("a commit has a position"),
        ),
        other => panic!("fixture birth did not commit: {other:?}"),
    }
}

/// A successor owns a new executable address on the lane; its ancestor's
/// address stays readable history and refuses execution as expired.
pub(in crate::resident) fn assert_successor_address(
    bus: &Bus,
    lane: CanonicalUuid,
    (ancestor, ancestor_occupant): (wire::OccupantAddress, CanonicalUuid),
    successor_occupant: CanonicalUuid,
) {
    let snapshot = bus.task_snapshot().expect("successor heads read");
    let successor = snapshot
        .facts()
        .occupant_address(lane, successor_occupant)
        .expect("the successor's birth is recorded");
    assert_ne!(successor, ancestor);
    let resolved = bus.resolve_occupant(successor).expect("resolver answers");
    assert_eq!(resolved.map(|birth| birth.occupant), Ok(successor_occupant));
    let historical = bus.occupant_birth(ancestor).expect("resolver answers");
    let historical = historical.expect("the ancestor birth stays proved");
    assert_eq!(
        (historical.lane, historical.occupant),
        (lane, ancestor_occupant)
    );
    let expired = bus.resolve_occupant(ancestor).expect("resolver answers");
    assert_eq!(
        expired,
        Err(bus::AddressRefusal::Expired { birth: historical })
    );
}

#[allow(
    clippy::too_many_arguments,
    reason = "the fixture seeds one exact claim and residency boundary"
)]
fn seed_restart_claim(
    bus: &Bus,
    task: &wire::TaskStamp,
    durable: &wire::LaneResidency,
    old_epoch: CanonicalUuid,
    new_epoch: CanonicalUuid,
    start: wire::Instant,
    expiry: wire::Instant,
    maximum: wire::Instant,
) {
    seed_task_claim(bus, task, durable, old_epoch, start, expiry, maximum);
    seed_restart_residency(bus, durable, new_epoch, start);
}

pub(in crate::resident) fn seed_task_claim(
    bus: &Bus,
    task: &wire::TaskStamp,
    durable: &wire::LaneResidency,
    old_epoch: CanonicalUuid,
    start: wire::Instant,
    expiry: wire::Instant,
    maximum: wire::Instant,
) {
    let phase: wire::Phase = "todo".parse().expect("fixture phase");
    let origin: wire::Origin = format!("task:{task}").parse().expect("fixture origin");
    let actor = Actor::Lane(durable.lane);
    let mut write = JournalWrite::new(actor.clone(), origin.clone(), start, Producer::CommandDoor);
    let (added, won) =
        append_seed_claim_facts(&mut write, &phase, &actor, task, durable, old_epoch, expiry);
    let statement_task = task.clone();
    let statement_phase = phase.clone();
    write.task_head(added, move |position| wire::TaskHead {
        born: statement_task,
        queue_origin: position,
        title: "Restore claimed work".to_owned(),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        project: "task.claim".to_owned(),
        tags: Vec::new(),
        dependencies: Vec::new(),
        provenance: origin,
        creation: wire::body::TaskCreationContext::unavailable(
            wire::body::TaskCreationUnavailable::SpiceActor,
        ),
        revision: position,
    });
    let workflow_task = task.clone();
    write.workflow_head(workflow_task, added, move |_| wire::WorkflowHead {
        flow: vec![statement_phase.clone()],
        phase: statement_phase,
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
        generation: 0,
    });
    let claim_task = task.clone();
    let lane = durable.lane;
    let occupant = durable.occupant.expect("durable occupant");
    write.claim_head(claim_task, won, move |position| wire::ClaimHead {
        actor,
        fact: position,
        claimed: position,
        cell: wire::ClaimCell {
            project: "task.claim".to_owned(),
            phase,
        },
        lane,
        occupant,
        epoch: old_epoch,
        recovery: None,
        disposition: wire::ClaimDisposition::Active,
        acknowledged: 0,
    });
    let lease_task = task.clone();
    write.lease_head(lease_task, won, move |_| wire::LeaseHead {
        start,
        renewal: start,
        expiry,
        maximum,
        epoch: old_epoch,
    });
    assert!(matches!(
        bus.commit(write).expect("fixture task authority commits"),
        bus::CommitOutcome::Committed(_)
    ));
}

fn append_seed_claim_facts(
    write: &mut JournalWrite,
    phase: &wire::Phase,
    actor: &Actor,
    task: &wire::TaskStamp,
    durable: &wire::LaneResidency,
    old_epoch: CanonicalUuid,
    expiry: wire::Instant,
) -> (PendingFact, PendingFact) {
    let added = write
        .fact(
            wire::body::TaskAdded {
                task: task.clone(),
                title: "Restore claimed work".to_owned(),
                description: None,
                acceptance: Vec::new(),
                requirements: wire::EnvironmentRequirements::default(),
                project: "task.claim".to_owned(),
                tags: Vec::new(),
                depends: Vec::new(),
                flow: vec![phase.clone()],
                phase: phase.clone(),
                creation: wire::body::TaskCreationContext::unavailable(
                    wire::body::TaskCreationUnavailable::SpiceActor,
                ),
                revision: 1,
            },
            FactCause::Root,
        )
        .expect("task fact builds");
    let won = write
        .fact(
            wire::body::ClaimWon {
                task: task.clone(),
                actor: actor.clone(),
                lane: durable.lane,
                occupant: durable.occupant.expect("durable occupant"),
                epoch: old_epoch,
                cell: Some(wire::ClaimCell {
                    project: "task.claim".to_owned(),
                    phase: phase.clone(),
                }),
                until: expiry,
                git: None,
                recovery: None,
                source: wire::body::SourceBoundary::Proven { heads: Vec::new() },
            },
            FactCause::Pending(added),
        )
        .expect("claim fact builds");
    (added, won)
}

fn seed_restart_residency(
    bus: &Bus,
    durable: &wire::LaneResidency,
    new_epoch: CanonicalUuid,
    start: wire::Instant,
) {
    let resident_origin: wire::Origin = format!("resident:{new_epoch}")
        .parse()
        .expect("resident origin");
    let mut resident =
        JournalWrite::new(Actor::Spice, resident_origin, start, Producer::CommandDoor);
    let seated = resident
        .fact(wire::body::UpSeated { epoch: new_epoch }, FactCause::Root)
        .expect("resident fact builds");
    let row = durable.clone();
    resident.residency_head(seated, move |_| wire::ResidencyHead {
        epoch: new_epoch,
        state: wire::ResidentState::Seated,
        topology_revision: 1,
        lanes: vec![row],
        subscriptions: Vec::new(),
        renewals: Vec::new(),
    });
    assert!(matches!(
        bus.commit(resident).expect("fixture residency commits"),
        bus::CommitOutcome::Committed(_)
    ));
}
