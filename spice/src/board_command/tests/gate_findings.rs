use super::*;

use std::ffi::OsString;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

// GOV-09 and GOV-45..GOV-51 are exercised here through CASES 104 and 250..256:
// explicit request only, exact finding-to-task filing, stable identity reuse,
// completed predecessor recurrence, deferred work, and acting-claim origin.

#[test]
fn gate_findings_empty_dispatch_creates_no_board_work() {
    // GOV-45 / CASE 250: no requested findings means no board work is filed.
    let (root, bus, epoch, _) = fixture("gate-findings-empty", 1);
    let dispatch = focused_dispatch(&root, "empty-report", &[]);
    let report = dispatch.body.report.clone();
    let answer = assert_committed(issue_gate_findings(&bus, epoch, Actor::Operator, &dispatch));

    assert_eq!(
        text(answer),
        format!(
            concat!(
                "gate findings instrument=file-shape state=clean report={} ",
                "findings=0 created=0 reused=0"
            ),
            report
        )
    );
    assert!(gate_tasks(&gate_board(&bus, epoch, Actor::Operator)).is_empty());
    clean(&root, bus);
}

#[test]
fn gate_findings_file_exact_deferred_tasks() {
    // GOV-09, GOV-49, GOV-51 / CASES 104, 254, 256.
    let (root, bus, epoch, _) = fixture("gate-findings-create", 1);
    let dispatch = focused_dispatch(&root, "create-report", &["one", "two"]);
    let report = dispatch.body.report.clone();
    let answer = assert_committed(issue_gate_findings(&bus, epoch, Actor::Operator, &dispatch));

    assert!(text(answer).contains("findings=2 created=2 reused=0"));
    let board = gate_board(&bus, epoch, Actor::Operator);
    let tasks = gate_tasks(&board);
    assert_eq!(tasks.len(), dispatch.findings.len());
    let causal: wire::Origin = format!("gate:{report}")
        .parse()
        .expect("gate report forms origin");
    for (task, finding) in tasks.iter().zip(&dispatch.findings) {
        assert_eq!(task.statement.body.project, "gate.findings");
        assert_eq!(task.presentation_title, finding.title);
        assert_eq!(
            task.statement
                .body
                .handle
                .incepted()
                .expect("derived finding handle carries its inception stamp"),
            task.statement.body.incepted,
        );
        assert!(task.workflow.body.deferral.is_some());
        assert_eq!(task.statement.body.provenance, causal);
        assert_eq!(
            creation_fact_origin(&bus, &task.statement.body.handle),
            causal
        );
        let tag = finding_tag(finding);
        assert!(task.statement.body.tags.contains(&tag));
        assert_eq!(task.statement.body.acceptance, finding.acceptance);
        let description = task
            .statement
            .body
            .description
            .as_deref()
            .expect("filed finding carries its exact evidence");
        for evidence in [
            finding.description.as_str(),
            &format!("Finding identity: {}", finding.identity),
            &format!("Severity: {}", finding.severity),
            &format!("Locus: {}", finding.locus),
            &format!("Gate report: {report}"),
        ] {
            assert!(
                description.contains(evidence),
                "filed task omitted projected evidence {evidence:?}: {description}"
            );
        }
    }
    clean(&root, bus);
}

#[test]
fn gate_findings_reuse_one_open_task_by_identity() {
    // GOV-46 and GOV-47 / CASES 251 and 252.
    let (root, bus, epoch, _) = fixture("gate-findings-reuse", 1);
    let first_dispatch = focused_dispatch(&root, "first-report", &["repeat"]);
    assert_committed(issue_gate_findings(
        &bus,
        epoch,
        Actor::Operator,
        &first_dispatch,
    ));
    let first = gate_tasks(&gate_board(&bus, epoch, Actor::Operator))[0]
        .statement
        .body
        .handle
        .clone();

    let second_dispatch = focused_dispatch(&root, "second-report", &["repeat"]);
    assert_ne!(first_dispatch.body.report, second_dispatch.body.report);
    assert_eq!(
        first_dispatch.findings[0].identity,
        second_dispatch.findings[0].identity
    );
    let answer = assert_committed(issue_gate_findings(
        &bus,
        epoch,
        Actor::Operator,
        &second_dispatch,
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
    let old_dispatch = focused_dispatch(&root, "old-report", &["recur"]);
    assert_committed(issue_gate_findings(
        &bus,
        epoch,
        Actor::Operator,
        &old_dispatch,
    ));
    let old = gate_tasks(&gate_board(&bus, epoch, Actor::Operator))[0]
        .statement
        .body
        .handle
        .clone();
    complete(&bus, epoch, &old);

    let new_dispatch = focused_dispatch(&root, "new-report", &["recur"]);
    assert_ne!(old_dispatch.body.report, new_dispatch.body.report);
    assert_eq!(
        old_dispatch.findings[0].identity,
        new_dispatch.findings[0].identity
    );
    let answer = assert_committed(issue_gate_findings(
        &bus,
        epoch,
        Actor::Operator,
        &new_dispatch,
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

    let dispatch = focused_dispatch(&root, "claimed-report", &["claimed"]);
    assert_committed(issue_gate_findings(&bus, epoch, lane, &dispatch));

    let task = gate_tasks(&gate_board(&bus, epoch, Actor::Operator))
        .pop()
        .expect("filed gate finding task");
    assert_eq!(
        task.statement.body.provenance.to_string(),
        format!(
            "task:{}",
            parent
                .incepted()
                .expect("parent handle carries an inception stamp")
        )
    );
    clean(&root, bus);
}

#[test]
fn gate_findings_refuse_changed_report_before_board_work() {
    let (root, bus, epoch, _) = fixture("gate-findings-digest-mismatch", 1);
    let outcome = issue_gate_findings_raw(
        &bus,
        epoch,
        Actor::Operator,
        GateFindings {
            report: digest("different-report"),
            evidence: "{}".to_owned(),
        },
    );

    assert!(matches!(
        outcome,
        Outcome::Refusal(ref refusal)
            if refusal.reason() == "gate::finding-report-digest-mismatch"
    ));
    assert!(gate_tasks(&gate_board(&bus, epoch, Actor::Operator)).is_empty());
    clean(&root, bus);
}

#[test]
fn gate_findings_refuse_invalid_focused_report_before_board_work() {
    let (root, bus, epoch, _) = fixture("gate-findings-report-invalid", 1);
    let evidence = "{}".to_owned();
    let outcome = issue_gate_findings_raw(
        &bus,
        epoch,
        Actor::Operator,
        GateFindings {
            report: Digest::compute(Domain::GateFocusedReport, evidence.as_bytes()),
            evidence,
        },
    );

    assert!(matches!(
        outcome,
        Outcome::Refusal(ref refusal)
            if refusal.reason() == "gate::finding-report-invalid"
    ));
    assert!(gate_tasks(&gate_board(&bus, epoch, Actor::Operator)).is_empty());
    clean(&root, bus);
}

#[test]
fn gate_findings_refuse_fold_state_disagreement_before_board_work() {
    let (root, bus, epoch, _) = fixture("gate-findings-fold-mismatch", 1);
    let body = GateFindings {
        report: digest("fold-mismatch"),
        evidence: "{}".to_owned(),
    };
    let command = fixture_command(
        epoch,
        Actor::Operator,
        CommandKind::GateFindings,
        Vec::new(),
        &body,
    );
    let outcome = gate_findings_folded(
        &bus,
        &command,
        &body,
        &[finding("injected")],
        gate::GateId::FileShape,
        true,
        &gate_projects(),
        &board::NextConfig::default(),
    );

    assert!(matches!(
        outcome,
        Outcome::Refusal(ref refusal)
            if refusal.reason() == "gate::finding-report-fold-mismatch"
    ));
    assert!(gate_tasks(&gate_board(&bus, epoch, Actor::Operator)).is_empty());
    clean(&root, bus);
}

#[derive(Clone)]
struct FocusedDispatch {
    body: GateFindings,
    findings: Vec<FindingRow>,
}

fn focused_dispatch(root: &Path, marker: &str, finding_names: &[&str]) -> FocusedDispatch {
    let report = focused_file_shape_report(root, marker, finding_names);
    let findings = report
        .findings()
        .into_iter()
        .map(FindingRow::from)
        .collect::<Vec<_>>();
    assert_eq!(findings.len(), finding_names.len());
    assert_eq!(report.admits(), findings.is_empty());
    let (report_digest, evidence) = crate::gate_command::focused_report_evidence(&report)
        .unwrap_or_else(|outcome| {
            panic!("focused report did not cross the CLI evidence encoder: {outcome:?}")
        });
    assert_eq!(
        gate::FocusedArmReport::decode(evidence.as_bytes())
            .expect("CLI evidence is a canonical focused report"),
        report
    );
    FocusedDispatch {
        body: GateFindings {
            report: report_digest,
            evidence,
        },
        findings,
    }
}

fn focused_file_shape_report(
    root: &Path,
    marker: &str,
    finding_names: &[&str],
) -> gate::FocusedArmReport {
    static NEXT_REPORT: AtomicU64 = AtomicU64::new(1);
    let repository = root.join(format!(
        "focused-report-{}",
        NEXT_REPORT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(repository.join("src")).expect("focused repository directories exist");
    gate_git(&repository, &["init", "-q"]);
    gate_git(
        &repository,
        &["config", "user.email", "gate-findings@example.invalid"],
    );
    gate_git(
        &repository,
        &["config", "user.name", "Gate Findings Fixture"],
    );

    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("spice crate has a workspace parent");
    for document in ["spice-rust.toml", "baseline.toml"] {
        fs::copy(workspace.join(document), repository.join(document))
            .unwrap_or_else(|error| panic!("copy focused fixture {document}: {error}"));
    }
    fs::write(repository.join("src/base.rs"), "pub fn base() {}\n")
        .expect("write focused fixture baseline source");
    gate_git(&repository, &["add", "--all"]);
    gate_git(&repository, &["commit", "-qm", "focused report baseline"]);

    let marker_path = "src/report_marker.rs";
    fs::write(
        repository.join(marker_path),
        format!("pub const REPORT_MARKER: &str = {marker:?};\n"),
    )
    .expect("write focused report marker");
    let mut changes = vec![marker_path.to_owned()];
    let oversized = "//\n".repeat(2_000);
    for name in finding_names {
        let path = format!("src/{name}.rs");
        fs::write(repository.join(&path), &oversized)
            .unwrap_or_else(|error| panic!("write focused finding {path}: {error}"));
        changes.push(path);
    }
    changes.sort();
    gate_git(&repository, &["add", "--all"]);

    let rim = gate::RimCapture::new(
        repository.clone(),
        Some(OsString::from("/usr/bin:/bin")),
        changes,
    )
    .expect("focused fixture rim is absolute");
    let actor = gate::ActorCapture::Acting {
        actor: "gate-findings-fixture".to_owned(),
        bound_claims: Vec::new(),
    };
    let mut capture = gate::BatteryCapture::capture_arm(
        &repository,
        &gate::Selector::staged(),
        gate::ClaimCapture::Idle,
        actor,
        rim,
        gate::HygieneConfiguration::current(),
        gate::GateId::FileShape,
    )
    .expect("capture production focused file-shape report");
    let report = capture
        .execute_arm_report(gate::GateId::FileShape)
        .expect("execute production focused file-shape report");
    assert_eq!(report.gate_id(), gate::GateId::FileShape);
    report
}

fn gate_git(root: &Path, arguments: &[&str]) {
    let output = Command::new("/usr/bin/git")
        .args(arguments)
        .current_dir(root)
        .env_clear()
        .env("HOME", root)
        .output()
        .unwrap_or_else(|error| panic!("run focused fixture Git {arguments:?}: {error}"));
    assert!(
        output.status.success(),
        "focused fixture Git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn issue_gate_findings(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    dispatch: &FocusedDispatch,
) -> Outcome<CommandAnswer> {
    issue_gate_findings_raw(bus, epoch, actor, dispatch.body.clone())
}

fn issue_gate_findings_raw(
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

fn finding(name: &str) -> FindingRow {
    FindingRow {
        identity: digest(name),
        severity: "high".to_owned(),
        locus: format!("gate=file-shape metric=file.lines key=path=\"src/{name}.rs\""),
        title: format!("Gate finding {name}"),
        description: format!("Finding {name} exceeded its bound"),
        acceptance: vec![format!("Finding {name} is no longer reported")],
    }
}

fn finding_tag(finding: &FindingRow) -> String {
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
    let incepted = current.statement.body.incepted.clone();
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
                task: incepted.clone(),
                validation: "fixture completion".to_owned(),
                git: None,
                source: body::SourceBoundary::unrecorded(),
            },
            FactCause::Root,
        )
        .expect("completion fact builds");
    write.workflow_head(incepted, done, move |_| workflow);
    assert!(matches!(
        bus.commit(write).expect("completion commits"),
        CommitOutcome::Committed(_)
    ));
}
