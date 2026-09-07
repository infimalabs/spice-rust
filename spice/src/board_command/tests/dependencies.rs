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
    let edits = edit_bodies(&bus, &todo);
    let notice = edits
        .last()
        .and_then(|(_, _, edited)| edited.notice.as_ref())
        .expect("claimed dependency edit carries a contract notice");
    assert_eq!(notice.claim, claim);
    assert_eq!(notice.changes.len(), 1);
    assert_eq!(notice.changes[0].field, "dependencies");
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
fn foreign_claim_authority_refuses_without_an_edge() {
    let (root, bus, epoch, seats) = fixture("dependency-authority", 2);
    let owner = Actor::Lane(seats[0].lane);
    let foreign = Actor::Lane(seats[1].lane);
    let task = todo_task(&bus, epoch, "AUTH-task0001", "Claimed task");
    claim_task(&bus, epoch, &seats[0], &owner, &task);
    let dependency = todo_task(&bus, epoch, "AUTH-after001", "Prerequisite");

    let foreign_view = board(&bus, epoch, foreign.clone());
    let outcome = issue(
        &bus,
        epoch,
        foreign,
        CommandKind::TaskDepends,
        dependency_witnesses(&foreign_view, &task, &[&dependency], None, false),
        &TaskDepends {
            task: task.clone(),
            after: vec![dependency.clone()],
        },
    );
    assert!(
        matches!(outcome, Outcome::Refusal(ref refusal) if refusal.reason() == "claim::ownership-refused"),
        "foreign claimant must refuse: {outcome:?}",
    );
    assert!(
        at(&board(&bus, epoch, Actor::Operator), &task)
            .statement
            .body
            .dependencies
            .is_empty(),
    );
    clean(&root, bus);
}

#[test]
fn fresh_freed_claim_authority_refuses_without_an_edge() {
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
    let outcome = issue(
        &bus,
        epoch,
        owner,
        CommandKind::TaskDepends,
        dependency_witnesses(&fresh, &task, &[&dependency], None, false),
        &TaskDepends {
            task: task.clone(),
            after: vec![dependency],
        },
    );
    assert!(
        matches!(outcome, Outcome::Refusal(ref refusal) if refusal.reason() == "claim::ownership-refused"),
        "moved claim authority must refuse: {outcome:?}",
    );
    assert!(
        at(&board(&bus, epoch, Actor::Operator), &task)
            .statement
            .body
            .dependencies
            .is_empty(),
    );
    clean(&root, bus);
}
