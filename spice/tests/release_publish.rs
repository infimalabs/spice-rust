//! Native release-publication boundary locks.

#[test]
fn release_correctness_uses_the_canonical_gate_not_the_retired_verifier() {
    let source = include_str!("../src/release_execution.rs");

    assert!(source.contains("spice::gate::run_strict_at("));
    assert!(source.contains("gate::Selector::revision(source_commit)"));
    assert!(source.contains("run_release_gate(&self.root, &plan.source_commit)?"));
    assert!(!source.contains("\"xtask\", \"verify\""));
}
