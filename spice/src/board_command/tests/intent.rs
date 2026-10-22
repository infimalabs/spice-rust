use super::*;

use wire::{TaskIntent, TaskIntentEdit};

fn add_declared(
    bus: &Bus,
    epoch: CanonicalUuid,
    task: &str,
    intent: Option<TaskIntent>,
) -> TaskHandle {
    let born = stamp(task);
    let handle = TaskHandle::for_project("board.test", &born);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: born }],
        &TaskAdd {
            preparation: None,
            title: format!("Declared {task}"),
            pitch: "This work keeps the fixture honest.".to_owned(),
            project: Some("board.test".to_owned()),
            description: None,
            acceptance: vec!["The declared intent holds".to_owned()],
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some(fixture_origin()),
            flow: Some("todo,review".to_owned()),
            depends: Vec::new(),
            claim: false,
            intent,
            draft: false,
        },
    ));
    handle
}

fn edit(task: &TaskHandle) -> TaskEdit {
    TaskEdit {
        task: task.clone(),
        title: None,
        pitch: None,
        description: None,
        acceptance: Vec::new(),
        requirements: None,
        priority: None,
        project: None,
        intent: None,
        flow: None,
    }
}

fn declared(bus: &Bus, epoch: CanonicalUuid, task: &TaskHandle) -> Option<TaskIntent> {
    at(&board(bus, epoch, Actor::Operator), task)
        .statement
        .body
        .intent
}

#[test]
fn declared_planning_intent_survives_phases_and_corrections_without_reinterpretation() {
    let (root, bus, epoch, seats) = fixture("task-intent-authority", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = add_declared(&bus, epoch, "INTENT-PLAN001", Some(TaskIntent::Plan));
    let other = add_declared(&bus, epoch, "INTENT-plain01", None);
    assert_eq!(declared(&bus, epoch, &task), Some(TaskIntent::Plan));
    assert_eq!(
        declared(&bus, epoch, &other),
        None,
        "intent is never inferred"
    );

    let retitled = TaskEdit {
        title: Some("Retitled plan".to_owned()),
        ..edit(&task)
    };
    assert_committed(correct(&bus, epoch, Actor::Operator, &retitled));
    assert_eq!(
        declared(&bus, epoch, &task),
        Some(TaskIntent::Plan),
        "an ordinary correction keeps the declared intent",
    );

    let child = stamp("INTENT-child01");
    let current = board(&bus, epoch, Actor::Operator);
    let plan = at(&current, &task);
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![
            Witness::Absence { task: child },
            statement_witness(plan),
            workflow_witness(plan),
        ],
        &TaskAdd {
            preparation: Some(task.clone()),
            intent: None,
            title: "Planned output".to_owned(),
            pitch: "This work keeps the fixture honest.".to_owned(),
            project: Some("board.test".to_owned()),
            description: None,
            acceptance: vec!["The planned output holds".to_owned()],
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some(fixture_origin()),
            flow: None,
            depends: Vec::new(),
            claim: false,
            draft: false,
        },
    ));
    claim_and_complete_task(&bus, epoch, &seats[0], &lane, &task);
    let reviewing = board(&bus, epoch, Actor::Operator);
    assert_eq!(at(&reviewing, &task).workflow.body.phase.as_str(), "review");
    assert_eq!(
        at(&reviewing, &task).statement.body.intent,
        Some(TaskIntent::Plan),
        "a phase change keeps the declared intent",
    );

    let redesign = TaskEdit {
        intent: Some(TaskIntentEdit::Set {
            intent: TaskIntent::Design,
        }),
        ..edit(&task)
    };
    assert!(
        matches!(correct(&bus, epoch, Actor::Operator, &redesign), Outcome::Refusal(refusal) if refusal.reason() == "task::planning-intent-stable")
    );
    assert_eq!(declared(&bus, epoch, &task), Some(TaskIntent::Plan));
    let withdraw = TaskEdit {
        intent: Some(TaskIntentEdit::Clear {}),
        ..edit(&task)
    };
    assert!(
        matches!(correct(&bus, epoch, Actor::Operator, &withdraw), Outcome::Refusal(refusal) if refusal.reason() == "task::planning-intent-stable")
    );
    assert_eq!(declared(&bus, epoch, &task), Some(TaskIntent::Plan));

    // A Design walks only its one open phase, so an undeclared task with
    // another flow cannot become one.
    let design = TaskEdit {
        intent: Some(TaskIntentEdit::Set {
            intent: TaskIntent::Design,
        }),
        ..edit(&other)
    };
    assert!(
        matches!(correct(&bus, epoch, Actor::Operator, &design), Outcome::Refusal(refusal) if refusal.reason() == "task::design-flow")
    );
    assert_eq!(declared(&bus, epoch, &other), None);

    // Verification folds the admitted intent facts against every head.
    clean(&root, bus);
}
