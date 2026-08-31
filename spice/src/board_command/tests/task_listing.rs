use super::*;

#[test]
fn native_task_handles_use_the_predecessor_stamp_alphabet() {
    const PREDECESSOR_ALPHABET: &[u8; 52] = b"0123456789BCDFGHJKLMNPQRSTVWXYZbcdfghjklmnpqrstvwxyz";

    for (value, expected) in PREDECESSOR_ALPHABET.iter().copied().enumerate() {
        let handle = mint_handle_at(Some("task.identity"), "ignored", value as i64);
        let (_, stamp) = handle
            .as_str()
            .split_once('-')
            .expect("native handle carries a stem and stamp");
        assert_eq!(stamp.len(), steer::Key::WIDTH);
        assert!(
            stamp
                .bytes()
                .all(|byte| PREDECESSOR_ALPHABET.contains(&byte)),
            "{stamp:?} is outside the predecessor task alphabet"
        );
        assert_eq!(stamp.as_bytes()[steer::Key::WIDTH - 1], expected);
    }
}
/// Shape CASE 147 / INVARIANTS ID-02: lexical identity order is birth order.
#[test]
fn plain_string_stamp_and_handle_order_is_birth_order() {
    let millis = [2_705_i64, 51, 2_704, 52, 53];
    let mut stamps = millis
        .into_iter()
        .map(|millis| wire::TaskStamp::for_millis(millis).expect("fixture birth stamp"))
        .collect::<Vec<_>>();
    stamps.sort();
    let expected = [51_i64, 52, 53, 2_704, 2_705]
        .into_iter()
        .map(|millis| wire::TaskStamp::for_millis(millis).expect("fixture birth stamp"))
        .collect::<Vec<_>>();
    assert_eq!(stamps, expected);

    let mut handles = millis
        .into_iter()
        .map(|millis| {
            TaskHandle::for_project(
                "task.identity",
                &wire::TaskStamp::for_millis(millis).expect("fixture birth stamp"),
            )
        })
        .collect::<Vec<_>>();
    handles.sort();
    assert_eq!(
        handles
            .iter()
            .map(|handle| handle.born().expect("sorted handle stamp"))
            .collect::<Vec<_>>(),
        expected
    );
}

/// Shape CASE 148 / INVARIANTS ID-03: every creation route advances collisions.
#[test]
fn same_millisecond_adds_followups_and_documents_receive_ordered_distinct_stamps() {
    let alphabet = (0..52_i64)
        .map(|millis| {
            wire::TaskStamp::for_millis(millis)
                .expect("alphabet fixture birth stamp")
                .as_str()
                .as_bytes()[wire::TaskStamp::WIDTH - 1]
        })
        .collect::<Vec<_>>();
    assert!(alphabet.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(
        alphabet.iter().all(|byte| !b"AEIOUaeiou".contains(byte)),
        "task stamp alphabet must remain vowel-free"
    );

    let (root, bus, epoch, _seats) = fixture("identity-collision", 0);
    let candidate_millis = 90_000_i64;
    let first_stamp =
        wire::TaskStamp::for_millis(candidate_millis).expect("first fixture birth stamp");
    add_born(
        &bus,
        epoch,
        first_stamp.clone(),
        "First same-millisecond task",
    );

    let after_first = board(&bus, epoch, Actor::Operator);
    let second_stamp = allocate_task_stamp(&after_first, candidate_millis);
    assert_eq!(
        second_stamp,
        wire::TaskStamp::for_millis(candidate_millis + 1).expect("second fixture birth stamp")
    );
    add_born(
        &bus,
        epoch,
        second_stamp.clone(),
        "Second same-millisecond task",
    );

    let after_second = board(&bus, epoch, Actor::Operator);
    let followups = allocate_task_stamps(&after_second, candidate_millis, 3);
    let expected = (2..5)
        .map(|offset| {
            wire::TaskStamp::for_millis(candidate_millis + offset)
                .expect("advanced fixture birth stamp")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        followups, expected,
        "review follow-up planning advances +1 ms"
    );

    let preview = board::Preview::classify("# Goal\n\n- First node\n- Second node\n")
        .expect("document collision fixture classifies");
    let document = document_handles(preview.document(), &after_second, candidate_millis);
    assert_eq!(
        document
            .iter()
            .map(|handle| handle.born().expect("document handle stamp"))
            .collect::<Vec<_>>(),
        expected,
        "authored-document creation advances over the same occupied instants"
    );
    clean(&root, bus);
}

#[test]
fn a_nonfinal_review_phase_prints_the_transition_it_can_accept() {
    let (root, bus, epoch, seats) = fixture("nonfinal-review-next-action", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "FLOW-review01",
        "Advance through a nonfinal review phase",
        Some("review,verify"),
    );
    let available = board(&bus, epoch, lane.clone());
    let answer = assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        claim_expected(&available, &task, &seats[0]),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
            recovery: None,
        },
    ));
    let CommandAnswer::Claim(claim) = answer else {
        panic!("claim win returned another answer shape");
    };
    let expected_next = format!("spice task show {task}");
    assert_eq!(
        claim.next.as_deref(),
        Some(expected_next.as_str()),
        "a fresh claim hands activation to the full task packet first",
    );
    let claimed = board(&bus, epoch, lane);
    let shown = task_show(&claimed, at(&claimed, &task), &[]);
    assert!(
        shown.contains(&format!(
            "next: spice task review {task} --finding clean --note \"description current; ...\""
        )),
        "the detail packet exposes the phase-specific command after activation"
    );
    clean(&root, bus);
}

#[test]
fn active_todo_task_show_prints_the_parser_owned_done_command() {
    let (root, bus, epoch, seats) = fixture("todo-show-next-action", 1);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "SHOW-todo0001",
        "Render the active todo next action",
        Some("todo"),
    );
    let claim = capture_for(&bus, epoch, &seats[0], &task);
    assert_eq!(claim.task, task);

    let claimed = board(&bus, epoch, Actor::Lane(seats[0].lane));
    let shown = task_show(&claimed, at(&claimed, &task), &[]);
    assert!(
        shown.contains("next: spice task done --validation \"...\""),
        "the detail packet exposes the parser-owned task done form: {shown}"
    );
    assert!(
        !shown.contains(&format!("next: spice task done {task}")),
        "the detail packet must not advertise the rejected handle form: {shown}"
    );
    clean(&root, bus);
}

#[test]
fn active_plan_task_show_prints_the_execution_contract_and_native_commands() {
    let (root, bus, epoch, seats) = fixture("plan-show-guidance", 1);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "SHOW-plan0001",
        "Render actionable plan guidance",
        Some("plan,todo,review"),
    );
    let claim = capture_for(&bus, epoch, &seats[0], &task);
    assert_eq!(
        claim.next.as_deref(),
        Some(format!("spice task show {task}").as_str())
    );

    let claimed = board(&bus, epoch, Actor::Lane(seats[0].lane));
    let shown = task_show(&claimed, at(&claimed, &task), &[]);
    assert!(shown.contains("phase_guidance:\n  phase:plan makes the execution contract explicit"));
    assert!(shown.contains(&format!("spice task modify {task} --acceptance \"...\"")));
    assert!(shown.contains("spice task add \"...\" --project PROJECT --acceptance \"...\""));
    assert!(shown.contains(&format!("spice task depends {task} CHILD")));
    assert!(shown.contains(&format!("spice task note {task} \"...\"")));
    assert!(shown.contains("Planning changes board state, not repository files"));
    assert!(shown.contains("spice task done --validation \"...\""));
    clean(&root, bus);
}

#[test]
fn plan_only_task_show_requires_an_accepted_connected_child() {
    let (root, bus, epoch, seats) = fixture("plan-only-show-guidance", 1);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "SHOW-plan0002",
        "Render plan-only guidance",
        Some("plan"),
    );
    capture_for(&bus, epoch, &seats[0], &task);

    let claimed = board(&bus, epoch, Actor::Lane(seats[0].lane));
    let shown = task_show(&claimed, at(&claimed, &task), &[]);
    assert!(shown.contains("phase:plan is this task's entire flow"));
    assert!(shown.contains("acceptance on this task alone cannot complete it"));
    assert!(shown.contains("connect at least one pending child task carrying acceptance"));
    assert!(shown.contains("spice task add \"...\" --project PROJECT --acceptance \"...\""));
    assert!(shown.contains(&format!("spice task depends {task} CHILD")));
    assert!(shown.contains("complete the planning bookend with spice task done"));
    clean(&root, bus);
}

#[test]
fn task_status_counts_one_snapshot_and_task_add_retains_authored_context() {
    let (root, bus, epoch, _) = fixture("task-bootstrap", 1);
    let born = wire::TaskStamp::for_millis(8_001).expect("fixture task stamp");
    let task = TaskHandle::for_project("task.cli", &born);
    let origin: wire::Origin = "ack:1kLGp2NJ".parse().expect("fixture origin");
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: born }],
        &TaskAdd {
            title: "Carry native bootstrap context".to_owned(),
            project: Some("task.cli".to_owned()),
            description: Some("The agent needs the complete handoff".to_owned()),
            acceptance: vec![
                "Status reports board counts".to_owned(),
                "Creation provenance survives".to_owned(),
            ],
            origin: Some(origin.clone()),
            flow: Some("todo".to_owned()),
            depends: Vec::new(),
        },
    ));
    add(
        &bus,
        epoch,
        Actor::Operator,
        "CLI-review001",
        "Review native bootstrap",
        Some("review"),
    );

    let answer = board(&bus, epoch, Actor::Operator);
    let statement = &at(&answer, &task).statement.body;
    assert_eq!(
        statement.description.as_deref(),
        Some("The agent needs the complete handoff")
    );
    assert_eq!(
        statement.acceptance,
        [
            "Status reports board counts",
            "Creation provenance survives"
        ]
    );
    assert_eq!(statement.provenance, origin);
    assert_eq!(creation_fact_origin(&bus, &task), origin);
    assert_eq!(at(&answer, &task).origin_thread, "operator");

    let status = task_status(&answer).expect("validated board yields status");
    for expected in [
        "claim -",
        "actor operator",
        concat!(
            "filter ( project:agent.operator.task or origin_thread.is:operator or ",
            "( project:board or project:rim or project:serve or project:task ) )"
        ),
        "public task project depth 2..3 dotted segments",
        "active 0",
        "ready 1",
        "review 1",
        "blocked 0",
        "waiting 0",
        "stale 0",
        "oops 0",
    ] {
        assert!(status.lines().any(|line| line == expected), "{status}");
    }
    clean(&root, bus);
}

#[test]
fn task_list_preserves_scope_status_project_and_newest_limit_semantics() {
    let (root, bus, epoch, _) = fixture("task-list", 0);
    let (answer, first, descendant, waiting) = task_list_fixture(&bus, epoch);
    assert_task_list_queries(&answer, &first, &descendant, &waiting);
    clean(&root, bus);
}

#[test]
fn shortened_task_list_names_original_and_withheld_row_counts() {
    let (root, bus, epoch, _) = fixture("task-list-shortened", 0);
    let (answer, _, _, _) = task_list_fixture(&bus, epoch);
    let listed = task_list(
        &answer,
        &TaskListOptions {
            all: true,
            project: None,
            limit: Some(2),
            status: None,
        },
    );

    assert!(
        listed
            .lines()
            .any(|line| line == "limited --limit 2 original 4 withheld 2"),
        "{listed}"
    );
    clean(&root, bus);
}

#[test]
fn complete_task_list_emits_no_withheld_notice() {
    let (root, bus, epoch, _) = fixture("task-list-complete", 0);
    let (answer, _, _, _) = task_list_fixture(&bus, epoch);
    let listed = task_list(
        &answer,
        &TaskListOptions {
            all: true,
            project: None,
            limit: Some(4),
            status: None,
        },
    );

    assert!(!listed.contains("withheld"), "{listed}");
    clean(&root, bus);
}

#[test]
fn actor_route_projects_the_current_lane_session_identity() {
    let (root, bus, epoch, seats) = fixture("actor-route-session", 1);
    let answer = board(&bus, epoch, Actor::Operator);
    let mut residency = answer.residency.body;
    let identity = uuid(9_001);
    let row = residency.lanes.first_mut().expect("fixture lane");
    let occupant = row.occupant.expect("fixture occupant");
    let legacy = actor_route(&Actor::Lane(seats[0].lane), &residency)
        .expect("a legacy occupied seat names its occupant route");
    assert_eq!(legacy.actor, occupant.get().simple().to_string());

    let row = residency.lanes.first_mut().expect("fixture lane");
    row.session = Some(wire::LaneSession {
        lane: row.lane,
        occupant,
        worktree: row.worktree.clone(),
        vendor: row.vendor.clone(),
        launch: Some(wire::body::LaneLaunchIdentity::new("sonnet", "high")),
        epoch,
        stream: "stdout".to_owned(),
        identity: identity.to_string(),
        locus: wire::TapeLocus {
            member: 0,
            offset: 0,
            length: 1,
        },
    });

    let route = actor_route(&Actor::Lane(seats[0].lane), &residency)
        .expect("current session identity names a route");
    let actor = identity.get().simple().to_string();
    assert_eq!(route.actor, actor);
    assert_eq!(route.project, format!("agent.{actor}.task"));
    assert_eq!(route.origin_thread, actor);

    residency.lanes[0]
        .session
        .as_mut()
        .expect("fixture session")
        .identity = "session-id-with-hyphens".to_owned();
    assert!(
        actor_route(&Actor::Lane(seats[0].lane), &residency).is_err(),
        "a current malformed session route must fail instead of falling back"
    );
    clean(&root, bus);
}

#[test]
fn lane_task_origin_thread_comes_from_its_durable_creation_session() {
    let (root, bus, epoch, seats) = fixture("task-origin-session", 1);
    let first_identity = uuid(9_101);
    declare_session(&bus, epoch, &seats[0], first_identity, 101);
    let first = add(
        &bus,
        epoch,
        Actor::Lane(seats[0].lane),
        "CLI-origin001",
        "Retain the creating session",
        Some("todo"),
    );
    let second_identity = uuid(9_102);
    declare_session(&bus, epoch, &seats[0], second_identity, 102);
    let second = add(
        &bus,
        epoch,
        Actor::Lane(seats[0].lane),
        "CLI-origin002",
        "Retain the successor session",
        Some("todo"),
    );
    let answer = board(&bus, epoch, Actor::Lane(seats[0].lane));
    let first_actor = first_identity.get().simple().to_string();
    let second_actor = second_identity.get().simple().to_string();
    assert_eq!(answer.actor_route.origin_thread, second_actor);
    assert_eq!(at(&answer, &first).origin_thread, first_actor);
    assert_eq!(at(&answer, &second).origin_thread, second_actor);
    clean(&root, bus);
}

fn task_list_fixture(
    bus: &Bus,
    epoch: CanonicalUuid,
) -> (BoardAnswer, TaskHandle, TaskHandle, TaskHandle) {
    let first = add_project(
        bus,
        epoch,
        Actor::Operator,
        "CLI-list0001",
        "First task route",
        "task.cli",
        Some("todo"),
    );
    let descendant = add_project(
        bus,
        epoch,
        Actor::Operator,
        "CLI-list0002",
        "Descendant task route",
        "task.cli.deep",
        Some("todo"),
    );
    let waiting = add_project(
        bus,
        epoch,
        Actor::Operator,
        "CLI-list0003",
        "Deferred serve route",
        "serve.ops",
        Some("todo"),
    );
    let completed = add_project(
        bus,
        epoch,
        Actor::Operator,
        "CLI-list0004",
        "Completed board route",
        "board.test",
        Some("todo"),
    );
    let mut answer = board(bus, epoch, Actor::Operator);
    answer
        .tasks
        .iter_mut()
        .find(|task| task.statement.body.handle == first)
        .expect("first row")
        .statement
        .body
        .tags
        .push("priority:medium".to_owned());
    answer
        .tasks
        .iter_mut()
        .find(|task| task.statement.body.handle == waiting)
        .expect("waiting row")
        .workflow
        .body
        .deferral = Some(fixture_instant("fixture deferral"));
    let completed_row = answer
        .tasks
        .iter_mut()
        .find(|task| task.statement.body.handle == completed)
        .expect("completed row");
    completed_row.workflow.body.status = "completed".to_owned();
    completed_row.workflow.body.completion = Some(fixture_instant("fixture completion"));
    (answer, first, descendant, waiting)
}

fn assert_task_list_queries(
    answer: &BoardAnswer,
    first: &TaskHandle,
    descendant: &TaskHandle,
    waiting: &TaskHandle,
) {
    let default = task_list(
        answer,
        &TaskListOptions {
            all: false,
            project: None,
            limit: None,
            status: None,
        },
    );
    assert!(default.starts_with(concat!(
        "scope actor-route filter ",
        "( project:agent.operator.task or origin_thread.is:operator or ",
        "( project:board or project:rim or project:serve or project:task ) )\n"
    )));
    assert!(default.contains(&format!("{first} [todo] P:M task.cli First task route")));
    assert!(default.contains(&format!(
        "{descendant} [todo] task.cli.deep Descendant task route"
    )));
    assert!(!default.contains(waiting.as_str()), "{default}");

    let project = task_list(
        answer,
        &TaskListOptions {
            all: false,
            project: Some("task.cli".to_owned()),
            limit: None,
            status: Some(TaskListStatus::Pending),
        },
    );
    assert!(project.starts_with("scope explicit-project project:task.cli\n"));
    assert!(project.contains(first.as_str()));
    assert!(project.contains(descendant.as_str()));
    assert!(!project.contains(waiting.as_str()));

    let waiting_only = task_list(
        answer,
        &TaskListOptions {
            all: false,
            project: None,
            limit: None,
            status: Some(TaskListStatus::Waiting),
        },
    );
    assert!(waiting_only.contains(waiting.as_str()));
    assert!(!waiting_only.contains(first.as_str()));

    let completed_only = task_list(
        answer,
        &TaskListOptions {
            all: false,
            project: None,
            limit: None,
            status: Some(TaskListStatus::Completed),
        },
    );
    assert!(completed_only.contains("board.test Completed board route"));
    assert!(!completed_only.contains(first.as_str()));

    let newest_global = task_list(
        answer,
        &TaskListOptions {
            all: true,
            project: None,
            limit: Some(1),
            status: None,
        },
    );
    let completed = answer
        .tasks
        .iter()
        .find(|task| task.presentation_title() == "Completed board route")
        .expect("completed fixture row");
    assert_eq!(
        newest_global,
        format!(
            concat!(
                "scope global --all\n",
                "limited --limit 1 original 4 withheld 3\n",
                "{} [done] board.test Completed board route"
            ),
            completed.statement.body.handle
        )
    );
}
