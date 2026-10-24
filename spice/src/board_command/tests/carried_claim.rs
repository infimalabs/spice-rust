use super::*;

fn phases(flow: &[&str]) -> Vec<wire::Phase> {
    flow.iter()
        .map(|phase| phase.parse().expect("fixture phase"))
        .collect()
}

fn advance(from: &str, to: &str) -> board::TransitionKind {
    board::TransitionKind::Advance {
        from: from.parse().expect("fixture phase"),
        to: to.parse().expect("fixture phase"),
    }
}

fn boundary(local_commits: u32) -> body::GitBoundary {
    body::GitBoundary {
        head: "lane-head".to_owned(),
        integrated: "integrated-head".to_owned(),
        upstream: None,
        upstream_head: None,
        local_commits,
    }
}

fn complete(bus: &Bus, epoch: CanonicalUuid, seat: &Seat, task: &TaskHandle) -> CommandAnswer {
    let lane = Actor::Lane(seat.lane);
    let held = board(bus, epoch, lane.clone());
    let expected = progress_witnesses(&held, at(&held, task), seat.lane, seat.occupant, false)
        .expect("completion witnesses");
    assert_committed(issue(
        bus,
        epoch,
        lane,
        CommandKind::TaskDone,
        expected,
        &TaskDone {
            task: task.clone(),
            validation: "phase evidence retained".to_owned(),
            disposition: None,
            operation: Some(completion_operation()),
        },
    ))
}

fn claim_win(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    task: &TaskHandle,
    rule: &str,
) -> Outcome<CommandAnswer> {
    let lane = Actor::Lane(seat.lane);
    let available = board(bus, epoch, lane.clone());
    let heads = at(&available, task);
    let rule: Name = rule.parse().expect("claim rule");
    issue(
        bus,
        epoch,
        lane,
        CommandKind::ClaimWin,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
            Witness::Eligibility { rule: rule.clone() },
            residency(&available, seat),
        ],
        &ClaimWin {
            rule,
            recovery: None,
        },
    )
}

#[test]
fn done_into_a_continuing_phase_carries_the_claim_to_its_lane() {
    let (root, bus, epoch, seats) = fixture("carried-claim", 2);
    let task = add_project_with_acceptance(
        &bus,
        epoch,
        "CARRY-1kCarry01",
        "Carry the claim across the plan phase",
        "board.test",
        Some("plan,todo,review"),
        &["the next phase has a holder"],
    );
    let holder = Actor::Lane(seats[0].lane);
    claim_task(&bus, epoch, &seats[0], &holder, &task);

    let CommandAnswer::Task(answer) = complete(&bus, epoch, &seats[0], &task) else {
        panic!("task done returns a task answer")
    };
    let carried = answer
        .claim
        .expect("a continuing done names the carried claim");
    assert_eq!(carried.holder, Some(holder.clone()));
    let phase = carried.phase.expect("the carried claim names its phase");
    assert_eq!(phase.name.as_str(), "todo");
    assert_eq!(
        phase.deliverable.as_deref(),
        board::phase::deliverable(&phase.name)
    );
    assert_eq!(answer.next, carried.next);

    let view = board(&bus, epoch, Actor::Operator);
    let heads = at(&view, &task);
    assert_eq!(heads.workflow.body.phase.as_str(), "todo");
    let claim = &heads.claim.as_ref().expect("claim head").body;
    assert_eq!(claim.disposition, ClaimDisposition::Active);
    assert_eq!(claim.actor, holder);
    assert_eq!(claim.lane, seats[0].lane);
    assert_eq!(claim.occupant, seats[0].occupant);
    let lease = &heads
        .lease
        .as_ref()
        .expect("the carried claim opens a lease");
    assert_eq!(
        lease.position, claim.claimed,
        "the lease is born with the carried win"
    );
    let (expiry, maximum) = (
        instant_after(lease.body.start, 60 * 60).expect("lease expiry"),
        instant_after(lease.body.start, 2 * 60 * 60).expect("lease maximum"),
    );
    assert_eq!((lease.body.expiry, lease.body.maximum), (expiry, maximum));
    let Some(NextWorkState::Active { task: active }) = next_board(&bus, epoch, holder.clone()).next
    else {
        panic!("the carrying lane's task next holds the carried claim")
    };
    assert_eq!(active.statement.body.born, heads.statement.body.born);

    let rival = Actor::Lane(seats[1].lane);
    assert_eq!(allocated(&bus, epoch, &rival), None);
    for rule in ["next", "capture"] {
        let refused = claim_win(&bus, epoch, &seats[1], &task, rule);
        assert!(
            matches!(refused, Outcome::Refusal(_)),
            "a peer's {rule} cannot take a carried claim: {refused:?}"
        );
    }
    let after = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        at(&after, &task)
            .claim
            .as_ref()
            .map(|claim| &claim.body.actor),
        Some(&holder)
    );
    clean(&root, bus);
}

#[test]
fn done_into_the_final_phase_frees_the_claim() {
    let (root, bus, epoch, seats) = fixture("carried-claim-final", 1);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "CARRY-1kCarry02",
        "Free the claim into review",
        Some("todo,review"),
    );
    let holder = Actor::Lane(seats[0].lane);
    claim_task(&bus, epoch, &seats[0], &holder, &task);
    let CommandAnswer::Task(answer) = complete(&bus, epoch, &seats[0], &task) else {
        panic!("task done returns a task answer")
    };
    assert!(answer.claim.is_none());
    let view = board(&bus, epoch, Actor::Operator);
    let heads = at(&view, &task);
    assert_eq!(heads.workflow.body.phase.as_str(), "review");
    assert_eq!(
        heads.claim.as_ref().map(|claim| claim.body.disposition),
        Some(ClaimDisposition::Freed)
    );
    assert!(!matches!(
        next_board(&bus, epoch, holder).next,
        Some(NextWorkState::Active { .. })
    ));
    clean(&root, bus);
}

#[test]
fn only_an_unpublished_advance_before_another_non_review_phase_carries() {
    let flow = phases(&["plan", "todo", "review"]);
    let into_todo = advance("plan", "todo");
    assert!(progress::carries_claim(&into_todo, &flow, None, false));
    assert!(progress::carries_claim(
        &into_todo,
        &flow,
        Some(&boundary(0)),
        false
    ));
    assert!(!progress::carries_claim(
        &into_todo,
        &flow,
        Some(&boundary(1)),
        false
    ));
    assert!(!progress::carries_claim(&into_todo, &flow, None, true));
    assert!(!progress::carries_claim(
        &advance("todo", "review"),
        &flow,
        None,
        false
    ));
    assert!(!progress::carries_claim(
        &advance("plan", "review"),
        &phases(&["plan", "review", "todo"]),
        None,
        false
    ));
    assert!(!progress::carries_claim(
        &board::TransitionKind::Complete {
            phase: "review".parse().expect("fixture phase"),
        },
        &flow,
        None,
        false
    ));
}
