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
    let report = complete
        .execute()
        .expect("execute complete production battery");

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
