use super::*;

fn add_planning(
    bus: &Bus,
    epoch: CanonicalUuid,
    name: &str,
    intent: wire::TaskIntent,
    flow: Option<&str>,
) -> TaskHandle {
    let born = stamp(name);
    let task = TaskHandle::for_project("board.test", &born);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: born }],
        &TaskAdd {
            preparation: None,
            title: name.to_owned(),
            project: Some("board.test".to_owned()),
            description: None,
            acceptance: vec!["The declared planning deliverable is supported".to_owned()],
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some("fixture:planning".parse().expect("origin")),
            flow: flow.map(str::to_owned),
            depends: Vec::new(),
            intent: Some(intent),
            claim: false,
        },
    ));
    task
}

fn done(bus: &Bus, epoch: CanonicalUuid, seat: &Seat, task: &TaskHandle) -> Outcome<CommandAnswer> {
    let lane = Actor::Lane(seat.lane);
    claim_task(bus, epoch, seat, &lane, task);
    let held = board(bus, epoch, lane.clone());
    let expected = progress_witnesses(&held, at(&held, task), seat.lane, seat.occupant, false)
        .expect("completion witnesses");
    issue(
        bus,
        epoch,
        lane,
        CommandKind::TaskDone,
        expected,
        &TaskDone {
            task: task.clone(),
            validation: "Planning evidence retained".to_owned(),
            operation: Some(completion_operation()),
        },
    )
}

#[test]
fn declared_planning_defaults_preserve_the_review_deliverable() {
    let (root, bus, epoch, _) = fixture("planning-defaults", 1);
    for (name, intent, first) in [
        ("Plan contracts", wire::TaskIntent::Plan, "plan"),
        ("Design assembly", wire::TaskIntent::Design, "design"),
    ] {
        let task = add_planning(&bus, epoch, name, intent, None);
        let view = board(&bus, epoch, Actor::Operator);
        let task = at(&view, &task);
        assert_eq!(task.statement.body.intent, Some(intent));
        assert_eq!(
            task.workflow
                .body
                .flow
                .iter()
                .map(wire::Phase::as_str)
                .collect::<Vec<_>>(),
            [first, "review"]
        );
    }
    clean(&root, bus);
}

#[test]
fn plan_review_cannot_replace_required_child_construction_with_own_acceptance() {
    let (root, bus, epoch, seats) = fixture("planning-construction", 1);
    let task = add_planning(
        &bus,
        epoch,
        "Construct contracts",
        wire::TaskIntent::Plan,
        Some("plan,review"),
    );
    let result = done(&bus, epoch, &seats[0], &task);
    let Outcome::Refusal(refusal) = result else {
        panic!("planning without a constructed child must refuse: {result:?}");
    };
    assert_eq!(refusal.reason(), "task::plan-incomplete");
    let view = board(&bus, epoch, Actor::Operator);
    assert_eq!(at(&view, &task).workflow.body.phase.as_str(), "plan");
    clean(&root, bus);
}

#[test]
fn operator_closed_child_cannot_satisfy_plan_construction() {
    let (root, bus, epoch, seats) = fixture("planning-closed-construction", 1);
    let plan = add_planning(
        &bus,
        epoch,
        "Construct retained contracts",
        wire::TaskIntent::Plan,
        Some("plan,review"),
    );
    let child = add_project_with_acceptance(
        &bus,
        epoch,
        Actor::Operator,
        "CONTRACT-negative",
        "Investigate one possible contract",
        "board.test",
        Some("todo"),
        &["The possible contract is justified"],
    );
    connect_child(&bus, epoch, &plan, &child);
    let view = board(&bus, epoch, Actor::Operator);
    let heads = at(&view, &child);
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskClose,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
        ],
        &TaskClose {
            task: child,
            validation: "Investigation rejected this contract".to_owned(),
            force_claimed: false,
        },
    ));

    let result = done(&bus, epoch, &seats[0], &plan);
    let Outcome::Refusal(refusal) = result else {
        panic!("operator-closed work must not construct a plan: {result:?}");
    };
    assert_eq!(refusal.reason(), "task::plan-incomplete");
    let view = board(&bus, epoch, Actor::Operator);
    assert_eq!(at(&view, &plan).workflow.body.phase.as_str(), "plan");
    clean(&root, bus);
}

#[test]
fn ordinary_done_cannot_supply_a_required_review_verdict() {
    let (root, bus, epoch, seats) = fixture("planning-review-verdict", 1);
    let task = add_project_with_acceptance(
        &bus,
        epoch,
        Actor::Operator,
        "REVIEW-verdict",
        "Review the declared deliverable",
        "board.test",
        Some("review"),
        &["The review judges the deliverable"],
    );
    let result = done(&bus, epoch, &seats[0], &task);
    let Outcome::Refusal(refusal) = result else {
        panic!("ordinary completion cannot supply a verdict: {result:?}");
    };
    assert_eq!(refusal.reason(), "task::review-required");
    let view = board(&bus, epoch, Actor::Operator);
    assert_eq!(at(&view, &task).workflow.body.status, "pending");
    clean(&root, bus);
}

fn add_associated(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    name: &str,
    parent: Option<&TaskHandle>,
    intent: Option<wire::TaskIntent>,
) -> TaskHandle {
    let born = stamp(name);
    let task = TaskHandle::for_project("board.test", &born);
    let add = TaskAdd {
        preparation: parent.cloned(),
        intent,
        title: name.to_owned(),
        project: Some("board.test".to_owned()),
        description: None,
        acceptance: vec!["The executable contract supports the requested behavior".to_owned()],
        requirements: wire::EnvironmentRequirements::default(),
        origin: if actor == Actor::Operator {
            Some("fixture:planning".parse().expect("origin"))
        } else {
            None
        },
        flow: None,
        depends: Vec::new(),
        claim: false,
    };
    let view = board(bus, epoch, actor.clone());
    let mut expected = vec![Witness::Absence { task: born }];
    append_preparation_witnesses(&view, &add, &mut expected).expect("preparation witnesses");
    assert_committed(issue(
        bus,
        epoch,
        actor,
        CommandKind::TaskAdd,
        expected,
        &add,
    ));
    task
}

#[test]
fn preparation_runs_inside_an_unfinished_design_but_outputs_wait_for_both_boundaries() {
    let (root, bus, epoch, seats) = fixture("preparation-boundaries", 2);
    let design = add_planning(
        &bus,
        epoch,
        "Choose assembly",
        wire::TaskIntent::Design,
        None,
    );
    let design_actor = Actor::Lane(seats[0].lane);
    claim_task(&bus, epoch, &seats[0], &design_actor, &design);
    let plan = add_associated(
        &bus,
        epoch,
        Actor::Operator,
        "Prepare contracts",
        Some(&design),
        Some(wire::TaskIntent::Plan),
    );
    let plan_actor = Actor::Lane(seats[1].lane);
    claim_task(&bus, epoch, &seats[1], &plan_actor, &plan);
    authorize_lane_creation(&bus, epoch, &seats[1]);
    let output = add_associated(
        &bus,
        epoch,
        plan_actor.clone(),
        "Execute contract",
        None,
        None,
    );
    let view = board(&bus, epoch, plan_actor);
    let entries = actor_entries(&view).expect("entries");
    let output_entry = task_entry(&entries, &output).expect("output");
    assert!(!board::eligible_entry(
        &board::resolved_tasks(&entries),
        output_entry,
        &Actor::Spice,
        &view.environment()
    ));
    let output_head = at(&view, &output);
    assert!(
        output_head
            .statement
            .body
            .dependencies
            .contains(&plan.born().expect("plan stamp"))
    );
    assert!(
        output_head
            .statement
            .body
            .dependencies
            .contains(&design.born().expect("design stamp"))
    );
    assert_eq!(at(&view, &plan).workflow.body.status, "pending");
    assert_eq!(at(&view, &design).workflow.body.status, "pending");
    clean(&root, bus);
}

#[test]
fn negative_controller_closure_resolves_ordinary_edges_without_releasing_controlled_output() {
    let (root, bus, epoch, seats) = fixture("negative-controller", 1);
    let plan = add_planning(
        &bus,
        epoch,
        "Construct selected contract",
        wire::TaskIntent::Plan,
        None,
    );
    let output = add_associated(
        &bus,
        epoch,
        Actor::Operator,
        "Controlled implementation",
        Some(&plan),
        None,
    );
    let ordinary = add_project_with_acceptance(
        &bus,
        epoch,
        Actor::Operator,
        "TASK-ordinary-edge",
        "Unrelated follow-on",
        "board.test",
        Some("todo"),
        &["The ordinary follow-on is supported"],
    );
    connect_child(&bus, epoch, &ordinary, &plan);
    let view = board(&bus, epoch, Actor::Operator);
    let heads = at(&view, &plan);
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskClose,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
        ],
        &TaskClose {
            task: plan,
            validation: "No executable output is selected".to_owned(),
            force_claimed: false,
        },
    ));
    let actor = Actor::Lane(seats[0].lane);
    let view = board(&bus, epoch, actor.clone());
    let entries = actor_entries(&view).expect("entries");
    let resolved = board::resolved_tasks(&entries);
    assert!(!board::eligible_entry(
        &resolved,
        task_entry(&entries, &output).expect("output"),
        &actor,
        &view.environment()
    ));
    assert!(board::eligible_entry(
        &resolved,
        task_entry(&entries, &ordinary).expect("ordinary"),
        &actor,
        &view.environment()
    ));
    clean(&root, bus);
}

#[test]
fn planning_corrections_keep_the_original_construction_duty_and_later_declared_phases() {
    let (root, bus, epoch, seats) = fixture("planning-correction-duty", 1);
    let actor = Actor::Lane(seats[0].lane);
    authorize_lane_creation(&bus, epoch, &seats[0]);
    let plan = add_planning(
        &bus,
        epoch,
        "Construct executable contract",
        wire::TaskIntent::Plan,
        Some("review,verify"),
    );
    claim_task(&bus, epoch, &seats[0], &actor, &plan);
    let view = board(&bus, epoch, actor.clone());
    let correction_born = stamp("Construct missing output");
    let correction = TaskHandle::for_project("board.test", &correction_born);
    let mut expected = progress_witnesses(
        &view,
        at(&view, &plan),
        seats[0].lane,
        seats[0].occupant,
        true,
    )
    .expect("review witnesses");
    expected.push(Witness::Absence {
        task: correction_born.clone(),
    });
    assert_committed(issue(
        &bus,
        epoch,
        actor.clone(),
        CommandKind::TaskReview,
        expected,
        &TaskReview {
            task: plan.clone(),
            finding: "The executable contract is absent".to_owned(),
            note: None,
            followup: Vec::new(),
            then: vec![wire::intent::ReviewFollowup {
                task: correction_born,
                add: TaskAdd {
                    preparation: None,
                    intent: None,
                    title: "Construct missing output".to_owned(),
                    project: Some("board.test".to_owned()),
                    description: None,
                    acceptance: Vec::new(),
                    requirements: wire::EnvironmentRequirements::default(),
                    origin: None,
                    flow: None,
                    depends: Vec::new(),
                    claim: false,
                },
            }],
        },
    ));
    claim_task(&bus, epoch, &seats[0], &actor, &correction);
    let view = board(&bus, epoch, actor.clone());
    let verdict = TaskReview {
        task: correction.clone(),
        finding: "clean".to_owned(),
        note: None,
        then: Vec::new(),
        followup: Vec::new(),
    };
    let refused = issue(
        &bus,
        epoch,
        actor.clone(),
        CommandKind::TaskReview,
        progress_witnesses(
            &view,
            at(&view, &correction),
            seats[0].lane,
            seats[0].occupant,
            true,
        )
        .expect("correction witnesses"),
        &verdict,
    );
    assert!(
        matches!(refused, Outcome::Refusal(refusal) if refusal.reason() == "task::plan-incomplete")
    );
    add_associated(
        &bus,
        epoch,
        Actor::Operator,
        "Executable output contract",
        Some(&plan),
        None,
    );
    let view = board(&bus, epoch, actor.clone());
    let reply = assert_committed(issue(
        &bus,
        epoch,
        actor.clone(),
        CommandKind::TaskReview,
        progress_witnesses(
            &view,
            at(&view, &correction),
            seats[0].lane,
            seats[0].occupant,
            true,
        )
        .expect("constructed correction witnesses"),
        &verdict,
    ));
    let CommandAnswer::Task(answer) = reply else {
        panic!("review returns task state");
    };
    assert_eq!(answer.status, "pending");
    let view = board(&bus, epoch, actor);
    assert_eq!(
        at(&view, &correction).workflow.body.phase.as_str(),
        "verify"
    );
    assert_eq!(at(&view, &plan).workflow.body.status, "pending");
    clean(&root, bus);
}
