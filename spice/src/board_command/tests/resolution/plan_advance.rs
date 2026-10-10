use super::*;

#[test]
fn plan_claim_allows_board_team_but_classifies_repository_work_as_forbidden() {
    let (root, bus, epoch, seats) = fixture("plan-team-contract", 1);
    let lane = Actor::Lane(seats[0].lane);
    let plan = add_raw_task(
        &bus,
        epoch,
        "PLAN-1kRKw9Qj",
        "Organize implementation work",
        &["implementation is decomposed"],
        Some("plan,todo"),
    );
    claim_task(&bus, epoch, &seats[0], &lane, &plan);

    let held = board(&bus, epoch, lane.clone());
    assert_eq!(
        claim_capture_from_board(&held),
        gate::ClaimCapture::Active {
            task: plan.clone(),
            phase: "plan".parse().expect("fixture plan phase"),
        },
    );
    let Err(Outcome::Refusal(refusal)) = require_no_plan_implementation(&held, "task capture")
    else {
        panic!("plan claim must refuse repository capture");
    };
    assert_eq!(refusal.reason(), "task::plan-implementation");

    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::TaskEdit,
        vec![statement_witness(at(&held, &plan))],
        &TaskEdit {
            task: plan.clone(),
            title: None,
            description: Some("Sequence the implementation children".to_owned()),
            acceptance: Vec::new(),
            requirements: None,
            priority: None,
            project: None,
        },
    ));
    let edited = board(&bus, epoch, lane.clone());
    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::TaskNote,
        vec![Witness::Existence {
            task: at(&edited, &plan).statement.body.born.clone(),
        }],
        &TaskNote {
            task: plan.clone(),
            note: "The implementation boundary is explicit".to_owned(),
        },
    ));
    let child = add_raw_task(
        &bus,
        epoch,
        "PLAN-1kRKw9Qk",
        "Implement the organized change",
        &["repository behavior is verified"],
        None,
    );
    let held = board(&bus, epoch, lane.clone());
    let child_heads = at(&held, &child);
    let mut expected = progress_witnesses(
        &held,
        at(&held, &plan),
        seats[0].lane,
        seats[0].occupant,
        false,
    )
    .expect("claimed plan has dependency-edit witnesses");
    expected.extend([
        statement_witness(child_heads),
        workflow_witness(child_heads),
        Witness::Edge {
            task: at(&held, &plan).statement.body.born.clone(),
            after: child_heads.statement.body.born.clone(),
        },
    ]);
    assert_committed(issue(
        &bus,
        epoch,
        lane,
        CommandKind::TaskDepends,
        expected,
        &TaskDepends {
            task: plan.clone(),
            after: vec![child.clone()],
            remove: false,
        },
    ));

    let organized = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        at(&organized, &plan).statement.body.description.as_deref(),
        Some("Sequence the implementation children"),
    );
    assert_eq!(
        at(&organized, &plan).statement.body.dependencies,
        vec![at(&organized, &child).statement.body.born.clone()],
    );
    clean(&root, bus);
}

#[test]
fn plan_advance_prefers_its_todo_continuation_over_competing_work() {
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
    prepare_plan_advance_competitor(&bus, epoch, &plan);
    // The operator edited the held plan, so the holder acknowledges it first.
    reclaim_held(&bus, epoch, &lane, &seats[0], &plan);
    let (done, answer) = complete_plan_phase(&bus, epoch, &seats[0], &lane, &plan);
    let released =
        assert_plan_release_and_replay(&bus, epoch, &seats[0], &lane, &plan, &done, &answer);
    let CommandAnswer::Claim(claimed) = assert_committed(issue(
        &bus,
        epoch,
        lane,
        CommandKind::ClaimWin,
        claim_expected(&released, &plan, &seats[0]),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
            recovery: None,
        },
    )) else {
        panic!("task next did not return a claim answer")
    };
    assert_eq!(claimed.task, plan);
    clean(&root, bus);
}

#[test]
fn an_assignment_names_the_phase_it_landed_in() {
    let (root, bus, epoch, seats) = fixture("assignment-phase", 2);
    let plan = add_raw_task(
        &bus,
        epoch,
        "PLAN-1kRKw9Ql",
        "Organize implementation work",
        &["implementation is decomposed"],
        Some("plan,todo"),
    );
    let ordinary = add_raw_task(
        &bus,
        epoch,
        "PLAN-1kRKw9Qm",
        "Implement the organized change",
        &["repository behavior is verified"],
        None,
    );
    for (seat, task, phase) in [(0, &plan, "plan"), (1, &ordinary, "todo")] {
        let lane = Actor::Lane(seats[seat].lane);
        let available = board(&bus, epoch, lane.clone());
        let CommandAnswer::Claim(claimed) = assert_committed(issue(
            &bus,
            epoch,
            lane,
            CommandKind::ClaimWin,
            claim_expected(&available, task, &seats[seat]),
            &ClaimWin {
                rule: "next".parse().expect("allocator rule"),
                recovery: None,
            },
        )) else {
            panic!("task next did not return a claim answer")
        };
        assert_eq!(&claimed.task, task);
        let assigned = claimed.phase.expect("an assignment names its phase");
        assert_eq!(assigned.name.as_str(), phase);
        assert_eq!(
            assigned.deliverable.as_deref(),
            board::phase::deliverable(&assigned.name),
            "a declared phase hands the lane the deliverable it authorizes",
        );
    }
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
            requirements: None,
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
            requirements: None,
            priority: Some("critical".to_owned()),
            project: None,
        },
    ));
    sibling
}

fn complete_plan_phase(
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
    assert_eq!(answer.guidance, None);
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

#[test]
fn gated_plan_reverses_its_edge_and_then_completes() {
    let (root, bus, epoch, seats) = fixture("reverse-gated-plan", 1);
    let lane = Actor::Lane(seats[0].lane);
    let plan = add_raw_task(
        &bus,
        epoch,
        "PLAN-1kRKw9Qn",
        "Plan-only parent",
        &[],
        Some("plan"),
    );
    claim_task(&bus, epoch, &seats[0], &lane, &plan);
    let child = add_raw_task(
        &bus,
        epoch,
        "PLAN-1kRKw9Qp",
        "Accepted implementation",
        &["the implementation behavior is verified"],
        Some("todo"),
    );
    connect_child(&bus, epoch, &child, &plan);

    let gated = board(&bus, epoch, Actor::Operator);
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDepends,
        dependency_removal_witnesses(&gated, &child, &[&plan], None, false),
        &TaskDepends {
            task: child.clone(),
            after: vec![plan.clone()],
            remove: true,
        },
    ));
    let ungated = board(&bus, epoch, Actor::Operator);
    assert!(at(&ungated, &child).statement.body.dependencies.is_empty(),);
    connect_child(&bus, epoch, &plan, &child);
    let organized = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        at(&organized, &plan).statement.body.dependencies,
        [at(&organized, &child).statement.body.born.clone()],
    );

    let held = board(&bus, epoch, lane.clone());
    let done = TaskDone {
        task: plan.clone(),
        validation: "accepted child now follows the plan".to_owned(),
        operation: Some(completion_operation()),
    };
    let CommandAnswer::Task(answer) = assert_committed(issue(
        &bus,
        epoch,
        lane,
        CommandKind::TaskDone,
        progress_witnesses(
            &held,
            at(&held, &plan),
            seats[0].lane,
            seats[0].occupant,
            false,
        )
        .expect("plan-only completion carries child witnesses"),
        &done,
    )) else {
        panic!("plan-only completion returned another answer")
    };
    assert_eq!(answer.status, "completed");
    assert_eq!(
        at(&board(&bus, epoch, Actor::Operator), &plan)
            .workflow
            .body
            .status,
        "completed",
    );
    clean(&root, bus);
}
