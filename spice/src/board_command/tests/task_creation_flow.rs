use super::*;

fn dependent_add(dependencies: Vec<TaskHandle>) -> TaskAdd {
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
        flow: Some("plan,todo,review".to_owned()),
        depends: dependencies,
    }
}

fn add_with_dependencies(
    bus: &Bus,
    epoch: CanonicalUuid,
    task: &str,
    dependencies: Vec<TaskHandle>,
) -> TaskHandle {
    let born = stamp(task);
    let handle = TaskHandle::for_project("task.workflow", &born);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: born.clone() }],
        &dependent_add(dependencies),
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
fn creation_omits_review_only_when_every_dependency_is_in_review() {
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

    let all_review = add_with_dependencies(&bus, epoch, "FLOW-allreview", vec![reviewed.clone()]);
    let mixed = add_with_dependencies(&bus, epoch, "FLOW-mixed001", vec![reviewed, todo]);
    let settled = add_with_dependencies(&bus, epoch, "FLOW-settled01", vec![completed]);
    let none = add_with_dependencies(&bus, epoch, "FLOW-none0001", Vec::new());
    let current = board(&bus, epoch, Actor::Operator);

    assert_eq!(flow(&current, &all_review), ["plan", "todo"]);
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
    let add = dependent_add(vec![dependency.clone()]);
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
