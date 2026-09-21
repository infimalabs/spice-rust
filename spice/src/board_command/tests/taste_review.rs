use super::*;

fn configured_repository(root: &Path, words: &[&str]) -> Repository {
    fs::create_dir_all(root.join(".git")).expect("taste-review fixture is a repository");
    let words = words
        .iter()
        .map(|word| format!("{word:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    fs::write(
        root.join("spice-rust.toml"),
        format!(
            "[taste.taskwording]\nwords = [{words}]\nmessage = \"Rewrite the task wording.\"\n"
        ),
    )
    .expect("taste-review policy is written");
    Repository::discover(root).expect("taste-review repository resolves")
}

fn issue_repository<B: Serialize>(
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    actor: Actor,
    kind: CommandKind,
    expected: Vec<Witness>,
    body: &B,
) -> Outcome<CommandAnswer> {
    let command = fixture_command(epoch, actor, kind, expected, body);
    let projects = ProjectPolicy::new(
        ["board".to_owned(), "rim".to_owned()],
        ["sandbox".to_owned()],
        dialect::PROJECT_MIN_DEPTH as u64,
        dialect::PROJECT_MAX_DEPTH as u64,
    )
    .expect("taste-review project policy");
    dispatch_with_repository(
        bus,
        &command,
        Some(repository),
        &projects,
        &board::NextConfig::default(),
    )
    .expect("board command owns taste-review fixture kind")
}

fn add_with_repository(
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    task: &str,
    title: &str,
    acceptance: &[&str],
) -> (TaskHandle, Outcome<CommandAnswer>) {
    let born = stamp(task);
    let handle = TaskHandle::for_project("board.test", &born);
    let outcome = issue_repository(
        bus,
        repository,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: born }],
        &TaskAdd {
            title: title.to_owned(),
            project: Some("board.test".to_owned()),
            description: None,
            acceptance: acceptance.iter().map(|item| (*item).to_owned()).collect(),
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some("fixture:taste-review".parse().expect("fixture origin")),
            flow: None,
            depends: Vec::new(),
        },
    );
    (handle, outcome)
}

fn modify_with_repository(
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    actor: Actor,
    task: &TaskHandle,
    title: &str,
) -> Outcome<CommandAnswer> {
    let visible = board(bus, epoch, actor.clone());
    issue_repository(
        bus,
        repository,
        epoch,
        actor,
        CommandKind::TaskEdit,
        vec![statement_witness(at(&visible, task))],
        &TaskEdit {
            task: task.clone(),
            title: Some(title.to_owned()),
            description: None,
            acceptance: Vec::new(),
            requirements: None,
            priority: None,
            project: None,
        },
    )
}

fn ledger_text(bus: &Bus, epoch: CanonicalUuid, actor: Actor, task: &TaskHandle) -> String {
    let outcome = issue(
        bus,
        epoch,
        actor,
        CommandKind::DocLedger,
        Vec::new(),
        &DocLedger { task: task.clone() },
    );
    let Outcome::Reply(Reply {
        body: CommandAnswer::Text(TextAnswer { text }),
        ..
    }) = outcome
    else {
        panic!("ledger read did not reply with text: {outcome:?}");
    };
    text
}

#[test]
fn configured_match_routes_accepted_task_through_durable_taste_review() {
    let (root, bus, epoch, _seats) = fixture("taste-review-create", 0);
    let repository = configured_repository(&root, &["delay"]);

    let (clean_task, clean_outcome) = add_with_repository(
        &bus,
        &repository,
        epoch,
        "TASTE-clean001",
        "Ship the lifecycle change",
        &["focused tests pass"],
    );
    assert_committed(clean_outcome);
    let (review_task, review) = add_with_repository(
        &bus,
        &repository,
        epoch,
        "TASTE-review01",
        "Delay the lifecycle change",
        &["focused tests pass"],
    );
    assert_committed(review);

    let visible = board(&bus, epoch, Actor::Operator);
    let clean_entry = at(&visible, &clean_task);
    assert_eq!(
        clean_entry
            .workflow
            .body
            .flow
            .iter()
            .map(wire::Phase::as_str)
            .collect::<Vec<_>>(),
        ["todo", "review"],
    );
    assert!(!has_taste_review(&clean_entry.statement.body.tags));

    let held = at(&visible, &review_task);
    assert_eq!(held.workflow.body.phase.as_str(), "plan");
    assert_eq!(
        held.workflow
            .body
            .flow
            .iter()
            .map(wire::Phase::as_str)
            .collect::<Vec<_>>(),
        ["plan", "todo", "review"],
    );
    assert!(has_taste_review(&held.statement.body.tags));

    let shown = task_show(&visible, held, &[]);
    assert!(shown.contains("taste_review required"));
    assert!(shown.contains("complete current taste policy rechecks it"));
    assert!(shown.contains(&format!("next: spice task modify {review_task}")));
    assert!(!shown.contains("next: spice task done"));
    assert!(!shown.contains(TASTE_REVIEW_TAG));
    assert!(!shown.contains("delay"));
    assert!(!shown.contains("Rewrite the task wording"));

    let audit = bus
        .fact_snapshot()
        .expect("taste-review facts read")
        .facts()
        .iter()
        .filter_map(|fact| fact.fact.read::<body::TaskNoted>())
        .find(|note| note.task == held.statement.body.born)
        .expect("taste-review audit note exists");
    assert!(audit.note.contains("taste=\"taskwording\""));
    assert!(audit.note.contains("trigger=\"delay\""));
    assert!(audit.note.contains("policy="));

    clean(&root, bus);
}

#[test]
fn modification_rechecks_the_complete_current_policy_before_clearing() {
    let (root, bus, epoch, _seats) = fixture("taste-review-recheck", 0);
    let repository = configured_repository(&root, &["delay"]);
    let (task, outcome) = add_with_repository(
        &bus,
        &repository,
        epoch,
        "TASTE-recheck1",
        "Delay the lifecycle change",
        &["focused tests pass"],
    );
    assert_committed(outcome);

    configured_repository(&root, &["current"]);
    assert_committed(modify_with_repository(
        &bus,
        &repository,
        epoch,
        Actor::Operator,
        &task,
        "Current lifecycle change",
    ));
    let still_held = board(&bus, epoch, Actor::Operator);
    assert!(has_taste_review(
        &at(&still_held, &task).statement.body.tags
    ));

    configured_repository(&root, &["unrelated"]);
    assert_committed(modify_with_repository(
        &bus,
        &repository,
        epoch,
        Actor::Operator,
        &task,
        "Current lifecycle change",
    ));
    let cleared = board(&bus, epoch, Actor::Operator);
    assert!(!has_taste_review(&at(&cleared, &task).statement.body.tags));
    let audits = bus
        .fact_snapshot()
        .expect("taste-review recheck facts read")
        .facts()
        .iter()
        .filter_map(|fact| fact.fact.read::<body::TaskNoted>())
        .filter(|note| note.task == task.born().expect("fixture task stamp"))
        .map(|note| note.note.clone())
        .collect::<Vec<_>>();
    assert!(audits.iter().any(|note| note.contains("outcome=required")));
    assert!(
        audits
            .last()
            .is_some_and(|note| { note.contains("outcome=clear") && note.contains("matches=[]") })
    );

    clean(&root, bus);
}

#[test]
fn blank_acceptance_is_refused_and_whitespace_is_absent_for_default_flow() {
    let (root, bus, epoch, _seats) = fixture("taste-review-blank-acceptance", 0);
    let repository = configured_repository(&root, &["unrelated"]);
    let (_task, outcome) = add_with_repository(
        &bus,
        &repository,
        epoch,
        "TASTE-blank001",
        "Ship the lifecycle change",
        &["   "],
    );
    assert!(
        matches!(outcome, Outcome::Refusal(ref refusal) if refusal.reason() == "task::acceptance-empty"),
        "blank acceptance did not refuse: {outcome:?}",
    );
    assert_eq!(
        task_flow(None, &[" \t".to_owned()])
            .expect("legacy blank acceptance chooses a flow")
            .iter()
            .map(wire::Phase::as_str)
            .collect::<Vec<_>>(),
        ["plan", "todo", "review"],
    );
    clean(&root, bus);
}

fn claim_held(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    task: &TaskHandle,
) -> wire::intent::ClaimAnswer {
    let lane = Actor::Lane(seat.lane);
    let available = board(bus, epoch, lane.clone());
    let won = assert_committed(issue(
        bus,
        epoch,
        lane,
        CommandKind::ClaimWin,
        claim_expected(&available, task, seat),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
            recovery: None,
        },
    ));
    let CommandAnswer::Claim(won) = won else {
        panic!("claim win returned another answer shape");
    };
    *won
}

fn plan_done(
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    seat: &Seat,
    task: &TaskHandle,
) -> Outcome<CommandAnswer> {
    let lane = Actor::Lane(seat.lane);
    let held = board(bus, epoch, lane.clone());
    issue_repository(
        bus,
        repository,
        epoch,
        lane,
        CommandKind::TaskDone,
        progress_witnesses(&held, at(&held, task), seat.lane, seat.occupant, false)
            .expect("held taste review has progress witnesses"),
        &TaskDone {
            task: task.clone(),
            validation: "acceptance is already substantive".to_owned(),
            operation: Some(completion_operation()),
        },
    )
}

#[test]
fn plan_completion_refuses_while_taste_review_marker_remains() {
    let (root, bus, epoch, seats) = fixture("taste-review-plan-gate", 1);
    let repository = configured_repository(&root, &["delay"]);
    let (task, outcome) = add_with_repository(
        &bus,
        &repository,
        epoch,
        "TASTE-plan0001",
        "Delay the lifecycle change",
        &["focused tests pass"],
    );
    assert_committed(outcome);
    let won = claim_held(&bus, epoch, &seats[0], &task);
    assert_eq!(
        won.next.as_deref(),
        Some(format!("spice task modify {task} --title \"...\"").as_str()),
        "assignment named a read instead of the rewording the hold needs",
    );

    let outcome = plan_done(&bus, &repository, epoch, &seats[0], &task);
    let Outcome::Refusal(refusal) = outcome else {
        panic!("taste review plan completion did not refuse: {outcome:?}");
    };
    assert_eq!(refusal.reason(), "task::taste-review-required");
    let diagnostic = format!("{refusal:?}");
    assert!(diagnostic.contains("complete current taste policy"));
    assert!(diagnostic.contains("the filed statement has not been revised"));
    assert!(!diagnostic.contains("delay"));
    assert!(!diagnostic.contains("Rewrite the task wording"));

    // A reworded statement that still matches must read differently from the
    // first refusal, or a lane cannot tell a recheck from a repetition.
    assert_committed(modify_with_repository(
        &bus,
        &repository,
        epoch,
        Actor::Lane(seats[0].lane),
        &task,
        "Delay the lifecycle change once more",
    ));
    let outcome = plan_done(&bus, &repository, epoch, &seats[0], &task);
    let Outcome::Refusal(refusal) = outcome else {
        panic!("reworded taste review plan completion did not refuse: {outcome:?}");
    };
    let diagnostic = format!("{refusal:?}");
    assert!(
        diagnostic.contains("one revision has been rechecked"),
        "the second refusal repeated the first: {diagnostic}",
    );
    assert!(!diagnostic.contains("delay"));

    clean(&root, bus);
}

#[test]
fn a_hold_speaks_on_status_while_its_match_stays_withheld_from_the_lane() {
    let (root, bus, epoch, seats) = fixture("taste-review-hold-visibility", 1);
    let repository = configured_repository(&root, &["delay"]);
    let (task, outcome) = add_with_repository(
        &bus,
        &repository,
        epoch,
        "TASTE-hold0001",
        "Delay the lifecycle change",
        &["focused tests pass"],
    );
    assert_committed(outcome);
    let lane = Actor::Lane(seats[0].lane);
    claim_held(&bus, epoch, &seats[0], &task);

    let held = board(&bus, epoch, lane.clone());
    let status = task_status(&held).expect("validated board yields status");
    assert!(
        status
            .lines()
            .any(|line| line == format!("taste-review {task}")),
        "status did not name the held task: {status}",
    );

    let read = ledger_text(&bus, epoch, lane, &task);
    assert!(read.contains(AUDIT_PREFIX));
    assert!(read.contains("field=\"title\""));
    assert!(read.contains("trigger=withheld"));
    assert!(read.contains("message=withheld"));
    assert!(read.contains("column=withheld"));
    assert!(!read.contains("\"delay\""));
    assert!(!read.contains("Rewrite the task wording"));

    let owned = ledger_text(&bus, epoch, Actor::Operator, &task);
    assert!(owned.contains("trigger=\"delay\""));
    assert!(owned.contains("Rewrite the task wording"));
    assert!(!owned.contains("column=withheld"));

    clean(&root, bus);
}
