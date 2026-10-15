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

/// Record one principal's birth holding the operator role.
fn admit(bus: &Bus, epoch: CanonicalUuid, principal: CanonicalUuid) {
    let origin = format!("resident:{epoch}")
        .parse()
        .expect("resident fixture origin");
    let now = Instant::new(jiff::Timestamp::now()).expect("current fixture instant");
    let mut write = JournalWrite::new(Actor::Spice, origin, now, Producer::CommandDoor);
    let roles = std::collections::BTreeSet::from([wire::principal::PrincipalRole::Operator]);
    let born = write
        .fact(
            wire::principal::PrincipalAdmitted {
                principal,
                profile: wire::principal::PrincipalProfile::default(),
                roles: roles.clone(),
                capability: None,
            },
            FactCause::Root,
        )
        .expect("principal birth fact builds");
    write.principal_head(born, move |born| wire::principal::PrincipalHead {
        principal,
        born,
        profile: wire::principal::PrincipalProfile::default(),
        roles,
        capability: None,
    });
    assert!(matches!(
        bus.commit(write).expect("principal birth commits"),
        CommitOutcome::Committed(_)
    ));
}

fn note_authors(bus: &Bus, epoch: CanonicalUuid, task: &wire::TaskStamp) -> Vec<Actor> {
    let Outcome::Reply(Reply {
        body: CommandAnswer::TaskDetail(answer),
        ..
    }) = detail(bus, epoch, task.clone())
    else {
        panic!("task detail did not return a snapshot")
    };
    answer.notes.into_iter().map(|note| note.author).collect()
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
        Some(repository),
        &projects,
        &board::NextConfig::default(),
    )
    .expect("board command owns task detail")
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

    // The recorded operator note names the first-born principal once it is
    // born, and a later admission holding the same role never moves it.
    let lane = Actor::Lane(seats[0].lane);
    let bootstrap: CanonicalUuid = "018f82d4-6f59-7d06-92e3-bce9131163f0"
        .parse()
        .expect("fixture principal");
    admit(&bus, epoch, bootstrap);
    let resolved = vec![Actor::Human(bootstrap), lane];
    assert_eq!(note_authors(&bus, epoch, &born), resolved);
    admit(
        &bus,
        epoch,
        "018f82d4-6f59-7d06-92e3-bce9131163a0"
            .parse()
            .expect("fixture principal"),
    );
    assert_eq!(note_authors(&bus, epoch, &born), resolved);

    clean(&root, bus);
}

#[test]
fn task_show_prints_only_the_named_tasks_notes() {
    let (root, bus, epoch, _) = fixture("task-show-notes", 1);
    let shown_born = stamp("task-show-notes-shown");
    let shown = add_born(&bus, epoch, shown_born.clone(), "Shown task");
    let dependency_born = stamp("task-show-notes-dependency");
    let dependency = add_born(&bus, epoch, dependency_born.clone(), "Dependency task");
    super::hidden::add_dependency(&bus, epoch, &shown, &dependency);
    let note = |task: &TaskHandle, born: &wire::TaskStamp, text: &str| {
        assert_committed(issue(
            &bus,
            epoch,
            Actor::Operator,
            CommandKind::TaskNote,
            vec![Witness::Existence { task: born.clone() }],
            &TaskNote {
                task: task.clone(),
                note: text.to_owned(),
            },
        ));
    };
    note(&shown, &shown_born, "first line\n\nthird line");
    note(&dependency, &dependency_born, "dependency evidence");
    note(&shown, &shown_born, "later evidence");

    let read = |notes: bool| match issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::BoardRead,
        Vec::new(),
        &BoardRead::Tasks {
            tasks: vec![shown_born.clone()],
            dependencies: true,
            claims: false,
            notes,
        },
    ) {
        Outcome::Reply(Reply {
            body: CommandAnswer::Board(answer),
            ..
        }) => answer,
        other => panic!("task board read did not reply: {other:?}"),
    };

    let answer = read(true);
    assert!(
        answer
            .tasks
            .iter()
            .any(|task| task.statement.body.born == dependency_born)
    );
    assert_eq!(answer.notes.keys().collect::<Vec<_>>(), [&shown_born]);
    let texts = answer.notes[&shown_born]
        .iter()
        .map(|note| note.text.as_str())
        .collect::<Vec<_>>();
    assert_eq!(texts, ["first line\n\nthird line", "later evidence"]);
    let text = task_show_with_origin(
        &answer,
        at(&answer, &shown),
        &[],
        TaskShowHistory {
            notes: &answer.notes[&shown_born],
            acknowledgments: &[],
            closure: None,
            steps: &[],
        },
        None,
    );
    assert!(text.contains("notes:\n  "));
    assert!(text.contains("    first line\n\n    third line\n"));
    assert!(text.contains("    later evidence"));
    assert!(!text.contains("dependency evidence"));
    assert!(read(false).notes.is_empty());

    clean(&root, bus);
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
        &TaskShowOptions {
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
        "description:\n  -",
        "phase ",
        "status ",
        "claim ",
        "requirements:",
        "acceptance:",
        "depends:",
        "artifacts:",
        "next: ",
    ] {
        assert!(current.contains(required), "{current}");
    }
    for historical in [
        "origin ",
        "creation_context ",
        "claim_context ",
        "git_context ",
        "notes:",
    ] {
        assert!(!current.contains(historical), "{current}");
    }

    let notes = (1..=205)
        .map(|index| wire::intent::TaskDetailNote {
            position: Seq::new(1_000 + index).expect("fixture position"),
            at: "2026-10-02T00:00:00.000Z".parse().expect("fixture instant"),
            author: Actor::Operator,
            text: format!("note-{index}"),
        })
        .collect::<Vec<_>>();
    let history = task_show_bounded(
        &answer,
        row,
        &[],
        TaskShowHistory {
            notes: &notes,
            acknowledgments: &[],
            closure: None,
            steps: &[],
        },
        None,
        &TaskShowOptions {
            context: false,
            history: true,
            limit: None,
            max_bytes: None,
            max_text: None,
            after: None,
        },
    )
    .expect("first history page has no cursor");
    assert_eq!(history.matches("    note-").count(), 20, "{history}");
    assert!(history.contains("continue: spice task show"), "{history}");
    assert!(history.contains("--after"), "{history}");

    let oversized = "x".repeat(5_000);
    let context = task_show_bounded(
        &answer,
        row,
        &[],
        TaskShowHistory::default(),
        Some(&oversized),
        &TaskShowOptions {
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

/// The bare view prints the authored statement whole: a description past the
/// text limit keeps every line, and context does not repeat statement fields.
#[test]
fn task_show_prints_the_authored_statement_whole() {
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
    row.statement.body.tags.push("priority:high".to_owned());
    let show = |context: bool| {
        task_show_bounded(
            &answer,
            &row,
            &[],
            TaskShowHistory::default(),
            None,
            &TaskShowOptions {
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
    let indented = format!(
        "  First paragraph {}\n\n    indented second paragraph",
        "x".repeat(300)
    );
    let current = show(false);
    assert!(current.contains("priority high"), "{current}");
    assert!(
        current.contains(&format!("description:\n{indented}\nacceptance:")),
        "{current}"
    );
    let context = show(true);
    assert_eq!(
        context.matches("indented second paragraph").count(),
        1,
        "{context}"
    );
    assert!(context.contains("tags -"), "{context}");
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
        &TaskShowOptions {
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
