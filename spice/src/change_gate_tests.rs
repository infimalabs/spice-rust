use super::*;

use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

#[test]
fn report_rejects_missing_and_source_mismatched_policy_evidence() {
    let source = wire::Digest::compute(wire::Domain::GateTree, b"change gate report");
    let selection =
        gate::select_current_source_change_gates(source.clone(), &[]).expect("empty selection");
    let tests = gate::TestEvidenceGraph::select_tests(
        source,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        gate::CoverageCompleteness::Complete,
    )
    .expect("empty test selection");

    let policy = ChangePolicyReport::Focused {
        source: selection.source().clone(),
        results: Vec::new(),
    };
    let error = ChangeGateReport::new(selection, tests, policy)
        .expect_err("selected always-on arms require focused evidence");
    assert_eq!(error.reason(), "gate::change-gate-policy-incomplete");
}

#[test]
fn exhaustive_report_requires_test_edges_for_every_changed_source() {
    let source = wire::Digest::compute(wire::Domain::GateTree, b"exhaustive report");
    let selection =
        gate::select_current_source_change_gates(source.clone(), &["assets/opaque.bin".into()])
            .expect("unknown path escalates");
    let tests = gate::TestEvidenceGraph::select_tests(
        source,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        gate::CoverageCompleteness::Complete,
    )
    .expect("empty test evidence remains structurally valid");

    let policy = ChangePolicyReport::Focused {
        source: selection.source().clone(),
        results: Vec::new(),
    };
    let error = ChangeGateReport::new(selection, tests, policy)
        .expect_err("exhaustive selection cannot omit its changed test source");
    assert_eq!(
        error.reason(),
        "gate::change-gate-exhaustive-test-incomplete"
    );
}

#[test]
fn exhaustive_refusal_requires_and_returns_its_bounded_battery_fold() {
    let source = wire::Digest::compute(wire::Domain::GateTree, b"exhaustive refusal");
    let changed = vec!["assets/opaque.bin".to_owned()];
    let selection = gate::select_current_source_change_gates(source.clone(), &changed)
        .expect("unknown path escalates");
    let test = "fixture::lib:fixture".to_owned();
    let edge = gate::TestSelectionEdge::new(changed[0].clone(), test.clone())
        .expect("synthetic exhaustive edge");
    let tests = gate::TestEvidenceGraph::select_tests(
        source.clone(),
        changed,
        vec![test],
        vec![edge],
        gate::CoverageCompleteness::Complete,
    )
    .expect("complete exhaustive test evidence");
    let authority = wire::Digest::compute(wire::Domain::GateBatteryReport, b"full report");
    let manifest = BatteryManifest::canonical().expect("canonical manifest");
    let results = manifest
        .arms()
        .iter()
        .enumerate()
        .map(|(position, arm)| {
            let ArmDisposition::Compiled {
                gate_id,
                family,
                declaration,
            } = arm.disposition();
            PolicyArmSummary {
                position,
                gate_id: *gate_id,
                family: *family,
                declaration: declaration.clone(),
                state: if position == 0 {
                    BatteryState::Failed
                } else {
                    BatteryState::Passed
                },
                reason: if position == 0 {
                    "synthetic refusal\n  README.md: 21 characters (cap 20)"
                } else {
                    "synthetic pass"
                }
                .to_owned(),
                authority: authority.clone(),
            }
        })
        .collect::<Vec<_>>();
    let policy = ChangePolicyReport::Exhaustive {
        source: source.clone(),
        report_digest: authority.clone(),
        report_bytes: 1,
        results: results.clone(),
        refusal_fold: Some("exact bounded finding\n".to_owned()),
    };
    let report = ChangeGateReport::new(selection.clone(), tests.clone(), policy)
        .expect("complete refusing summary admits structurally");
    assert_eq!(
        report.policy().iter().next().expect("first arm").reason(),
        "synthetic refusal\n  README.md: 21 characters (cap 20)"
    );
    assert_eq!(report.refusal_fold(), "exact bounded finding\n");

    let missing = ChangePolicyReport::Exhaustive {
        source,
        report_digest: authority,
        report_bytes: 1,
        results,
        refusal_fold: None,
    };
    let error = ChangeGateReport::new(selection, tests, missing)
        .expect_err("a refusing complete summary requires its bounded fold");
    assert_eq!(
        error.reason(),
        "gate::change-gate-exhaustive-policy-invalid"
    );
}

#[test]
fn malformed_staged_policy_refuses_before_any_compiled_inventory_is_selected() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "spice-change-policy-malformed-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    ));
    fs::create_dir(&root).expect("create malformed-policy fixture");
    fs::write(root.join("spice-rust.toml"), "[policy\ninvalid = true\n")
        .expect("write malformed staged policy");
    let paths = vec!["spice-rust.toml".to_owned()];

    let error = packaged_selection_policy(&root, &paths)
        .expect_err("malformed staged policy cannot use a compiled inventory");

    assert_eq!(error.reason(), "gate::change-selection-policy-refused");
    assert!(error.detail().contains("configuration"), "{error}");
    fs::remove_dir_all(root).expect("remove malformed-policy fixture");
}
