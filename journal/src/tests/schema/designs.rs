//! Retained Designs cross once into the open Design lifecycle.

use super::*;

fn write() -> crate::JournalWrite {
    crate::JournalWrite::new(
        Actor::Operator,
        "task:designs".parse().expect("origin"),
        instant(),
        wire::Producer::CommandDoor,
    )
}

/// One task as a predecessor release created it.
struct Shape<'a> {
    offset: i64,
    intent: Option<wire::TaskIntent>,
    flow: &'a [&'a str],
    phase: &'a str,
    status: &'a str,
    review: &'a str,
    preparation: Option<(&'a TaskStamp, &'a TaskStamp)>,
    dependencies: Vec<TaskStamp>,
}

fn add(journal: &mut Journal, shape: Shape<'_>) -> TaskStamp {
    let task = TaskStamp::for_micros(30_000 + shape.offset).expect("task stamp");
    let preparation = shape
        .preparation
        .map(|(parent, controller)| wire::TaskPreparation {
            parent: parent.clone(),
            controller: controller.clone(),
            construction: task.clone(),
        });
    let flow = shape
        .flow
        .iter()
        .map(|name| phase(name))
        .collect::<Vec<_>>();
    let mut write = write();
    let fact = write
        .fact(
            wire::body::TaskAdded {
                preparation: preparation.clone(),
                intent: shape.intent,
                task: task.clone(),
                title: format!("Design crossing {}", shape.offset),
                pitch: "A Design crossing must keep its shape.".to_owned(),
                description: None,
                acceptance: vec!["accepted".to_owned()],
                requirements: wire::EnvironmentRequirements::default(),
                project: "designs".to_owned(),
                filing: wire::TaskFiling::Filed,
                priority: None,
                taste: None,
                document_slug: None,
                depends: shape.dependencies.clone(),
                flow: flow.clone(),
                phase: phase(shape.flow[0]),
                creation: wire::body::TaskCreationContext::unavailable(
                    wire::body::TaskCreationUnavailable::OperatorActor,
                ),
                revision: 1,
            },
            crate::FactCause::Root,
        )
        .expect("task fact builds");
    let born = task.clone();
    let dependencies = shape.dependencies;
    write.task_head(fact, move |revision| wire::TaskHead {
        preparation,
        intent: shape.intent,
        born,
        queue_origin: revision,
        title: format!("Design crossing {}", shape.offset),
        pitch: "A Design crossing must keep its shape.".to_owned(),
        description: None,
        acceptance: vec!["accepted".to_owned()],
        requirements: wire::EnvironmentRequirements::default(),
        resources: wire::ResourceRequirements::default(),
        project: "designs".to_owned(),
        filing: wire::TaskFiling::Filed,
        priority: None,
        taste: None,
        document_slug: None,
        dependencies,
        provenance: wire::TaskProvenance::Parentless,
        creation: wire::body::TaskCreationContext::unavailable(
            wire::body::TaskCreationUnavailable::OperatorActor,
        ),
        revision,
    });
    let status = match (shape.status, shape.review) {
        ("pending", "unassigned" | "none") => wire::WorkflowStatus::pending(),
        ("completed", "clean") => wire::WorkflowStatus::reviewed(instant()),
        (status, review) => panic!("unsupported fixture workflow {status}/{review}"),
    };
    let current = phase(shape.phase);
    write.workflow_head(task.clone(), fact, move |_| wire::WorkflowHead {
        flow,
        phase: current,
        status,
        deferral: None,
        generation: 0,
    });
    journal.apply(write).expect("task commits");
    task
}

/// A predecessor holding an open Design in its review phase and a concluded
/// Design, each with outputs that carry ordinary edges to their Design.
struct Retained {
    open: TaskStamp,
    concluded: TaskStamp,
    plan: TaskStamp,
    output: TaskStamp,
    planned: TaskStamp,
    concluded_output: TaskStamp,
    blocker: TaskStamp,
}

fn retained(journal: &mut Journal) -> Retained {
    let design = |offset, status, review| Shape {
        offset,
        intent: Some(wire::TaskIntent::Design),
        flow: &["design", "review"],
        phase: "review",
        status,
        review,
        preparation: None,
        dependencies: Vec::new(),
    };
    let open = add(journal, design(1, "pending", "unassigned"));
    let concluded = add(journal, design(2, "completed", "clean"));
    let blocker = add(
        journal,
        Shape {
            offset: 3,
            intent: None,
            flow: &["todo"],
            phase: "todo",
            status: "pending",
            review: "none",
            preparation: None,
            dependencies: Vec::new(),
        },
    );
    let output = |offset, intent, preparation, dependencies| Shape {
        offset,
        intent,
        flow: &["todo"],
        phase: "todo",
        status: "pending",
        review: "none",
        preparation: Some(preparation),
        dependencies,
    };
    let plan = add(
        journal,
        Shape {
            flow: &["plan", "review"],
            phase: "plan",
            ..output(
                4,
                Some(wire::TaskIntent::Plan),
                (&open, &open),
                vec![open.clone()],
            )
        },
    );
    let direct = add(
        journal,
        output(5, None, (&open, &open), vec![open.clone(), blocker.clone()]),
    );
    let planned = add(
        journal,
        output(6, None, (&plan, &open), vec![plan.clone(), open.clone()]),
    );
    let concluded_output = add(
        journal,
        output(7, None, (&concluded, &concluded), vec![concluded.clone()]),
    );
    Retained {
        open,
        concluded,
        plan,
        output: direct,
        planned,
        concluded_output,
        blocker,
    }
}

fn workflow(journal: &Journal, task: &TaskStamp) -> wire::WorkflowHead {
    journal
        .read(|read| read.workflow_head(task))
        .expect("workflow reads")
        .expect("workflow exists")
        .into_body()
}

fn dependencies(journal: &Journal, task: &TaskStamp) -> Vec<TaskStamp> {
    journal
        .read(|read| read.task_head(task))
        .expect("statement reads")
        .expect("statement exists")
        .into_body()
        .dependencies
}

#[test]
fn open_designs_cross_once_onto_their_open_phase_and_dependency_edges() {
    let mut journal = Journal::in_memory().expect("journal");
    let retained = retained(&mut journal);
    let concluded = workflow(&journal, &retained.concluded);
    restore_predecessor(&journal, SHAPE_PREDECESSOR_SCHEMA);

    let migration = crate::migrate::admit(&mut journal.connection)
        .expect("retained Designs cross")
        .expect("predecessor crosses");
    assert_eq!(
        (migration.from(), migration.to()),
        (SHAPE_PREDECESSOR_SCHEMA, SCHEMA_VERSION)
    );

    let open = workflow(&journal, &retained.open);
    assert_eq!(open.flow, vec![phase("design")]);
    assert_eq!(open.phase, phase("design"));
    assert!(open.status.is_pending(), "an open Design stays open");
    assert_eq!(open.status.reviewer(), None);
    assert_eq!(
        workflow(&journal, &retained.concluded),
        concluded,
        "a concluded Design keeps its head"
    );
    for (task, kept) in [
        (&retained.plan, vec![retained.open.clone()]),
        (
            &retained.output,
            vec![retained.blocker.clone(), retained.open.clone()],
        ),
        (&retained.planned, vec![retained.plan.clone()]),
        (&retained.concluded_output, Vec::new()),
    ] {
        assert_eq!(
            dependencies(&journal, task),
            kept,
            "dependencies for {task}"
        );
    }
    assert!(
        workflow(&journal, &retained.concluded_output)
            .status
            .is_completed(),
        "retained output behind a concluded Design closes"
    );
    journal.verify().expect("crossed Designs verify");

    let crossed = (
        workflow(&journal, &retained.open),
        dependencies(&journal, &retained.output),
    );
    assert_eq!(
        crate::migrate::admit(&mut journal.connection).expect("current admits"),
        None
    );
    assert_eq!(
        (
            workflow(&journal, &retained.open),
            dependencies(&journal, &retained.output),
        ),
        crossed
    );
}

#[test]
fn an_undecodable_open_design_refuses_before_design_shape_migration() {
    let mut journal = Journal::in_memory().expect("journal");
    let retained = retained(&mut journal);
    journal
        .connection
        .execute(
            "UPDATE workflow_heads SET body = CAST('{\"status\":\"pending\"}' AS BLOB) \
             WHERE stamp = ?1",
            [retained.open.as_str()],
        )
        .expect("retired workflow is planted");
    restore_predecessor(&journal, SHAPE_PREDECESSOR_SCHEMA);

    let error =
        crate::migrate::admit(&mut journal.connection).expect_err("an undecodable Design refuses");
    assert!(
        matches!(
            &error,
            OpenError::Migration(MigrationError::RetainedRow { table, row, release, .. })
                if *table == "workflow_heads"
                    && *row == retained.open.as_str()
                    && *release == "schema-72"
        ),
        "{error}"
    );
    assert_eq!(
        journal
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .expect("schema identity reads"),
        SHAPE_PREDECESSOR_SCHEMA,
        "a refused crossing leaves the predecessor identity"
    );
}

#[test]
fn retained_opportunistic_design_initializes_one_verified_generation() {
    let mut journal = Journal::in_memory().expect("journal");
    let design = add(
        &mut journal,
        Shape {
            offset: 100,
            intent: Some(wire::TaskIntent::OpportunisticDesign),
            flow: &["design"],
            phase: "design",
            status: "pending",
            review: "none",
            preparation: None,
            dependencies: Vec::new(),
        },
    );
    restore_predecessor(&journal, crate::migrate::OOPS_GENERATION_SCHEMA - 1);
    let migration = crate::migrate::admit(&mut journal.connection)
        .expect("retained OOPS authority crosses")
        .expect("predecessor crosses");
    assert_eq!(
        (migration.from(), migration.to()),
        (crate::migrate::OOPS_GENERATION_SCHEMA - 1, SCHEMA_VERSION)
    );
    let body = journal
        .connection
        .query_row(
            "SELECT body FROM oops_generation_head WHERE singleton = 1",
            [],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .expect("generation head exists");
    assert_eq!(
        wire::decode::<wire::OopsGenerationHead>(&body).expect("generation head decodes"),
        wire::OopsGenerationHead {
            revision: 1,
            design: Some(design),
        }
    );
    journal.verify().expect("crossed generation verifies");
}

#[test]
fn retained_ambiguous_opportunistic_designs_refuse_their_owning_schema() {
    let mut journal = Journal::in_memory().expect("journal");
    for offset in [200, 201] {
        add(
            &mut journal,
            Shape {
                offset,
                intent: Some(wire::TaskIntent::OpportunisticDesign),
                flow: &["design"],
                phase: "design",
                status: "pending",
                review: "none",
                preparation: None,
                dependencies: Vec::new(),
            },
        );
    }
    restore_predecessor(&journal, crate::migrate::OOPS_GENERATION_SCHEMA - 1);
    let error = crate::migrate::admit(&mut journal.connection)
        .expect_err("ambiguous OOPS authority refuses");
    assert!(
        matches!(
            error,
            OpenError::Migration(MigrationError::RetainedRow {
                table: "task_heads",
                release: "schema-76",
                ..
            })
        ),
        "{error}"
    );
}
