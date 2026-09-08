use super::*;

mod delivery;
mod focused;
use focused::*;

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
                "findings=0 created=0 updated=0 reused=0 advanced=0 cleared=0 recovery_pending=0"
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

    assert!(text(answer).contains("findings=2 created=2 updated=0 reused=0 advanced=0 cleared=0"));
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
                .born()
                .expect("derived finding handle carries its birth stamp"),
            task.statement.body.born,
        );
        assert!(task.workflow.body.deferral.is_some());
        assert_eq!(task.statement.body.provenance, causal);
        assert_eq!(
            creation_fact_origin(&bus, &task.statement.body.handle),
            causal
        );
        let tag = finding_tag(finding);
        assert!(task.statement.body.tags.contains(&tag));
        assert!(
            task.statement
                .body
                .tags
                .contains(&"gate-instrument:file-shape".to_owned())
        );
        for tag in [
            format!("gate-repository:{}", dispatch.body.source.repository),
            format!("gate-selection:{}", dispatch.body.source.selection),
            format!("gate-source-tree:{}", dispatch.body.source.tree),
            format!("gate-source-anchor:{}", dispatch.body.source.anchor),
            "gate-source-kind:staged-index".to_owned(),
            "gate-mainline-ref:refs/remotes/origin/main".to_owned(),
            "gate-mainline-base:mainline-base-create-report".to_owned(),
        ] {
            assert!(task.statement.body.tags.contains(&tag), "missing {tag}");
        }
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
            &format!("Affected authority: {}", finding.authority),
            &format!("Locus: {}", finding.locus),
            &format!("Gate report: {report}"),
            &format!("Repository authority: {}", dispatch.body.source.repository),
            &format!("Selected tree: {}", dispatch.body.source.tree),
            &format!("Source anchor: {}", dispatch.body.source.anchor),
            "Source kind: staged-index",
            "Mainline reference: refs/remotes/origin/main",
            "Mainline merge base: mainline-base-create-report",
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
fn gate_finding_planning_rewinds_ordered_collisions_and_refuses_underflow() {
    let (root, bus, epoch, _) = fixture("gate-finding-stamp-plan", 1);
    let occupied_name = "gate-finding-observation";
    let observation_micros = occupied_name.bytes().fold(0_i64, |value, byte| {
        (value * 131 + i64::from(byte)) % 10_000_000_000_000
    });
    add_project(
        &bus,
        epoch,
        Actor::Operator,
        occupied_name,
        "Occupy the observed task stamp",
        "board.test",
        Some("todo,review"),
    );
    let projects = gate_projects();
    let allocation = board::NextConfig::default();
    let snapshot = validated_snapshot(&bus, Actor::Operator, &projects, &allocation)
        .expect("gate finding planning snapshot validates");
    let dispatch = focused_dispatch(&root, "stamp-plan", &["first", "second"]);
    let planned = plan_gate_finding_creations(
        &bus,
        &dispatch.findings,
        &snapshot,
        observation_micros,
        gate::GateId::FileShape,
        &dispatch.repository,
        &dispatch.body.source,
        &dispatch.body.report,
        &Actor::Operator,
    )
    .expect("occupied observation stamp rewinds");
    assert_eq!(
        planned,
        dispatch
            .findings
            .iter()
            .zip([-2_i64, -1])
            .map(|(finding, offset)| {
                (
                    finding.identity.clone(),
                    wire::TaskStamp::for_micros(observation_micros + offset)
                        .expect("rewound stamp remains representable"),
                )
            })
            .collect::<Vec<_>>(),
    );
    clean(&root, bus);

    let (root, bus, epoch, _) = fixture("gate-finding-stamp-underflow", 1);
    add_project(
        &bus,
        epoch,
        Actor::Operator,
        "",
        "Occupy zero",
        "board.test",
        Some("todo,review"),
    );
    let projects = gate_projects();
    let allocation = board::NextConfig::default();
    let snapshot = validated_snapshot(&bus, Actor::Operator, &projects, &allocation)
        .expect("underflow planning snapshot validates");
    let dispatch = focused_dispatch(&root, "stamp-underflow", &["underflow"]);
    let error = plan_gate_finding_creations(
        &bus,
        &dispatch.findings,
        &snapshot,
        0,
        gate::GateId::FileShape,
        &dispatch.repository,
        &dispatch.body.source,
        &dispatch.body.report,
        &Actor::Operator,
    )
    .expect_err("occupied zero cannot rewind");
    let Outcome::Fault(fault) = error else {
        panic!("gate-finding stamp underflow must be an environment fault");
    };
    assert_eq!(fault.reason, "task::stamp-clock-invalid");
    clean(&root, bus);
}

#[test]
fn gate_findings_refresh_one_open_task_by_identity() {
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

    assert!(text(answer).contains("findings=1 created=0 updated=1 reused=0 advanced=0 cleared=0"));
    let tasks = gate_tasks(&gate_board(&bus, epoch, Actor::Operator));
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].statement.body.handle, first);
    let description = tasks[0]
        .statement
        .body
        .description
        .as_deref()
        .expect("refreshed evidence description");
    assert!(description.contains(&second_dispatch.body.report.to_string()));
    assert!(!description.contains(&first_dispatch.body.report.to_string()));
    clean(&root, bus);
}

#[test]
fn gate_findings_reuse_byte_identical_open_evidence_without_a_revision() {
    let (root, bus, epoch, _) = fixture("gate-findings-idempotent", 1);
    let dispatch = focused_dispatch(&root, "same-report", &["same"]);
    assert_committed(issue_gate_findings(&bus, epoch, Actor::Operator, &dispatch));
    let first = gate_tasks(&gate_board(&bus, epoch, Actor::Operator))[0].clone();

    let answer = assert_committed(issue_gate_findings(&bus, epoch, Actor::Operator, &dispatch));

    assert!(text(answer).contains("findings=1 created=0 updated=0 reused=1 advanced=0 cleared=0"));
    let second = gate_tasks(&gate_board(&bus, epoch, Actor::Operator))[0].clone();
    assert_eq!(second.statement, first.statement);
    clean(&root, bus);
}

#[test]
fn gate_findings_forward_migrate_legacy_open_task_on_redispatch() {
    let (root, bus, epoch, _) = fixture("gate-findings-forward-migration", 1);
    let first = focused_dispatch(&root, "legacy-first", &["legacy"]);
    assert_committed(issue_gate_findings(&bus, epoch, Actor::Operator, &first));
    let task = gate_tasks(&gate_board(&bus, epoch, Actor::Operator))[0]
        .statement
        .body
        .handle
        .clone();
    revise_gate_task(&bus, &task, |statement| {
        statement
            .tags
            .retain(|tag| !tag.starts_with("gate-instrument:"));
    });
    let second = focused_dispatch(&root, "legacy-second", &["legacy"]);

    let answer = assert_committed(issue_gate_findings(&bus, epoch, Actor::Operator, &second));

    assert!(text(answer).contains("findings=1 created=0 updated=1 reused=0 advanced=0 cleared=0"));
    let board = gate_board(&bus, epoch, Actor::Operator);
    let migrated = task_heads(&board, &task).expect("migrated gate repair task");
    assert!(
        migrated
            .statement
            .body
            .tags
            .contains(&"gate-instrument:file-shape".to_owned())
    );
    clean(&root, bus);
}

#[test]
fn gate_findings_refuse_ambiguous_legacy_task_instead_of_hiding_it() {
    let (root, bus, epoch, _) = fixture("gate-findings-migration-refusal", 1);
    let first = focused_dispatch(&root, "legacy-malformed", &["legacy"]);
    assert_committed(issue_gate_findings(&bus, epoch, Actor::Operator, &first));
    let task = gate_tasks(&gate_board(&bus, epoch, Actor::Operator))[0]
        .statement
        .body
        .handle
        .clone();
    revise_gate_task(&bus, &task, |statement| {
        statement
            .tags
            .retain(|tag| !tag.starts_with("gate-instrument:"));
        statement.description = Some("legacy evidence without its owning Gate line".to_owned());
    });
    let clean_report = focused_dispatch(&root, "legacy-clean", &[]);

    let outcome = issue_gate_findings(&bus, epoch, Actor::Operator, &clean_report);

    assert!(matches!(
        outcome,
        Outcome::Refusal(ref refusal)
            if refusal.reason() == "gate::finding-migration-required"
    ));
    let board = gate_board(&bus, epoch, Actor::Operator);
    let still_open = task_heads(&board, &task).expect("refused legacy task remains visible");
    assert_eq!(still_open.workflow.body.status, "pending");
    clean(&root, bus);
}

#[test]
fn gate_findings_refuse_legacy_task_without_mainline_authority() {
    let (root, bus, epoch, _) = fixture("gate-findings-source-migration", 1);
    let first = focused_dispatch(&root, "source-legacy-first", &["source-legacy"]);
    assert_committed(issue_gate_findings(&bus, epoch, Actor::Operator, &first));
    let task = gate_tasks(&gate_board(&bus, epoch, Actor::Operator))[0]
        .statement
        .body
        .handle
        .clone();
    revise_gate_task(&bus, &task, |statement| {
        statement.tags.retain(|tag| {
            !tag.starts_with("gate-mainline-ref:") && !tag.starts_with("gate-mainline-base:")
        });
    });
    let second = focused_dispatch(&root, "source-legacy-second", &["source-legacy"]);

    let outcome = issue_gate_findings(&bus, epoch, Actor::Operator, &second);

    let Outcome::Refusal(refusal) = outcome else {
        panic!("legacy finding source must refuse");
    };
    assert_eq!(refusal.reason(), "gate::finding-source-migration-required");
    assert!(
        refusal
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::observed)
            .is_some_and(|observed| observed.contains("gate-finding-mainline-v1"))
    );
    clean(&root, bus);
}

#[test]
fn gate_findings_refuse_clean_report_from_another_repository_authority() {
    let (root, bus, epoch, _) = fixture("gate-findings-repository-boundary", 1);
    let finding = focused_dispatch_in(
        &root,
        "finding-repository",
        "repository-finding",
        &["repository-bound"],
    );
    assert_committed(issue_gate_findings(&bus, epoch, Actor::Operator, &finding));
    let clean_report = focused_dispatch_in(&root, "unrelated-repository", "repository-clean", &[]);

    let outcome = issue_gate_findings_for_repository(
        &bus,
        epoch,
        Actor::Operator,
        &clean_report,
        &finding.repository,
    );

    assert!(matches!(
        outcome,
        Outcome::Refusal(ref refusal)
            if refusal.reason() == "gate::finding-repository-mismatch"
    ));
    let tasks = gate_tasks(&gate_board(&bus, epoch, Actor::Operator));
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].workflow.body.status, "pending");
    clean(&root, bus);
}

#[test]
fn gate_findings_accept_descendant_staged_recovery_through_review() {
    let (root, bus, epoch, seats) = fixture("gate-findings-descendant-recovery", 1);
    let finding = focused_dispatch(&root, "descendant-finding", &["descendant"]);
    assert_committed(issue_gate_findings(&bus, epoch, Actor::Operator, &finding));
    let task = gate_tasks(&gate_board(&bus, epoch, Actor::Operator))[0]
        .statement
        .body
        .handle
        .clone();
    let clean_report = focused_dispatch(&root, "descendant-clean", &[]);
    assert_ne!(finding.body.source.anchor, clean_report.body.source.anchor);
    let lane = claim_gate_task(&bus, epoch, &seats[0], &task);

    let answer = assert_committed(issue_gate_findings(&bus, epoch, lane, &clean_report));
    assert!(text(answer).contains("advanced=1 cleared=0"));
    let lane = claim_gate_task(&bus, epoch, &seats[0], &task);
    let answer = assert_committed(issue_gate_findings(&bus, epoch, lane, &clean_report));
    assert!(text(answer).contains("advanced=0 cleared=1"));
    let board = gate_board(&bus, epoch, Actor::Operator);
    let closed = task_heads(&board, &task).expect("descendant recovery task remains auditable");
    assert_eq!(closed.workflow.body.status, "completed");
    clean(&root, bus);
}

#[test]
fn gate_findings_clean_exact_instrument_retest_closes_open_repair() {
    let (root, bus, epoch, seats) = fixture("gate-findings-clear", 1);
    let finding = focused_dispatch(&root, "finding-report", &["recovered"]);
    assert_committed(issue_gate_findings(&bus, epoch, Actor::Operator, &finding));
    let task = gate_tasks(&gate_board(&bus, epoch, Actor::Operator))[0]
        .statement
        .body
        .handle
        .clone();
    let clean_report = focused_dispatch(&root, "clean-report", &[]);
    let lane = claim_gate_task(&bus, epoch, &seats[0], &task);

    let answer = assert_committed(issue_gate_findings(&bus, epoch, lane, &clean_report));

    assert!(text(answer).contains(
        "findings=0 created=0 updated=0 reused=0 advanced=1 cleared=0 recovery_pending=0"
    ));
    let review_board = gate_board(&bus, epoch, Actor::Operator);
    let review = task_heads(&review_board, &task).expect("recovery entered review");
    assert_eq!(review.workflow.body.status, "pending");
    assert_eq!(review.workflow.body.phase.as_str(), "review");
    assert_eq!(
        review.claim.as_ref().map(|claim| claim.body.disposition),
        Some(ClaimDisposition::Freed)
    );

    let lane = claim_gate_task(&bus, epoch, &seats[0], &task);
    let answer = assert_committed(issue_gate_findings(&bus, epoch, lane, &clean_report));
    assert!(text(answer).contains(
        "findings=0 created=0 updated=0 reused=0 advanced=0 cleared=1 recovery_pending=0"
    ));
    let board = gate_board(&bus, epoch, Actor::Operator);
    let closed = task_heads(&board, &task).expect("recovered repair task remains auditable");
    assert_eq!(closed.workflow.body.status, "completed");
    assert!(closed.workflow.body.completion.is_some());
    assert_eq!(closed.workflow.body.deferral, None);
    assert_eq!(
        closed.claim.as_ref().map(|claim| claim.body.disposition),
        Some(ClaimDisposition::Freed)
    );
    let validation = task_done_validation(&bus, &task);
    assert!(validation.contains(&clean_report.body.report.to_string()));
    assert!(validation.contains(&finding.findings[0].identity.to_string()));
    clean(&root, bus);
}

#[test]
fn gate_findings_verified_recovery_waits_for_exact_repair_claim() {
    let (root, bus, epoch, _) = fixture("gate-findings-recovery-pending", 1);
    let finding = focused_dispatch(&root, "pending-finding", &["pending"]);
    assert_committed(issue_gate_findings(&bus, epoch, Actor::Operator, &finding));
    let clean_report = focused_dispatch(&root, "pending-clean", &[]);

    let answer = assert_committed(issue_gate_findings(
        &bus,
        epoch,
        Actor::Operator,
        &clean_report,
    ));

    assert!(text(answer).contains("cleared=0 recovery_pending=1"));
    let tasks = gate_tasks(&gate_board(&bus, epoch, Actor::Operator));
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].workflow.body.status, "pending");
    assert!(tasks[0].workflow.body.completion.is_none());
    clean(&root, bus);
}

#[test]
fn gate_findings_recovery_cannot_mutate_another_lanes_claim() {
    let (root, bus, epoch, seats) = fixture("gate-findings-foreign-claim", 2);
    let finding = focused_dispatch(&root, "foreign-finding", &["foreign"]);
    assert_committed(issue_gate_findings(&bus, epoch, Actor::Operator, &finding));
    let task = gate_tasks(&gate_board(&bus, epoch, Actor::Operator))[0]
        .statement
        .body
        .handle
        .clone();
    let owner = claim_gate_task(&bus, epoch, &seats[0], &task);
    let observer = Actor::Lane(seats[1].lane);
    let clean_report = focused_dispatch(&root, "foreign-clean", &[]);

    let answer = assert_committed(issue_gate_findings(&bus, epoch, observer, &clean_report));

    assert!(text(answer).contains("cleared=0 recovery_pending=1"));
    let board = gate_board(&bus, epoch, Actor::Operator);
    let still_owned = task_heads(&board, &task).expect("foreign repair remains claimed");
    assert_eq!(still_owned.workflow.body.status, "pending");
    assert_eq!(
        still_owned.claim.as_ref().map(|claim| &claim.body.actor),
        Some(&owner)
    );
    clean(&root, bus);
}

#[test]
fn gate_findings_partial_retest_updates_present_and_closes_absent_identity() {
    let (root, bus, epoch, seats) = fixture("gate-findings-partial", 1);
    let first = focused_dispatch(&root, "partial-first", &["held", "recovered"]);
    assert_committed(issue_gate_findings(&bus, epoch, Actor::Operator, &first));
    let second = focused_dispatch(&root, "partial-second", &["held"]);
    let recovered = gate_tasks(&gate_board(&bus, epoch, Actor::Operator))
        .into_iter()
        .find(|task| {
            task.statement
                .body
                .tags
                .contains(&finding_tag(&first.findings[1]))
        })
        .expect("recovered repair task")
        .statement
        .body
        .handle;
    let lane = claim_gate_task(&bus, epoch, &seats[0], &recovered);

    let answer = assert_committed(issue_gate_findings(&bus, epoch, lane, &second));
    assert!(text(answer).contains("findings=1 created=0 updated=1 reused=0 advanced=1 cleared=0"));

    let lane = claim_gate_task(&bus, epoch, &seats[0], &recovered);
    let answer = assert_committed(issue_gate_findings(&bus, epoch, lane, &second));
    assert!(text(answer).contains("findings=1 created=0 updated=0 reused=1 advanced=0 cleared=1"));
    let tasks = gate_tasks(&gate_board(&bus, epoch, Actor::Operator));
    assert_eq!(tasks.len(), 2);
    let open = tasks
        .iter()
        .filter(|task| task.workflow.body.status == "pending")
        .collect::<Vec<_>>();
    let closed = tasks
        .iter()
        .filter(|task| task.workflow.body.status == "completed")
        .collect::<Vec<_>>();
    assert_eq!(open.len(), 1);
    assert_eq!(closed.len(), 1);
    assert!(
        open[0]
            .statement
            .body
            .tags
            .contains(&finding_tag(&second.findings[0]))
    );
    assert!(
        closed[0]
            .statement
            .body
            .tags
            .contains(&finding_tag(&first.findings[1]))
    );
    clean(&root, bus);
}

#[test]
fn gate_findings_recovery_never_crosses_instrument_authority() {
    let (root, bus, epoch, seats) = fixture("gate-findings-instrument-scope", 1);
    let file_shape = focused_dispatch(&root, "file-standing", &["file-standing"]);
    let complexity = finding("complexity-standing");
    assert_committed(issue_folded_gate_findings(
        &bus,
        epoch,
        Actor::Operator,
        "complexity-report",
        std::slice::from_ref(&complexity),
        gate::GateId::Complexity,
        false,
        &file_shape.repository,
        &file_shape.body.source,
    ));
    assert_committed(issue_gate_findings(
        &bus,
        epoch,
        Actor::Operator,
        &file_shape,
    ));

    let clean_file_shape = focused_dispatch(&root, "file-clean", &[]);
    let file_task = gate_tasks(&gate_board(&bus, epoch, Actor::Operator))
        .into_iter()
        .find(|task| {
            task.statement
                .body
                .tags
                .contains(&finding_tag(&file_shape.findings[0]))
        })
        .expect("file-shape repair task")
        .statement
        .body
        .handle;
    let lane = claim_gate_task(&bus, epoch, &seats[0], &file_task);
    let answer = assert_committed(issue_gate_findings(&bus, epoch, lane, &clean_file_shape));
    assert!(text(answer).contains("advanced=1 cleared=0"));

    let lane = claim_gate_task(&bus, epoch, &seats[0], &file_task);
    let answer = assert_committed(issue_gate_findings(&bus, epoch, lane, &clean_file_shape));
    assert!(text(answer).contains("advanced=0 cleared=1"));
    let tasks = gate_tasks(&gate_board(&bus, epoch, Actor::Operator));
    let complexity_task = tasks
        .iter()
        .find(|task| task.statement.body.tags.contains(&finding_tag(&complexity)))
        .expect("complexity repair remains visible");
    assert_eq!(complexity_task.workflow.body.status, "pending");
    assert!(
        complexity_task
            .statement
            .body
            .tags
            .contains(&"gate-instrument:complexity".to_owned())
    );
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

    assert!(text(answer).contains("findings=1 created=1 updated=0 reused=0 advanced=0 cleared=0"));
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
    authorize_lane_creation(&bus, epoch, &seats[0]);
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
            recovery: None,
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
            parent.born().expect("parent handle carries a birth stamp")
        )
    );
    clean(&root, bus);
}

#[test]
fn gate_findings_refuse_changed_report_before_board_work() {
    let (root, bus, epoch, _) = fixture("gate-findings-digest-mismatch", 1);
    let source = focused_dispatch(&root, "digest-source", &[]);
    let outcome = issue_gate_findings_raw(
        &bus,
        epoch,
        Actor::Operator,
        GateFindings {
            report: digest("different-report"),
            evidence: "{}".to_owned(),
            source: source.body.source.clone(),
        },
        &source.repository,
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
    let source = focused_dispatch(&root, "invalid-source", &[]);
    let evidence = "{}".to_owned();
    let outcome = issue_gate_findings_raw(
        &bus,
        epoch,
        Actor::Operator,
        GateFindings {
            report: Digest::compute(Domain::GateFocusedReport, evidence.as_bytes()),
            evidence,
            source: source.body.source.clone(),
        },
        &source.repository,
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
    let source = focused_dispatch(&root, "fold-source", &[]);
    let body = GateFindings {
        report: digest("fold-mismatch"),
        evidence: "{}".to_owned(),
        source: source.body.source.clone(),
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
        &source.repository,
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

fn issue_gate_findings(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    dispatch: &FocusedDispatch,
) -> Outcome<CommandAnswer> {
    issue_gate_findings_for_repository(bus, epoch, actor, dispatch, &dispatch.repository)
}

fn issue_gate_findings_for_repository(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    dispatch: &FocusedDispatch,
    repository: &Repository,
) -> Outcome<CommandAnswer> {
    let command = fixture_command(
        epoch,
        actor,
        CommandKind::GateFindings,
        Vec::new(),
        &dispatch.body,
    );
    gate_findings_folded_in_process(
        bus,
        &command,
        &dispatch.body,
        &dispatch.findings,
        gate::GateId::FileShape,
        dispatch.findings.is_empty(),
        repository,
        &gate_projects(),
        &board::NextConfig::default(),
    )
}

fn issue_gate_findings_raw(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    body: GateFindings,
    repository: &Repository,
) -> Outcome<CommandAnswer> {
    let command = fixture_command(epoch, actor, CommandKind::GateFindings, Vec::new(), &body);
    dispatch_with_repository(
        bus,
        &command,
        Some(repository),
        &gate_projects(),
        &board::NextConfig::default(),
    )
    .expect("gate finding dispatch is handled")
}

fn issue_gate_board_command<B: serde::Serialize>(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    kind: CommandKind,
    expected: Vec<Witness>,
    body: &B,
) -> Outcome<CommandAnswer> {
    let command = fixture_command(epoch, actor, kind, expected, body);
    dispatch_with_repository(
        bus,
        &command,
        None,
        &gate_projects(),
        &board::NextConfig::default(),
    )
    .expect("gate board command is handled")
}

#[allow(clippy::too_many_arguments)]
fn issue_folded_gate_findings(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    marker: &str,
    findings: &[FindingRow],
    instrument: gate::GateId,
    admits: bool,
    repository: &Repository,
    source: &wire::intent::GateFindingSource,
) -> Outcome<CommandAnswer> {
    let body = GateFindings {
        report: digest(marker),
        evidence: "{}".to_owned(),
        source: source.clone(),
    };
    let command = fixture_command(epoch, actor, CommandKind::GateFindings, Vec::new(), &body);
    gate_findings_folded(
        bus,
        &command,
        &body,
        findings,
        instrument,
        admits,
        repository,
        &gate_projects(),
        &board::NextConfig::default(),
    )
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
        authority: "file shape declaration fixture".to_owned(),
        locus: format!("gate=file-shape metric=file.lines key=path=\"src/{name}.rs\""),
        title: format!("Gate finding {name}"),
        description: format!("Finding {name} exceeded its bound\nGate: file-shape"),
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

fn task_done_validation(bus: &Bus, task: &TaskHandle) -> String {
    let born = task
        .born()
        .expect("gate finding handle carries its birth stamp");
    let snapshot = bus
        .fact_snapshot()
        .expect("gate finding completion facts read");
    let fact = snapshot
        .facts()
        .iter()
        .find(|fact| {
            fact.fact.kind() == wire::FactKind::TaskDone
                && fact
                    .fact
                    .body()
                    .get("task")
                    .and_then(serde_json::Value::as_str)
                    == Some(born.as_str())
        })
        .expect("recovered task carries a typed completion fact");
    assert!(
        fact.fact.body().get("source").is_some(),
        "live recovery completion must carry its proven transcript boundary"
    );
    fact.fact
        .body()
        .get("validation")
        .and_then(serde_json::Value::as_str)
        .expect("recovered task carries typed completion validation")
        .to_owned()
}

fn revise_gate_task(bus: &Bus, task: &TaskHandle, revise: impl FnOnce(&mut TaskHead)) {
    let snapshot = bus.task_snapshot().expect("gate task statement reads");
    let current = snapshot
        .tasks()
        .iter()
        .find(|heads| {
            task.born()
                .is_ok_and(|born| heads.statement().body().born == born)
        })
        .expect("gate task exists before legacy revision");
    let born = current.statement().body().born.clone();
    let mut statement = current.statement().body().clone();
    revise(&mut statement);
    let at = Instant::new(jiff::Timestamp::now()).expect("legacy revision instant");
    let origin = format!("task:{task}").parse().expect("gate task origin");
    let revision = u32::try_from(statement.revision.get())
        .unwrap_or(u32::MAX)
        .saturating_add(1);
    let mut write = JournalWrite::new(Actor::Operator, origin, at, Producer::CommandDoor);
    let edited = write
        .fact(
            body::TaskEdited {
                task: born,
                revision,
                notice: None,
            },
            FactCause::Root,
        )
        .expect("legacy fixture edit builds");
    write.task_head(edited, move |position| TaskHead {
        revision: position,
        ..statement
    });
    assert!(matches!(
        bus.commit(write).expect("legacy fixture edit commits"),
        CommitOutcome::Committed(_)
    ));
}

fn claim_gate_task(bus: &Bus, epoch: CanonicalUuid, seat: &Seat, task: &TaskHandle) -> Actor {
    let deferred = gate_board(bus, epoch, Actor::Operator);
    let heads = task_heads(&deferred, task).expect("deferred gate repair task");
    if heads.workflow.body.deferral.is_some() {
        assert_committed(issue_gate_board_command(
            bus,
            epoch,
            Actor::Operator,
            CommandKind::TaskWake,
            vec![workflow_witness(heads), deferral_witness(heads)],
            &TaskWake {
                task: task.clone(),
                into: None,
            },
        ));
    }

    let lane = Actor::Lane(seat.lane);
    let ready = gate_board(bus, epoch, lane.clone());
    let heads = task_heads(&ready, task).expect("woken gate repair task");
    let rule: Name = "capture".parse().expect("capture possession rule");
    assert_committed(issue_gate_board_command(
        bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
            Witness::Eligibility { rule: rule.clone() },
            residency(&ready, seat),
        ],
        &ClaimWin {
            rule,
            recovery: None,
        },
    ));
    lane
}

fn complete(bus: &Bus, epoch: CanonicalUuid, task: &TaskHandle) {
    let before = gate_board(bus, epoch, Actor::Operator);
    let current = task_heads(&before, task).expect("task exists before completion");
    let born = current.statement.body.born.clone();
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
                task: born.clone(),
                validation: "fixture completion".to_owned(),
                terminal: None,
                git: None,
                source: body::SourceBoundary::unrecorded(),
            },
            FactCause::Root,
        )
        .expect("completion fact builds");
    write.workflow_head(born, done, move |_| workflow);
    assert!(matches!(
        bus.commit(write).expect("completion commits"),
        CommitOutcome::Committed(_)
    ));
}
