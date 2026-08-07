// The allocator's re-read guard refuses three separate ways, and one scenario
// that moves everything at once cannot tell them apart. Each test below moves
// only what a single comparison reads, so severing that comparison alone reds
// exactly one of them. Measured by disabling each comparison in turn:
//
// | Severed comparison in `guard_allocation` | Reds | Stays green |
// | --- | --- | --- |
// | residency, `read_guard.rs:621` | `..._residency_head_moved_...` | other 3 |
// | head set, `read_guard.rs:639` | `..._task_added_after_...` | other 3 |
// | winner, `read_guard.rs:665` | `..._winner_that_was_never_...` | other 3 |
//
// The control that the guard is reached at all is separate from the control in
// this file: making the residency comparison refuse unconditionally reds 13
// production claim-path tests in this binary beyond the ones written here, so
// `guard_allocation` is wired into the claim path and not an orphan.
fn guard_projects() -> ProjectPolicy {
    ProjectPolicy::new(
        ["board".to_owned(), "rim".to_owned()],
        ["sandbox".to_owned()],
        dialect::PROJECT_MIN_DEPTH as u64,
        dialect::PROJECT_MAX_DEPTH as u64,
    )
    .expect("allocation guard test project policy")
}

/// The task the allocator would hand this actor from one snapshot.
///
/// This is the ranking itself, through the same `board::select` the guard calls,
/// so the winner is fixture setup rather than anything under test here.
fn guard_winner(answer: &BoardAnswer, actor: &Actor) -> TaskHandle {
    let entries = actor_entries(answer).expect("allocation guard board entries validate");
    let subscription = allocation_subscription(&answer.residency.body, actor)
        .expect("allocation guard board has one lineage subscription");
    board::select(&entries, actor, subscription, answer.allocation_band)
        .expect("allocation guard fixture selects a task")
        .entry()
        .statement()
        .handle
        .clone()
}

fn guard_observation(
    answer: &BoardAnswer,
    actor: &Actor,
    winner: TaskHandle,
    projects: &ProjectPolicy,
) -> AllocationObservation {
    AllocationObservation {
        heads: allocation_heads(answer),
        winner,
        actor: actor.clone(),
        subscription: allocation_subscription(&answer.residency.body, actor)
            .expect("allocation guard fixture lane carries a subscription")
            .clone(),
        residency_position: answer.residency.position,
        projects: projects.clone(),
        band: answer.allocation_band,
    }
}

/// Rewrite the residency head over the same seats.
///
/// The lanes and subscriptions are rebuilt identically, so the only thing that
/// moves is the head's position -- which is the single field the allocator's
/// residency comparison reads.
fn reseat(bus: &Bus, epoch: CanonicalUuid, seats: &[Seat]) {
    let lanes = seats
        .iter()
        .map(|seat| wire::LaneResidency {
            lane: seat.lane,
            lineage_id: seat.lane,
            name: None,
            worktree: "/fixture".to_owned(),
            life: "drive".to_owned(),
            vendor: "claude".parse().expect("reseat fixture vendor"),
            occupant: Some(seat.occupant),
            lineage: vec![seat.occupant],
            capabilities: vec![seat.occupant],
            session: None,
            disposition: "occupied".to_owned(),
        })
        .collect::<Vec<_>>();
    let subscriptions = lanes
        .iter()
        .map(|lane| wire::LineageSubscription {
            lineage_id: lane.lineage_id,
            policy: wire::intent::LineageLife::Drain,
            manual_projects: Vec::new(),
            auto_projects: Vec::new(),
        })
        .collect::<Vec<_>>();
    let origin = format!("resident:{epoch}")
        .parse()
        .expect("reseat fixture origin");
    let at = Instant::new(jiff::Timestamp::now()).expect("reseat fixture instant");
    let mut write = JournalWrite::new(Actor::Spice, origin, at, Producer::CommandDoor);
    let seated = write
        .fact(body::UpSeated { epoch }, FactCause::Root)
        .expect("reseat fact builds");
    write.residency_head(seated, move |_| ResidencyHead {
        epoch,
        state: ResidentState::Seated,
        lanes,
        subscriptions,
    });
    match bus.commit(write).expect("reseat reaches writer") {
        CommitOutcome::Committed(_) => {}
        outcome => panic!("reseat did not commit: {outcome:?}"),
    }
}

/// Commit a write carrying only the allocator's guard, so the refusal observed
/// is the guard's and not some other check on the claim path.
fn commit_guarded(
    bus: &Bus,
    epoch: CanonicalUuid,
    observed: AllocationObservation,
) -> CommitOutcome {
    let origin = format!("resident:{epoch}")
        .parse()
        .expect("allocation guard fixture origin");
    let at = Instant::new(jiff::Timestamp::now()).expect("allocation guard fixture instant");
    let mut write = JournalWrite::new(Actor::Spice, origin, at, Producer::CommandDoor);
    write
        .fact(body::UpSeated { epoch }, FactCause::Root)
        .expect("allocation guard fixture fact builds");
    guard_allocation(&mut write, Vec::new(), observed);
    bus.commit(write).expect("allocation guard write reaches writer")
}

fn refusal_reason(outcome: CommitOutcome) -> String {
    match outcome {
        CommitOutcome::Refused(refusal) => refusal.reason().to_owned(),
        other => panic!("expected the allocator guard to refuse, found {other:?}"),
    }
}

/// Positive control for the three refusal tests below.
///
/// Every one of them asserts that a guard *refuses*, which stays true if the
/// guard refuses everything. This pins the other side: with nothing moved
/// between the observation and the write, the same guard commits.
#[test]
fn an_unmoved_observation_lets_the_allocator_write_commit() {
    let (_root, bus, epoch, seats) = fixture("allocation-guard-quiet", 1);
    let actor = Actor::Lane(seats[0].lane);
    add(&bus, epoch, actor.clone(), "GUARD-1", "first", None);
    add(&bus, epoch, actor.clone(), "GUARD-2", "second", None);

    let projects = guard_projects();
    let answer = board(&bus, epoch, actor.clone());
    let winner = guard_winner(&answer, &actor);
    let observed = guard_observation(&answer, &actor, winner, &projects);

    assert!(
        matches!(
            commit_guarded(&bus, epoch, observed),
            CommitOutcome::Committed(_)
        ),
        "the allocator guard commits when nothing moved under it"
    );
}

/// Pins the head-set comparison at `read_guard.rs:639` on its own.
///
/// A task added after the observation moves the head set without moving the
/// residency head, so the residency comparison ahead of it passes and this is
/// the comparison that has to refuse.
#[test]
fn a_task_added_after_the_observation_refuses_the_allocator_write() {
    let (_root, bus, epoch, seats) = fixture("allocation-guard-heads", 1);
    let actor = Actor::Lane(seats[0].lane);
    add(&bus, epoch, actor.clone(), "GUARD-1", "first", None);
    add(&bus, epoch, actor.clone(), "GUARD-2", "second", None);

    let projects = guard_projects();
    let answer = board(&bus, epoch, actor.clone());
    let winner = guard_winner(&answer, &actor);
    let observed = guard_observation(&answer, &actor, winner.clone(), &projects);

    add(&bus, epoch, actor.clone(), "GUARD-3", "arrived late", None);

    let moved = board(&bus, epoch, actor.clone());
    assert_ne!(
        allocation_heads(&moved),
        observed.heads,
        "the added task moved the head set the guard re-reads"
    );
    assert_eq!(
        guard_winner(&moved, &actor),
        winner,
        "the added task must not move the winner, or this scenario would be \
         caught by the winner comparison instead of the head-set comparison"
    );

    assert_eq!(
        refusal_reason(commit_guarded(&bus, epoch, observed)),
        "board::witness-moved: allocator head set changed"
    );
}

/// Pins the residency comparison at `read_guard.rs:621` on its own.
///
/// Re-seating moves the residency head and touches no task head, so the two
/// comparisons after it would both pass and this one has to refuse.
#[test]
fn a_residency_head_moved_after_the_observation_refuses_the_allocator_write() {
    let (_root, bus, epoch, seats) = fixture("allocation-guard-residency", 1);
    let actor = Actor::Lane(seats[0].lane);
    add(&bus, epoch, actor.clone(), "GUARD-1", "first", None);
    add(&bus, epoch, actor.clone(), "GUARD-2", "second", None);

    let projects = guard_projects();
    let answer = board(&bus, epoch, actor.clone());
    let winner = guard_winner(&answer, &actor);
    let observed = guard_observation(&answer, &actor, winner, &projects);

    reseat(&bus, epoch, &seats);

    let moved = board(&bus, epoch, actor.clone());
    assert_ne!(
        moved.residency.position, observed.residency_position,
        "re-seating moved the residency head the guard re-reads"
    );
    assert_eq!(
        allocation_heads(&moved),
        observed.heads,
        "re-seating must leave every task head still, or the head-set \
         comparison would refuse before the residency comparison is reached"
    );

    assert_eq!(
        refusal_reason(commit_guarded(&bus, epoch, observed)),
        "board::witness-moved: allocator subscription head changed"
    );
}

/// Pins the winner comparison at `read_guard.rs:665` on its own.
///
/// Nothing moves here, because nothing that moves can reach this comparison.
/// `AllocationHeads` is a tuple of journal positions, so an unmoved head set
/// means identical facts, identical entries, and -- `board::select` reading no
/// clock -- an identical winner. The comparison can therefore only refuse an
/// observation whose winner was never the selection, which the allocator does
/// not produce. It is a backstop, and this pins it as one.
#[test]
fn a_winner_that_was_never_the_selection_refuses_the_allocator_write() {
    let (_root, bus, epoch, seats) = fixture("allocation-guard-winner", 1);
    let actor = Actor::Lane(seats[0].lane);
    add(&bus, epoch, actor.clone(), "GUARD-1", "first", None);
    add(&bus, epoch, actor.clone(), "GUARD-2", "second", None);

    let projects = guard_projects();
    let answer = board(&bus, epoch, actor.clone());
    let selected = guard_winner(&answer, &actor);
    let other = answer
        .tasks
        .iter()
        .map(|task| task.statement.body.handle.clone())
        .find(|handle| *handle != selected)
        .expect("the fixture holds a task the allocator did not select");
    let observed = guard_observation(&answer, &actor, other, &projects);

    assert_eq!(
        refusal_reason(commit_guarded(&bus, epoch, observed)),
        "board::witness-moved: allocator winner changed"
    );
}
