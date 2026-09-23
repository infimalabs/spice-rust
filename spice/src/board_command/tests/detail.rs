use super::*;

fn detail(bus: &Bus, epoch: CanonicalUuid, task: wire::TaskStamp) -> Outcome<CommandAnswer> {
    issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDetail,
        Vec::new(),
        &wire::intent::TaskDetail { task },
    )
}

#[test]
fn task_detail_returns_one_task_and_ordered_attributed_notes() {
    let (root, bus, epoch, seats) = fixture("task-detail", 1);
    let born = stamp("task-detail");
    let task = add_born(&bus, epoch, born.clone(), "Inspect one task");
    let statement = at(&board(&bus, epoch, Actor::Operator), &task)
        .statement
        .body
        .born
        .clone();

    for (actor, text) in [
        (Actor::Operator, "operator observation"),
        (Actor::Lane(seats[0].lane), "resident observation"),
    ] {
        assert_committed(issue(
            &bus,
            epoch,
            actor,
            CommandKind::TaskNote,
            vec![Witness::Existence {
                task: statement.clone(),
            }],
            &TaskNote {
                task: task.clone(),
                note: text.to_owned(),
            },
        ));
    }

    let Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(position),
        body: CommandAnswer::TaskDetail(answer),
    }) = detail(&bus, epoch, born.clone())
    else {
        panic!("task detail did not return a snapshot")
    };
    assert!(position >= answer.notes[1].position);
    assert_eq!(answer.task.statement.body.born, born);
    assert_eq!(answer.notes.len(), 2);
    assert_eq!(answer.notes[0].text, "operator observation");
    assert_eq!(answer.notes[0].author, Actor::Operator);
    assert_eq!(answer.notes[1].text, "resident observation");
    assert_eq!(answer.notes[1].author, Actor::Lane(seats[0].lane));
    assert!(answer.notes[0].position < answer.notes[1].position);
    assert!(answer.notes[0].at <= answer.notes[1].at);

    clean(&root, bus);
}

#[test]
fn task_detail_refuses_an_unknown_durable_identity() {
    let (root, bus, epoch, _) = fixture("task-detail-unknown", 0);
    let outcome = detail(&bus, epoch, stamp("missing-detail"));
    assert!(matches!(
        &outcome,
        Outcome::Refusal(refusal) if refusal.reason() == "task::detail-unknown"
    ));
    clean(&root, bus);
}
