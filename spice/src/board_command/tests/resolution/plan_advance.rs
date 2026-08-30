use super::*;

#[test]
fn plan_advance_warns_that_next_may_select_a_competing_eligible_task() {
    let (root, bus, epoch, seats) = fixture("plan-advance-ownership", 1);
    let lane = Actor::Lane(seats[0].lane);
    let plan = add_raw_task(
        &bus,
        epoch,
        "PLAN-1kLGp016",
        "Advance only the held plan phase",
        &[],
        None,
    );
    claim_task(&bus, epoch, &seats[0], &lane, &plan);
    let sibling = prepare_plan_advance_competitor(&bus, epoch, &plan);
    let (done, answer) =
        complete_plan_with_ownership_guidance(&bus, epoch, &seats[0], &lane, &plan);
    let released =
        assert_plan_release_and_replay(&bus, epoch, &seats[0], &lane, &plan, &done, &answer);
    let CommandAnswer::Claim(claimed) = assert_committed(issue(
        &bus,
        epoch,
        lane,
        CommandKind::ClaimWin,
        claim_expected(&released, &sibling, &seats[0]),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
            recovery: None,
        },
    )) else {
        panic!("task next did not return a claim answer")
    };
    assert_eq!(claimed.task, sibling);
    clean(&root, bus);
}

fn prepare_plan_advance_competitor(
    bus: &Bus,
    epoch: CanonicalUuid,
    plan: &TaskHandle,
) -> TaskHandle {
    let visible = board(bus, epoch, Actor::Operator);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskEdit,
        vec![statement_witness(at(&visible, plan))],
        &TaskEdit {
            task: plan.clone(),
            title: None,
            description: None,
            acceptance: vec!["implementation has a checkable contract".to_owned()],
            priority: None,
            project: None,
        },
    ));
    let sibling = add(
        bus,
        epoch,
        Actor::Operator,
        "PLAN-sibling01",
        "Competing eligible implementation",
        Some("todo,review"),
    );
    let visible = board(bus, epoch, Actor::Operator);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskEdit,
        vec![statement_witness(at(&visible, &sibling))],
        &TaskEdit {
            task: sibling.clone(),
            title: None,
            description: None,
            acceptance: Vec::new(),
            priority: Some("critical".to_owned()),
            project: None,
        },
    ));
    sibling
}

fn complete_plan_with_ownership_guidance(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    lane: &Actor,
    plan: &TaskHandle,
) -> (TaskDone, wire::intent::TaskAnswer) {
    let held = board(bus, epoch, lane.clone());
    let done = TaskDone {
        task: plan.clone(),
        validation: "plan acceptance is explicit".to_owned(),
        operation: Some(completion_operation()),
    };
    let CommandAnswer::Task(answer) = assert_committed(issue(
        bus,
        epoch,
        lane.clone(),
        CommandKind::TaskDone,
        progress_witnesses(&held, at(&held, plan), seat.lane, seat.occupant, false)
            .expect("accepted plan has progress witnesses"),
        &done,
    )) else {
        panic!("plan completion did not return a task answer")
    };
    assert_eq!(answer.status, "pending");
    assert_eq!(answer.next.as_deref(), Some("spice task next"));
    assert_eq!(
        answer.guidance.map(wire::intent::TaskGuidance::as_str),
        Some(concat!(
            "phase ownership: this advance released your claim; only the held plan phase ",
            "was completed. Run spice task next for another assignment; it may select any ",
            "eligible work and does not guarantee this task's next phase."
        ))
    );
    (done, answer)
}

fn assert_plan_release_and_replay(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    lane: &Actor,
    plan: &TaskHandle,
    done: &TaskDone,
    answer: &wire::intent::TaskAnswer,
) -> BoardAnswer {
    let released = board(bus, epoch, lane.clone());
    assert_workflow(&released, plan, "todo", &["plan", "todo", "review"]);
    assert_eq!(
        at(&released, plan)
            .claim
            .as_ref()
            .expect("advanced plan retains its claim head")
            .body
            .disposition,
        ClaimDisposition::Freed
    );
    let CommandAnswer::Task(replayed) = assert_committed(issue(
        bus,
        epoch,
        lane.clone(),
        CommandKind::TaskDone,
        progress_witnesses(
            &released,
            at(&released, plan),
            seat.lane,
            seat.occupant,
            false,
        )
        .expect("released plan retains replay witnesses"),
        done,
    )) else {
        panic!("plan completion replay did not return a task answer")
    };
    assert_eq!(&replayed, answer);
    released
}
