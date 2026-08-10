//! Staged-source selection cases for workspace Rust test targets.

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

/// Explicit, conventional, and included nested sources belong to one integration target without
/// making same-directory peers part of the selection.
#[test]
fn staged_path_module_selects_only_its_declaring_integration_target() {
    let root = fixture_root("workspace-path-module-selection");
    let tests = root.join("fixture/tests");
    fs::create_dir_all(tests.join("resident/lane_continuity")).expect("create path-module fixture");
    fs::write(
        tests.join("resident.rs"),
        concat!(
            "#[path = \"resident/lane_continuity.rs\"]\n",
            "mod lane_continuity;\n",
            "include!(\"resident/tail.rs\");\n",
        ),
    )
    .expect("write declaring integration target");
    fs::write(
        tests.join("resident/lane_continuity.rs"),
        "mod nested;\n#[test]\nfn continuity() {}\n",
    )
    .expect("write declared path module");
    fs::write(
        tests.join("resident/lane_continuity/nested.rs"),
        "#[test]\nfn nested() {}\n",
    )
    .expect("write conventionally nested module");
    fs::write(
        tests.join("resident/tail.rs"),
        "#[test]\nfn included() {}\n",
    )
    .expect("write included test tail");
    fs::write(tests.join("other.rs"), "#[test]\nfn other() {}\n")
        .expect("write unrelated integration target");
    fs::write(
        root.join("fixture/Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\n",
    )
    .expect("write path-module fixture manifest");
    let root = fs::canonicalize(root).expect("canonical path-module fixture root");
    let tests = root.join("fixture/tests");
    let inventory = inventory_from_metadata(
        Metadata {
            workspace_members: vec!["fixture-id".to_owned()],
            target_directory: root.join("target"),
            build_directory: root.join("target/cargo-build"),
            packages: vec![MetadataPackage {
                id: "fixture-id".to_owned(),
                name: "fixture".to_owned(),
                manifest_path: root.join("fixture/Cargo.toml"),
                targets: vec![
                    MetadataTarget {
                        name: "other".to_owned(),
                        kind: vec!["test".to_owned()],
                        test: true,
                        src_path: tests.join("other.rs"),
                    },
                    MetadataTarget {
                        name: "resident".to_owned(),
                        kind: vec!["test".to_owned()],
                        test: true,
                        src_path: tests.join("resident.rs"),
                    },
                ],
            }],
        },
        &root,
    )
    .expect("acquire path-module target inventory");

    let report = selection_report(
        &inventory,
        &root,
        wire::Digest::compute(wire::Domain::GateTree, b"declared path module"),
        &[
            "fixture/tests/resident/lane_continuity.rs".to_owned(),
            "fixture/tests/resident/lane_continuity/nested.rs".to_owned(),
            "fixture/tests/resident/tail.rs".to_owned(),
        ],
    )
    .expect("declared path module maps to its integration target");

    assert!(report.admits());
    assert_eq!(
        report.changed_sources(),
        [
            "fixture/tests/resident/lane_continuity.rs",
            "fixture/tests/resident/lane_continuity/nested.rs",
            "fixture/tests/resident/tail.rs",
        ],
        "every nested source remains explicit in the selected-tree report"
    );
    assert_eq!(report.tests(), ["fixture::test:resident"]);
    assert_eq!(report.edges().len(), 3);
    assert_eq!(
        report
            .edges()
            .iter()
            .map(crate::TestSelectionEdge::source)
            .collect::<Vec<_>>(),
        [
            "fixture/tests/resident/lane_continuity.rs",
            "fixture/tests/resident/lane_continuity/nested.rs",
            "fixture/tests/resident/tail.rs",
        ]
    );
    fs::remove_dir_all(root).expect("remove path-module fixture");
}

#[test]
fn staged_library_source_selects_every_test_enabled_target_that_may_link_it() {
    let mut inventory = fixture_inventory();
    inventory.packages[0].targets.push(TargetIdentity {
        package: "fixture-a-id".to_owned(),
        kind: "test".to_owned(),
        name: "behavior".to_owned(),
        source: PathBuf::from("/workspace/fixture-a/tests/behavior.rs"),
        declared_path_sources: Vec::new(),
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
