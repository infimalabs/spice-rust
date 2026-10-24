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

fn detail_with_repository(
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    task: wire::TaskStamp,
) -> Outcome<CommandAnswer> {
    let command = fixture_command(
        epoch,
        Actor::Operator,
        CommandKind::TaskDetail,
        Vec::new(),
        &wire::intent::TaskDetail { task },
    );
    let projects = ProjectPolicy::new(
        ["sandbox".to_owned()],
        dialect::PROJECT_MIN_DEPTH as u64,
        dialect::PROJECT_MAX_DEPTH as u64,
    )
    .expect("board command test project policy");
    dispatch_with_repository(
        bus,
        &command,
        Some(host(repository)),
        &projects,
        &board::NextConfig::default(),
    )
    .expect("board command owns task detail")
}

#[test]
fn task_show_separates_current_context_and_bounded_history() {
    let (root, bus, epoch, _) = fixture("task-show-bounded-sections", 0);
    let task = add_born(
        &bus,
        epoch,
        stamp("task-show-bounded-sections"),
        "Bound one task read",
    );
    let answer = board(&bus, epoch, Actor::Operator);
    let row = at(&answer, &task);
    let current = task_show_bounded(
        &answer,
        row,
        &[],
        TaskShowHistory::default(),
        None,
        None,
        &TaskShowOptions {
            full: false,
            context: false,
            history: false,
            limit: None,
            max_bytes: None,
            max_text: None,
            after: None,
        },
    )
    .expect("current task detail has no cursor");
    for required in [
        "handle ",
        "title Bound one task read",
        "project ",
        "priority -",
        "description: -",
        "phase ",
        "status ",
        "claim ",
        "requirements:",
        "acceptance: -",
        "history: -",
        "depends:",
        "artifacts:",
        "next: ",
    ] {
        assert!(current.contains(required), "{current}");
    }
    assert!(!current.contains("questions:"), "{current}");
    for historical in [
        "origin ",
        "creation_context ",
        "claim_context ",
        "git_context ",
    ] {
        assert!(!current.contains(historical), "{current}");
    }

    let steps = (1..=205)
        .map(|index| wire::intent::TaskDetailStep {
            position: Seq::new(1_000 + index).expect("fixture position"),
            at: "2026-10-02T00:00:00.000Z".parse().expect("fixture instant"),
            author: Actor::Operator,
            step: wire::intent::TaskStep::Done {
                phase: "todo".parse().expect("fixture phase"),
                validation: format!("evidence-{index}"),
                disposition: None,
            },
        })
        .collect::<Vec<_>>();
    let history = task_show_bounded(
        &answer,
        row,
        &[],
        TaskShowHistory {
            steps: &steps,
            ..TaskShowHistory::default()
        },
        None,
        None,
        &TaskShowOptions {
            full: false,
            context: false,
            history: true,
            limit: None,
            max_bytes: None,
            max_text: None,
            after: None,
        },
    )
    .expect("first history page has no cursor");
    assert_eq!(history.matches("    evidence-").count(), 20, "{history}");
    assert!(history.contains("continue: spice task show"), "{history}");
    assert!(history.contains("--after"), "{history}");
    assert!(!history.contains("history: "), "{history}");

    let oversized = "x".repeat(5_000);
    let context = task_show_bounded(
        &answer,
        row,
        &[],
        TaskShowHistory::default(),
        Some(&oversized),
        None,
        &TaskShowOptions {
            full: false,
            context: true,
            history: false,
            limit: None,
            max_bytes: None,
            max_text: None,
            after: None,
        },
    )
    .expect("context page has no cursor");
    assert!(context.contains("spice doc ledger"), "{context}");

    assert!(!context.contains(&oversized), "{context}");
    clean(&root, bus);
}

#[test]
fn task_show_counts_the_history_it_collapses() {
    let (root, bus, epoch, _) = fixture("task-show-collapsed-history", 0);
    let task = add_born(
        &bus,
        epoch,
        stamp("task-show-collapsed-history"),
        "Count one task's history",
    );
    let answer = board(&bus, epoch, Actor::Operator);
    let row = at(&answer, &task);
    let steps = (1..=3)
        .map(|index| wire::intent::TaskDetailStep {
            position: Seq::new(1_000 + index).expect("fixture position"),
            at: "2026-10-02T00:00:00.000Z".parse().expect("fixture instant"),
            author: Actor::Operator,
            step: wire::intent::TaskStep::Done {
                phase: "todo".parse().expect("fixture phase"),
                validation: format!("evidence-{index}"),
                disposition: None,
            },
        })
        .collect::<Vec<_>>();
    let show = |full: bool| {
        task_show_bounded(
            &answer,
            row,
            &[],
            TaskShowHistory {
                steps: &steps,
                ..TaskShowHistory::default()
            },
            None,
            None,
            &TaskShowOptions {
                full,
                context: false,
                history: false,
                limit: None,
                max_bytes: None,
                max_text: None,
                after: None,
            },
        )
        .expect("unpaged detail has no cursor")
    };
    let collapsed = show(false);
    assert!(
        collapsed.contains(&format!("\nhistory: 3 steps spice://task/{task}/history\n")),
        "{collapsed}"
    );
    assert!(!collapsed.contains("evidence-"), "{collapsed}");
    let full = show(true);
    assert!(!full.contains("history: "), "{full}");
    clean(&root, bus);
}

#[test]
fn task_show_labels_design_items_as_questions() {
    let (root, bus, epoch, _) = fixture("task-show-design-questions", 0);
    let task = add_born(
        &bus,
        epoch,
        stamp("task-show-design-questions"),
        "Question one Design",
    );
    let answer = board(&bus, epoch, Actor::Operator);
    let mut design = at(&answer, &task).clone();
    design.statement.body.intent = Some(wire::TaskIntent::Design);
    design.statement.body.acceptance = vec!["Which boundary remains open?".to_owned()];
    let show = |full: bool| {
        task_show_bounded(
            &answer,
            &design,
            &[],
            TaskShowHistory::default(),
            None,
            None,
            &TaskShowOptions {
                full,
                context: false,
                history: false,
                limit: None,
                max_bytes: None,
                max_text: None,
                after: None,
            },
        )
        .expect("Design detail has no cursor")
    };
    let handle = &design.statement.body.handle;
    let collapsed = show(false);
    assert!(
        collapsed.contains(&format!(
            "\nquestions: 1 item spice://task/{handle}/acceptance\n"
        )),
        "{collapsed}"
    );
    assert!(!collapsed.contains("Which boundary"), "{collapsed}");
    let full = show(true);
    assert!(
        full.contains("questions:\n  - Which boundary remains open?"),
        "{full}"
    );
    for text in [collapsed, full] {
        assert!(!text.contains("acceptance:"), "{text}");
    }
    clean(&root, bus);
}

/// Without `--full` the statement's prose collapses to its count lines, each
/// naming the resource that expands exactly it; `--full` prints it whole: a
/// description past the text limit keeps every line, and context does not
/// repeat statement fields.
#[test]
fn task_show_collapses_prose_and_full_prints_it_whole() {
    let (root, bus, epoch, _) = fixture("task-show-authored-statement", 0);
    let task = add_born(
        &bus,
        epoch,
        stamp("task-show-authored-statement"),
        "Read the statement whole",
    );
    let answer = board(&bus, epoch, Actor::Operator);
    let mut row = at(&answer, &task).clone();
    let description = format!(
        "First paragraph {}\n\n  indented second paragraph",
        "x".repeat(300)
    );
    row.statement.body.description = Some(description);
    row.statement.body.priority = Some(wire::TaskPriority::High);
    row.statement.body.acceptance = vec!["One".to_owned(), "Two".to_owned()];
    let show = |full: bool, context: bool| {
        task_show_bounded(
            &answer,
            &row,
            &[],
            TaskShowHistory::default(),
            None,
            None,
            &TaskShowOptions {
                full,
                context,
                history: false,
                limit: None,
                max_bytes: None,
                max_text: None,
                after: None,
            },
        )
        .expect("task detail has no cursor")
    };
    let handle = &row.statement.body.handle;
    let collapsed = show(false, false);
    assert!(
        collapsed.contains(&format!(
            "\ndescription: 3 lines spice://task/{handle}/description\n\
             acceptance: 2 items spice://task/{handle}/acceptance\n\
             history: -\n"
        )),
        "{collapsed}"
    );
    assert!(!collapsed.contains("First paragraph"), "{collapsed}");
    let indented = format!(
        "  First paragraph {}\n\n    indented second paragraph",
        "x".repeat(300)
    );
    let current = show(true, false);
    assert!(current.contains("priority high"), "{current}");
    assert!(
        current.contains(&format!(
            "description:\n{indented}\nacceptance:\n  - One\n  - Two\ndepends:"
        )),
        "{current}"
    );
    assert!(!current.contains("history: "), "{current}");
    let context = show(true, true);
    assert_eq!(
        context.matches("indented second paragraph").count(),
        1,
        "{context}"
    );
    clean(&root, bus);
}

#[test]
fn task_show_context_uses_aggregate_detail_budget_and_exact_continuation() {
    let (root, bus, epoch, _) = fixture("task-show-context-budget", 0);
    let task = add_born(
        &bus,
        epoch,
        stamp("task-show-context-budget"),
        "Budget context rows",
    );
    let answer = board(&bus, epoch, Actor::Operator);
    let bounded = task_show_bounded(
        &answer,
        at(&answer, &task),
        &[],
        TaskShowHistory::default(),
        None,
        None,
        &TaskShowOptions {
            full: false,
            context: true,
            history: false,
            limit: None,
            max_bytes: Some(1),
            max_text: None,
            after: None,
        },
    )
    .expect("first context page has no cursor");
    assert!(
        bounded.contains("continue: spice task show")
            && bounded.contains("--context")
            && bounded.contains("--after context:0"),
        "{bounded}"
    );
    clean(&root, bus);
}

#[test]
fn task_detail_carries_browser_renderable_artifacts_without_text_sidecars() {
    let (root, bus, epoch, _) = fixture("task-detail-media", 0);
    let born = stamp("task-detail-media");
    add_born(&bus, epoch, born.clone(), "Inspect task media");
    fs::create_dir(root.join(".git")).expect("fixture Git directory exists");
    let repository = Repository::discover(&*root).expect("fixture repository resolves");
    let state = repository
        .admit_state()
        .expect("fixture repository state admits");
    fs::create_dir_all(state.artifacts()).expect("artifact directory exists");
    fs::create_dir_all(state.locks()).expect("artifact lock directory exists");
    let store = artifact::Store::new(state.artifacts(), state.locks().join("task-artifacts.lock"));
    let created_at = "2030-04-12T18:44:09.271Z".parse().expect("fixture instant");
    store
        .add_bytes(artifact::AddBytesRequest {
            task: &born,
            bytes: b"\x89PNG\r\n\x1a\n",
            name: "operator-capture.png",
            media_type: Some(artifact::MediaType::IMAGE_PNG),
            retention: artifact::Retention::Permanent,
            created_at,
        })
        .expect("image artifact publishes");
    store
        .add_bytes(artifact::AddBytesRequest {
            task: &born,
            bytes: b"diagnostic text",
            name: "diagnostic.txt",
            media_type: Some(artifact::MediaType::TEXT_PLAIN),
            retention: artifact::Retention::Permanent,
            created_at,
        })
        .expect("text artifact publishes");

    let Outcome::Reply(Reply {
        body: CommandAnswer::TaskDetail(answer),
        ..
    }) = detail_with_repository(&bus, &repository, epoch, born)
    else {
        panic!("task detail did not return media")
    };
    assert_eq!(answer.media.len(), 1);
    assert_eq!(answer.media[0].name, "operator-capture.png");
    assert_eq!(answer.media[0].content_type, "image/png");
    assert_eq!(
        answer.media[0]
            .decoded_bytes()
            .expect("projected media remains admitted"),
        b"\x89PNG\r\n\x1a\n"
    );

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
