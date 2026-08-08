// The allocator's re-read guard refuses three separate ways, and one scenario
// that moves everything at once cannot tell them apart. Each test in this file
// moves only what a single comparison reads, so severing that comparison alone
// reds only the tests that read it. Measured by severing each in turn, over all
// seven tests here:
//
// Sites here are named rather than numbered: a line number retargets silently
// whenever anything above it moves, while a name that goes stale stops
// resolving instead of resolving to something else. In order,
// `guard_allocation` runs `witnesses_hold`, then refuses with
// `allocator subscription head changed`, `allocator head set changed`, and
// `allocator winner changed`. Each of those three strings is asserted below, so
// renaming one reds a test here rather than quietly orphaning a citation.
//
// | Severed in `guard_allocation` | Reds | Stays green |
// | --- | --- | --- |
// | `witnesses_hold` | `a_moved_witness_...` | other 6 |
// | residency | `..._residency_head_moved_...`, `a_reseat_between_...` | other 5 |
// | head set | `..._task_added_after_...`, `a_task_added_between_...` | other 5 |
// | winner | `..._winner_that_was_never_...` | other 6 |
//
// The last row is why no `..._between_the_claim_plan_...` test claims the
// winner comparison. `AllocationHeads` is a tuple of journal positions, so an
// unmoved head set means identical entries and -- `board::select` reading no
// clock -- an identical winner, and the head-set comparison refuses ahead of it
// either way. Only a hand-built observation reaches the winner comparison.
//
// One more severing separates the two kinds of test here. Making `commit_claim`
// re-read the board instead of consuming the snapshot its plan decided against
// reds both `..._between_the_claim_plan_...` tests and leaves all four
// hand-built ones green: an observation this file builds itself cannot notice
// that production stopped carrying its own. That is the coverage the end-to-end
// drives own, and it is also the argument the seam exists for.
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

/// Pins the head-set comparison in `read_guard.rs` on its own.
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

/// Pins the residency comparison in `read_guard.rs` on its own.
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

/// Pins the winner comparison in `read_guard.rs` on its own.
///
/// Nothing moves here, because nothing that moves can reach this comparison.
/// `AllocationHeads` is a tuple of journal positions, so an unmoved head set
/// means identical facts, identical entries, and -- `board::select` reading no
/// clock -- an identical winner. The comparison can therefore only refuse an
/// observation whose winner was never the selection, which the allocator does
/// not produce. It is a backstop, and this pins it as one. Keeping it was
/// decided rather than inherited; `guard_allocation` carries the reason and the
/// one change that would make it reachable, and this test is the only proof it
/// works, so it stays as long as the comparison does.
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

// The tests above hand `guard_allocation` an observation built by this file.
// The ones below build no observation at all: they run the production claim
// path, `claim_plan` then `commit_claim`, which is the one sequence `claim_win`
// runs, and let a competing write land between those two steps. That ordering
// is ordinary sequential code -- no sleep, no thread, no clock -- so the write
// arrives late on purpose on every run.
//
// Every refusal below is asserted by its exact reason rather than by refusing
// at all. `committed` flattens every guard refusal to the single outer reason
// `board::witness-moved` and carries the comparison that actually refused in
// `observed`, and the client's own witnesses are checked by `witnesses_hold`
// ahead of all three comparisons, so a bare "it refused" assertion would pass
// without the guard ever reaching an allocator comparison.

/// The reason the guard itself refused, from inside the outer refusal.
fn guard_refusal(outcome: Outcome<CommandAnswer>) -> String {
    let Outcome::Refusal(Refusal {
        reason,
        observed: Some(observed),
        ..
    }) = outcome
    else {
        panic!("expected the claim commit to refuse, found {outcome:?}");
    };
    assert_eq!(
        reason, "board::witness-moved",
        "every guard refusal reaches the caller under this one outer reason"
    );
    observed
}

/// The `task next` command a lane sends for the winner it just read.
fn claim_next_command(
    epoch: CanonicalUuid,
    actor: &Actor,
    answer: &BoardAnswer,
    winner: &TaskHandle,
    seat: &Seat,
) -> wire::AdmittedCommand {
    fixture_command(
        epoch,
        actor.clone(),
        CommandKind::ClaimWin,
        claim_expected(answer, winner, seat),
        &next_rule(),
    )
}

fn next_rule() -> ClaimWin {
    ClaimWin {
        rule: "next".parse().expect("allocator rule"),
    }
}

/// Drive the production claim path up to the point it has decided.
///
/// The project policy and allocator config are the ones `dispatch` builds, so
/// the plan is the plan the ordinary command door would have made.
fn plan_claim(bus: &Bus, command: &wire::AdmittedCommand) -> ClaimPlan {
    match claim_plan(
        bus,
        command,
        &next_rule(),
        None,
        &guard_projects(),
        &board::NextConfig::default(),
    ) {
        Ok(plan) => plan,
        Err(outcome) => panic!("the claim plan did not decide: {outcome:?}"),
    }
}

fn commit_planned_claim(bus: &Bus, command: &wire::AdmittedCommand, plan: ClaimPlan) -> String {
    guard_refusal(commit_claim(
        bus,
        command,
        &next_rule(),
        &guard_projects(),
        plan,
    ))
}

/// Reaches the head-set comparison in `read_guard.rs` through the claim path.
///
/// The competing task is added through `dispatch`, the same door production
/// adds tasks through, after the claim is decided and before it is written. It
/// moves no head the command witnessed and does not reseat, so the guard passes
/// `617` and `621` and this is the comparison left to refuse.
#[test]
fn a_task_added_between_the_claim_plan_and_its_commit_refuses_the_claim() {
    let (root, bus, epoch, seats) = fixture("claim-plan-heads", 1);
    let actor = Actor::Lane(seats[0].lane);
    add(&bus, epoch, actor.clone(), "GUARD-1", "first", None);
    add(&bus, epoch, actor.clone(), "GUARD-2", "second", None);

    let answer = board(&bus, epoch, actor.clone());
    let winner = guard_winner(&answer, &actor);
    let command = claim_next_command(epoch, &actor, &answer, &winner, &seats[0]);
    let plan = plan_claim(&bus, &command);
    assert_eq!(
        plan.selected.statement().handle,
        winner,
        "the production plan decided the winner this command witnessed"
    );

    add(&bus, epoch, actor.clone(), "GUARD-3", "arrived late", None);

    let moved = board(&bus, epoch, actor.clone());
    assert_ne!(
        allocation_heads(&moved),
        allocation_heads(&answer),
        "the added task moved the head set the guard re-reads"
    );
    assert_eq!(
        moved.residency.position, answer.residency.position,
        "the added task must leave the residency head still, or the residency \
         comparison ahead of the head set would be the one refusing"
    );

    assert_eq!(
        commit_planned_claim(&bus, &command, plan),
        "board::witness-moved: allocator head set changed"
    );
    assert!(
        at(&board(&bus, epoch, actor), &winner).claim.is_none(),
        "the refused claim left no claim head behind"
    );
    clean(&root, bus);
}

/// Reaches the residency comparison in `read_guard.rs` through the claim path.
///
/// Re-seating rewrites the residency head over the same epoch, lane, occupant
/// and disposition, which is every field `Witness::Residency` pins, so the
/// command's own witnesses still hold at `617` and the moved head position is
/// only visible to this comparison.
#[test]
fn a_reseat_between_the_claim_plan_and_its_commit_refuses_the_claim() {
    let (root, bus, epoch, seats) = fixture("claim-plan-residency", 1);
    let actor = Actor::Lane(seats[0].lane);
    add(&bus, epoch, actor.clone(), "GUARD-1", "first", None);
    add(&bus, epoch, actor.clone(), "GUARD-2", "second", None);

    let answer = board(&bus, epoch, actor.clone());
    let winner = guard_winner(&answer, &actor);
    let command = claim_next_command(epoch, &actor, &answer, &winner, &seats[0]);
    let plan = plan_claim(&bus, &command);

    reseat(&bus, epoch, &seats);

    let moved = board(&bus, epoch, actor.clone());
    assert_ne!(
        moved.residency.position, answer.residency.position,
        "re-seating moved the residency head the guard re-reads"
    );
    assert_eq!(
        allocation_heads(&moved),
        allocation_heads(&answer),
        "re-seating must leave every task head still, or the head-set \
         comparison could not be the one this scenario passes through"
    );

    assert_eq!(
        commit_planned_claim(&bus, &command, plan),
        "board::witness-moved: allocator subscription head changed"
    );
    assert!(
        at(&board(&bus, epoch, actor), &winner).claim.is_none(),
        "the refused claim left no claim head behind"
    );
    clean(&root, bus);
}

/// Pins the premise the two assertions above depend on.
///
/// `witnesses_hold` in `read_guard.rs` runs before every allocator
/// comparison, so a claim whose own witnessed head moved refuses there and
/// never reaches one. Both scenarios above would still "refuse" if they landed
/// here instead, which is why neither of them settles for that.
#[test]
fn a_moved_witness_refuses_before_any_allocator_comparison_is_reached() {
    let (root, bus, epoch, seats) = fixture("claim-plan-witness", 1);
    let actor = Actor::Lane(seats[0].lane);
    add(&bus, epoch, actor.clone(), "GUARD-1", "first", None);
    add(&bus, epoch, actor.clone(), "GUARD-2", "second", None);

    let answer = board(&bus, epoch, actor.clone());
    let winner = guard_winner(&answer, &actor);
    let command = claim_next_command(epoch, &actor, &answer, &winner, &seats[0]);
    let plan = plan_claim(&bus, &command);

    assert_committed(issue(
        &bus,
        epoch,
        actor.clone(),
        CommandKind::TaskEdit,
        vec![statement_witness(at(&answer, &winner))],
        &TaskEdit {
            task: winner.clone(),
            title: Some("Retitled while the claim was in flight".to_owned()),
            description: None,
            acceptance: Vec::new(),
            priority: None,
            project: None,
        },
    ));

    let refusal = commit_planned_claim(&bus, &command, plan);
    assert!(
        refusal.contains(winner.as_str()),
        "the refusal names the client witness that moved, not a comparison: {refusal}"
    );
    assert!(
        !refusal.contains("allocator"),
        "the moved witness refuses ahead of every allocator comparison: {refusal}"
    );
    clean(&root, bus);
}
