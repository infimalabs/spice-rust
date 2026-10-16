use super::*;

pub(super) fn configured_repository(root: &Path, words: &[&str]) -> Repository {
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

pub(super) fn issue_repository<B: Serialize>(
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
            preparation: None,
            intent: None,
            title: title.to_owned(),
            project: Some("board.test".to_owned()),
            description: None,
            acceptance: acceptance.iter().map(|item| (*item).to_owned()).collect(),
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some(fixture_origin()),
            flow: None,
            depends: Vec::new(),
            claim: false,
            draft: false,
        },
    );
    (handle, outcome)
}

pub(super) fn modify_with_repository(
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
            intent: None,
            evidence: None,
            task: task.clone(),
            title: Some(title.to_owned()),
            description: None,
            acceptance: Vec::new(),
            requirements: None,
            priority: None,
            project: None,
            flow: None,
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
    assert_eq!(clean_entry.statement.body.taste, None);

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
    assert_eq!(held.statement.body.taste, Some(wire::TasteHold::Prepended));

    let shown = task_show(&visible, held, &[]);
    assert!(shown.contains("taste_review required"));
    assert!(shown.contains(&format!("next: spice task modify {review_task}")));
    assert!(!shown.contains("next: spice task done"));
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
fn compound_capture_taste_review_refuses_before_creating_task_or_claim() {
    let (root, bus, epoch, seats) = fixture("taste-review-capture", 1);
    let repository = configured_repository(&root, &["delay"]);
    let seat = &seats[0];
    authorize_lane_creation(&bus, epoch, seat);
    let actor = Actor::Lane(seat.lane);
    let before = board(&bus, epoch, actor.clone());
    let born = stamp("TASTE-capture1");
    let handle = TaskHandle::for_project("board.test", &born);
    let facts_before = bus
        .fact_snapshot()
        .expect("taste-review facts read")
        .facts()
        .len();
    let outcome = issue_repository(
        &bus,
        &repository,
        epoch,
        actor.clone(),
        CommandKind::TaskAdd,
        vec![
            Witness::Absence { task: born },
            Witness::Eligibility {
                rule: CAPTURE_WORK_RULE
                    .parse()
                    .expect("capturework is a wire name"),
            },
            residency(&before, seat),
        ],
        &TaskAdd {
            preparation: None,
            intent: None,
            title: "Delay the captured implementation".to_owned(),
            project: Some("board.test".to_owned()),
            description: None,
            acceptance: Vec::new(),
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some("ack:1kLGp2NJ".parse().expect("fixture steering origin")),
            flow: None,
            depends: Vec::new(),
            claim: true,
            draft: false,
        },
    );
    assert!(
        matches!(outcome, Outcome::Refusal(ref refusal)
            if refusal.reason() == "task::capture-taste-review"),
        "taste-required compound capture must refuse: {outcome:?}",
    );
    assert_eq!(
        bus.fact_snapshot()
            .expect("taste-review facts read")
            .facts()
            .len(),
        facts_before,
    );
    assert!(
        board(&bus, epoch, actor)
            .tasks
            .iter()
            .all(|task| task.statement.body.handle != handle),
        "refused compound capture must not leave a task",
    );
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
    assert!(at(&still_held, &task).statement.body.taste.is_some());

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
    assert_eq!(at(&cleared, &task).statement.body.taste, None);
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
fn clearing_a_hold_withdraws_only_the_plan_phase_the_hold_inserted() {
    let (root, bus, epoch, _seats) = fixture("taste-review-withdraw", 0);
    let repository = configured_repository(&root, &["delay"]);
    let (inserted, outcome) = add_with_repository(
        &bus,
        &repository,
        epoch,
        "TASTE-inserted",
        "Delay the lifecycle change",
        &["focused tests pass"],
    );
    assert_committed(outcome);
    let authored_born = stamp("TASTE-authored");
    let authored = TaskHandle::for_project("board.test", &authored_born);
    assert_committed(issue_repository(
        &bus,
        &repository,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence {
            task: authored_born,
        }],
        &TaskAdd {
            preparation: None,
            intent: None,
            title: "Delay the planned change".to_owned(),
            project: Some("board.test".to_owned()),
            description: None,
            acceptance: vec!["focused tests pass".to_owned()],
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some(fixture_origin()),
            flow: Some("plan,todo".to_owned()),
            depends: Vec::new(),
            claim: false,
            draft: false,
        },
    ));
    let held = board(&bus, epoch, Actor::Operator);
    let phases = |board: &BoardAnswer, task: &TaskHandle| {
        let workflow = &at(board, task).workflow.body;
        (
            workflow.phase.as_str().to_owned(),
            workflow
                .flow
                .iter()
                .map(|phase| phase.as_str().to_owned())
                .collect::<Vec<_>>(),
        )
    };
    assert_eq!(
        phases(&held, &inserted),
        (
            "plan".to_owned(),
            vec!["plan".into(), "todo".into(), "review".into()]
        ),
    );
    assert_eq!(
        phases(&held, &authored),
        ("plan".to_owned(), vec!["plan".into(), "todo".into()]),
    );
    assert_eq!(
        at(&held, &inserted).statement.body.taste,
        Some(wire::TasteHold::Prepended),
    );
    assert_eq!(
        at(&held, &authored).statement.body.taste,
        Some(wire::TasteHold::Authored),
    );

    for (task, title) in [
        (&inserted, "Ship the lifecycle change"),
        (&authored, "Ship the planned change"),
    ] {
        assert_committed(modify_with_repository(
            &bus,
            &repository,
            epoch,
            Actor::Operator,
            task,
            title,
        ));
    }
    let cleared = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        phases(&cleared, &inserted),
        ("todo".to_owned(), vec!["todo".into(), "review".into()]),
        "the plan phase the hold inserted leaves with the hold",
    );
    assert_eq!(
        phases(&cleared, &authored),
        ("plan".to_owned(), vec!["plan".into(), "todo".into()]),
        "an authored plan phase outlives the hold",
    );
    for task in [&inserted, &authored] {
        assert_eq!(at(&cleared, task).statement.body.taste, None);
    }
    assert!(
        at(&cleared, &inserted).claim.is_none(),
        "clearing an unclaimed synthetic hold does not invent claim history",
    );
    clean(&root, bus);
}

#[test]
fn clearing_a_claimed_synthetic_hold_releases_before_authored_work() {
    let (root, bus, epoch, seats) = fixture("taste-review-claimed-release", 1);
    let repository = configured_repository(&root, &["delay"]);
    let seat = &seats[0];
    let lane = Actor::Lane(seat.lane);
    let (task, outcome) = add_with_repository(
        &bus,
        &repository,
        epoch,
        "TASTE-release01",
        "Delay the lifecycle change",
        &["focused tests pass"],
    );
    assert_committed(outcome);
    claim_held(&bus, epoch, seat, &task);
    let held = board(&bus, epoch, lane.clone());
    let held_heads = at(&held, &task);
    let held_claim = held_heads
        .claim
        .as_ref()
        .expect("synthetic plan is claimed");
    let stale_completion = progress_witnesses(&held, held_heads, seat.lane, seat.occupant, false)
        .expect("active synthetic-plan claim has completion witnesses");

    let CommandAnswer::Task(edited) = assert_committed(modify_with_repository(
        &bus,
        &repository,
        epoch,
        lane.clone(),
        &task,
        "Ship the lifecycle change",
    )) else {
        panic!("taste clearance returned another answer shape");
    };
    assert_eq!(edited.status, "edited; phase:todo");
    assert_eq!(edited.next.as_deref(), Some("spice task next"));

    let released = board(&bus, epoch, lane.clone());
    let released_heads = at(&released, &task);
    assert_eq!(released_heads.workflow.body.phase.as_str(), "todo");
    let released_claim = released_heads
        .claim
        .as_ref()
        .expect("released claim remains durable");
    assert_eq!(released_claim.body.disposition, ClaimDisposition::Freed);
    assert_eq!(released_claim.body.claimed, held_claim.body.claimed);
    assert_eq!(released_claim.body.actor, held_claim.body.actor);
    assert_ne!(released_claim.position, held_claim.position);
    let facts = bus.fact_snapshot().expect("taste-plan release facts read");
    let born = &released_heads.statement.body.born;
    let freed = facts
        .facts()
        .iter()
        .find(|fact| {
            fact.fact
                .read::<body::ClaimFreed>()
                .is_some_and(|released| released.task == *born)
        })
        .expect("claim release fact exists");
    let phased = facts
        .facts()
        .iter()
        .find(|fact| {
            fact.fact
                .read::<body::TaskPhased>()
                .is_some_and(|phased| phased.task == *born)
        })
        .expect("authored phase restoration exists");
    assert!(
        freed.seq < phased.seq,
        "the claim leaves before the authored execution phase becomes replayable",
    );

    let stale = issue_repository(
        &bus,
        &repository,
        epoch,
        lane.clone(),
        CommandKind::TaskDone,
        stale_completion,
        &TaskDone {
            task: task.clone(),
            validation: "planning validation cannot complete execution".to_owned(),
            operation: Some(completion_operation()),
        },
    );
    let Outcome::Refusal(refusal) = stale else {
        panic!("the prior synthetic-plan claim completed authored work: {stale:?}");
    };
    assert_eq!(refusal.reason(), "claim::active-refused");
    assert_eq!(
        at(&board(&bus, epoch, lane.clone()), &task)
            .workflow
            .body
            .phase
            .as_str(),
        "todo",
    );

    assert_release_survives_restart_and_reallocates(
        &root,
        bus,
        epoch,
        seat,
        lane,
        &task,
        released_heads.clone(),
    );
}

fn assert_release_survives_restart_and_reallocates(
    root: &Path,
    bus: Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    lane: Actor,
    task: &TaskHandle,
    expected: BoardTask,
) {
    bus.verify()
        .expect("taste-plan release authority verifies before restart");
    bus.shutdown().expect("taste-plan release fixture stops");
    let reopened = journal::Journal::open(root.join("authority.sqlite3"))
        .expect("taste-plan release journal reopens");
    let reopened = Bus::start(reopened).expect("taste-plan release bus restarts");
    assert_eq!(at(&board(&reopened, epoch, lane.clone()), task), &expected);

    let won = claim_held(&reopened, epoch, seat, task);
    assert_eq!(won.holder.as_ref(), Some(&lane));
    let reclaimed = board(&reopened, epoch, lane);
    assert_eq!(
        at(&reclaimed, task)
            .claim
            .as_ref()
            .expect("authored execution is newly claimed")
            .body
            .disposition,
        ClaimDisposition::Active,
    );
    clean(root, reopened);
}

#[test]
fn clearing_an_authored_plan_retains_its_active_claim() {
    let (root, bus, epoch, seats) = fixture("taste-review-authored-plan", 1);
    let repository = configured_repository(&root, &["delay"]);
    let seat = &seats[0];
    let lane = Actor::Lane(seat.lane);
    let born = stamp("TASTE-authplan");
    let task = TaskHandle::for_project("board.test", &born);
    assert_committed(issue_repository(
        &bus,
        &repository,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: born }],
        &TaskAdd {
            preparation: None,
            intent: None,
            title: "Delay the planned change".to_owned(),
            project: Some("board.test".to_owned()),
            description: None,
            acceptance: vec!["focused tests pass".to_owned()],
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some(fixture_origin()),
            flow: Some("plan,todo".to_owned()),
            depends: Vec::new(),
            claim: false,
            draft: false,
        },
    ));
    claim_held(&bus, epoch, seat, &task);
    let held = board(&bus, epoch, lane.clone());
    let held_claim = at(&held, &task)
        .claim
        .as_ref()
        .expect("authored plan is claimed")
        .clone();

    let CommandAnswer::Task(edited) = assert_committed(modify_with_repository(
        &bus,
        &repository,
        epoch,
        lane.clone(),
        &task,
        "Ship the planned change",
    )) else {
        panic!("authored-plan clearance returned another answer shape");
    };
    assert_eq!(edited.status, "edited");
    assert_eq!(edited.next.as_deref(), Some("spice task done"));
    let cleared = board(&bus, epoch, lane);
    let cleared_heads = at(&cleared, &task);
    assert_eq!(cleared_heads.workflow.body.phase.as_str(), "plan");
    let cleared_claim = cleared_heads
        .claim
        .as_ref()
        .expect("authored plan retains its active claim");
    assert_eq!(cleared_claim.body.disposition, ClaimDisposition::Active);
    assert_eq!(cleared_claim.body.claimed, held_claim.body.claimed);
    assert_eq!(cleared_claim.body.actor, held_claim.body.actor);
    assert_eq!(cleared_heads.statement.body.taste, None);
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
    assert!(!diagnostic.contains("delay"));
    assert!(!diagnostic.contains("Rewrite the task wording"));

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
    assert_eq!(refusal.reason(), "task::taste-review-required");
    let diagnostic = format!("{refusal:?}");
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

#[test]
fn an_unmeasurable_audit_value_withholds_the_rest_of_the_note() {
    let malformed = format!(
        "{AUDIT_PREFIX}policy=abc outcome=required matches=[field=\"title\" trigger=\"open"
    );
    let read = lane_audit(&malformed).expect("an audit projects for a lane");
    assert!(read.ends_with("trigger=withheld"), "{read}");
    assert!(!read.contains("open"), "{read}");
    assert!(
        lane_audit("an authored note").is_none(),
        "a note that is not an audit was rewritten",
    );
}
