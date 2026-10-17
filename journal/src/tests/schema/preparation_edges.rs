//! Retained preparation gates cross into ordinary dependency edges once.

use super::*;

#[derive(Clone, Copy)]
enum Settlement {
    DesignDone,
    Closed,
    Deleted,
}

fn write() -> crate::JournalWrite {
    crate::JournalWrite::new(
        Actor::Operator,
        "task:preparation-edges".parse().expect("origin"),
        instant(),
        wire::Producer::CommandDoor,
    )
}

fn add(
    journal: &mut Journal,
    offset: i64,
    intent: Option<wire::TaskIntent>,
    preparation: Option<(&TaskStamp, &TaskStamp)>,
) -> TaskStamp {
    let task = TaskStamp::for_micros(80_000 + offset).expect("task stamp");
    let preparation = preparation.map(|(parent, controller)| wire::TaskPreparation {
        parent: parent.clone(),
        controller: controller.clone(),
        construction: task.clone(),
    });
    let current = if intent.is_some_and(wire::TaskIntent::is_design) {
        phase("design")
    } else {
        phase("todo")
    };
    let flow = vec![current.clone()];
    let mut write = write();
    let added = write
        .fact(
            wire::body::TaskAdded {
                preparation: preparation.clone(),
                intent,
                task: task.clone(),
                title: format!("Preparation edge crossing {offset}"),
                description: None,
                acceptance: vec!["crosses exactly once".to_owned()],
                requirements: wire::EnvironmentRequirements::default(),
                project: "preparation.edges".to_owned(),
                filing: wire::TaskFiling::Filed,
                priority: None,
                taste: None,
                document_slug: None,
                taste_proposal: None,
                depends: Vec::new(),
                flow: flow.clone(),
                phase: current.clone(),
                creation: wire::body::TaskCreationContext::unavailable(
                    wire::body::TaskCreationUnavailable::OperatorActor,
                ),
                revision: 1,
            },
            crate::FactCause::Root,
        )
        .expect("task fact builds");
    let born = task.clone();
    write.task_head(added, move |revision| wire::TaskHead {
        preparation,
        evidence: Vec::new(),
        intent,
        born,
        queue_origin: revision,
        title: format!("Preparation edge crossing {offset}"),
        description: None,
        acceptance: vec!["crosses exactly once".to_owned()],
        requirements: wire::EnvironmentRequirements::default(),
        project: "preparation.edges".to_owned(),
        filing: wire::TaskFiling::Filed,
        priority: None,
        taste: None,
        document_slug: None,
        taste_proposal: None,
        dependencies: Vec::new(),
        provenance: wire::TaskProvenance::Parentless,
        creation: wire::body::TaskCreationContext::unavailable(
            wire::body::TaskCreationUnavailable::OperatorActor,
        ),
        revision,
    });
    let workflow_task = task.clone();
    write.workflow_head(workflow_task, added, move |_| wire::WorkflowHead {
        flow,
        phase: current,
        status: wire::WorkflowStatus::pending(),
        deferral: None,
        generation: 0,
    });
    journal.apply(write).expect("task commits");
    task
}
fn depend(journal: &mut Journal, task: &TaskStamp, after: &TaskStamp) {
    let mut statement = journal
        .read(|read| read.task_head(task))
        .expect("statement reads")
        .expect("statement exists")
        .into_body();
    let mut write = write();
    let fact = write
        .fact(
            wire::body::TaskEdited {
                task: task.clone(),
                revision: u32::try_from(statement.revision.get() + 1).expect("revision fits"),
                dependencies: Some(vec![after.clone()]),
                intent: None,
                evidence: None,
            },
            crate::FactCause::Root,
        )
        .expect("dependency fact builds");
    statement.dependencies = vec![after.clone()];
    write.task_head(fact, move |revision| wire::TaskHead {
        revision,
        ..statement
    });
    journal.apply(write).expect("dependency commits");
}

fn settle(journal: &mut Journal, task: &TaskStamp, settlement: Settlement) {
    let mut workflow = journal
        .read(|read| read.workflow_head(task))
        .expect("workflow reads")
        .expect("workflow exists")
        .into_body();
    let mut write = write();
    let fact = match settlement {
        Settlement::DesignDone => write.fact(
            wire::body::TaskConcluded {
                task: task.clone(),
                conclusion: wire::body::DesignConclusion::Done {},
            },
            crate::FactCause::Root,
        ),
        Settlement::Closed => write.fact(
            wire::body::TaskClosed {
                task: task.clone(),
                validation: "retained closure".to_owned(),
                phase: workflow.phase.clone(),
                claimed: None,
            },
            crate::FactCause::Root,
        ),
        Settlement::Deleted => write.fact(
            wire::body::TaskDeleted {
                task: task.clone(),
                reason: "retained deletion".to_owned(),
                claimed: None,
            },
            crate::FactCause::Root,
        ),
    }
    .expect("settlement fact builds");
    workflow.status = match settlement {
        Settlement::DesignDone => wire::WorkflowStatus::reviewed(instant()),
        Settlement::Closed => wire::WorkflowStatus::closed(instant()),
        Settlement::Deleted => wire::WorkflowStatus::deleted(instant()),
    };
    workflow.generation += 1;
    write.workflow_head(task.clone(), fact, move |_| workflow);
    journal.apply(write).expect("settlement commits");
}

fn select_in_retained_statement(journal: &Journal, output: &TaskStamp, controller: &TaskStamp) {
    let authorization = serde_json::to_string(&wire::rooms::DecisionRef {
        controller: controller.clone(),
        decision: wire::SequenceCode::new(1),
    })
    .expect("authorization encodes");
    journal
        .connection
        .execute(
            "UPDATE task_heads SET body = CAST(json_set(CAST(body AS TEXT), '$.authorization', json(?1)) AS BLOB) WHERE stamp = ?2",
            rusqlite::params![authorization, output.as_str()],
        )
        .expect("retained authorization is injected");
}

fn statement(journal: &Journal, task: &TaskStamp) -> wire::TaskHead {
    journal
        .read(|read| read.task_head(task))
        .expect("statement reads")
        .expect("statement exists")
        .into_body()
}

fn workflow(journal: &Journal, task: &TaskStamp) -> wire::WorkflowHead {
    journal
        .read(|read| read.workflow_head(task))
        .expect("workflow reads")
        .expect("workflow exists")
        .into_body()
}

#[test]
fn schema_seventy_seven_crosses_live_gates_and_closes_unreleasable_branches() {
    let scratch = Scratch::new();
    let path = scratch.path().join("preparation-edges.sqlite3");
    let mut journal = Journal::open(&path).expect("journal creates");

    let open_design = add(&mut journal, 1, Some(wire::TaskIntent::Design), None);
    let open_output = add(&mut journal, 2, None, Some((&open_design, &open_design)));
    let selected_output = add(&mut journal, 3, None, Some((&open_design, &open_design)));

    let done_design = add(&mut journal, 4, Some(wire::TaskIntent::Design), None);
    let done_output = add(&mut journal, 5, None, Some((&done_design, &done_design)));
    settle(&mut journal, &done_design, Settlement::DesignDone);

    let deleted_design = add(&mut journal, 6, Some(wire::TaskIntent::Design), None);
    let deleted_output = add(
        &mut journal,
        7,
        None,
        Some((&deleted_design, &deleted_design)),
    );
    let deleted_descendant = add(
        &mut journal,
        8,
        None,
        Some((&deleted_output, &deleted_design)),
    );
    depend(&mut journal, &deleted_descendant, &deleted_output);
    settle(&mut journal, &deleted_design, Settlement::Deleted);

    let open_plan = add(&mut journal, 9, Some(wire::TaskIntent::Plan), None);
    let open_plan_output = add(&mut journal, 10, None, Some((&open_plan, &open_plan)));
    let open_subplan = add(
        &mut journal,
        11,
        Some(wire::TaskIntent::Plan),
        Some((&open_plan, &open_plan)),
    );

    let closed_plan = add(&mut journal, 12, Some(wire::TaskIntent::Plan), None);
    let closed_plan_output = add(&mut journal, 13, None, Some((&closed_plan, &closed_plan)));
    let closed_plan_descendant = add(
        &mut journal,
        14,
        None,
        Some((&closed_plan_output, &closed_plan)),
    );
    depend(&mut journal, &closed_plan_descendant, &closed_plan_output);
    settle(&mut journal, &closed_plan, Settlement::Closed);

    select_in_retained_statement(&journal, &selected_output, &open_design);
    restore_predecessor(&journal, 77);
    drop(journal);

    let journal = Journal::open(&path).expect("schema-77 preparation gates cross");
    assert_eq!(
        journal.migration().map(|migration| migration.from()),
        Some(77)
    );
    assert!(
        statement(&journal, &open_output)
            .dependencies
            .contains(&open_design)
    );
    assert!(
        !statement(&journal, &selected_output)
            .dependencies
            .contains(&open_design)
    );
    assert!(
        statement(&journal, &open_plan_output)
            .dependencies
            .contains(&open_plan)
    );
    assert!(
        statement(&journal, &open_subplan)
            .dependencies
            .contains(&open_plan)
    );

    for task in [
        done_output,
        deleted_output,
        deleted_descendant,
        closed_plan_output,
        closed_plan_descendant,
    ] {
        assert!(
            matches!(
                workflow(&journal, &task).status,
                wire::WorkflowStatus::Settled {
                    conclusion: wire::WorkflowConclusion::Negative(wire::WorkflowNegative::Closed),
                    ..
                }
            ),
            "{task} closes"
        );
    }
    journal
        .verify()
        .expect("crossed preparation authority verifies");
}
