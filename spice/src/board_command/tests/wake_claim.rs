use super::*;

fn add_parked(bus: &Bus, epoch: CanonicalUuid, name: &str) -> TaskHandle {
    let born = stamp(name);
    let task = TaskHandle::for_project(".oops", &born);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskOopsAdd,
        vec![Witness::Absence { task: born }],
        &TaskOopsAdd {
            title: format!("Triage {name}"),
            description: format!("Evidence for {name}"),
            origin: Some("fixture:wake-claim".parse().expect("fixture origin")),
        },
    ));
    task
}

fn claim_wake_expected(
    answer: &BoardAnswer,
    task: &TaskHandle,
    seat: &Seat,
    promotion: bool,
) -> Vec<Witness> {
    let heads = at(answer, task);
    let mut expected = vec![workflow_witness(heads), deferral_witness(heads)];
    if promotion {
        expected.push(statement_witness(heads));
    }
    expected.push(Witness::Eligibility {
        rule: "capture".parse().expect("fixture capture rule"),
    });
    expected.push(residency(answer, seat));
    expected
}

fn fact_count(bus: &Bus) -> usize {
    bus.fact_snapshot()
        .expect("fixture facts remain readable")
        .facts()
        .len()
}

#[test]
fn deferred_wake_claims_in_one_transaction_and_an_existing_claim_refuses_without_waking() {
    let (root, bus, epoch, seats) = fixture("wake-claim", 1);
    let seat = &seats[0];
    let actor = Actor::Lane(seat.lane);
    authorize_lane_creation(&bus, epoch, seat);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "WAKE-claim001",
        "Wake and hold deferred work",
        Some("todo"),
    );
    let before = board(&bus, epoch, actor.clone());
    let heads = at(&before, &task);
    let until = instant_after(current_instant().expect("clock"), 300).expect("future instant");
    assert_committed(issue(
        &bus,
        epoch,
        actor.clone(),
        CommandKind::TaskDefer,
        vec![workflow_witness(heads), deferral_witness(heads)],
        &TaskDefer {
            task: task.clone(),
            wait: until,
        },
    ));

    let deferred = board(&bus, epoch, actor.clone());
    let Outcome::Reply(Reply {
        position: ReplyPosition::Committed(range),
        body: CommandAnswer::Task(answer),
    }) = issue(
        &bus,
        epoch,
        actor.clone(),
        CommandKind::TaskWake,
        claim_wake_expected(&deferred, &task, seat, false),
        &TaskWake {
            task: task.clone(),
            into: None,
            claim: true,
        },
    )
    else {
        panic!("wake-and-claim did not commit")
    };
    let held = answer.claim.expect("wake --claim returns its held claim");
    assert_eq!(held.task, task);
    assert_eq!(held.holder, actor);
    let committed = bus
        .fact_snapshot()
        .expect("fixture facts remain readable")
        .facts()
        .iter()
        .filter(|fact| fact.seq >= range.first && fact.seq <= range.last)
        .map(|fact| fact.fact.kind())
        .collect::<Vec<_>>();
    assert!(committed.contains(&wire::FactKind::TaskWoken));
    assert!(committed.contains(&wire::FactKind::ClaimWon));
    let current = board(&bus, epoch, actor.clone());
    let heads = at(&current, &task);
    assert_eq!(heads.workflow.body.deferral, None);
    let claim = heads.claim.as_ref().expect("woken task is held");
    assert_eq!(claim.body.actor, actor);
    assert_eq!(claim.body.disposition, ClaimDisposition::Active);

    let second = add(
        &bus,
        epoch,
        Actor::Operator,
        "WAKE-claim002",
        "Refuse a second held wake",
        Some("todo"),
    );
    let available = board(&bus, epoch, actor.clone());
    let heads = at(&available, &second);
    assert_committed(issue(
        &bus,
        epoch,
        actor.clone(),
        CommandKind::TaskDefer,
        vec![workflow_witness(heads), deferral_witness(heads)],
        &TaskDefer {
            task: second.clone(),
            wait: until,
        },
    ));
    let deferred = board(&bus, epoch, actor.clone());
    let facts = fact_count(&bus);
    let refused = issue(
        &bus,
        epoch,
        actor.clone(),
        CommandKind::TaskWake,
        claim_wake_expected(&deferred, &second, seat, false),
        &TaskWake {
            task: second.clone(),
            into: None,
            claim: true,
        },
    );
    assert!(
        matches!(refused, Outcome::Refusal(ref refusal)
            if refusal.reason() == "claim::active-exists"),
        "existing claim must refuse atomic wake: {refused:?}",
    );
    assert_eq!(fact_count(&bus), facts, "refused wake wrote facts");
    assert_eq!(
        at(&board(&bus, epoch, actor), &second)
            .workflow
            .body
            .deferral,
        Some(until),
        "refused claim must leave the task deferred",
    );
    clean(&root, bus);
}

#[test]
fn parked_oops_requires_into_then_promotes_and_claims_in_one_transaction() {
    let (root, bus, epoch, seats) = fixture("wake-promote-claim", 1);
    let seat = &seats[0];
    let actor = Actor::Lane(seat.lane);
    authorize_lane_creation(&bus, epoch, seat);
    let parked = add_parked(&bus, epoch, "WAKE-oops0001");
    let visible = board(&bus, epoch, actor.clone());
    let facts = fact_count(&bus);
    let missing_into = issue(
        &bus,
        epoch,
        actor.clone(),
        CommandKind::TaskWake,
        claim_wake_expected(&visible, &parked, seat, false),
        &TaskWake {
            task: parked.clone(),
            into: None,
            claim: true,
        },
    );
    assert!(
        matches!(missing_into, Outcome::Refusal(ref refusal)
            if refusal.reason() == "task::promotion-required"),
        "parked claim wake must require --into: {missing_into:?}",
    );
    assert_eq!(fact_count(&bus), facts, "missing --into wrote facts");

    let Outcome::Reply(Reply {
        position: ReplyPosition::Committed(range),
        body: CommandAnswer::Task(answer),
    }) = issue(
        &bus,
        epoch,
        actor.clone(),
        CommandKind::TaskWake,
        claim_wake_expected(&visible, &parked, seat, true),
        &TaskWake {
            task: parked.clone(),
            into: Some("board.test".to_owned()),
            claim: true,
        },
    )
    else {
        panic!("promote-and-claim did not commit")
    };
    let promoted = TaskHandle::for_project(
        "board.test",
        &parked.born().expect("fixture handle carries a birth stamp"),
    );
    assert_eq!(answer.task, promoted);
    let held = answer
        .claim
        .expect("wake --into --claim returns its held claim");
    assert_eq!(held.task, promoted);
    assert_eq!(held.holder, actor);
    let committed = bus
        .fact_snapshot()
        .expect("fixture facts remain readable")
        .facts()
        .iter()
        .filter(|fact| fact.seq >= range.first && fact.seq <= range.last)
        .map(|fact| fact.fact.kind())
        .collect::<Vec<_>>();
    assert!(committed.contains(&wire::FactKind::TaskWoken));
    assert!(committed.contains(&wire::FactKind::TaskEdited));
    assert!(committed.contains(&wire::FactKind::ClaimWon));
    let current = board(&bus, epoch, actor.clone());
    let heads = at(&current, &promoted);
    assert_eq!(heads.statement.body.project, "board.test");
    assert_eq!(heads.workflow.body.deferral, None);
    let claim = heads.claim.as_ref().expect("promoted task is held");
    assert_eq!(claim.body.actor, actor);
    assert_eq!(claim.body.disposition, ClaimDisposition::Active);
    clean(&root, bus);
}

#[test]
fn settled_task_refuses_wake_claim_without_side_effects() {
    let (root, bus, epoch, seats) = fixture("wake-claim-settled", 1);
    let seat = &seats[0];
    let actor = Actor::Lane(seat.lane);
    authorize_lane_creation(&bus, epoch, seat);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "WAKE-settled1",
        "Completed work cannot wake",
        Some("todo"),
    );
    claim_and_complete_task(&bus, epoch, seat, &actor, &task);
    let completed = board(&bus, epoch, actor.clone());
    let facts = fact_count(&bus);
    let refused = issue(
        &bus,
        epoch,
        actor,
        CommandKind::TaskWake,
        claim_wake_expected(&completed, &task, seat, false),
        &TaskWake {
            task,
            into: None,
            claim: true,
        },
    );
    assert!(
        matches!(refused, Outcome::Refusal(ref refusal)
            if refusal.reason() == "task::settled"),
        "settled task must refuse atomic wake: {refused:?}",
    );
    assert_eq!(fact_count(&bus), facts, "settled refusal wrote facts");
    clean(&root, bus);
}
