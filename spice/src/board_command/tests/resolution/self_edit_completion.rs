use super::*;

#[test]
fn a_holder_self_edit_refreshes_one_stale_completion_and_commits() {
    let (root, bus, epoch, seats) = fixture("self-edit-completion", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = add_project_with_acceptance(
        &bus,
        epoch,
        Actor::Operator,
        "EDIT-selfdone1",
        "Edit then complete",
        "board.test",
        Some("todo"),
        &["the completion lands"],
    );
    claim_task(&bus, epoch, &seats[0], &lane, &task);
    let before = board(&bus, epoch, lane.clone());
    let stale = progress_witnesses(
        &before,
        at(&before, &task),
        seats[0].lane,
        seats[0].occupant,
        false,
    )
    .expect("held task has completion witnesses");
    let operation = TaskDone::operation_for_claim(
        at(&before, &task)
            .claim
            .as_ref()
            .expect("task is claimed")
            .body
            .claimed,
    );

    edit(&bus, epoch, lane.clone(), &task, "Current title");
    let after = board(&bus, epoch, lane.clone());
    assert!(
        completion_retryable_after_task_change(&before, &after, &task),
        "the holder's edit is acknowledged and only its task witnesses moved"
    );
    let stale_outcome = issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::TaskDone,
        stale,
        &TaskDone {
            task: task.clone(),
            validation: "fixture validation".to_owned(),
            operation: Some(operation),
        },
    );
    assert!(matches!(
        &stale_outcome,
        Outcome::Refusal(refusal) if refusal.reason() == "board::witness-moved"
    ));
    let fresh = progress_witnesses(
        &after,
        at(&after, &task),
        seats[0].lane,
        seats[0].occupant,
        false,
    )
    .expect("edited task has current completion witnesses");
    assert_committed(issue(
        &bus,
        epoch,
        lane,
        CommandKind::TaskDone,
        fresh,
        &TaskDone {
            task,
            validation: "fixture validation".to_owned(),
            operation: Some(operation),
        },
    ));
    clean(&root, bus);
}

#[test]
fn another_actors_edit_never_earns_a_completion_retry() {
    let (root, bus, epoch, seats) = fixture("foreign-edit-completion", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = add_project_with_acceptance(
        &bus,
        epoch,
        Actor::Operator,
        "EDIT-foreign01",
        "Do not cross a foreign edit",
        "board.test",
        Some("todo"),
        &["the foreign edit remains visible"],
    );
    claim_task(&bus, epoch, &seats[0], &lane, &task);
    let before = board(&bus, epoch, lane.clone());
    let stale = progress_witnesses(
        &before,
        at(&before, &task),
        seats[0].lane,
        seats[0].occupant,
        false,
    )
    .expect("held task has completion witnesses");
    let operation = TaskDone::operation_for_claim(
        at(&before, &task)
            .claim
            .as_ref()
            .expect("task is claimed")
            .body
            .claimed,
    );

    edit(&bus, epoch, Actor::Operator, &task, "Operator title");
    let after = board(&bus, epoch, lane.clone());
    assert!(
        !completion_retryable_after_task_change(&before, &after, &task),
        "a foreign edit leaves the holder's generation acknowledgement behind"
    );
    assert!(
        completion_foreign_task_change(&before, &after, &task),
        "the client classifies the moved statement under a stale holder as foreign"
    );
    let stale_outcome = issue(
        &bus,
        epoch,
        lane,
        CommandKind::TaskDone,
        stale,
        &TaskDone {
            task,
            validation: "fixture validation".to_owned(),
            operation: Some(operation),
        },
    );
    assert!(matches!(
        &stale_outcome,
        Outcome::Refusal(refusal) if refusal.reason() == "claim::generation-stale"
    ));
    clean(&root, bus);
}

fn edit(bus: &Bus, epoch: CanonicalUuid, actor: Actor, task: &TaskHandle, title: &str) {
    let visible = board(bus, epoch, actor.clone());
    assert_committed(issue(
        bus,
        epoch,
        actor,
        CommandKind::TaskEdit,
        vec![statement_witness(at(&visible, task))],
        &TaskEdit {
            intent: None,
            evidence: None,
            task: task.clone(),
            title: Some(title.to_owned()),
            description: None,
            acceptance: Vec::new(),
            requirements: None,
            priority: None,
            project: None,
            flow: None,
        },
    ));
}
