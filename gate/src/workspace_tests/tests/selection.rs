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
        source_bytes: 1,
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
fn package_manifest_and_build_script_select_every_test_target_in_their_package() {
    let mut inventory = fixture_inventory();
    inventory.packages[0].targets.push(TargetIdentity {
        package: "fixture-a-id".to_owned(),
        kind: "test".to_owned(),
        name: "behavior".to_owned(),
        source: PathBuf::from("/workspace/fixture-a/tests/behavior.rs"),
        declared_path_sources: Vec::new(),
        source_bytes: 1,
    });

    for source in ["fixture-a/Cargo.toml", "fixture-a/build.rs"] {
        let report = selection_report(
            &inventory,
            Path::new("/workspace"),
            wire::Digest::compute(wire::Domain::GateTree, source.as_bytes()),
            &[source.to_owned()],
        )
        .expect("package-wide build source selects tests");

        assert!(report.admits());
        assert_eq!(
            report.tests(),
            ["fixture-a::lib:fixture-a", "fixture-a::test:behavior"]
        );
        assert!(report.edges().iter().all(|edge| edge.source() == source));
    }
}

#[test]
fn workspace_and_gate_authorities_select_the_complete_test_inventory() {
    let inventory = fixture_inventory();
    for source in ["Cargo.lock", "spice-rust.toml"] {
        let report = selection_report(
            &inventory,
            Path::new("/workspace"),
            wire::Digest::compute(wire::Domain::GateTree, source.as_bytes()),
            &[source.to_owned()],
        )
        .expect("workspace authority selects every test target");

        assert!(report.admits());
        assert_eq!(
            report.tests(),
            ["fixture-a::lib:fixture-a", "fixture-b::bin:fixture-b"]
        );
        assert!(report.edges().iter().all(|edge| edge.source() == source));
    }
}

#[test]
fn explicit_exhaustive_mode_maps_even_unknown_non_build_paths_to_every_target() {
    let source = wire::Digest::compute(wire::Domain::GateTree, b"exhaustive unknown path");
    let report = exhaustive_selection_report(
        &fixture_inventory(),
        source.clone(),
        &["assets/opaque.bin".to_owned()],
    )
    .expect("exhaustive selection maps every changed path");

    assert!(report.admits());
    assert_eq!(report.source(), &source);
    assert_eq!(report.changed_sources(), ["assets/opaque.bin"]);
    assert_eq!(
        report.tests(),
        ["fixture-a::lib:fixture-a", "fixture-b::bin:fixture-b"]
    );
    assert_eq!(report.edges().len(), 2);
    assert!(
        report
            .edges()
            .iter()
            .all(|edge| edge.source() == "assets/opaque.bin")
    );
    assert_eq!(report.coverage(), &crate::CoverageCompleteness::Complete);
}

#[test]
fn rust_source_inside_a_package_but_outside_target_roots_escalates_to_the_workspace() {
    let report = selection_report(
        &fixture_inventory(),
        Path::new("/workspace"),
        wire::Digest::compute(wire::Domain::GateTree, b"unreachable package source"),
        &["fixture-a/tools/unreachable.rs".to_owned()],
    )
    .expect("unreachable package source remains reportable");

    assert!(report.admits());
    assert_eq!(
        report.tests(),
        ["fixture-a::lib:fixture-a", "fixture-b::bin:fixture-b"]
    );
    assert_eq!(report.edges().len(), 2);
    assert!(
        report
            .edges()
            .iter()
            .all(|edge| edge.source() == "fixture-a/tools/unreachable.rs")
    );
    assert_eq!(report.coverage(), &crate::CoverageCompleteness::Complete);
}

#[test]
fn rust_source_outside_workspace_packages_escalates_to_the_workspace() {
    let report = selection_report(
        &fixture_inventory(),
        Path::new("/workspace"),
        wire::Digest::compute(wire::Domain::GateTree, b"unknown Rust tree"),
        &["tools/outside.rs".to_owned()],
    )
    .expect("unknown selection remains reportable");

    assert!(report.admits());
    assert_eq!(
        report.tests(),
        ["fixture-a::lib:fixture-a", "fixture-b::bin:fixture-b"]
    );
    assert_eq!(report.edges().len(), 2);
    assert!(
        report
            .edges()
            .iter()
            .all(|edge| edge.source() == "tools/outside.rs")
    );
    assert_eq!(report.coverage(), &crate::CoverageCompleteness::Complete);
    let document = String::from_utf8(report.encode().expect("encode escalation")).unwrap();
    assert!(document.contains("tools/outside.rs"));
}

#[test]
fn partially_mapped_rust_sources_retain_edges_and_escalate_unknown_reach() {
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

    assert!(report.admits());
    assert_eq!(
        report.tests(),
        ["fixture-a::lib:fixture-a", "fixture-b::bin:fixture-b"]
    );
    assert_eq!(report.edges().len(), 3);
    assert_eq!(
        report
            .edges()
            .iter()
            .filter(|edge| edge.source() == "tools/outside.rs")
            .count(),
        2,
    );
    assert_eq!(report.coverage(), &crate::CoverageCompleteness::Complete);
}
