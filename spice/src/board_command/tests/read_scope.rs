use super::*;

fn scoped(bus: &Bus, epoch: CanonicalUuid, actor: Actor, read: BoardRead) -> BoardAnswer {
    match issue(bus, epoch, actor, CommandKind::BoardRead, Vec::new(), &read) {
        Outcome::Reply(Reply {
            body: CommandAnswer::Board(answer),
            ..
        }) => *answer,
        outcome => panic!("scoped board read failed: {outcome:?}"),
    }
}

fn handles(answer: &BoardAnswer) -> std::collections::BTreeSet<TaskHandle> {
    answer
        .tasks
        .iter()
        .map(|task| task.statement.body.handle.clone())
        .collect()
}

#[test]
fn scoped_reads_preserve_completed_dependencies_and_claim_conflicts_without_unrelated_rows() {
    let (root, bus, epoch, seats) = fixture("scoped-board", 1);
    let actor = Actor::Lane(seats[0].lane);
    let completed = todo_task(&bus, epoch, "completed", "Completed prerequisite");
    claim_task(&bus, epoch, &seats[0], &actor, &completed);
    let held = board(&bus, epoch, actor.clone());
    let heads = at(&held, &completed);
    assert_committed(issue(
        &bus,
        epoch,
        actor.clone(),
        CommandKind::TaskDone,
        progress_witnesses(&held, heads, seats[0].lane, seats[0].occupant, false)
            .expect("held task has completion witnesses"),
        &TaskDone {
            task: completed.clone(),
            validation: "scoped history fixture".to_owned(),
            operation: Some(TaskDone::operation_for_claim(
                heads.claim.as_ref().expect("claim").body.claimed,
            )),
        },
    ));
    let claimed = todo_task(&bus, epoch, "claimed", "Conflicting active claim");
    claim_task(&bus, epoch, &seats[0], &actor, &claimed);
    let selected = todo_task(&bus, epoch, "selected", "Selected task");
    let middle = todo_task(&bus, epoch, "middle", "Intermediate prerequisite");
    let unrelated = todo_task(&bus, epoch, "unrelated", "Unrelated history");
    connect_child(&bus, epoch, &selected, &middle);
    connect_child(&bus, epoch, &middle, &completed);

    let context = scoped(
        &bus,
        epoch,
        actor.clone(),
        BoardRead::Tasks {
            tasks: vec![selected.born().expect("task stamp")],
            dependencies: true,
            claims: true,
        },
    );
    assert_eq!(
        handles(&context),
        [
            selected.clone(),
            middle.clone(),
            completed.clone(),
            claimed.clone()
        ]
        .into()
    );
    assert!(at(&context, &completed).workflow.body.completion.is_some());
    assert!(matches!(recovery_preflight(&context, &selected),
        Err(Outcome::Refusal(refusal)) if refusal.reason() == "claim::active-exists"));

    let exact = scoped(
        &bus,
        epoch,
        actor.clone(),
        BoardRead::Tasks {
            tasks: vec![completed.born().expect("task stamp")],
            dependencies: false,
            claims: false,
        },
    );
    assert_eq!(handles(&exact), [completed.clone()].into());
    assert!(at(&exact, &completed).workflow.body.completion.is_some());

    let trust = scoped(&bus, epoch, actor.clone(), BoardRead::Trust);
    assert!(trust.tasks.is_empty());
    assert_eq!(
        handles(&board(&bus, epoch, actor)),
        [selected, middle, completed, claimed, unrelated].into()
    );
    clean(&root, bus);
}
