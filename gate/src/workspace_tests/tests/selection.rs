use super::*;

#[test]
fn staged_rust_source_selects_its_direct_cargo_test_target_with_exact_edge() {
    let inventory = fixture_inventory();
    let source = wire::Digest::compute(wire::Domain::GateTree, b"selected Rust tree");

    let report = selection_report(
        &inventory,
        Path::new("/workspace"),
        source.clone(),
        &["fixture-a/src/behavior.rs".to_owned()],
    )
    .expect("workspace reach graph selects tests");

    assert!(report.admits());
    assert_eq!(report.source(), &source);
    assert_eq!(report.changed_sources(), ["fixture-a/src/behavior.rs"]);
    assert_eq!(report.tests(), ["fixture-a::lib:fixture-a"]);
    assert_eq!(report.edges().len(), 1);
    assert!(
        report
            .edges()
            .iter()
            .all(|edge| edge.source() == "fixture-a/src/behavior.rs")
    );
    assert_eq!(
        crate::TestSelectionReport::decode(&report.encode().expect("encode selection"))
            .expect("decode selection"),
        report
    );
}

#[test]
fn staged_integration_test_submodule_selects_its_own_cargo_test_target() {
    let mut inventory = fixture_inventory();
    inventory.packages[0].targets.push(TargetIdentity {
        package: "fixture-a-id".to_owned(),
        kind: "test".to_owned(),
        name: "behavior".to_owned(),
        source: PathBuf::from("/workspace/fixture-a/tests/behavior.rs"),
    });

    let report = selection_report(
        &inventory,
        Path::new("/workspace"),
        wire::Digest::compute(wire::Domain::GateTree, b"integration submodule"),
        &["fixture-a/tests/behavior/nested.rs".to_owned()],
    )
    .expect("integration-test submodule reach selects tests");

    assert!(report.admits());
    assert_eq!(
        report.changed_sources(),
        ["fixture-a/tests/behavior/nested.rs"]
    );
    assert_eq!(report.tests(), ["fixture-a::test:behavior"]);
    assert_eq!(report.edges().len(), 1);
    assert_eq!(
        report.edges()[0].source(),
        "fixture-a/tests/behavior/nested.rs"
    );
}

#[test]
fn staged_library_source_selects_every_test_enabled_target_that_may_link_it() {
    let mut inventory = fixture_inventory();
    inventory.packages[0].targets.push(TargetIdentity {
        package: "fixture-a-id".to_owned(),
        kind: "test".to_owned(),
        name: "behavior".to_owned(),
        source: PathBuf::from("/workspace/fixture-a/tests/behavior.rs"),
    });

    let report = selection_report(
        &inventory,
        Path::new("/workspace"),
        wire::Digest::compute(wire::Domain::GateTree, b"library-dependent tests"),
        &["fixture-a/src/behavior.rs".to_owned()],
    )
    .expect("library reach expands through its package test targets");

    assert!(report.admits());
    assert_eq!(
        report.tests(),
        ["fixture-a::lib:fixture-a", "fixture-a::test:behavior"]
    );
    assert_eq!(report.edges().len(), 2);
    assert!(
        report
            .edges()
            .iter()
            .all(|edge| edge.source() == "fixture-a/src/behavior.rs")
    );
}

#[test]
fn rust_source_inside_a_package_but_outside_target_roots_refuses_narrowing() {
    let report = selection_report(
        &fixture_inventory(),
        Path::new("/workspace"),
        wire::Digest::compute(wire::Domain::GateTree, b"unreachable package source"),
        &["fixture-a/tools/unreachable.rs".to_owned()],
    )
    .expect("unreachable package source remains reportable");

    assert!(!report.admits());
    assert!(report.tests().is_empty());
    assert!(report.edges().is_empty());
    let crate::CoverageCompleteness::Unknown { reason } = report.coverage() else {
        panic!("unreachable package source must remain unknown");
    };
    assert!(reason.contains("reaches no direct Cargo test target"));
}

#[test]
fn rust_source_outside_workspace_packages_refuses_narrowing_loudly() {
    let report = selection_report(
        &fixture_inventory(),
        Path::new("/workspace"),
        wire::Digest::compute(wire::Domain::GateTree, b"unknown Rust tree"),
        &["tools/outside.rs".to_owned()],
    )
    .expect("unknown selection remains reportable");

    assert!(!report.admits());
    assert!(report.tests().is_empty());
    assert!(report.edges().is_empty());
    let crate::CoverageCompleteness::Unknown { reason } = report.coverage() else {
        panic!("unmapped Rust source must remain unknown");
    };
    assert!(reason.contains("outside every workspace package"));
    let document = String::from_utf8(report.encode().expect("encode refusal")).unwrap();
    assert!(document.contains("tools/outside.rs"));
    assert!(document.contains("outside every workspace package"));
}

#[test]
fn partially_mapped_rust_sources_retain_edges_but_refuse_as_incomplete() {
    let report = selection_report(
        &fixture_inventory(),
        Path::new("/workspace"),
        wire::Digest::compute(wire::Domain::GateTree, b"incomplete Rust tree"),
        &[
            "fixture-a/src/behavior.rs".to_owned(),
            "tools/outside.rs".to_owned(),
        ],
    )
    .expect("incomplete selection remains reportable");

    assert!(!report.admits());
    assert_eq!(report.tests(), ["fixture-a::lib:fixture-a"]);
    assert_eq!(report.edges().len(), 1);
    assert_eq!(report.edges()[0].source(), "fixture-a/src/behavior.rs");
    let crate::CoverageCompleteness::Incomplete { reason } = report.coverage() else {
        panic!("partially mapped Rust selection must remain incomplete");
    };
    assert!(reason.contains("tools/outside.rs"));
}
