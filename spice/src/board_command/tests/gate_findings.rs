use super::*;

// GOV-09 and GOV-45..GOV-51 are exercised here through CASES 104 and 250..256:
// explicit request only, exact finding-to-task filing, stable identity reuse,
// completed predecessor recurrence, deferred work, and acting-claim origin.

#[test]
fn gate_findings_empty_dispatch_creates_no_board_work() {
    // GOV-45 / CASE 250: no requested findings means no board work is filed.
    let (root, bus, epoch, _) = fixture("gate-findings-empty", 1);
    let answer = assert_committed(issue_gate_findings(
        &bus,
        epoch,
        Actor::Operator,
        GateFindings {
            report: digest("empty-report"),
            findings: Vec::new(),
        },
    ));

    assert_eq!(
        text(answer),
        format!(
            "gate findings report={} findings=0 created=0 reused=0",
            digest("empty-report")
        )
    );
    assert!(gate_tasks(&gate_board(&bus, epoch, Actor::Operator)).is_empty());
    clean(&root, bus);
}

#[test]
fn gate_findings_file_exact_deferred_tasks() {
    // GOV-09, GOV-49, GOV-51 / CASES 104, 254, 256.
    let (root, bus, epoch, _) = fixture("gate-findings-create", 1);
    let first = finding("one");
    let second = finding("two");
    let report = digest("create-report");
    let answer = assert_committed(issue_gate_findings(
        &bus,
        epoch,
        Actor::Operator,
        GateFindings {
            report: report.clone(),
            findings: vec![first.clone(), second.clone()],
        },
    ));

    assert!(text(answer).contains("findings=2 created=2 reused=0"));
    let board = gate_board(&bus, epoch, Actor::Operator);
    let tasks = gate_tasks(&board);
    assert_eq!(tasks.len(), 2);
    let causal: wire::Origin = format!("gate:{report}")
        .parse()
        .expect("gate report forms origin");
    for (task, finding) in tasks.iter().zip([first, second]) {
        assert_eq!(task.statement.body.project, "gate.findings");
        assert!(task.workflow.body.deferral.is_some());
        assert_eq!(task.statement.body.provenance, causal);
        assert_eq!(
            creation_fact_origin(&bus, &task.statement.body.handle),
            causal
        );
        assert!(task.statement.body.tags.contains(&finding_tag(&finding)));
    }
    clean(&root, bus);
}

#[test]
fn gate_findings_reuse_one_open_task_by_identity() {
    // GOV-46 and GOV-47 / CASES 251 and 252.
    let (root, bus, epoch, _) = fixture("gate-findings-reuse", 1);
    let finding = finding("repeat");
    assert_committed(issue_gate_findings(
        &bus,
        epoch,
        Actor::Operator,
        GateFindings {
            report: digest("first-report"),
            findings: vec![finding.clone()],
        },
    ));
    let first = gate_tasks(&gate_board(&bus, epoch, Actor::Operator))[0]
        .statement
        .body
        .handle
        .clone();

    let answer = assert_committed(issue_gate_findings(
        &bus,
        epoch,
        Actor::Operator,
        GateFindings {
            report: digest("second-report"),
            findings: vec![finding],
        },
    ));

    assert!(text(answer).contains("findings=1 created=0 reused=1"));
    let tasks = gate_tasks(&gate_board(&bus, epoch, Actor::Operator));
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].statement.body.handle, first);
    clean(&root, bus);
}

#[test]
fn gate_findings_recur_after_completion_with_predecessor_note() {
    // GOV-48 / CASE 253.
    let (root, bus, epoch, _) = fixture("gate-findings-recur", 1);
    let finding = finding("recur");
    assert_committed(issue_gate_findings(
        &bus,
        epoch,
        Actor::Operator,
        GateFindings {
            report: digest("old-report"),
            findings: vec![finding.clone()],
        },
    ));
    let old = gate_tasks(&gate_board(&bus, epoch, Actor::Operator))[0]
        .statement
        .body
        .handle
        .clone();
    complete(&bus, epoch, &old);

    let answer = assert_committed(issue_gate_findings(
        &bus,
        epoch,
        Actor::Operator,
        GateFindings {
            report: digest("new-report"),
            findings: vec![finding],
        },
    ));

    assert!(text(answer).contains("findings=1 created=1 reused=0"));
    let tasks = gate_tasks(&gate_board(&bus, epoch, Actor::Operator));
    assert_eq!(tasks.len(), 2);
    let successor = tasks
        .iter()
        .find(|task| task.statement.body.handle != old)
        .expect("successor task");
    assert!(
        successor.statement.body.description.as_deref().is_some_and(
            |description| description.contains(&format!("Previous completed task: {old}"))
        )
    );
    clean(&root, bus);
}

#[test]
fn gate_findings_use_the_active_claim_as_origin_when_present() {
    // GOV-50 / CASE 255.
    let (root, bus, epoch, seats) = fixture("gate-findings-origin", 1);
    let lane = Actor::Lane(seats[0].lane);
    let parent = add_project_with_acceptance(
        &bus,
        epoch,
        Actor::Operator,
        "PARENT-origin1",
        "Hold a parent claim",
        "board.test",
        Some("todo,review"),
        &["parent claim can hold origin"],
    );
    let before = gate_board(&bus, epoch, lane.clone());
    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        claim_expected(&before, &parent, &seats[0]),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
        },
    ));

    assert_committed(issue_gate_findings(
        &bus,
        epoch,
        lane,
        GateFindings {
            report: digest("claimed-report"),
            findings: vec![finding("claimed")],
        },
    ));

    let task = gate_tasks(&gate_board(&bus, epoch, Actor::Operator))
        .pop()
        .expect("filed gate finding task");
    assert_eq!(
        task.statement.body.provenance.to_string(),
        format!("task:{parent}")
    );
    clean(&root, bus);
}

fn issue_gate_findings(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    body: GateFindings,
) -> Outcome<CommandAnswer> {
    let command = fixture_command(epoch, actor, CommandKind::GateFindings, Vec::new(), &body);
    dispatch_with_repository(
        bus,
        &command,
        None,
        &gate_projects(),
        &board::NextConfig::default(),
    )
    .expect("gate finding dispatch is handled")
}

fn gate_board(bus: &Bus, epoch: CanonicalUuid, actor: Actor) -> BoardAnswer {
    let command = fixture_command(
        epoch,
        actor,
        CommandKind::BoardRead,
        Vec::new(),
        &BoardRead { family: None },
    );
    let outcome = dispatch_with_repository(
        bus,
        &command,
        None,
        &gate_projects(),
        &board::NextConfig::default(),
    )
    .expect("board read is handled");
    let Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(position),
        body: CommandAnswer::Board(answer),
    }) = outcome
    else {
        panic!("gate board read did not return a snapshot: {outcome:?}");
    };
    assert_eq!(position, answer.cursor);
    *answer
}

fn gate_projects() -> ProjectPolicy {
    ProjectPolicy::new(
        ["board".to_owned(), "rim".to_owned(), "gate".to_owned()],
        ["sandbox".to_owned()],
        dialect::PROJECT_MIN_DEPTH as u64,
        dialect::PROJECT_MAX_DEPTH as u64,
    )
    .expect("gate findings fixture projects")
}

fn gate_tasks(board: &BoardAnswer) -> Vec<BoardTask> {
    let mut tasks = board
        .tasks
        .iter()
        .filter(|task| task.statement.body.project == "gate.findings")
        .cloned()
        .collect::<Vec<_>>();
    tasks.sort_by(|left, right| left.statement.body.handle.cmp(&right.statement.body.handle));
    tasks
}

fn finding(name: &str) -> wire::intent::GateFindingWork {
    wire::intent::GateFindingWork {
        identity: digest(name),
        severity: body::Severity::High,
        locus: format!("gate=file-shape metric=file.lines key=path=\"src/{name}.rs\""),
        title: format!("Gate finding {name}"),
        description: format!("Finding {name} exceeded its bound"),
        acceptance: vec![format!("Finding {name} is no longer reported")],
    }
}

fn finding_tag(finding: &wire::intent::GateFindingWork) -> String {
    format!("gate-finding:{}", finding.identity)
}

fn digest(name: &str) -> Digest {
    Digest::compute(Domain::GateFinding, name.as_bytes())
}

fn text(answer: CommandAnswer) -> String {
    let CommandAnswer::Text(TextAnswer { text }) = answer else {
        panic!("expected text answer: {answer:?}");
    };
    text
}

fn complete(bus: &Bus, epoch: CanonicalUuid, task: &TaskHandle) {
    let before = gate_board(bus, epoch, Actor::Operator);
    let current = task_heads(&before, task).expect("task exists before completion");
    let at = Instant::new(jiff::Timestamp::now()).expect("completion instant");
    let origin = format!("task:{task}").parse().expect("task origin");
    let mut workflow = current.workflow.body.clone();
    workflow.status = "completed".to_owned();
    workflow.deferral = None;
    workflow.completion = Some(at);
    let mut write = JournalWrite::new(Actor::Operator, origin, at, Producer::CommandDoor);
    let done = write
        .fact(
            body::TaskDone {
                task: task.clone(),
                validation: "fixture completion".to_owned(),
                git: None,
                source: body::SourceBoundary::unrecorded(),
            },
            FactCause::Root,
        )
        .expect("completion fact builds");
    write.workflow_head(task.clone(), done, move |_| workflow);
    assert!(matches!(
        bus.commit(write).expect("completion commits"),
        CommitOutcome::Committed(_)
    ));
}
