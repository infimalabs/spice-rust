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

#[test]
fn pretest_screen_covers_reusable_and_cheap_static_policy() {
    let screened = GateId::BATTERY
        .into_iter()
        .filter(|gate_id| pretest_screen_gate(*gate_id))
        .collect::<Vec<_>>();

    assert_eq!(
        screened,
        vec![
            GateId::RustTypecheck,
            GateId::CanvasWebTypecheck,
            GateId::Complexity,
        ],
    );
}

#[test]
fn policy_schedule_starts_known_long_arms_before_short_remainder_work() {
    assert!(policy_schedule_rank(GateId::EnvNameLedger) < policy_schedule_rank(GateId::RepoDocs));
    assert!(policy_schedule_rank(GateId::MarkdownLinks) < policy_schedule_rank(GateId::RepoShape));
    assert!(policy_schedule_rank(GateId::Formatters) < policy_schedule_rank(GateId::LocalPaths));
    assert!(policy_schedule_rank(GateId::MagicNumbers) < policy_schedule_rank(GateId::Taste));

    let source = wire::Digest::compute(wire::Domain::GateTree, b"policy schedule order");
    let selection =
        gate::select_current_source_change_gates(source, &["spice/src/change_gate.rs".to_owned()])
            .expect("select the benchmark policy inventory");
    let mut scheduled = selection
        .reach()
        .iter()
        .filter(|reach| !pretest_screen_gate(reach.gate_id()) && reach.gate_id() != GateId::Staging)
        .enumerate()
        .collect::<Vec<_>>();
    scheduled.sort_by_key(|(index, reach)| (policy_schedule_rank(reach.gate_id()), *index));
    assert_eq!(
        scheduled
            .into_iter()
            .map(|(_, reach)| reach.gate_id())
            .take(3)
            .collect::<Vec<_>>(),
        [
            GateId::Formatters,
            GateId::EnvNameLedger,
            GateId::MarkdownLinks,
        ],
    );
}

#[test]
fn budget_publication_warns_only_above_the_exact_advisory_share() {
    let below = published_budget(CargoBudget::from_allocations(12, 1, 1, 1));
    assert!(below.contains("advisory_share=3"));
    assert!(!below.contains("change-gate-budget-warning"));

    let equal = published_budget(CargoBudget::from_allocations(12, 1, 1, 2));
    assert!(!equal.contains("change-gate-budget-warning"));

    let above = published_budget(CargoBudget::from_allocations(12, 1, 1, 3));
    assert_eq!(above.matches("[change-gate-budget-warning]").count(), 1);
    assert!(above.contains("[change-gate-budget-warning] reason=advisory-lane-share-exceeded"));
    assert!(above.contains("host_capacity=12 ordinary_lanes=4 advisory_share=3"));
    assert!(above.contains("requested_slots=4"));
    assert!(above.contains(
        "allocations=policy_jobs:1+test_prebuild_shards:1x3=3 action=continue-unchanged"
    ));
}

#[test]
fn measured_twelve_cpu_budget_caps_one_gate_at_three_direct_workers() {
    let budget = CargoBudget::from_capacity(12, 4);
    let published = published_budget(budget);

    assert_eq!(budget.worker_processes(), 3);
    assert_eq!(budget.policy_jobs(), 1);
    assert_eq!(budget.test_concurrency(), 1);
    assert_eq!(budget.shard_jobs(), 1);
    assert!(published.contains(
        "cpus=12 ordinary_lanes=4 advisory_share=3 worker_process_cap=3 policy_jobs=1 test_concurrency=1 shard_jobs=1"
    ));
    assert!(published.contains("worker_scope=direct-roots nested_children=inherit-parent-slot"));
    assert!(!published.contains("change-gate-budget-warning"));
}

fn published_budget(budget: CargoBudget) -> String {
    let mut output = Vec::new();
    publish_budget(&mut output, budget, "overlapped-policy-tests").expect("publish gate budget");
    String::from_utf8(output).expect("budget publication is text")
}
