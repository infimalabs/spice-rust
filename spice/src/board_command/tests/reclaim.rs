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
    reclaim_at_for(
        bus,
        epoch,
        lane,
        held_task,
        expected,
        at,
        spice::claim_lease::DEFAULT_RECLAIM_SECONDS,
    )
}

fn reclaim_at_for(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: &Actor,
    held_task: &TaskHandle,
    expected: Vec<Witness>,
    at: Instant,
    lease_seconds: u64,
) -> Outcome<CommandAnswer> {
    let renew = ClaimRenew {
        task: held_task.clone(),
        lease_seconds,
    };
    let command = fixture_command(
        epoch,
        lane.clone(),
        CommandKind::ClaimRenew,
        expected,
        &renew,
    );
    claim_reclaim_with_clock(bus, &command, &renew, || Ok(at))
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

/// Explicitly reclaim `task` one second before its lease would expire.
///
/// The injected instant keeps the renewal deterministically extending, so a
/// scenario acknowledging a changed task never races the lease clock.
pub(super) fn reclaim_held(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: &Actor,
    seat: &Seat,
    task: &TaskHandle,
) {
    let held = board(bus, epoch, lane.clone());
    let expiry = at(&held, task)
        .lease
        .as_ref()
        .expect("active lease")
        .body
        .expiry;
    assert_committed(reclaim_at(
        bus,
        epoch,
        lane,
        task,
        active_witnesses(&held, task, seat),
        fixture_instant_after(expiry, ONE_SECOND_BEFORE_EXPIRY),
    ));
}

/// The holder's own change left `before` in place: the same claim, current,
/// with only its acknowledgement following the task generation.
pub(super) fn assert_holder_kept(heads: &BoardTask, before: &ClaimHead) {
    let mut kept = before.clone();
    kept.acknowledged = heads.workflow.body.generation;
    assert!(
        kept.acknowledged > before.acknowledged,
        "the holder's change advanced the task"
    );
    assert_eq!(heads.claim.as_ref().map(|claim| &claim.body), Some(&kept));
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

pub(super) fn free_active(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: &Actor,
    seat: &Seat,
    task: &TaskHandle,
) {
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
fn client_reclaim_without_handle_routes_a_missing_claim_through_status() {
    let (root, bus, epoch, seats) = fixture("reclaim-client-no-handle", 1);
    let lane = Actor::Lane(seats[0].lane);
    let available = board(&bus, epoch, lane);

    assert_authority_recovery(
        reclaim_active(&available).expect_err("the lane has no active claim"),
        "claim::active-missing",
    );
    clean(&root, bus);
}

#[test]
fn reclaim_refuses_residency_outside_the_claim_team_without_mutation() {
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

#[test]
fn expired_claim_refuses_reclaim_without_mutation() {
    let (root, bus, epoch, seats) = fixture("reclaim-expired", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "RECLAIM-exp01",
        "Refuse an expired claim",
        Some("todo"),
    );
    claim_next(&bus, epoch, &lane, &seats[0], &task);
    let before = board(&bus, epoch, lane.clone());
    let expiry = at(&before, &task)
        .lease
        .as_ref()
        .expect("active lease")
        .body
        .expiry;
    let before_facts = bus
        .fact_snapshot()
        .expect("facts before expired reclaim")
        .facts()
        .len();

    assert_authority_recovery(
        reclaim_at(
            &bus,
            epoch,
            &lane,
            &task,
            active_witnesses(&before, &task, &seats[0]),
            expiry,
        ),
        "claim::active-refused",
    );

    assert_eq!(board(&bus, epoch, lane), before);
    assert_eq!(
        bus.fact_snapshot()
            .expect("facts after expired reclaim")
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
            cause: None,
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

    assert_committed(reclaim_at_for(
        &bus,
        epoch,
        &lane,
        &task,
        active_witnesses(&before, &task, &seats[0]),
        reclaimed_at,
        90 * 60,
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
        reclaim_at_for(
            &bus,
            epoch,
            &lane,
            &task,
            active_witnesses(&after, &task, &seats[0]),
            reclaimed_at,
            90 * 60,
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
fn repeated_explicit_reclaims_roll_the_same_claim_past_its_original_maximum() {
    let (root, bus, epoch, seats) = fixture("reclaim-rolling-maximum", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "RECLAIM-roll01",
        "Roll one held claim without ownership churn",
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
    let first_at = fixture_instant_after(first_lease.expiry, ONE_SECOND_BEFORE_EXPIRY);
    assert_committed(reclaim_at(
        &bus,
        epoch,
        &lane,
        &task,
        active_witnesses(&first, &task, &seats[0]),
        first_at,
    ));
    let once = board(&bus, epoch, lane.clone());
    let once_heads = at(&once, &task);
    let once_lease = once_heads.lease.as_ref().expect("first renewed lease");
    assert_eq!(once_heads.claim, first_heads.claim);
    assert_eq!(
        once_lease.body.expiry,
        fixture_instant_after(first_at, spice::claim_lease::DEFAULT_RECLAIM_SECONDS as i64)
    );
    assert_eq!(once_lease.body.maximum, first_lease.maximum);

    let second_at = fixture_instant_after(first_at, 2);
    assert!(second_at < once_lease.body.expiry);
    assert_committed(reclaim_at(
        &bus,
        epoch,
        &lane,
        &task,
        active_witnesses(&once, &task, &seats[0]),
        second_at,
    ));
    let twice = board(&bus, epoch, lane.clone());
    let twice_heads = at(&twice, &task);
    let twice_lease = twice_heads.lease.as_ref().expect("second renewed lease");
    assert_eq!(twice_heads.claim, first_heads.claim);
    assert_eq!(
        twice_lease.body.expiry,
        fixture_instant_after(
            second_at,
            spice::claim_lease::DEFAULT_RECLAIM_SECONDS as i64
        )
    );
    assert_eq!(twice_lease.body.maximum, twice_lease.body.expiry);
    assert!(twice_lease.body.maximum > first_lease.maximum);
    assert!(matches!(
        reclaim_at(
            &bus,
            epoch,
            &lane,
            &task,
            active_witnesses(&twice, &task, &seats[0]),
            second_at,
        ),
        Outcome::Refusal(Refusal::Bare { ref diagnostic })
            if diagnostic.reason() == "claim::lease-nonextending"
    ));
    let facts = bus.fact_snapshot().expect("rolling reclaim facts");
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
        2
    );
    clean(&root, bus);
}

#[test]
fn completion_and_review_wait_for_a_reclaim_after_an_operator_change() {
    let (root, bus, epoch, seats) = fixture("reclaim-generation", 2);
    let author = Actor::Lane(seats[0].lane);
    let reviewer = Actor::Lane(seats[1].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "RECLAIM-gen01",
        "Finish only what the operator last described",
        Some("todo,review"),
    );

    claim_next(&bus, epoch, &author, &seats[0], &task);
    rewrite_description(&bus, epoch, &task, "the operator narrowed the scope");
    let completion = |bus: &Bus| {
        let held = board(bus, epoch, author.clone());
        issue(
            bus,
            epoch,
            author.clone(),
            CommandKind::TaskDone,
            progress_witnesses(
                &held,
                at(&held, &task),
                seats[0].lane,
                seats[0].occupant,
                false,
            )
            .expect("held task has progress witnesses"),
            &TaskDone {
                task: task.clone(),
                validation: "implementation and focused checks complete".to_owned(),
                disposition: None,
                operation: Some(completion_operation()),
            },
        )
    };
    assert_stale(completion(&bus), &task);
    reclaim_held(&bus, epoch, &author, &seats[0], &task);
    assert_committed(completion(&bus));

    claim_next(&bus, epoch, &reviewer, &seats[1], &task);
    rewrite_description(&bus, epoch, &task, "the operator added a review concern");
    let verdict = |bus: &Bus| {
        let assigned = board(bus, epoch, reviewer.clone());
        issue(
            bus,
            epoch,
            reviewer.clone(),
            CommandKind::TaskReview,
            progress_witnesses(
                &assigned,
                at(&assigned, &task),
                seats[1].lane,
                seats[1].occupant,
                true,
            )
            .expect("assigned review has exact witnesses"),
            &TaskReview {
                task: task.clone(),
                finding: "clean".to_owned(),
                note: Some("reviewed against the current statement".to_owned()),
                then: Vec::new(),
                followup: Vec::new(),
            },
        )
    };
    assert_stale(verdict(&bus), &task);
    reclaim_held(&bus, epoch, &reviewer, &seats[1], &task);
    assert_committed(verdict(&bus));
    clean(&root, bus);
}

/// The operator rewrites the held task's description: a statement change its
/// holder must acknowledge.
fn rewrite_description(bus: &Bus, epoch: CanonicalUuid, task: &TaskHandle, description: &str) {
    assert_committed(super::edit::correct(
        bus,
        epoch,
        Actor::Operator,
        &TaskEdit {
            description: Some(description.to_owned()),
            ..super::edit::only(task)
        },
    ));
}

/// The refusal names the task and the reclaim that acknowledges it.
fn assert_stale(outcome: Outcome<CommandAnswer>, task: &TaskHandle) {
    let Outcome::Refusal(Refusal::Bare { diagnostic }) = outcome else {
        panic!("a stale claim was not refused: {outcome:?}");
    };
    assert_eq!(
        diagnostic.reason(),
        "claim::generation-stale",
        "{diagnostic:?}"
    );
    let detail = diagnostic.detail().expect("stale refusal guidance");
    assert!(detail.contains(&format!("spice task reclaim {task}")));
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
            &allocation_subscription(&after_restart.residency.body, lane)
                .expect("restarted lane subscription"),
            &after_restart.environment(),
        )
        .expect("allocator still has unclaimed work")
        .entry()
        .handle(),
        available_task,
        "the renewed claim remains excluded from allocation after restart",
    );
    clean(root, reopened);
}

/// Spice ices the held task's lease at the indefinite approval horizon the
/// way an unmet trust grant does: one statement edit under its own actor,
/// which leaves the holder stale, with the lease head moved to the horizon.
fn ice_held_lease(bus: &Bus, epoch: CanonicalUuid, task: &TaskHandle) {
    let held = board(bus, epoch, Actor::Operator);
    let heads = at(&held, task);
    let statement = super::edit::statement_head(bus, task);
    let born = statement.body.born.clone();
    let iced = spice::claim_lease::iced(&heads.lease.as_ref().expect("active lease").body);
    let mut write = JournalWrite::new(
        Actor::Spice,
        format!("task:{born}").parse().expect("task origin"),
        fixture_instant_after(iced.start, 1),
        Producer::CommandDoor,
    );
    let revision = u32::try_from(statement.position.get())
        .expect("fixture revision")
        .saturating_add(1);
    let edited = write
        .fact(
            body::TaskEdited {
                intent: None,
                task: born.clone(),
                revision,
                dependencies: None,
            },
            FactCause::Root,
        )
        .expect("ice edit builds");
    let statement = statement.body;
    write.task_head(edited, move |position| wire::TaskHead {
        revision: position,
        ..statement
    });
    write.lease_head(born, edited, move |_| iced);
    assert!(matches!(
        bus.commit(write).expect("iced lease commits"),
        CommitOutcome::Committed(_)
    ));
}

#[test]
fn horizon_reclaim_acknowledges_an_edit_and_keeps_the_hold() {
    let (root, bus, epoch, seats) = fixture("reclaim-horizon", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "RECLAIM-ice01",
        "Finish the iced task against the edited statement",
        Some("todo"),
    );
    claim_next(&bus, epoch, &lane, &seats[0], &task);
    ice_held_lease(&bus, epoch, &task);

    let iced = board(&bus, epoch, lane.clone());
    let heads = at(&iced, &task);
    let claim = heads.claim.as_ref().expect("active claim");
    let lease = heads.lease.as_ref().expect("iced lease");
    let horizon = wire::Instant::indefinite();
    assert_eq!(lease.body.expiry, horizon);
    assert!(claim.body.is_stale(&heads.workflow.body), "{heads:?}");

    let answer = assert_committed(reclaim_at(
        &bus,
        epoch,
        &lane,
        &task,
        active_witnesses(&iced, &task, &seats[0]),
        fixture_instant_after(lease.body.renewal, 2),
    ));
    let CommandAnswer::Claim(answer) = answer else {
        panic!("reclaim returned a non-claim answer")
    };
    assert_eq!(answer.expiry.as_deref(), Some(&horizon));
    assert_eq!(answer.acknowledged, Some(heads.workflow.body.generation));
    assert!(answer.held, "{answer:?}");

    let acknowledged = board(&bus, epoch, lane.clone());
    let after = at(&acknowledged, &task);
    let same_claim = after.claim.as_ref().expect("claim stays held");
    assert_eq!(
        same_claim.body,
        wire::ClaimHead {
            fact: same_claim.body.fact,
            acknowledged: heads.workflow.body.generation,
            ..claim.body.clone()
        }
    );
    assert!(!same_claim.body.is_stale(&after.workflow.body));
    let kept = after.lease.as_ref().expect("lease stays");
    assert_eq!(kept.body.expiry, horizon);
    assert_eq!(kept.body.maximum, horizon);
    assert_eq!(kept.body.epoch, lease.body.epoch);
    assert_eq!(after.workflow, heads.workflow);

    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::TaskDone,
        progress_witnesses(
            &acknowledged,
            after,
            seats[0].lane,
            seats[0].occupant,
            false,
        )
        .expect("held task has progress witnesses"),
        &TaskDone {
            task: task.clone(),
            validation: "implementation and focused checks complete".to_owned(),
            disposition: None,
            operation: Some(completion_operation()),
        },
    ));
    clean(&root, bus);
}

#[test]
fn ordinary_reclaim_that_does_not_pass_the_expiry_is_refused_with_it() {
    let (root, bus, epoch, seats) = fixture("reclaim-nonextending", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "RECLAIM-short1",
        "Keep a short request from shortening the lease",
        Some("todo"),
    );
    claim_next(&bus, epoch, &lane, &seats[0], &task);
    reclaim_held(&bus, epoch, &lane, &seats[0], &task);

    let held = board(&bus, epoch, lane.clone());
    let lease = at(&held, &task).lease.as_ref().expect("one-hour lease");
    assert_eq!(
        lease.body.expiry,
        fixture_instant_after(
            lease.body.renewal,
            spice::claim_lease::DEFAULT_RECLAIM_SECONDS as i64
        )
    );
    let outcome = reclaim_at_for(
        &bus,
        epoch,
        &lane,
        &task,
        active_witnesses(&held, &task, &seats[0]),
        fixture_instant_after(lease.body.renewal, 1),
        1,
    );
    let Outcome::Refusal(Refusal::Bare { diagnostic }) = outcome else {
        panic!("a nonextending reclaim was admitted: {outcome:?}");
    };
    assert_eq!(diagnostic.reason(), "claim::lease-nonextending");
    assert_eq!(
        diagnostic.observed(),
        Some(lease.body.expiry.to_string().as_str())
    );
    assert_eq!(
        at(&board(&bus, epoch, lane), &task).lease,
        Some(lease.clone())
    );
    clean(&root, bus);
}
