//! Production selection reaches exactly one analysis in every arm family.

use super::*;

const REPRESENTATIVE_ARMS: [(GateId, ArmFamily); 4] = [
    (GateId::PlanPhase, ArmFamily::Core),
    (GateId::EnvNameLedger, ArmFamily::Toolchain),
    (GateId::FileShape, ArmFamily::Metrics),
    (GateId::Reachability, ArmFamily::Hygiene),
];

#[test]
fn focused_reports_distinguish_clean_findings_and_executor_failure() {
    let fixture = Fixture::new();
    let inputs = fixture.inputs();
    let gate_id = GateId::FileShape;
    let mut clean_executor = RecordingExecutor::all_passed(&fixture.source);
    let clean =
        run_focused_arm(&inputs, &mut clean_executor, gate_id).expect("execute clean focused arm");

    assert_eq!(clean_executor.calls, [gate_id]);
    assert!(clean.admits());
    assert!(clean.findings().is_empty());
    let bytes = clean.encode().expect("encode focused report");
    assert_eq!(
        FocusedArmReport::decode(&bytes).expect("decode canonical focused report"),
        clean
    );

    let mut findings_executor = RecordingExecutor::nonpassing(&fixture.source);
    let findings = run_focused_arm(&inputs, &mut findings_executor, gate_id)
        .expect("execute non-admitting focused arm");
    assert_eq!(findings_executor.calls, [gate_id]);
    assert!(!findings.admits());
    assert!(!findings.findings().is_empty());
    let canonical = findings.encode().expect("encode focused refusal");
    let fold = findings.refusal_fold();
    assert_eq!(fold, findings.refusal_fold());
    assert!(fold.as_bytes().len() <= BATTERY_REFUSAL_FOLD_MAX_BYTES);
    assert!(fold.as_str().contains("gate=file-shape state=unmeasured"));
    assert!(fold.as_str().contains(concat!(
        "full-evidence: \"$(/usr/bin/git rev-parse --path-format=absolute ",
        "--git-common-dir)/.spice-install/bin/spice\" dev xtask -- gate --report file-shape"
    )));
    assert!(fold.as_str().contains(concat!(
        "battery refusal summary: total_arms=1 admitted_arms=0 ",
        "finding_arms=0 cannot_run_arms=1"
    )));
    assert_eq!(
        findings.encode().expect("encode after focused fold"),
        canonical,
        "the human fold must not mutate canonical focused evidence"
    );

    let mut refused_executor = RecordingExecutor::all_passed(&fixture.source);
    refused_executor.refusal = Some(clean.result().position());
    let refused = run_focused_arm(&inputs, &mut refused_executor, gate_id)
        .expect("executor refusal is retained as a focused result");
    assert_eq!(refused.result().state(), BatteryState::Failed);
    assert!(matches!(
        refused.result().evidence(),
        ArmEvidence::ExecutorFailure { .. }
    ));
    assert_eq!(refused.findings().len(), 1);
}

#[test]
fn focused_report_decode_rejects_mismatched_or_repositioned_evidence() {
    let fixture = Fixture::new();
    let inputs = fixture.inputs();
    let mut executor = RecordingExecutor::all_passed(&fixture.source);
    let report =
        run_focused_arm(&inputs, &mut executor, GateId::FileShape).expect("execute focused arm");
    let value = serde_json::to_value(report).expect("focused report JSON");

    for candidate in [
        mutate(&value, |candidate| {
            candidate["result"]["gate_id"] = serde_json::json!(GateId::PlanPhase);
        }),
        mutate(&value, |candidate| {
            candidate["result"]["position"] = serde_json::json!(usize::MAX);
        }),
        mutate(&value, |candidate| {
            candidate["context"]["source"] = serde_json::json!(fixture_baseline("foreign"));
        }),
    ] {
        let bytes = serde_json::to_vec(&candidate).expect("encode forged focused report");
        FocusedArmReport::decode(&bytes).expect_err("forged focused report must be refused");
    }
}

#[test]
fn selected_production_arms_execute_alone_and_match_their_full_rows() {
    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    // Reach a real native hygiene parse while keeping selected/full equality
    // independent of completed-run wall-clock evidence.
    fs::write(repository.join("app/main.py"), "def broken(:\n")
        .expect("write selected invalid Python source");
    fixture_git(&repository, &["add", "app/main.py"]);
    let selector = crate::Selector::staged();
    let mut complete = crate::BatteryCapture::capture(
        &repository,
        &selector,
        crate::ClaimCapture::Idle,
        acting(),
        rim(&repository),
        hygiene(),
    )
    .expect("capture complete production battery");
    let structural = complete
        .execute_structural_preflight()
        .expect("project complete capture structural preflight");
    let report = complete
        .execute()
        .expect("execute complete production battery");
    assert_eq!(
        structural
            .iter()
            .map(FocusedArmReport::gate_id)
            .collect::<Vec<_>>(),
        [GateId::FileShape, GateId::Complexity]
    );
    for focused in &structural {
        assert_eq!(
            focused.result(),
            report
                .results()
                .iter()
                .find(|result| result.gate_id() == focused.gate_id())
                .expect("complete battery reuses each structural result")
        );
    }

    for (gate_id, family) in REPRESENTATIVE_ARMS {
        let mut events = Vec::new();
        let mut record = |event| events.push(event);
        let mut selected = crate::BatteryCapture::capture_arm_traced(
            &repository,
            &selector,
            crate::ClaimCapture::Idle,
            acting(),
            rim(&repository),
            hygiene(),
            gate_id,
            &mut record,
        )
        .expect("capture selected production arm");
        let complete_error = selected
            .execute()
            .expect_err("selected capture must refuse complete execution");
        assert!(
            complete_error
                .detail()
                .contains("cannot execute a complete battery")
        );
        let sibling = GateId::BATTERY
            .into_iter()
            .find(|candidate| *candidate != gate_id)
            .expect("the battery contains a sibling arm");
        let mismatch = selected
            .execute_arm(sibling)
            .expect_err("selected capture must refuse a different arm");
        assert!(
            mismatch
                .detail()
                .contains("requires a capture selected for that arm")
        );
        let result = selected
            .execute_arm(gate_id)
            .expect("execute selected production arm");
        drop(selected);

        assert_eq!(
            &result,
            report
                .results()
                .iter()
                .find(|candidate| candidate.gate_id() == gate_id)
                .expect("complete battery contains selected row")
        );
        assert_eq!(
            analyses(&events),
            [(gate_id, family)],
            "{gate_id} executed a sibling analysis"
        );
        assert_eq!(invoked_arms(&events), [(gate_id, family)]);
        assert_eq!(invoked_families(&events), [family]);
    }
}

#[test]
fn reached_file_shape_measures_only_the_authored_path_and_binds_its_source() {
    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    fs::write(repository.join("src/lib.rs"), "unrelated\n".repeat(4_000))
        .expect("write unrelated committed file-shape breach");
    fixture_git(&repository, &["add", "src/lib.rs"]);
    fixture_git(
        &repository,
        &["commit", "-q", "-m", "unrelated file-shape breach"],
    );
    fs::create_dir_all(repository.join("app/src")).expect("reached Rust source directory");
    let reached = "app/src/reached.rs";
    fs::write(repository.join(reached), "pub fn reached() {}\n")
        .expect("write reached Rust source");
    fixture_git(&repository, &["add", reached]);

    let selection = reached_selection(&repository, reached);
    let report = reached_report(&repository, &selection, reached, GateId::FileShape);
    assert!(report.admits(), "{}", report.result().reason());
    let ArmEvidence::Metrics { report: metrics } = report.result().evidence() else {
        panic!("reached file-shape arm must retain structural evidence")
    };
    assert_eq!(metrics.metrics().len(), 2);
    assert!(
        metrics.metrics().iter().all(|metric| {
            metric.key() == &crate::metrics::MetricKey::path(reached).expect("reached metric key")
        }),
        "unexpected reached file-shape metrics: {:?}",
        metrics.metrics()
    );

    let direct = crate::ChangeReachObject::Path {
        path: reached.to_owned(),
    };
    let extra = crate::ChangeReachObject::Path {
        path: "src/lib.rs".to_owned(),
    };
    let mut expanded_reach = selection.reach().to_vec();
    let index = expanded_reach
        .iter()
        .position(|reach| reach.gate_id() == GateId::FileShape)
        .expect("file-shape reach record");
    expanded_reach[index] = crate::ChangeArmReach::focused(
        GateId::FileShape,
        vec![direct.clone(), extra.clone()],
        vec![
            crate::ChangeReachEdge::changed(reached, direct.clone()),
            crate::ChangeReachEdge::dependency(reached, direct, extra),
        ],
    );
    let expanded = selection
        .clone()
        .with_reach(expanded_reach)
        .expect("dependency-expanded reach remains structurally valid");
    let state_run =
        crate::BatteryStateRun::capture(&repository).expect("capture expanded reach run");
    let error = match crate::BatteryCapture::capture_arm_in_state_reached(
        &repository,
        &crate::Selector::staged(),
        crate::ClaimCapture::Idle,
        acting(),
        reached_rim(&repository, reached),
        hygiene(),
        &expanded,
        GateId::FileShape,
        state_run,
    ) {
        Ok(_) => panic!("path-local reach must not admit dependency-expanded paths"),
        Err(error) => error,
    };
    assert!(
        error.detail().contains("direct authored path selection"),
        "{error}"
    );

    fs::write(
        repository.join(reached),
        "pub fn reached_after_selection() {}\n",
    )
    .expect("change selected source after reach was authorized");
    fixture_git(&repository, &["add", reached]);
    let state_run = crate::BatteryStateRun::capture(&repository).expect("capture stale reach run");
    let error = match crate::BatteryCapture::capture_arm_in_state_reached(
        &repository,
        &crate::Selector::staged(),
        crate::ClaimCapture::Idle,
        acting(),
        reached_rim(&repository, reached),
        hygiene(),
        &selection,
        GateId::FileShape,
        state_run,
    ) {
        Ok(_) => panic!("stale reach authority must not capture a different source"),
        Err(error) => error,
    };
    assert!(
        error.detail().contains("different immutable source"),
        "{error}"
    );
}

#[test]
fn reached_markdown_parses_only_authored_docs_but_resolves_unmodified_targets() {
    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    fs::create_dir_all(repository.join("docs")).expect("reached documentation directory");
    let reached = "docs/reached.md";
    fs::write(
        repository.join(reached),
        "# Reached\n\nThe unchanged implementation is [here](../src/lib.rs).\n",
    )
    .expect("write reached Markdown source");
    fixture_git(&repository, &["add", reached]);

    let selection = reached_selection(&repository, reached);
    let report = reached_report(&repository, &selection, reached, GateId::MarkdownLinks);
    assert!(
        report.admits(),
        "the reached link must resolve through the complete tracked inventory while the unrelated bad-case link stays out of scope: {}",
        report.result().reason()
    );
    let ArmEvidence::Metrics { report: metrics } = report.result().evidence() else {
        panic!("reached Markdown arm must retain structural evidence")
    };
    assert!(!metrics.metrics().is_empty());
    assert!(
        metrics.metrics().iter().all(|metric| {
            matches!(metric.key(), crate::metrics::MetricKey::Arm)
                || metric.key()
                    == &crate::metrics::MetricKey::path(reached).expect("reached metric key")
        }),
        "unexpected reached Markdown metrics: {:?}",
        metrics.metrics()
    );
}

#[test]
fn reached_assertion_scan_does_not_report_an_unrelated_committed_test() {
    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    let reached = "app/main.py";
    fs::write(repository.join(reached), "def entry():\n    return 2\n")
        .expect("write reached production source");
    fixture_git(&repository, &["add", reached]);

    let selection = reached_selection(&repository, reached);
    let report = reached_report(&repository, &selection, reached, GateId::AssertionFreeTests);
    assert!(
        report.admits(),
        "the committed tests/test_main.py assertion is outside authored reach: {}",
        report.result().reason()
    );
    let ArmEvidence::Hygiene { report: hygiene } = report.result().evidence() else {
        panic!("reached assertion arm must retain hygiene evidence")
    };
    assert!(hygiene.findings.is_empty());
}

#[test]
fn formatter_evidence_distinguishes_focused_reach_from_exhaustive_capture() {
    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    fs::create_dir_all(repository.join("app/src")).expect("reached Rust source directory");
    let reached = "app/src/reached.rs";
    fs::write(repository.join(reached), "pub fn reached() {}\n")
        .expect("write reached Rust source");
    fixture_git(&repository, &["add", reached]);

    let selection = reached_selection(&repository, reached);
    let focused = reached_report(&repository, &selection, reached, GateId::Formatters);
    let ArmEvidence::Toolchain { report: focused } = focused.result().evidence() else {
        panic!("reached formatter arm must retain toolchain evidence")
    };
    assert_eq!(
        focused.tree_input,
        crate::toolchain::TreeInput::SelectedChanges
    );

    let state_run = crate::BatteryStateRun::capture(&repository).expect("capture exhaustive run");
    let mut exhaustive = crate::BatteryCapture::capture_arm_in_state(
        &repository,
        &crate::Selector::staged(),
        crate::ClaimCapture::Idle,
        acting(),
        reached_rim(&repository, reached),
        hygiene(),
        GateId::Formatters,
        state_run,
    )
    .expect("capture legacy exhaustive formatter arm");
    let exhaustive = exhaustive
        .execute_arm_report(GateId::Formatters)
        .expect("execute legacy exhaustive formatter arm");
    let ArmEvidence::Toolchain { report: exhaustive } = exhaustive.result().evidence() else {
        panic!("exhaustive formatter arm must retain toolchain evidence")
    };
    assert_eq!(
        exhaustive.tree_input,
        crate::toolchain::TreeInput::CompleteSelectedTree
    );
}

#[test]
fn reached_formatter_deletion_is_inapplicable_before_tool_discovery() {
    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    fs::create_dir_all(repository.join("app/src")).expect("deleted Rust source directory");
    let reached = "app/src/deleted.rs";
    fs::write(repository.join(reached), "pub fn deleted() {}\n")
        .expect("write soon-deleted Rust source");
    fixture_git(&repository, &["add", reached]);
    fixture_git(&repository, &["commit", "-q", "-m", "add deleted source"]);
    fs::remove_file(repository.join(reached)).expect("delete reached Rust source");
    fixture_git(&repository, &["add", "-u", "--", reached]);

    let selection = reached_selection(&repository, reached);
    let report = reached_report(&repository, &selection, reached, GateId::Formatters);
    let ArmEvidence::Toolchain { report } = report.result().evidence() else {
        panic!("reached formatter arm must retain toolchain evidence")
    };
    assert_eq!(report.outcome, crate::toolchain::ArmOutcome::NotApplicable);
    assert!(report.tools.is_empty());
    assert_eq!(
        report.tree_input,
        crate::toolchain::TreeInput::SelectedChanges
    );
}

fn reached_selection(repository: &Path, reached: &str) -> crate::ChangeSelectionReport {
    let state_run =
        crate::BatteryStateRun::capture(repository).expect("capture selection state run");
    let anchor = crate::BatteryCapture::capture_arm_in_state(
        repository,
        &crate::Selector::staged(),
        crate::ClaimCapture::Idle,
        acting(),
        reached_rim(repository, reached),
        hygiene(),
        GateId::Staging,
        state_run,
    )
    .expect("capture immutable selection source");
    crate::select_current_source_change_gates(anchor.source().clone(), &[reached.to_owned()])
        .expect("select reached current-source arm plan")
}

fn reached_report(
    repository: &Path,
    selection: &crate::ChangeSelectionReport,
    reached: &str,
    gate_id: GateId,
) -> crate::FocusedArmReport {
    assert!(selection.gate_ids().contains(&gate_id));
    let state_run = crate::BatteryStateRun::capture(repository).expect("capture reached state run");
    let mut capture = crate::BatteryCapture::capture_arm_in_state_reached(
        repository,
        &crate::Selector::staged(),
        crate::ClaimCapture::Idle,
        acting(),
        reached_rim(repository, reached),
        hygiene(),
        selection,
        gate_id,
        state_run,
    )
    .expect("capture reached production arm");
    assert_eq!(
        capture.execution_objects(),
        selection.reach_for(gate_id).map(|reach| reach.objects()),
        "focused capture must retain exactly its validated reach objects"
    );
    capture
        .execute_arm_report(gate_id)
        .expect("execute reached production arm")
}

fn reached_rim(repository: &Path, reached: &str) -> crate::RimCapture {
    crate::RimCapture::new(repository, None, [reached.to_owned()])
        .expect("fixture repository root is absolute")
}

fn acting() -> crate::ActorCapture {
    crate::ActorCapture::Acting {
        actor: "battery-actor".to_owned(),
        bound_claims: Vec::new(),
    }
}

fn rim(repository: &Path) -> crate::RimCapture {
    crate::RimCapture::new(
        repository,
        None,
        ["app/main.py", "src/lib.rs", "tests/test_main.py"].map(str::to_owned),
    )
    .expect("fixture repository root is absolute")
}

fn hygiene() -> crate::HygieneConfiguration {
    crate::HygieneConfiguration::new(["app/main.py"], ["tests"])
        .expect("normalized applicable hygiene roots")
}

fn analyses(events: &[RepositoryExecutionEvent]) -> Vec<(GateId, ArmFamily)> {
    events
        .iter()
        .filter_map(|event| match event {
            RepositoryExecutionEvent::AnalysisInvoked { gate_id, family } => {
                Some((*gate_id, *family))
            }
            RepositoryExecutionEvent::ArmInvoked { .. }
            | RepositoryExecutionEvent::FamilyInvoked { .. }
            | RepositoryExecutionEvent::ToolchainAnalysisCompleted { .. } => None,
        })
        .collect()
}

fn invoked_arms(events: &[RepositoryExecutionEvent]) -> Vec<(GateId, ArmFamily)> {
    events
        .iter()
        .filter_map(|event| match event {
            RepositoryExecutionEvent::ArmInvoked {
                gate_id, family, ..
            } => Some((*gate_id, *family)),
            RepositoryExecutionEvent::FamilyInvoked { .. }
            | RepositoryExecutionEvent::AnalysisInvoked { .. }
            | RepositoryExecutionEvent::ToolchainAnalysisCompleted { .. } => None,
        })
        .collect()
}

fn invoked_families(events: &[RepositoryExecutionEvent]) -> Vec<ArmFamily> {
    events
        .iter()
        .filter_map(|event| match event {
            RepositoryExecutionEvent::FamilyInvoked { family } => Some(*family),
            RepositoryExecutionEvent::ArmInvoked { .. }
            | RepositoryExecutionEvent::AnalysisInvoked { .. }
            | RepositoryExecutionEvent::ToolchainAnalysisCompleted { .. } => None,
        })
        .collect()
}
