use super::*;

fn dependent_add(dependencies: Vec<TaskHandle>, flow: Option<&str>) -> TaskAdd {
    TaskAdd {
        title: "Dependent work".to_owned(),
        project: Some("task.workflow".to_owned()),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        origin: Some(
            "fixture:task-creation-flow"
                .parse()
                .expect("fixture origin"),
        ),
        flow: flow.map(str::to_owned),
        depends: dependencies,
        claim: false,
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
fn create_and_claim_is_one_transaction_and_active_claim_refusal_is_zero_write() {
    let (root, bus, epoch, seats) = fixture("create-and-claim", 1);
    let seat = &seats[0];
    let actor = Actor::Lane(seat.lane);
    authorize_lane_creation(&bus, epoch, seat);
    let before = board(&bus, epoch, actor.clone());
    let born = stamp("CLAIM-add00001");
    let handle = TaskHandle::for_project("task.workflow", &born);
    let add = TaskAdd {
        title: "Board and hold this work".to_owned(),
        project: Some("task.workflow".to_owned()),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        origin: Some("ack:1kLGp2NJ".parse().expect("fixture steering origin")),
        flow: Some("todo,review".to_owned()),
        depends: Vec::new(),
        claim: true,
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
