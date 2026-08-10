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

fn fixture_instant(context: &str) -> Instant {
    "2030-04-12T18:44:09.271Z"
        .parse()
        .unwrap_or_else(|error| panic!("{context}: {error}"))
}

/// A held task is named on the claim line and counted as active.
///
/// The snapshot above reports every claim field at rest, where a wrong
/// predicate and a right one agree: with nothing claimed, `claim -` and
/// `active 0` are what any filter produces. Bootstrap step three exists to
/// tell an agent which handle it already holds, so the reading that matters
/// is the one taken while a claim is live, and it has to distinguish the
/// principal asking from the other lanes holding rows of their own.
#[test]
fn task_status_names_the_claim_the_asking_principal_holds() {
    let (root, bus, epoch, seats) = fixture("task-bootstrap-claim", 2);
    let lane = Actor::Lane(seats[0].lane);
    let held = add(
        &bus,
        epoch,
        Actor::Operator,
        "CLI-held00001",
        "Hold one native bootstrap row",
        Some("todo"),
    );
    let available = board(&bus, epoch, lane.clone());
    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        claim_expected(&available, &held, &seats[0]),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
        },
    ));

    let holder = task_status(&board(&bus, epoch, lane)).expect("holder reads status");
    assert!(
        holder.lines().any(|line| line == format!("claim {held}")),
        "{holder}"
    );
    assert!(holder.lines().any(|line| line == "active 1"), "{holder}");

    // A peer's claim is still active work on the board, but it is not this
    // principal's, so the same row has to count without being named.
    let other =
        task_status(&board(&bus, epoch, Actor::Lane(seats[1].lane))).expect("peer reads status");
    assert!(other.lines().any(|line| line == "claim -"), "{other}");
    assert!(other.lines().any(|line| line == "active 1"), "{other}");
    clean(&root, bus);
}

#[test]
fn current_board_edits_are_guarded_deterministic_and_render_form_invariant() {
    let (root, bus, epoch, seats) = fixture("edits", 1);
    let initial = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        actor_seat(&initial).expect("sole operator seat"),
        (seats[0].lane, seats[0].occupant,)
    );
    let (alpha, beta, oops) = seed_edit_tasks(&bus, epoch);
    exercise_deferral_staleness(&bus, epoch, &alpha);
    add_dependency(&bus, epoch, &alpha, &beta);
    let graph = assert_graph_handout_and_ledger(&bus, epoch, &alpha);
    assert_render_form_parity(&graph);
    assert_oops_note_exists(&bus, &oops);
    clean(&root, bus);
}

#[test]
fn capture_claims_one_exact_eligible_non_allocator_task() {
    let (root, bus, epoch, seats) = fixture("capture", 1);
    let lane = Actor::Lane(seats[0].lane);
    let alpha = add(
        &bus,
        epoch,
        Actor::Operator,
        "CAP-alpha001",
        "Allocator winner",
        Some("todo"),
    );
    let beta = add(
        &bus,
        epoch,
        Actor::Operator,
        "CAP-beta0001",
        "Exact capture target",
        Some("todo"),
    );
    let available = board(&bus, epoch, lane.clone());
    assert_eq!(available.active_public_tasks().count(), 2);
    let entries = actor_entries(&available).expect("capture board entries are valid");
    assert_eq!(
        board::select(
            &entries,
            &lane,
            allocation_subscription(&available.residency.body, &lane)
                .expect("fixture lane subscription"),
            available.allocation_band,
            available.recent_cell.as_ref(),
        )
        .expect("allocator winner")
        .entry()
        .handle(),
        &alpha,
    );
    let heads = at(&available, &beta);
    let rule: Name = "capture".parse().expect("capture rule");
    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
            Witness::Eligibility { rule: rule.clone() },
            residency(&available, &seats[0]),
        ],
        &ClaimWin { rule },
    ));

    let claimed = board(&bus, epoch, lane);
    assert!(at(&claimed, &alpha).claim.is_none());
    assert_eq!(
        at(&claimed, &beta)
            .claim
            .as_ref()
            .expect("capture claim")
            .body
            .disposition,
        ClaimDisposition::Active,
    );
    clean(&root, bus);
}

#[test]
fn claims_warn_on_shared_acceptance_surface_not_shared_project_stem() {
    let (root, bus, epoch, seats) = fixture("claim-overlap", 3);
    let first = add_project_with_acceptance(
        &bus,
        epoch,
        Actor::Operator,
        "MOSAIC-1kKQTw1V",
        "Canvas lacks the mosaic lane interface",
        "board.alpha",
        Some("todo"),
        &["The watchband mosaic uses per-lane cards and stays stable"],
    );
    let overlapping = add_project_with_acceptance(
        &bus,
        epoch,
        Actor::Operator,
        "FIDELIT-1kKRSfyV",
        "Rebuild the lane watchband and message mosaic",
        "rim.beta",
        Some("todo"),
        &["The mosaic watchband keeps focused-agent state visible"],
    );
    let same_stem = add_project_with_acceptance(
        &bus,
        epoch,
        Actor::Operator,
        "MOSAIC-1kKQTw1W",
        "Audit unrelated board output",
        "board.gamma",
        Some("todo"),
        &["Export signed ledger receipts for audit and keep output stable"],
    );

    let first_answer = capture_for(&bus, epoch, &seats[0], &first);
    assert!(first_answer.warnings.is_empty());

    let overlap_answer = capture_for(&bus, epoch, &seats[1], &overlapping);
    assert_eq!(
        overlap_answer.warnings,
        [wire::intent::ClaimWarning {
            task: first,
            lane: seats[0].lane,
            surface: vec!["mosaic".to_owned(), "watchband".to_owned()],
        }]
    );

    let same_stem_answer = capture_for(&bus, epoch, &seats[2], &same_stem);
    assert!(same_stem_answer.warnings.is_empty());
    clean(&root, bus);
}

fn capture_for(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    task: &TaskHandle,
) -> wire::intent::ClaimAnswer {
    let lane = Actor::Lane(seat.lane);
    let available = board(bus, epoch, lane.clone());
    let heads = at(&available, task);
    let rule: Name = "capture".parse().expect("capture rule");
    let answer = assert_committed(issue(
        bus,
        epoch,
        lane,
        CommandKind::ClaimWin,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
            Witness::Eligibility { rule: rule.clone() },
            residency(&available, seat),
        ],
        &ClaimWin { rule },
    ));
    let CommandAnswer::Claim(answer) = answer else {
        panic!("claim win returned another answer shape: {answer:?}");
    };
    answer
}

#[test]
fn stealing_takes_a_live_claim_at_once_where_capture_is_refused() {
    let (root, bus, epoch, seats) = fixture("steal", 2);
    let holder = Actor::Lane(seats[0].lane);
    let rival = Actor::Lane(seats[1].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "STEAL-live01",
        "Held under a live lease",
        Some("todo"),
    );

    let available = board(&bus, epoch, holder.clone());
    let heads = at(&available, &task);
    let capture: Name = "capture".parse().expect("capture rule");
    assert_committed(issue(
        &bus,
        epoch,
        holder.clone(),
        CommandKind::ClaimWin,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
            Witness::Eligibility {
                rule: capture.clone(),
            },
            residency(&available, &seats[0]),
        ],
        &ClaimWin { rule: capture },
    ));

    let held = board(&bus, epoch, rival.clone());
    let heads = at(&held, &task);
    let witnesses = |rule: &Name| {
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
            Witness::Eligibility { rule: rule.clone() },
            residency(&held, &seats[1]),
        ]
    };

    // Same board, same rival, same witnesses: only the rule differs, so a
    // steal that behaved like a capture would be refused right here.
    let capture: Name = "capture".parse().expect("capture rule");
    let declined = issue(
        &bus,
        epoch,
        rival.clone(),
        CommandKind::ClaimWin,
        witnesses(&capture),
        &ClaimWin { rule: capture },
    );
    assert!(
        matches!(
            declined,
            Outcome::Refusal(ref refusal)
                if refusal.reason() == "claim::capture-ineligible"
        ),
        "capture took a held claim: {declined:?}",
    );

    let steal: Name = "steal".parse().expect("steal rule");
    assert_committed(issue(
        &bus,
        epoch,
        rival.clone(),
        CommandKind::ClaimWin,
        witnesses(&steal),
        &ClaimWin { rule: steal },
    ));

    // The lease the holder still had did not delay the transfer, and the
    // ClaimWon naming the new holder is what tells the old one.
    let after = board(&bus, epoch, rival.clone());
    let taken = at(&after, &task).claim.as_ref().expect("stolen claim");
    assert_eq!(taken.body.actor, rival);
    assert_eq!(taken.body.disposition, ClaimDisposition::Active);
    assert_ne!(taken.body.actor, holder);
    clean(&root, bus);
}

#[test]
fn reclaim_renews_only_the_lane_owned_claim_and_survives_restart() {
    let (root, bus, epoch, seats) = fixture("reclaim", 1);
    let lane = Actor::Lane(seats[0].lane);
    let held_task = add(
        &bus,
        epoch,
        Actor::Operator,
        "RECLAIM-held1",
        "Held recovery work",
        Some("todo"),
    );
    let available_task = add(
        &bus,
        epoch,
        Actor::Operator,
        "RECLAIM-next1",
        "Next allocator work",
        Some("todo"),
    );
    let available = board(&bus, epoch, lane.clone());
    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        claim_expected(&available, &held_task, &seats[0]),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
        },
    ));
    let held = board(&bus, epoch, lane.clone());
    let before = at(&held, &held_task).clone();
    let claim = before.claim.as_ref().expect("active claim");
    let lease = before.lease.as_ref().expect("active lease");
    let expected = vec![
        Witness::Claim {
            task: before.statement.body.born.clone(),
            head: Some(claim.position),
        },
        Witness::Lease {
            task: before.statement.body.born.clone(),
            head: lease.position,
        },
        residency(&held, &seats[0]),
    ];
    let after = renew_and_assert(&bus, epoch, &lane, &held_task, &before, expected);
    assert_reclaim_survives_restart(
        &root,
        bus,
        epoch,
        &lane,
        &held_task,
        &available_task,
        &after,
    );
}
