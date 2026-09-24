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
fn read(bus: &Bus, epoch: CanonicalUuid, read: &BoardRead) -> Outcome<CommandAnswer> {
    issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::BoardRead,
        Vec::new(),
        read,
    )
}

fn append_terminal_tasks(bus: &Bus, count: usize) -> std::collections::BTreeSet<TaskStamp> {
    let origin: wire::Origin = "fixture:paged-history"
        .parse()
        .expect("history fixture origin");
    let at = Instant::new(jiff::Timestamp::now()).expect("current fixture instant");
    let phase: wire::Phase = "todo".parse().expect("fixture phase");
    let flow = vec![phase.clone()];
    let creation = body::TaskCreationContext::legacy_unavailable();
    let mut write = JournalWrite::new(Actor::Operator, origin.clone(), at, Producer::CommandDoor);
    let mut tasks = std::collections::BTreeSet::new();
    for index in 0..count {
        let micros = 100_000_000_000_i64
            .checked_add(i64::try_from(index).expect("fixture count fits task stamp"))
            .expect("fixture task stamp stays bounded");
        let task = TaskStamp::for_micros(micros).expect("fixture task stamp");
        let title = format!("Terminal history {index}");
        let added = write
            .fact(
                body::TaskAdded {
                    task: task.clone(),
                    title: title.clone(),
                    description: None,
                    acceptance: Vec::new(),
                    requirements: wire::EnvironmentRequirements::default(),
                    project: "board.test".to_owned(),
                    tags: Vec::new(),
                    depends: Vec::new(),
                    flow: flow.clone(),
                    phase: phase.clone(),
                    revision: 1,
                    creation: creation.clone(),
                },
                FactCause::Root,
            )
            .expect("terminal fixture fact builds");
        let statement_task = task.clone();
        let statement_origin = origin.clone();
        let statement_creation = creation.clone();
        write.task_head(added, move |position| TaskHead {
            born: statement_task,
            queue_origin: position,
            title,
            description: None,
            acceptance: Vec::new(),
            requirements: wire::EnvironmentRequirements::default(),
            project: "board.test".to_owned(),
            tags: Vec::new(),
            dependencies: Vec::new(),
            provenance: statement_origin,
            creation: statement_creation,
            revision: position,
        });
        let workflow_task = task.clone();
        let workflow_flow = flow.clone();
        let workflow_phase = phase.clone();
        write.workflow_head(workflow_task, added, move |_| WorkflowHead {
            flow: workflow_flow,
            phase: workflow_phase,
            status: "completed".to_owned(),
            review: "clean".to_owned(),
            assignment: None,
            deferral: None,
            completion: Some(at),
        });
        tasks.insert(task);
    }
    assert!(matches!(
        bus.commit(write).expect("history fixture reaches writer"),
        CommitOutcome::Committed(_)
    ));
    tasks
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

#[test]
fn open_read_offers_the_full_board_allocation_order_from_its_bounded_rows() {
    let (root, bus, epoch, seats) = fixture("offered-board", 1);
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
            validation: "offer order fixture".to_owned(),
            operation: Some(TaskDone::operation_for_claim(
                heads.claim.as_ref().expect("claim").body.claimed,
            )),
        },
    ));
    append_terminal_tasks(&bus, 3);
    let claimed = todo_task(&bus, epoch, "claimed", "Held by a lane");
    claim_task(&bus, epoch, &seats[0], &actor, &claimed);
    let unblocked = todo_task(&bus, epoch, "unblocked", "Ready after its prerequisite");
    let waiting = todo_task(&bus, epoch, "waiting", "Waits on open work");
    let middle = todo_task(&bus, epoch, "middle", "Open prerequisite");
    connect_child(&bus, epoch, &unblocked, &completed);
    connect_child(&bus, epoch, &waiting, &middle);

    let open = scoped(&bus, epoch, Actor::Operator, BoardRead::Open);
    assert_eq!(
        handles(&open),
        [unblocked.clone(), waiting.clone(), middle.clone(), claimed].into(),
        "prerequisite evidence never leaves the resident"
    );
    let full = board(&bus, epoch, Actor::Operator);
    let entries = actor_entries(&full).expect("full board entries validate");
    assert_eq!(
        open.offer,
        crate::board_command::read_guard::board_offer(&entries, &full)
    );
    let offered = open
        .offer
        .iter()
        .map(|born| at_born(&open, born))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(offered, [unblocked, middle].into());
    clean(&root, bus);
}

fn at_born(answer: &BoardAnswer, born: &TaskStamp) -> TaskHandle {
    answer
        .tasks
        .iter()
        .find(|task| &task.statement.body.born == born)
        .expect("offered stamps name answered rows")
        .statement
        .body
        .handle
        .clone()
}
#[test]
fn open_read_excludes_large_history_and_cursor_pages_cover_it_once() {
    const HISTORY_TASKS: usize = 1_301;
    const PAGE_LIMIT: u16 = 64;

    let (root, bus, epoch, _seats) = fixture("paged-board-history", 0);
    let expected = append_terminal_tasks(&bus, HISTORY_TASKS);
    let open_task = todo_task(&bus, epoch, "only-open", "Only open task");
    let open = scoped(&bus, epoch, Actor::Operator, BoardRead::Open);
    assert_eq!(handles(&open), [open_task].into());
    assert_eq!(open.history, None);

    let mut before = None;
    let mut seen = std::collections::BTreeSet::new();
    loop {
        let page = scoped(
            &bus,
            epoch,
            Actor::Operator,
            BoardRead::History {
                before: before.clone(),
                limit: PAGE_LIMIT,
            },
        );
        let metadata = page.history.expect("history read carries page metadata");
        assert_eq!(metadata.before, before);
        assert_eq!(metadata.limit, PAGE_LIMIT);
        assert_eq!(
            metadata.total,
            u64::try_from(HISTORY_TASKS).expect("fixture count fits wire total")
        );
        assert!(page.tasks.len() <= usize::from(PAGE_LIMIT));
        for pair in page.tasks.windows(2) {
            assert!(pair[0].workflow.position > pair[1].workflow.position);
        }
        for task in page.tasks {
            assert!(
                seen.insert(task.statement.body.born),
                "a durable history identity appeared on more than one page"
            );
        }
        let Some(next) = metadata.next else {
            break;
        };
        before = Some(next);
    }
    assert_eq!(seen, expected);
    clean(&root, bus);
}

#[test]
fn history_bounds_empty_page_and_stale_cursor_are_explicit() {
    let (root, bus, epoch, _seats) = fixture("paged-board-history-errors", 0);
    let empty = scoped(
        &bus,
        epoch,
        Actor::Operator,
        BoardRead::History {
            before: None,
            limit: 10,
        },
    );
    assert!(empty.tasks.is_empty());
    assert_eq!(
        empty.history,
        Some(TaskHistoryPage {
            before: None,
            limit: 10,
            total: 0,
            next: None,
        })
    );

    for limit in [0, TASK_HISTORY_PAGE_LIMIT_MAX.saturating_add(1)] {
        assert!(matches!(
            read(
                &bus,
                epoch,
                &BoardRead::History {
                    before: None,
                    limit,
                },
            ),
            Outcome::Refusal(refusal) if refusal.reason() == "board::history-limit-invalid"
        ));
    }
    let stale = TaskHistoryCursor {
        workflow: Seq::new(1).expect("fixture sequence"),
        task: stamp("absent-history-cursor"),
    };
    assert!(matches!(
        read(
            &bus,
            epoch,
            &BoardRead::History {
                before: Some(stale),
                limit: 10,
            },
        ),
        Outcome::Refusal(refusal) if refusal.reason() == "board::history-cursor-invalid"
    ));
    clean(&root, bus);
}

#[test]
fn history_cursor_orders_equal_workflow_heads_by_durable_task_identity() {
    let workflow = Seq::new(7).expect("fixture workflow position");
    let first = TaskHistoryCursor {
        workflow,
        task: TaskStamp::for_micros(10).expect("first fixture task"),
    };
    let second = TaskHistoryCursor {
        workflow,
        task: TaskStamp::for_micros(11).expect("second fixture task"),
    };
    let newer = TaskHistoryCursor {
        workflow: Seq::new(8).expect("newer fixture workflow position"),
        task: TaskStamp::for_micros(12).expect("newer fixture task"),
    };
    let mut cursors = vec![second.clone(), first.clone(), newer.clone()];
    cursors.sort_by(history_cursor_order);
    assert_eq!(cursors, [newer, first, second]);
}
#[test]
fn implicit_completion_reads_completed_prerequisites_before_advancing_a_plan() {
    let (root, bus, epoch, seats) = fixture("scoped-plan-completion", 1);
    let seat = &seats[0];
    let actor = Actor::Lane(seat.lane);
    let completed = todo_task(&bus, epoch, "completed", "Completed prerequisite");
    claim_and_complete_task(&bus, epoch, seat, &actor, &completed);
    let plan = add_project_with_acceptance(
        &bus,
        epoch,
        Actor::Operator,
        "plan",
        "Reconcile completed work",
        "task.read",
        Some("plan,todo,review"),
        &["The completed work is reconciled"],
    );
    connect_child(&bus, epoch, &plan, &completed);
    claim_task(&bus, epoch, seat, &actor, &plan);
    let _unrelated = todo_task(&bus, epoch, "unrelated", "Unrelated work");
    let matches = crate::cli::command()
        .try_get_matches_from(["spice", "task", "done"])
        .expect("implicit completion parses");
    let contract = crate::cli::selected_contract(&matches).expect("completion contract");
    let (_, read) = mutation_read(contract, crate::cli::selected_matches(&matches))
        .expect("completion selects its read");
    let held = scoped(&bus, epoch, actor.clone(), read);
    let heads = at(&held, &plan);
    let expected = progress_witnesses(&held, heads, seat.lane, seat.occupant, false)
        .expect("completed prerequisites remain visible to plan witnesses");
    assert_eq!(handles(&held), [plan.clone(), completed].into());
    assert_committed(issue(
        &bus,
        epoch,
        actor.clone(),
        CommandKind::TaskDone,
        expected,
        &TaskDone {
            task: plan.clone(),
            validation: "Completed work reconciled".to_owned(),
            operation: Some(TaskDone::operation_for_claim(
                heads.claim.as_ref().expect("claim").body.claimed,
            )),
        },
    ));
    let advanced = board(&bus, epoch, actor);
    assert_eq!(at(&advanced, &plan).workflow.body.phase.as_str(), "todo");
    clean(&root, bus);
}
