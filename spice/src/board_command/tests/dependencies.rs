use super::*;

pub(super) fn dependency_witnesses(
    answer: &BoardAnswer,
    task: &TaskHandle,
    after: &[&TaskHandle],
    seat: Option<&Seat>,
    review: bool,
) -> Vec<Witness> {
    let heads = at(answer, task);
    let mut expected = if let Some(seat) = seat {
        progress_witnesses(answer, heads, seat.lane, seat.occupant, review)
            .expect("claimed dependency edit has possession witnesses")
    } else {
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
        ]
    };
    let mut new_edges = Vec::new();
    for dependency in after {
        let prerequisite = at(answer, dependency);
        expected.extend([
            statement_witness(prerequisite),
            workflow_witness(prerequisite),
            Witness::Edge {
                task: heads.statement.body.born.clone(),
                after: prerequisite.statement.body.born.clone(),
            },
        ]);
        if !heads
            .statement
            .body
            .dependencies
            .contains(&prerequisite.statement.body.born)
            && !new_edges.contains(&prerequisite.statement.body.born)
        {
            new_edges.push(prerequisite.statement.body.born.clone());
        }
    }
    append_dependency_reachability_witnesses(answer, &new_edges, &mut expected)
        .expect("fixture reachability witnesses resolve");
    expected
}

pub(super) fn dependency_removal_witnesses(
    answer: &BoardAnswer,
    task: &TaskHandle,
    after: &[&TaskHandle],
    seat: Option<&Seat>,
    review: bool,
) -> Vec<Witness> {
    let heads = at(answer, task);
    let mut expected = if let Some(seat) = seat {
        progress_witnesses(answer, heads, seat.lane, seat.occupant, review)
            .expect("claimed dependency removal has possession witnesses")
    } else {
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
        ]
    };
    for dependency in after {
        let prerequisite = at(answer, dependency);
        expected.extend([
            statement_witness(prerequisite),
            workflow_witness(prerequisite),
        ]);
    }
    if heads.workflow.body.phase.as_str() == "plan"
        && (heads.workflow.body.flow.len() == 1
            || !substantive_acceptance(&heads.statement.body.acceptance))
    {
        let removed = after.iter().map(|task| (*task).clone()).collect::<Vec<_>>();
        if let Some(child) = dependency_removal_plan_child(answer, heads, &removed)
            .expect("remaining plan child resolves")
        {
            expected.push(statement_witness(child));
            expected.push(workflow_witness(child));
        }
    }
    expected
}

pub(super) fn todo_task(bus: &Bus, epoch: CanonicalUuid, task: &str, title: &str) -> TaskHandle {
    add(bus, epoch, Actor::Operator, task, title, Some("todo"))
}

#[test]
fn active_todo_claim_appends_atomically_without_moving_possession() {
    let (root, bus, epoch, seats) = fixture("claimed-dependencies", 1);
    let lane = Actor::Lane(seats[0].lane);
    let todo = add(
        &bus,
        epoch,
        Actor::Operator,
        "DEP-todo00001",
        "Claimed todo",
        Some("todo,review"),
    );
    claim_task(&bus, epoch, &seats[0], &lane, &todo);
    let first = todo_task(&bus, epoch, "DEP-first0001", "First prerequisite");
    let second = todo_task(&bus, epoch, "DEP-second001", "Second prerequisite");
    let before = board(&bus, epoch, lane.clone());
    let before_heads = at(&before, &todo);
    let claim = before_heads.claim.as_ref().expect("todo claim").position;
    let lease = before_heads.lease.as_ref().expect("todo lease").position;
    let statement = before_heads.statement.position;
    let CommandAnswer::Task(edited) = assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::TaskDepends,
        dependency_witnesses(&before, &todo, &[&first, &second], Some(&seats[0]), false),
        &TaskDepends {
            task: todo.clone(),
            after: vec![first.clone(), first.clone(), second.clone()],
            remove: false,
        },
    )) else {
        panic!("dependency edit returned another answer")
    };
    assert_eq!(edited.status, "edited");
    let after = board(&bus, epoch, lane.clone());
    let after_heads = at(&after, &todo);
    assert_ne!(after_heads.statement.position, statement);
    assert_eq!(
        after_heads.claim.as_ref().expect("retained claim").position,
        claim
    );
    assert_eq!(
        after_heads.lease.as_ref().expect("retained lease").position,
        lease
    );
    assert_eq!(after_heads.workflow.body.phase.as_str(), "todo");
    assert_eq!(after_heads.statement.body.dependencies.len(), 2);
    clean(&root, bus);
}
#[test]
fn a_design_dependency_is_an_ordinary_editable_edge() {
    let (root, bus, epoch, _seats) = fixture("design-dependency-edit", 0);
    let design = super::preparation::add_controller(
        &bus,
        epoch,
        "Editable Design dependency",
        wire::TaskIntent::Design,
        None,
    );
    let task = todo_task(&bus, epoch, "DEP-design0001", "Held by a Design edge");

    let before = board(&bus, epoch, Actor::Operator);
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDepends,
        dependency_witnesses(&before, &task, &[&design], None, false),
        &TaskDepends {
            task: task.clone(),
            after: vec![design.clone()],
            remove: false,
        },
    ));
    let held = board(&bus, epoch, Actor::Operator);
    let task_stamp = task.born().expect("task stamp");
    let design_stamp = design.born().expect("Design stamp");
    assert!(
        at(&held, &task)
            .statement
            .body
            .dependencies
            .contains(&design_stamp)
    );
    assert!(!held.offer.contains(&task_stamp));

    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDepends,
        dependency_removal_witnesses(&held, &task, &[&design], None, false),
        &TaskDepends {
            task: task.clone(),
            after: vec![design],
            remove: true,
        },
    ));
    assert!(
        !at(&board(&bus, epoch, Actor::Operator), &task)
            .statement
            .body
            .dependencies
            .contains(&design_stamp)
    );
    clean(&root, bus);
}

#[test]
fn duplicate_claimed_dependencies_advance_the_revision_without_adding_edges() {
    let (root, bus, epoch, seats) = fixture("duplicate-claimed-dependencies", 1);
    let lane = Actor::Lane(seats[0].lane);
    let todo = add(
        &bus,
        epoch,
        Actor::Operator,
        "DEP-duplicate1",
        "Claimed duplicate",
        Some("todo,review"),
    );
    claim_task(&bus, epoch, &seats[0], &lane, &todo);
    let dependency = todo_task(&bus, epoch, "DEP-existing01", "Existing prerequisite");
    let before = board(&bus, epoch, lane.clone());
    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::TaskDepends,
        dependency_witnesses(&before, &todo, &[&dependency], Some(&seats[0]), false),
        &TaskDepends {
            task: todo.clone(),
            after: vec![dependency.clone()],
            remove: false,
        },
    ));
    let after = board(&bus, epoch, lane.clone());
    let after_heads = at(&after, &todo);
    let duplicate_statement = after_heads.statement.position;
    let CommandAnswer::Task(deduplicated) = assert_committed(issue(
        &bus,
        epoch,
        lane,
        CommandKind::TaskDepends,
        progress_witnesses(&after, after_heads, seats[0].lane, seats[0].occupant, false)
            .expect("retained claim witnesses"),
        &TaskDepends {
            task: todo.clone(),
            after: vec![dependency],
            remove: false,
        },
    )) else {
        panic!("duplicate dependency edit returned another answer")
    };
    assert_eq!(deduplicated.status, "edited");
    assert_ne!(
        at(&board(&bus, epoch, Actor::Operator), &todo)
            .statement
            .position,
        duplicate_statement,
    );
    assert_eq!(
        at(&board(&bus, epoch, Actor::Operator), &todo)
            .statement
            .body
            .dependencies
            .len(),
        1,
    );
    clean(&root, bus);
}

#[test]
fn active_review_claim_survives_a_dependency_edit_and_restart() {
    let (root, bus, epoch, seats) = fixture("review-dependencies", 1);
    let review = add(
        &bus,
        epoch,
        Actor::Operator,
        "DEP-review001",
        "Claimed review",
        Some("todo,review"),
    );
    claim_and_complete_task(&bus, epoch, &seats[0], &Actor::Lane(seats[0].lane), &review);
    let reviewer = Actor::Lane(seats[0].lane);
    claim_task(&bus, epoch, &seats[0], &reviewer, &review);
    let dependency = todo_task(&bus, epoch, "DEP-reviewdep1", "Review prerequisite");
    let held = board(&bus, epoch, reviewer.clone());
    let review_claim = at(&held, &review)
        .claim
        .as_ref()
        .expect("review claim")
        .position;
    assert_committed(issue(
        &bus,
        epoch,
        reviewer,
        CommandKind::TaskDepends,
        dependency_witnesses(&held, &review, &[&dependency], Some(&seats[0]), true),
        &TaskDepends {
            task: review.clone(),
            after: vec![dependency],
            remove: false,
        },
    ));
    let reviewed = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        at(&reviewed, &review).workflow.body.phase.as_str(),
        "review"
    );
    assert_eq!(
        at(&reviewed, &review)
            .claim
            .as_ref()
            .expect("review claim retained")
            .position,
        review_claim,
    );
    let expected = at(&reviewed, &review).clone();
    bus.verify()
        .expect("dependency authority verifies before restart");
    bus.shutdown().expect("dependency fixture bus stops");
    let reopened =
        journal::Journal::open(root.join("authority.sqlite3")).expect("dependency journal reopens");
    let reopened = Bus::start(reopened).expect("dependency bus restarts");
    assert_eq!(
        at(&board(&reopened, epoch, Actor::Operator), &review),
        &expected
    );
    clean(&root, reopened);
}

#[test]
fn claim_eligibility_ignores_unrelated_history_but_keeps_a_settled_prerequisite() {
    let (root, bus, epoch, seats) = fixture("claim-scoped-history", 1);
    let lane = Actor::Lane(seats[0].lane);
    let prerequisite = todo_task(&bus, epoch, "SCOPE-prereq1", "Completed prerequisite");
    claim_and_complete_task(&bus, epoch, &seats[0], &lane, &prerequisite);
    let target = todo_task(&bus, epoch, "SCOPE-target01", "Ready dependent");
    connect_child(&bus, epoch, &target, &prerequisite);
    super::read_scope::append_terminal_tasks(&bus, 64);

    let projects = guard_projects();
    let allocation = board::NextConfig::default();
    let complete = validated_snapshot(&bus, lane.clone(), &projects, &allocation)
        .expect("complete projection");
    let scoped = validated_working_snapshot(
        &bus,
        lane.clone(),
        std::slice::from_ref(&target),
        &projects,
        &allocation,
    )
    .expect("working projection");
    assert_eq!(at(&scoped.answer, &target), at(&complete.answer, &target));
    assert_eq!(
        at(&scoped.answer, &prerequisite),
        at(&complete.answer, &prerequisite),
    );
    let complete_target = task_entry(&complete.entries, &target).expect("complete target");
    let scoped_target = task_entry(&scoped.entries, &target).expect("scoped target");
    let complete_decision = board::eligible_entry(
        &board::resolved_tasks(&complete.entries),
        complete_target,
        &lane,
        &complete.answer.environment(),
    );
    let scoped_decision = board::eligible_entry(
        &board::resolved_tasks(&scoped.entries),
        scoped_target,
        &lane,
        &scoped.answer.environment(),
    );
    assert_eq!(scoped_decision, complete_decision);
    assert!(
        scoped_decision,
        "the settled prerequisite satisfies the edge"
    );

    claim_task(&bus, epoch, &seats[0], &lane, &target);
    assert_eq!(
        at(&board(&bus, epoch, lane), &target)
            .claim
            .as_ref()
            .map(|claim| claim.body.disposition),
        Some(ClaimDisposition::Active),
    );
    clean(&root, bus);
}

#[test]
fn dependency_cycle_refuses_the_whole_requested_edit() {
    let (root, bus, epoch, _) = fixture("dependency-cycle", 1);
    let first = todo_task(&bus, epoch, "CYCLE-first01", "First");
    let second = todo_task(&bus, epoch, "CYCLE-second1", "Second");
    connect_child(&bus, epoch, &first, &second);
    let before = board(&bus, epoch, Actor::Operator);
    let outcome = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDepends,
        dependency_witnesses(&before, &second, &[&first], None, false),
        &TaskDepends {
            task: second.clone(),
            after: vec![first],
            remove: false,
        },
    );
    assert!(
        matches!(outcome, Outcome::Refusal(ref refusal) if refusal.reason() == "task::dependency-refused"),
        "cycle must refuse atomically: {outcome:?}",
    );
    assert!(
        at(&board(&bus, epoch, Actor::Operator), &second)
            .statement
            .body
            .dependencies
            .is_empty(),
    );
    clean(&root, bus);
}

#[test]
fn omitted_intermediate_witness_refuses_after_the_graph_moves_toward_a_cycle() {
    let (root, bus, epoch, _) = fixture("dependency-omitted-intermediate", 1);
    let target = todo_task(&bus, epoch, "MISS-target001", "Target");
    let direct = todo_task(&bus, epoch, "MISS-direct001", "Direct prerequisite");
    let intermediate = todo_task(&bus, epoch, "MISS-middle001", "Intermediate prerequisite");
    connect_child(&bus, epoch, &direct, &intermediate);
    let stale = board(&bus, epoch, Actor::Operator);
    let intermediate_stamp = at(&stale, &intermediate).statement.body.born.clone();
    let mut witnesses = dependency_witnesses(&stale, &target, &[&direct], None, false);
    witnesses.retain(
        |witness| !matches!(witness, Witness::Task { task, .. } if task == &intermediate_stamp),
    );
    let intent = TaskDepends {
        task: target.clone(),
        after: vec![direct],
        remove: false,
    };
    let command = fixture_command(
        epoch,
        Actor::Operator,
        CommandKind::TaskDepends,
        witnesses,
        &intent,
    );

    let outcome = task_depends_with(
        &bus,
        &command,
        &intent,
        &guard_projects(),
        &board::NextConfig::default(),
        || connect_child(&bus, epoch, &intermediate, &target),
    );
    assert!(
        matches!(outcome, Outcome::Refusal(ref refusal)
            if refusal.reason() == "task::dependency-witness-missing"),
        "an omitted reachable statement must refuse: {outcome:?}",
    );
    let current = board(&bus, epoch, Actor::Operator);
    assert!(
        at(&current, &target).statement.body.dependencies.is_empty(),
        "the outer dependency edit remains zero-write",
    );
    assert_eq!(
        at(&current, &intermediate).statement.body.dependencies,
        [at(&current, &target).statement.body.born.clone()],
        "the adversarial return edge landed after validation",
    );
    clean(&root, bus);
}

#[test]
fn foreign_actor_edits_dependencies_without_moving_the_active_claim() {
    let (root, bus, epoch, seats) = fixture("dependency-authority", 2);
    let owner = Actor::Lane(seats[0].lane);
    let foreign = Actor::Lane(seats[1].lane);
    let task = todo_task(&bus, epoch, "AUTH-task0001", "Claimed task");
    claim_task(&bus, epoch, &seats[0], &owner, &task);
    let dependency = todo_task(&bus, epoch, "AUTH-after001", "Prerequisite");

    let foreign_view = board(&bus, epoch, foreign.clone());
    let claim = at(&foreign_view, &task)
        .claim
        .as_ref()
        .expect("active claim")
        .clone();
    assert_committed(issue(
        &bus,
        epoch,
        foreign,
        CommandKind::TaskDepends,
        dependency_witnesses(&foreign_view, &task, &[&dependency], None, false),
        &TaskDepends {
            task: task.clone(),
            after: vec![dependency.clone()],
            remove: false,
        },
    ));
    let edited = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        at(&edited, &task).statement.body.dependencies,
        [at(&edited, &dependency).statement.body.born.clone()],
    );
    assert_eq!(at(&edited, &task).claim.as_ref(), Some(&claim));
    clean(&root, bus);
}

#[test]
fn a_freed_claim_does_not_block_a_dependency_edit() {
    let (root, bus, epoch, seats) = fixture("moved-dependency-authority", 1);
    let owner = Actor::Lane(seats[0].lane);
    let task = todo_task(&bus, epoch, "MOVE-task0001", "Claimed task");
    claim_task(&bus, epoch, &seats[0], &owner, &task);
    let dependency = todo_task(&bus, epoch, "MOVE-after001", "Prerequisite");
    let held = board(&bus, epoch, owner.clone());
    let heads = at(&held, &task);
    assert_committed(issue(
        &bus,
        epoch,
        owner.clone(),
        CommandKind::ClaimFree,
        vec![
            claim_witness(heads),
            Witness::Lease {
                task: heads.statement.body.born.clone(),
                head: heads.lease.as_ref().expect("claim lease").position,
            },
            residency(&held, &seats[0]),
        ],
        &TaskRef { task: task.clone() },
    ));
    let fresh = board(&bus, epoch, owner.clone());
    let freed = at(&fresh, &task)
        .claim
        .as_ref()
        .expect("freed claim")
        .clone();
    assert_eq!(freed.body.disposition, ClaimDisposition::Freed);
    assert_committed(issue(
        &bus,
        epoch,
        owner,
        CommandKind::TaskDepends,
        dependency_witnesses(&fresh, &task, &[&dependency], None, false),
        &TaskDepends {
            task: task.clone(),
            after: vec![dependency.clone()],
            remove: false,
        },
    ));
    let edited = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        at(&edited, &task).statement.body.dependencies,
        [at(&edited, &dependency).statement.body.born.clone()],
    );
    assert_eq!(at(&edited, &task).claim.as_ref(), Some(&freed));
    clean(&root, bus);
}

#[test]
fn lane_removes_one_dependency_and_absent_edge_refuses_without_writing() {
    let (root, bus, epoch, seats) = fixture("remove-dependency", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = todo_task(&bus, epoch, "REMOVE-task001", "Task edited by a lane");
    let first = todo_task(&bus, epoch, "REMOVE-first01", "First prerequisite");
    let second = todo_task(&bus, epoch, "REMOVE-second1", "Second prerequisite");
    let before = board(&bus, epoch, lane.clone());
    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::TaskDepends,
        dependency_witnesses(&before, &task, &[&first, &second], None, false),
        &TaskDepends {
            task: task.clone(),
            after: vec![first.clone(), second.clone()],
            remove: false,
        },
    ));

    let connected = board(&bus, epoch, lane.clone());
    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::TaskDepends,
        dependency_removal_witnesses(&connected, &task, &[&first], None, false),
        &TaskDepends {
            task: task.clone(),
            after: vec![first.clone()],
            remove: true,
        },
    ));
    let removed = board(&bus, epoch, lane.clone());
    assert_eq!(
        at(&removed, &task).statement.body.dependencies,
        [at(&removed, &second).statement.body.born.clone()],
    );

    let statement = at(&removed, &task).statement.position;
    let outcome = issue(
        &bus,
        epoch,
        lane,
        CommandKind::TaskDepends,
        dependency_removal_witnesses(&removed, &task, &[&first], None, false),
        &TaskDepends {
            task: task.clone(),
            after: vec![first],
            remove: true,
        },
    );
    assert!(
        matches!(outcome, Outcome::Refusal(ref refusal)
            if refusal.reason() == "task::dependency-absent"),
        "an absent dependency removal must refuse: {outcome:?}",
    );
    assert_eq!(
        at(&board(&bus, epoch, Actor::Operator), &task)
            .statement
            .position,
        statement,
    );
    clean(&root, bus);
}

#[test]
fn plan_cannot_remove_the_only_pending_child_carrying_acceptance() {
    let (root, bus, epoch, _) = fixture("remove-plan-acceptance", 1);
    let plan = add_project_with_acceptance(
        &bus,
        epoch,
        "REMOVE-plan001",
        "Plan-only parent",
        "board.test",
        Some("plan"),
        &[],
    );
    let child = add_project_with_acceptance(
        &bus,
        epoch,
        "REMOVE-child01",
        "Accepted child",
        "board.test",
        Some("todo"),
        &["the implementation behavior is verified"],
    );
    connect_child(&bus, epoch, &plan, &child);
    let before = board(&bus, epoch, Actor::Operator);
    let outcome = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDepends,
        dependency_removal_witnesses(&before, &plan, &[&child], None, false),
        &TaskDepends {
            task: plan.clone(),
            after: vec![child.clone()],
            remove: true,
        },
    );
    assert!(
        matches!(outcome, Outcome::Refusal(ref refusal)
            if refusal.reason() == "task::plan-acceptance-orphaned"),
        "orphaning plan acceptance must refuse: {outcome:?}",
    );
    let current = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        at(&current, &plan).statement.body.dependencies,
        [at(&current, &child).statement.body.born.clone()],
    );
    clean(&root, bus);
}

#[test]
fn plan_removes_one_child_while_another_pending_child_carries_acceptance() {
    let (root, bus, epoch, _) = fixture("remove-one-plan-child", 1);
    let plan = add_project_with_acceptance(
        &bus,
        epoch,
        "REMOVE-plan002",
        "Plan-only parent",
        "board.test",
        Some("plan"),
        &[],
    );
    let first = add_project_with_acceptance(
        &bus,
        epoch,
        "REMOVE-child02",
        "First accepted child",
        "board.test",
        Some("todo"),
        &["the first implementation behavior is verified"],
    );
    let second = add_project_with_acceptance(
        &bus,
        epoch,
        "REMOVE-child03",
        "Remaining accepted child",
        "board.test",
        Some("todo"),
        &["the remaining implementation behavior is verified"],
    );
    connect_child(&bus, epoch, &plan, &first);
    connect_child(&bus, epoch, &plan, &second);
    let before = board(&bus, epoch, Actor::Operator);
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDepends,
        dependency_removal_witnesses(&before, &plan, &[&first], None, false),
        &TaskDepends {
            task: plan.clone(),
            after: vec![first],
            remove: true,
        },
    ));
    let current = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        at(&current, &plan).statement.body.dependencies,
        [at(&current, &second).statement.body.born.clone()],
    );
    clean(&root, bus);
}

#[test]
fn plan_removes_a_forward_child_while_an_accepted_child_depends_on_the_plan() {
    let (root, bus, epoch, _) = fixture("remove-plan-child-with-reverse-child", 1);
    let plan = add_project_with_acceptance(
        &bus,
        epoch,
        "REMOVE-plan003",
        "Plan-only parent",
        "board.test",
        Some("plan"),
        &[],
    );
    let forward = add_project_with_acceptance(
        &bus,
        epoch,
        "REMOVE-child04",
        "Forward accepted child",
        "board.test",
        Some("todo"),
        &["the forward implementation behavior is verified"],
    );
    let reverse = add_project_with_acceptance(
        &bus,
        epoch,
        "REMOVE-child05",
        "Reverse accepted child",
        "board.test",
        Some("todo"),
        &["the reverse implementation behavior is verified"],
    );
    connect_child(&bus, epoch, &plan, &forward);
    connect_child(&bus, epoch, &reverse, &plan);

    let stale = board(&bus, epoch, Actor::Operator);
    let stale_expected = dependency_removal_witnesses(&stale, &plan, &[&forward], None, false);
    let reverse_heads = at(&stale, &reverse);
    assert!(stale_expected.contains(&statement_witness(reverse_heads)));
    assert!(stale_expected.contains(&workflow_witness(reverse_heads)));

    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskEdit,
        vec![statement_witness(reverse_heads)],
        &TaskEdit {
            intent: None,
            task: reverse.clone(),
            title: None,
            pitch: None,
            description: None,
            acceptance: vec!["the revised reverse behavior is verified".to_owned()],
            requirements: None,
            priority: None,
            project: None,
            flow: None,
        },
    ));
    let refused = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDepends,
        stale_expected,
        &TaskDepends {
            task: plan.clone(),
            after: vec![forward.clone()],
            remove: true,
        },
    );
    assert!(
        matches!(refused, Outcome::Refusal(_)),
        "stale reverse-child witnesses must refuse: {refused:?}",
    );
    assert_eq!(
        at(&board(&bus, epoch, Actor::Operator), &plan)
            .statement
            .body
            .dependencies,
        [at(&stale, &forward).statement.body.born.clone()],
    );

    let refreshed = board(&bus, epoch, Actor::Operator);
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDepends,
        dependency_removal_witnesses(&refreshed, &plan, &[&forward], None, false),
        &TaskDepends {
            task: plan.clone(),
            after: vec![forward],
            remove: true,
        },
    ));
    let current = board(&bus, epoch, Actor::Operator);
    assert!(at(&current, &plan).statement.body.dependencies.is_empty());
    assert_eq!(
        at(&current, &reverse).statement.body.dependencies,
        [at(&current, &plan).statement.body.born.clone()],
    );
    clean(&root, bus);
}

#[test]
fn named_capture_of_a_settled_task_is_refused_as_ineligible_not_unknown() {
    let (root, bus, epoch, seats) = fixture("capture-settled", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = todo_task(&bus, epoch, "SCOPE-settled1", "Settled");
    claim_and_complete_task(&bus, epoch, &seats[0], &lane, &task);
    super::read_scope::append_terminal_tasks(&bus, 8);
    let available = board(&bus, epoch, lane.clone());
    let heads = at(&available, &task);
    let rule: Name = "capture".parse().expect("capture rule");
    let outcome = issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
            Witness::Eligibility { rule: rule.clone() },
            residency(&available, &seats[0]),
        ],
        &ClaimWin {
            rule,
            recovery: None,
        },
    );
    assert!(
        matches!(
            &outcome,
            Outcome::Refusal(Refusal::Bare { diagnostic })
                if diagnostic.reason() == "claim::capture-ineligible"
        ),
        "{outcome:?}"
    );
    clean(&root, bus);
}
