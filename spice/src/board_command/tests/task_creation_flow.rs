use super::*;

fn dependent_add(dependencies: Vec<TaskHandle>, flow: Option<&str>) -> TaskAdd {
    TaskAdd {
        preparation: None,
        intent: None,
        title: "Dependent work".to_owned(),
        pitch: "This work keeps the fixture honest.".to_owned(),
        project: Some("task.workflow".to_owned()),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        origin: Some(fixture_origin()),
        flow: flow.map(str::to_owned),
        depends: dependencies,
        claim: false,
        draft: false,
    }
}

fn add_with_dependencies(
    bus: &Bus,
    epoch: CanonicalUuid,
    task: &str,
    dependencies: Vec<TaskHandle>,
    flow: Option<&str>,
) -> TaskHandle {
    let born = stamp(task);
    let handle = TaskHandle::for_project("task.workflow", &born);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: born.clone() }],
        &dependent_add(dependencies, flow),
    ));
    handle
}

fn flow<'a>(answer: &'a BoardAnswer, task: &TaskHandle) -> Vec<&'a str> {
    at(answer, task)
        .workflow
        .body
        .flow
        .iter()
        .map(wire::Phase::as_str)
        .collect()
}

fn complete_review(bus: &Bus, epoch: CanonicalUuid, seat: &Seat, task: &TaskHandle) {
    let lane = Actor::Lane(seat.lane);
    claim_task(bus, epoch, seat, &lane, task);
    let held = board(bus, epoch, lane.clone());
    let expected = progress_witnesses(&held, at(&held, task), seat.lane, seat.occupant, true)
        .expect("held review has progress witnesses");
    assert_committed(issue(
        bus,
        epoch,
        lane,
        CommandKind::TaskReview,
        expected,
        &TaskReview {
            task: task.clone(),
            finding: "clean".to_owned(),
            note: None,
            then: Vec::new(),
            followup: Vec::new(),
        },
    ));
}

#[test]
fn task_plan_and_design_creation_admit_authored_roots_within_the_depth_bounds() {
    let (root, bus, epoch, _) = fixture("creation-authored-root", 1);
    let projects = guard_projects();
    for (name, intent, flow) in [
        ("ordinary", None, "todo,review"),
        ("plan", Some(wire::TaskIntent::Plan), "plan,review"),
        ("design", Some(wire::TaskIntent::Design), "design"),
    ] {
        let create = |label: &str, project: &str| {
            let born = stamp(&format!("{name}-{label}"));
            let mut add = dependent_add(Vec::new(), Some(flow));
            add.title = format!("Authored {name} project");
            add.project = Some(project.to_owned());
            add.intent = intent;
            add.acceptance = vec!["Reach the outcome".to_owned()];
            let command = fixture_command(
                epoch,
                Actor::Operator,
                CommandKind::TaskAdd,
                vec![Witness::Absence { task: born }],
                &add,
            );
            task_add(
                &bus,
                &command,
                &add,
                None,
                &projects,
                &board::NextConfig::default(),
            )
        };
        // Depth, not the configured stem list, bounds an authored root.
        for (label, refused) in [("shallow", "foreign"), ("deep", "foreign.one.two.three")] {
            let before = board(&bus, epoch, Actor::Operator).tasks.len();
            let Outcome::Refusal(refusal) = create(label, refused) else {
                panic!("{name} creation admitted out-of-depth {refused}");
            };
            assert_eq!(refusal.reason(), "task::add-refused");
            assert_eq!(
                board(&bus, epoch, Actor::Operator).tasks.len(),
                before,
                "{name} refusal wrote task state",
            );
        }
        for (label, project) in [
            ("configured", "board.work"),
            ("unconfigured", "foreign.work"),
            ("unconfigured_deep", "foreign.deep_work.unit2"),
        ] {
            assert_committed(create(label, project));
            assert!(
                board(&bus, epoch, Actor::Operator)
                    .tasks
                    .iter()
                    .any(|task| task.statement.body.project == project
                        && task.statement.body.intent == intent),
                "{name} {project} was not stored",
            );
        }
    }
    clean(&root, bus);
}

#[test]
fn create_and_claim_is_one_transaction_and_active_claim_refusal_is_zero_write() {
    let (root, bus, epoch, seats) = fixture("create-and-claim", 1);
    let seat = &seats[0];
    let actor = Actor::Lane(seat.lane);
    authorize_lane_creation(&bus, epoch, seat);
    let before = board(&bus, epoch, actor.clone());
    let born = stamp("CLAIM-add00001");
    let handle = TaskHandle::for_project("task.workflow", &born);
    let add = TaskAdd {
        preparation: None,
        intent: None,
        title: "Board and hold this work".to_owned(),
        pitch: "This work keeps the fixture honest.".to_owned(),
        project: Some("task.workflow".to_owned()),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        origin: Some("ack:1kLGp2NJ".parse().expect("fixture steering origin")),
        flow: Some("todo,review".to_owned()),
        depends: Vec::new(),
        claim: true,
        draft: false,
    };
    let expected = vec![
        Witness::Absence { task: born },
        Witness::Eligibility {
            rule: "capture".parse().expect("fixture capture rule"),
        },
        residency(&before, seat),
    ];

    let Outcome::Reply(Reply {
        position: ReplyPosition::Committed(range),
        body: CommandAnswer::Task(answer),
    }) = issue(
        &bus,
        epoch,
        actor.clone(),
        CommandKind::TaskAdd,
        expected,
        &add,
    )
    else {
        panic!("create-and-claim did not commit")
    };
    let held = answer
        .claim
        .expect("task add --claim returns its held claim");
    assert_eq!(answer.task, handle);
    assert_eq!(held.task, handle);
    assert_eq!(held.holder, Some(actor.clone()));
    assert_eq!(
        held.phase.as_ref().map(|phase| phase.name.as_str()),
        Some("todo")
    );
    let committed = bus
        .fact_snapshot()
        .expect("fixture facts remain readable")
        .facts()
        .iter()
        .filter(|fact| fact.seq >= range.first && fact.seq <= range.last)
        .map(|fact| fact.fact.kind())
        .collect::<Vec<_>>();
    assert!(committed.contains(&wire::FactKind::TaskAdded));
    assert!(committed.contains(&wire::FactKind::ClaimWon));
    let current = board(&bus, epoch, actor.clone());
    let claim = at(&current, &handle)
        .claim
        .as_ref()
        .expect("created task is held");
    assert_eq!(claim.body.actor, actor);
    assert_eq!(claim.body.disposition, ClaimDisposition::Active);

    let refused_born = stamp("CLAIM-add00002");
    let refused_handle = TaskHandle::for_project("task.workflow", &refused_born);
    let facts_before = bus
        .fact_snapshot()
        .expect("fixture facts remain readable")
        .facts()
        .len();
    let outcome = issue(
        &bus,
        epoch,
        actor.clone(),
        CommandKind::TaskAdd,
        vec![
            Witness::Absence { task: refused_born },
            Witness::Eligibility {
                rule: "capture".parse().expect("fixture capture rule"),
            },
            residency(&current, seat),
        ],
        &TaskAdd {
            title: "A second held task must refuse".to_owned(),
            claim: true,
            ..add
        },
    );
    assert!(
        matches!(outcome, Outcome::Refusal(ref refusal)
            if refusal.reason() == "claim::active-exists"),
        "active claim must refuse create-and-claim: {outcome:?}",
    );
    let after = board(&bus, epoch, actor);
    assert_eq!(
        bus.fact_snapshot()
            .expect("fixture facts remain readable")
            .facts()
            .len(),
        facts_before,
        "refusal must not write any facts",
    );
    assert!(
        after
            .tasks
            .iter()
            .all(|task| task.statement.body.handle != refused_handle),
        "refused task must not exist",
    );
    clean(&root, bus);
}

#[test]
fn a_defaulted_flow_omits_review_only_when_every_dependency_is_in_review() {
    let (root, bus, epoch, seats) = fixture("creation-dependency-flow", 1);
    let lane = Actor::Lane(seats[0].lane);
    let completed = add(
        &bus,
        epoch,
        Actor::Operator,
        "FLOW-complete1",
        "Completed review dependency",
        Some("review"),
    );
    complete_review(&bus, epoch, &seats[0], &completed);
    let reviewed = add(
        &bus,
        epoch,
        Actor::Operator,
        "FLOW-review001",
        "Review dependency",
        Some("todo,review"),
    );
    claim_and_complete_task(&bus, epoch, &seats[0], &lane, &reviewed);
    let todo = todo_task(&bus, epoch, "FLOW-todo0001", "Todo dependency");

    let all_review =
        add_with_dependencies(&bus, epoch, "FLOW-allreview", vec![reviewed.clone()], None);
    let explicit = add_with_dependencies(
        &bus,
        epoch,
        "FLOW-explicit1",
        vec![reviewed.clone()],
        Some("plan,todo,review"),
    );
    let mixed = add_with_dependencies(&bus, epoch, "FLOW-mixed001", vec![reviewed, todo], None);
    let settled = add_with_dependencies(&bus, epoch, "FLOW-settled01", vec![completed], None);
    let none = add_with_dependencies(&bus, epoch, "FLOW-none0001", Vec::new(), None);
    let current = board(&bus, epoch, Actor::Operator);

    assert_eq!(flow(&current, &all_review), ["plan", "todo"]);
    assert_eq!(flow(&current, &explicit), ["plan", "todo", "review"]);
    assert_eq!(flow(&current, &mixed), ["plan", "todo", "review"]);
    assert_eq!(flow(&current, &settled), ["plan", "todo", "review"]);
    assert_eq!(flow(&current, &none), ["plan", "todo", "review"]);
    clean(&root, bus);
}

#[test]
fn dependency_phase_movement_refuses_creation_with_the_stale_flow() {
    let (root, bus, epoch, seats) = fixture("creation-dependency-race", 1);
    let dependency = add(
        &bus,
        epoch,
        Actor::Operator,
        "FLOW-moving01",
        "Moving dependency",
        Some("review"),
    );
    let born = stamp("FLOW-stale0001");
    let handle = TaskHandle::for_project("task.workflow", &born);
    let add = dependent_add(vec![dependency.clone()], None);
    let command = fixture_command(
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: born }],
        &add,
    );

    let outcome = task_add_with(
        &bus,
        &command,
        &add,
        None,
        &guard_projects(),
        &board::NextConfig::default(),
        || complete_review(&bus, epoch, &seats[0], &dependency),
    );
    assert!(
        matches!(outcome, Outcome::Refusal(ref refusal)
            if refusal.reason() == "board::witness-moved"),
        "dependency movement must refuse the stale creation: {outcome:?}",
    );
    assert!(
        board(&bus, epoch, Actor::Operator)
            .tasks
            .iter()
            .all(|task| task.statement.body.handle != handle),
        "the refused task add remains zero-write",
    );
    clean(&root, bus);
}

fn public_task_add_expected(
    answer: &BoardAnswer,
    add: &TaskAdd,
    child: wire::TaskStamp,
) -> Vec<Witness> {
    let mut expected = vec![Witness::Absence { task: child }];
    append_preparation_witnesses(answer, add, &mut expected)
        .expect("public task add maps current board authority");
    expected
}

fn accepted_child() -> TaskAdd {
    let mut add = dependent_add(Vec::new(), Some("todo,review"));
    add.origin = None;
    add.acceptance = vec!["the implementation has a checkable contract".to_owned()];
    add
}

#[test]
fn an_ambient_plan_releases_its_output_on_positive_completion_without_selection() {
    let (root, bus, epoch, seats) = fixture("ambient-plan-child", 2);
    let seat = &seats[0];
    let lane = Actor::Lane(seat.lane);
    authorize_lane_creation(&bus, epoch, seat);
    let plan = add(
        &bus,
        epoch,
        Actor::Operator,
        "PLAN-ambient01",
        "Plan-only parent",
        Some("plan"),
    );
    claim_task(&bus, epoch, seat, &lane, &plan);
    let held = board(&bus, epoch, lane.clone());
    let child_born = stamp("PLAN-child0001");
    let child = TaskHandle::for_project("task.workflow", &child_born);
    let add = accepted_child();

    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::TaskAdd,
        public_task_add_expected(&held, &add, child_born.clone()),
        &add,
    ));
    let linked = board(&bus, epoch, lane.clone());
    assert_eq!(at(&linked, &plan).statement, at(&held, &plan).statement);
    assert_eq!(
        at(&linked, &child).statement.body.dependencies,
        [plan.born().expect("fixture plan birth")]
    );
    let worker = Actor::Lane(seats[1].lane);
    let offered = |eligible: bool, reason: &str| {
        let available = board(&bus, epoch, worker.clone());
        let entries = actor_entries(&available).expect("board entries");
        let subscription = allocation_subscription(&available.residency.body, &worker)
            .expect("worker subscription");
        let environment = available.environment();
        assert_eq!(
            board::select(
                &entries,
                &worker,
                &subscription,
                available.allocation_band,
                available.recent_cell.as_ref(),
                &environment,
            )
            .map(|selection| selection.entry().born().clone()),
            eligible.then(|| child_born.clone()),
            "{reason}"
        );
        let satisfied = board::resolved_tasks(&entries);
        let output = entries
            .iter()
            .find(|entry| entry.born() == &child_born)
            .expect("the output is on the board");
        assert_eq!(
            (
                board::eligible_entry(&satisfied, output, &worker, &environment),
                board::stealable_entry(&satisfied, output, &worker, &environment),
            ),
            (eligible, eligible),
            "{reason}: capture and steal share the gate"
        );
    };
    offered(false, "the held plan must keep its child out of allocation");

    let declare = TaskEdit {
        task: child.clone(),
        title: None,
        pitch: None,
        description: None,
        acceptance: Vec::new(),
        requirements: None,
        priority: None,
        project: None,
        intent: Some(wire::TaskIntentEdit::Set {
            intent: wire::TaskIntent::Plan,
        }),
        flow: None,
    };
    assert!(matches!(
        correct(&bus, epoch, Actor::Operator, &declare),
        Outcome::Refusal(refusal) if refusal.reason() == "task::preparation-intent-fixed"
    ));
    offered(false, "an output cannot declare its way past its Plan");

    let expected = progress_witnesses(&linked, at(&linked, &plan), seat.lane, seat.occupant, false)
        .expect("the linked accepted child witnesses the attempted conclusion");
    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::TaskDone,
        expected,
        &TaskDone {
            task: plan.clone(),
            validation: "accepted implementation child is connected".to_owned(),
            disposition: None,
            operation: Some(completion_operation()),
        },
    ));
    let concluded = board(&bus, epoch, worker.clone());
    assert!(at(&concluded, &plan).workflow.body.status.is_completed());
    assert!(
        at(&concluded, &child)
            .statement
            .body
            .dependencies
            .contains(&plan.born().expect("Plan stamp"))
    );
    offered(
        true,
        "the concluded plan releases its output without selection",
    );
    clean(&root, bus);
}

#[test]
fn explicit_dependencies_suppress_the_ambient_link() {
    let (root, bus, epoch, seats) = fixture("ambient-plan-override", 1);
    authorize_lane_creation(&bus, epoch, &seats[0]);
    let prerequisite = todo_task(&bus, epoch, "PLAN-prereq001", "Explicit prerequisite");
    let plan = add(
        &bus,
        epoch,
        Actor::Operator,
        "PLAN-override1",
        "Planning parent",
        Some("plan"),
    );
    let planner = Actor::Lane(seats[0].lane);
    claim_task(&bus, epoch, &seats[0], &planner, &plan);
    let explicit_born = stamp("PLAN-explicit1");
    let explicit = TaskHandle::for_project("task.workflow", &explicit_born);
    let held = board(&bus, epoch, planner.clone());
    let add = dependent_add(vec![prerequisite.clone()], Some("todo"));
    assert_committed(issue(
        &bus,
        epoch,
        planner,
        CommandKind::TaskAdd,
        public_task_add_expected(&held, &add, explicit_born.clone()),
        &add,
    ));
    let after_explicit = board(&bus, epoch, Actor::Operator);
    assert!(
        at(&after_explicit, &plan)
            .statement
            .body
            .dependencies
            .is_empty()
    );
    assert_eq!(
        at(&after_explicit, &explicit).statement.body.dependencies,
        [prerequisite
            .born()
            .expect("fixture prerequisite has a birth stamp")]
    );

    clean(&root, bus);
}
#[test]
fn every_ambient_planning_child_waits_on_its_parent_plan() {
    let (root, bus, epoch, seats) = fixture("ambient-subplan", 1);
    let seat = &seats[0];
    authorize_lane_creation(&bus, epoch, seat);
    let parent = add(
        &bus,
        epoch,
        Actor::Operator,
        "PLAN-subparent",
        "Parent Plan",
        Some("plan"),
    );
    let planner = Actor::Lane(seat.lane);
    claim_task(&bus, epoch, seat, &planner, &parent);
    let mut children = Vec::new();
    for (name, intent, flow) in [
        ("PLAN-subplan01", wire::TaskIntent::Plan, "plan,review"),
        ("DESIGN-child01", wire::TaskIntent::Design, "design"),
    ] {
        let born = stamp(name);
        let child = TaskHandle::for_project("task.workflow", &born);
        let held = board(&bus, epoch, planner.clone());
        let mut add = accepted_child();
        add.intent = Some(intent);
        add.flow = Some(flow.to_owned());
        assert_committed(issue(
            &bus,
            epoch,
            planner.clone(),
            CommandKind::TaskAdd,
            public_task_add_expected(&held, &add, born.clone()),
            &add,
        ));
        children.push((born, child));
    }

    let current = board(&bus, epoch, Actor::Operator);
    let parent = parent.born().expect("parent Plan stamp");
    for (born, child) in children {
        assert_eq!(
            at(&current, &child).statement.body.dependencies,
            std::slice::from_ref(&parent)
        );
        assert!(!current.offer.contains(&born));
    }
    clean(&root, bus);
}

#[test]
fn a_non_plan_claim_does_not_gain_the_created_task_as_a_dependency() {
    let (root, bus, epoch, seats) = fixture("ambient-non-plan", 1);
    let seat = &seats[0];
    authorize_lane_creation(&bus, epoch, seat);
    let todo = todo_task(&bus, epoch, "PLAN-todo0002", "Implementation parent");
    let implementer = Actor::Lane(seat.lane);
    claim_task(&bus, epoch, seat, &implementer, &todo);
    let ordinary_born = stamp("PLAN-ordinary1");
    let ordinary = TaskHandle::for_project("task.workflow", &ordinary_born);
    let held = board(&bus, epoch, implementer.clone());
    let add = accepted_child();
    assert_committed(issue(
        &bus,
        epoch,
        implementer,
        CommandKind::TaskAdd,
        public_task_add_expected(&held, &add, ordinary_born),
        &add,
    ));
    let current = board(&bus, epoch, Actor::Operator);
    assert!(at(&current, &todo).statement.body.dependencies.is_empty());
    assert!(
        at(&current, &ordinary)
            .statement
            .body
            .dependencies
            .is_empty()
    );
    clean(&root, bus);
}

#[test]
fn ambient_plan_authority_movement_refuses_child_creation() {
    for movement in ["statement", "workflow", "claim"] {
        let (root, bus, epoch, seats) = fixture("ambient-plan-race", 1);
        let seat = &seats[0];
        let lane = Actor::Lane(seat.lane);
        authorize_lane_creation(&bus, epoch, seat);
        let parent_born = stamp("PLAN-moving001");
        let plan = TaskHandle::for_project("task.workflow", &parent_born);
        let mut parent_add = dependent_add(Vec::new(), Some("plan,todo"));
        parent_add.acceptance = vec!["planning has an explicit execution contract".to_owned()];
        assert_committed(issue(
            &bus,
            epoch,
            Actor::Operator,
            CommandKind::TaskAdd,
            vec![Witness::Absence { task: parent_born }],
            &parent_add,
        ));
        claim_task(&bus, epoch, seat, &lane, &plan);
        let competing = todo_task(&bus, epoch, "PLAN-compete01", "Competing child");
        let held = board(&bus, epoch, lane.clone());
        let child_born = stamp("PLAN-stale0001");
        let child = TaskHandle::for_project("task.workflow", &child_born);
        let add = accepted_child();
        let command = fixture_command(
            epoch,
            lane.clone(),
            CommandKind::TaskAdd,
            public_task_add_expected(&held, &add, child_born),
            &add,
        );

        let outcome = task_add_with(
            &bus,
            &command,
            &add,
            None,
            &guard_projects(),
            &board::NextConfig::default(),
            || match movement {
                "statement" => add_dependency(&bus, epoch, &plan, &competing),
                "workflow" => {
                    assert_committed(issue(
                        &bus,
                        epoch,
                        lane.clone(),
                        CommandKind::TaskDone,
                        progress_witnesses(
                            &held,
                            at(&held, &plan),
                            seat.lane,
                            seat.occupant,
                            false,
                        )
                        .expect("held plan witnesses"),
                        &TaskDone {
                            task: plan.clone(),
                            validation: "execution contract is explicit".to_owned(),
                            disposition: None,
                            operation: Some(completion_operation()),
                        },
                    ));
                }
                "claim" => {
                    let heads = at(&held, &plan);
                    assert_committed(issue(
                        &bus,
                        epoch,
                        lane.clone(),
                        CommandKind::ClaimFree,
                        vec![
                            claim_witness(heads),
                            Witness::Lease {
                                task: heads.statement.body.born.clone(),
                                head: heads.lease.as_ref().expect("lease head").position,
                            },
                            residency(&held, seat),
                        ],
                        &TaskRef { task: plan.clone() },
                    ));
                }
                _ => unreachable!(),
            },
        );
        assert!(
            matches!(&outcome, Outcome::Refusal(refusal)
                if refusal.reason() == "board::witness-moved"),
            "{movement} movement must refuse stale child creation: {outcome:?}",
        );
        let current = board(&bus, epoch, Actor::Operator);
        assert!(
            current
                .tasks
                .iter()
                .all(|task| task.statement.body.handle != child),
            "stale {movement} authority must not publish a child"
        );
        clean(&root, bus);
    }
}
