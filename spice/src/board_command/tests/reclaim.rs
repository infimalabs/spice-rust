use super::*;

const ONE_SECOND_BEFORE_EXPIRY: i64 = -1;

fn reclaim_at(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: &Actor,
    held_task: &TaskHandle,
    expected: Vec<Witness>,
    at: Instant,
) -> Outcome<CommandAnswer> {
    let renew = ClaimRenew {
        task: held_task.clone(),
        lease_seconds: 90 * 60,
    };
    let command = fixture_command(
        epoch,
        lane.clone(),
        CommandKind::ClaimRenew,
        expected,
        &renew,
    );
    claim_reclaim_with_clock(
        bus,
        &command,
        &renew,
        &guard_projects(),
        &board::NextConfig::default(),
        || Ok(at),
    )
}

pub(super) fn renew_and_assert(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: &Actor,
    held_task: &TaskHandle,
    before: &BoardTask,
    expected: Vec<Witness>,
) -> BoardTask {
    let lease = before.lease.as_ref().expect("active lease");
    let first_at = fixture_instant_after(lease.body.start, 1);
    let answer = assert_committed(reclaim_at(
        bus,
        epoch,
        lane,
        held_task,
        expected.clone(),
        first_at,
    ));

    let renewed = board(bus, epoch, lane.clone());
    let after = at(&renewed, held_task).clone();
    assert_eq!(after.statement, before.statement);
    assert_eq!(after.workflow, before.workflow);
    assert_eq!(after.claim, before.claim);
    let renewed_lease = after.lease.as_ref().expect("renewed lease");
    let CommandAnswer::Claim(answer) = answer else {
        panic!("reclaim returned a non-claim answer")
    };
    assert_eq!(answer.expiry.as_deref(), Some(&renewed_lease.body.expiry));
    assert_ne!(renewed_lease.position, lease.position);
    assert_eq!(renewed_lease.body.maximum, lease.body.maximum);
    assert!(renewed_lease.body.expiry > lease.body.expiry);
    assert!(renewed_lease.body.expiry <= renewed_lease.body.maximum);
    let mut current = expected.clone();
    for witness in &mut current {
        if let Witness::Lease { head, .. } = witness {
            *head = renewed_lease.position;
        }
    }
    assert!(matches!(
        reclaim_at(bus, epoch, lane, held_task, current, first_at),
        Outcome::Refusal(Refusal::Bare { ref diagnostic })
            if diagnostic.reason() == "claim::lease-nonextending"
    ));
    assert!(matches!(
        reclaim_at(
            bus,
            epoch,
            lane,
            held_task,
            expected,
            fixture_instant_after(first_at, 1),
        ),
        Outcome::Refusal(ref refusal) if refusal.reason() == "board::witness-moved"
    ));
    assert!(
        bus.fact_snapshot()
            .expect("reclaim facts read")
            .facts()
            .iter()
            .any(|fact| fact.fact.kind() == wire::FactKind::ClaimRenewed)
    );
    after
}

fn claim_next(bus: &Bus, epoch: CanonicalUuid, lane: &Actor, seat: &Seat, task: &TaskHandle) {
    let available = board(bus, epoch, lane.clone());
    assert_committed(issue(
        bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        claim_expected(&available, task, seat),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
            recovery: None,
        },
    ));
}

fn active_witnesses(answer: &BoardAnswer, task: &TaskHandle, seat: &Seat) -> Vec<Witness> {
    let heads = at(answer, task);
    vec![
        Witness::Claim {
            task: heads.statement.body.born.clone(),
            head: Some(heads.claim.as_ref().expect("active claim").position),
        },
        Witness::Lease {
            task: heads.statement.body.born.clone(),
            head: heads.lease.as_ref().expect("active lease").position,
        },
        residency(answer, seat),
    ]
}

fn free_active(bus: &Bus, epoch: CanonicalUuid, lane: &Actor, seat: &Seat, task: &TaskHandle) {
    let active = board(bus, epoch, lane.clone());
    assert_committed(issue(
        bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimFree,
        active_witnesses(&active, task, seat),
        &TaskRef { task: task.clone() },
    ));
}

fn assert_authority_recovery(outcome: Outcome<CommandAnswer>, reason: &str) {
    let Outcome::Refusal(Refusal::Repairable {
        command,
        diagnostic,
    }) = outcome
    else {
        panic!("lost reclaim did not return repairable guidance: {outcome:?}");
    };
    assert_eq!(diagnostic.reason(), reason);
    assert_eq!(command.arguments(), ["task", "status"]);
    let detail = diagnostic.detail().expect("recovery detail is present");
    assert!(detail.contains("spice task next"));
    assert!(!detail.contains("spice task reclaim"));
}

#[test]
fn client_reclaim_preflight_routes_missing_inactive_and_foreign_claims_through_status() {
    let (root, bus, epoch, seats) = fixture("reclaim-client-authority", 2);
    let owner = Actor::Lane(seats[0].lane);
    let peer = Actor::Lane(seats[1].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "RECLAIM-client",
        "Read authoritative ownership before reclaim",
        Some("todo"),
    );
    let available = board(&bus, epoch, owner.clone());
    assert_authority_recovery(
        reclaim_claim(at(&available, &task), &task, &owner).unwrap_err(),
        "claim::head-missing",
    );

    claim_next(&bus, epoch, &owner, &seats[0], &task);
    let foreign = board(&bus, epoch, peer.clone());
    assert_authority_recovery(
        reclaim_claim(at(&foreign, &task), &task, &peer).unwrap_err(),
        "claim::ownership-refused",
    );

    free_active(&bus, epoch, &owner, &seats[0], &task);
    let inactive = board(&bus, epoch, owner.clone());
    assert_authority_recovery(
        reclaim_claim(at(&inactive, &task), &task, &owner).unwrap_err(),
        "claim::ownership-refused",
    );
    clean(&root, bus);
}

#[test]
fn reclaim_refuses_residency_outside_the_claim_lineage_without_mutation() {
    let (root, bus, epoch, seats) = fixture("reclaim-moved-residency", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "RECLAIM-moved1",
        "Refuse stale reclaim authority",
        Some("todo"),
    );
    claim_next(&bus, epoch, &lane, &seats[0], &task);
    let claimed = board(&bus, epoch, lane.clone());
    let mut residency = claimed.residency.body.clone();
    let successor = uuid(9_001);
    let row = residency.lanes.first_mut().expect("fixture lane exists");
    row.occupant = Some(successor);
    row.lineage = vec![successor];
    let mut write = JournalWrite::new(
        Actor::Spice,
        format!("resident:{epoch}")
            .parse()
            .expect("resident origin"),
        fixture_instant_after(
            at(&claimed, &task)
                .lease
                .as_ref()
                .expect("claim lease")
                .body
                .start,
            1,
        ),
        Producer::CommandDoor,
    );
    let seated = write
        .fact(body::UpSeated { epoch }, FactCause::Root)
        .expect("successor residency fact builds");
    write.residency_head(seated, move |_| residency);
    assert!(matches!(
        bus.commit(write).expect("successor residency commits"),
        CommitOutcome::Committed(_)
    ));

    let moved = board(&bus, epoch, lane.clone());
    let heads = at(&moved, &task);
    let before_facts = bus
        .fact_snapshot()
        .expect("facts before refusal")
        .facts()
        .len();
    let expected = vec![
        Witness::Claim {
            task: heads.statement.body.born.clone(),
            head: Some(heads.claim.as_ref().expect("active claim").position),
        },
        Witness::Lease {
            task: heads.statement.body.born.clone(),
            head: heads.lease.as_ref().expect("active lease").position,
        },
        residency_witness(&moved, seats[0].lane, successor),
    ];
    let reclaim_at_time =
        fixture_instant_after(heads.lease.as_ref().expect("active lease").body.start, 2);
    assert_authority_recovery(
        reclaim_at(&bus, epoch, &lane, &task, expected, reclaim_at_time),
        "claim::residency-moved",
    );

    let after = board(&bus, epoch, lane);
    assert_eq!(at(&after, &task), heads);
    assert_eq!(
        bus.fact_snapshot()
            .expect("facts after refusal")
            .facts()
            .len(),
        before_facts
    );
    clean(&root, bus);
}

fn claim_with_legacy_automatic_lease(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: &Actor,
    seat: &Seat,
    task: &TaskHandle,
) -> Instant {
    let available = board(bus, epoch, lane.clone());
    let heads = at(&available, task);
    let started = fixture_instant("automatic claim start");
    let expiry = fixture_instant_after(started, AUTOMATIC_PROVISIONAL_SECONDS);
    let mut write = JournalWrite::new(
        lane.clone(),
        format!("task:{task}").parse().expect("fixture task origin"),
        started,
        Producer::CommandDoor,
    );
    board::allocate::mint(
        &mut write,
        board::allocate::Mint {
            task: &heads.statement.body.born,
            actor: lane,
            workflow: &heads.workflow.body,
            project: &heads.statement.body.project,
            lane: seat.lane,
            occupant: seat.occupant,
            epoch,
            at: started,
            expiry,
            maximum: expiry,
            git: None,
            recovery: None,
            source: body::SourceBoundary::Proven { heads: Vec::new() },
        },
    )
    .expect("legacy automatic claim mints");
    assert!(matches!(
        bus.commit(write).expect("automatic claim commits"),
        CommitOutcome::Committed(_)
    ));
    started
}

#[test]
fn maximum_bound_automatic_claim_reclaims_in_place_for_one_hour() {
    let (root, bus, epoch, seats) = fixture("reclaim-automatic-maximum", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "RECLAIM-auto01",
        "Promote one healthy automatic claim",
        Some("todo"),
    );
    let started = claim_with_legacy_automatic_lease(&bus, epoch, &lane, &seats[0], &task);
    let before = board(&bus, epoch, lane.clone());
    let before_heads = at(&before, &task);
    let before_lease = before_heads.lease.as_ref().expect("automatic lease");
    assert_eq!(before_lease.body.expiry, before_lease.body.maximum);
    let reclaimed_at = fixture_instant_after(started, 1);

    assert_committed(reclaim_at(
        &bus,
        epoch,
        &lane,
        &task,
        active_witnesses(&before, &task, &seats[0]),
        reclaimed_at,
    ));

    let after = board(&bus, epoch, lane.clone());
    let after_heads = at(&after, &task);
    let after_lease = after_heads.lease.as_ref().expect("reclaimed lease");
    assert_eq!(after_heads.statement, before_heads.statement);
    assert_eq!(after_heads.workflow, before_heads.workflow);
    assert_eq!(after_heads.claim, before_heads.claim);
    assert_eq!(after_lease.body.expiry, after_lease.body.maximum);
    assert_eq!(
        after_lease.body.maximum,
        fixture_instant_after(reclaimed_at, AUTOMATIC_MAXIMUM_SECONDS)
    );
    assert!(matches!(
        reclaim_at(
            &bus,
            epoch,
            &lane,
            &task,
            active_witnesses(&after, &task, &seats[0]),
            reclaimed_at,
        ),
        Outcome::Refusal(Refusal::Bare { ref diagnostic })
            if diagnostic.reason() == "claim::lease-nonextending"
    ));
    let facts = bus.fact_snapshot().expect("automatic reclaim facts");
    assert_eq!(
        facts
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == wire::FactKind::ClaimWon)
            .count(),
        1
    );
    assert_eq!(
        facts
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == wire::FactKind::ClaimFreed)
            .count(),
        0
    );
    assert_eq!(
        facts
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == wire::FactKind::ClaimRenewed)
            .count(),
        1
    );
    clean(&root, bus);
}

#[test]
fn free_and_next_mints_a_fresh_durable_maximum() {
    let (root, bus, epoch, seats) = fixture("reclaim-maximum-reset", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "RECLAIM-reset1",
        "Reset one exhausted renewal maximum",
        Some("todo"),
    );
    claim_next(&bus, epoch, &lane, &seats[0], &task);
    let first = board(&bus, epoch, lane.clone());
    let first_heads = at(&first, &task);
    let first_lease = first_heads
        .lease
        .as_ref()
        .expect("first lease")
        .body
        .clone();
    let near_expiry = fixture_instant_after(first_lease.expiry, ONE_SECOND_BEFORE_EXPIRY);
    assert_committed(reclaim_at(
        &bus,
        epoch,
        &lane,
        &task,
        active_witnesses(&first, &task, &seats[0]),
        near_expiry,
    ));
    let maximum_bound = board(&bus, epoch, lane.clone());
    let maximum_heads = at(&maximum_bound, &task);
    let maximum_lease = maximum_heads.lease.as_ref().expect("maximum-bound lease");
    assert_eq!(maximum_lease.body.expiry, maximum_lease.body.maximum);
    let refused = reclaim_at(
        &bus,
        epoch,
        &lane,
        &task,
        active_witnesses(&maximum_bound, &task, &seats[0]),
        fixture_instant_after(near_expiry, 1),
    );
    assert!(matches!(
        refused,
        Outcome::Refusal(Refusal::Repairable { ref command, ref diagnostic })
            if diagnostic.reason() == "claim::lease-nonextending"
                && command.arguments() == ["task", "free", task.as_str()]
                && diagnostic.detail().is_some_and(|detail| detail.contains("spice task next"))
    ));
    free_active(&bus, epoch, &lane, &seats[0], &task);
    claim_next(&bus, epoch, &lane, &seats[0], &task);
    let reacquired = board(&bus, epoch, lane);
    let second = at(&reacquired, &task)
        .lease
        .as_ref()
        .expect("reacquired lease");

    assert_ne!(second.body.start, first_lease.start);
    assert_ne!(second.body.maximum, first_lease.maximum);
    assert_eq!(
        second
            .body
            .maximum
            .get()
            .duration_since(second.body.start.get()),
        first_lease
            .maximum
            .get()
            .duration_since(first_lease.start.get()),
    );
    clean(&root, bus);
}

pub(super) fn assert_reclaim_survives_restart(
    root: &Path,
    bus: Bus,
    epoch: CanonicalUuid,
    lane: &Actor,
    held_task: &TaskHandle,
    available_task: &TaskHandle,
    after: &BoardTask,
) {
    bus.verify().expect("reclaimed authority verifies");
    bus.shutdown().expect("reclaim fixture bus stops");
    let reopened =
        journal::Journal::open(root.join("authority.sqlite3")).expect("reclaim journal reopens");
    let reopened = Bus::start(reopened).expect("reclaim bus restarts");
    let after_restart = board(&reopened, epoch, lane.clone());
    assert_eq!(at(&after_restart, held_task), after);
    let entries = actor_entries(&after_restart).expect("restarted board validates");
    assert_eq!(
        board::select(
            &entries,
            lane,
            allocation_subscription(&after_restart.residency.body, lane)
                .expect("restarted lane subscription"),
            after_restart.allocation_band,
            after_restart.recent_cell.as_ref(),
        )
        .expect("allocator still has unclaimed work")
        .entry()
        .handle(),
        available_task,
        "the renewed claim remains excluded from allocation after restart",
    );
    clean(root, reopened);
}
