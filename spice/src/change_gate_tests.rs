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
fn ordinary_policy_wire_shape_rejects_removed_applicable_coverage() {
    let error = serde_json::from_slice::<ChangePolicyReport>(
        br#"{"coverage":"applicable","source":"removed","results":[]}"#,
    )
    .expect_err("ordinary policy evidence cannot retain applicable-wide execution");

    assert!(error.to_string().contains("unknown variant `applicable`"));
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
