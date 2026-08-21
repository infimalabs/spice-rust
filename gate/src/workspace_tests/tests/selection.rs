//! Staged-source selection cases for workspace Rust test targets.

use super::*;

#[test]
fn staged_rust_source_selects_its_direct_cargo_test_target_with_exact_edge() {
    let inventory = fixture_inventory();
    let source = wire::Digest::compute(wire::Domain::GateTree, b"selected Rust tree");

    let plan = selection_plan(
        &inventory,
        Path::new("/workspace"),
        source.clone(),
        &["fixture-a/src/behavior.rs".to_owned()],
    )
    .expect("workspace reach graph selects tests");
    let report = plan.report();

    assert!(report.admits());
    assert!(plan.escalations().is_empty());
    assert_eq!(report.source(), &source);
    assert_eq!(report.changed_sources(), ["fixture-a/src/behavior.rs"]);
    assert_eq!(report.engines(), ["cargo-workspace"]);
    assert_eq!(report.tests(), ["fixture-a::lib:fixture-a"]);
    assert_eq!(report.edges().len(), 1);
    assert!(
        report
            .edges()
            .iter()
            .all(|edge| edge.source() == "fixture-a/src/behavior.rs")
    );
    assert_eq!(
        &crate::TestSelectionReport::decode(&report.encode().expect("encode selection"))
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
                dependencies: Vec::new(),
                features: BTreeMap::new(),
                targets: vec![
                    MetadataTarget {
                        name: "other".to_owned(),
                        kind: vec!["test".to_owned()],
                        test: true,
                        src_path: tests.join("other.rs"),
                        required_features: Vec::new(),
                    },
                    MetadataTarget {
                        name: "resident".to_owned(),
                        kind: vec!["test".to_owned()],
                        test: true,
                        src_path: tests.join("resident.rs"),
                        required_features: Vec::new(),
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
        required_features: Vec::new(),
        source: PathBuf::from("/workspace/fixture-a/tests/behavior.rs"),
        declared_path_sources: Vec::new(),
        source_bytes: 1,
    });

    let plan = selection_plan(
        &inventory,
        Path::new("/workspace"),
        wire::Digest::compute(wire::Domain::GateTree, b"library-dependent tests"),
        &["fixture-a/src/behavior.rs".to_owned()],
    )
    .expect("library reach expands through its package test targets");
    let report = plan.report();

    assert!(report.admits());
    assert!(plan.escalations().is_empty());
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
fn staged_library_source_does_not_run_reverse_dependent_test_binaries() {
    let mut inventory = fixture_inventory();
    inventory.packages[1]
        .dependencies
        .push(PathBuf::from("/workspace/fixture-a"));
    inventory.packages.push(PackageInventory {
        id: "fixture-c-id".to_owned(),
        name: "fixture-c".to_owned(),
        root: PathBuf::from("/workspace/fixture-c"),
        dependencies: vec![PathBuf::from("/workspace/fixture-b")],
        declared_targets: vec![TargetIdentity {
            package: "fixture-c-id".to_owned(),
            kind: "test".to_owned(),
            name: "behavior".to_owned(),
            required_features: Vec::new(),
            source: PathBuf::from("/workspace/fixture-c/tests/behavior.rs"),
            declared_path_sources: Vec::new(),
            source_bytes: 1,
        }],
        targets: vec![TargetIdentity {
            package: "fixture-c-id".to_owned(),
            kind: "test".to_owned(),
            name: "behavior".to_owned(),
            required_features: Vec::new(),
            source: PathBuf::from("/workspace/fixture-c/tests/behavior.rs"),
            declared_path_sources: Vec::new(),
            source_bytes: 1,
        }],
    });

    let report = selection_report(
        &inventory,
        Path::new("/workspace"),
        wire::Digest::compute(wire::Domain::GateTree, b"reverse dependency reach"),
        &["fixture-a/src/behavior.rs".to_owned()],
    )
    .expect("library reach stays inside its owning package tests");

    assert_eq!(report.tests(), ["fixture-a::lib:fixture-a"]);
    assert_eq!(report.edges().len(), 1);
    assert!(
        report
            .edges()
            .iter()
            .all(|edge| edge.source() == "fixture-a/src/behavior.rs")
    );
}

#[test]
fn non_test_declared_target_still_owns_native_check_reach() {
    let mut inventory = fixture_inventory();
    inventory.packages[0].targets.clear();

    let reached = crate::workspace_tests::selection::cargo_check_selections(
        &inventory,
        Path::new("/workspace"),
        &["fixture-a/src/behavior.rs".to_owned()],
    )
    .expect("declared target check reach resolves");

    assert_eq!(
        reached
            .iter()
            .map(|check| check.name.as_str())
            .collect::<Vec<_>>(),
        ["fixture-a"],
        "a declared target remains an exact check owner even when Cargo disables its test harness"
    );
}

#[test]
fn non_test_binary_target_escalates_test_reach_instead_of_dropping_the_source() {
    let mut inventory = fixture_inventory();
    inventory.packages[1].targets.clear();

    let plan = selection_plan(
        &inventory,
        Path::new("/workspace"),
        wire::Digest::compute(wire::Domain::GateTree, b"non-test binary reach"),
        &["fixture-b/src/main.rs".to_owned()],
    )
    .expect("a source without a test-enabled owner escalates test reach");

    assert_eq!(plan.escalations().len(), 1);
    assert_eq!(plan.escalations()[0].source(), "fixture-b/src/main.rs");
    assert_eq!(
        plan.escalations()[0].reason(),
        &CargoReachEscalationReason::UnreachedInPackage {
            package: "fixture-b".to_owned(),
        }
    );
    assert_eq!(
        plan.report().tests(),
        ["fixture-a::lib:fixture-a"],
        "exhaustive fallback retains every available workspace test target"
    );
}

#[test]
fn custom_build_target_reaches_workspace_dependents() {
    let mut inventory = fixture_inventory();
    inventory.packages[0].declared_targets.push(TargetIdentity {
        package: "fixture-a-id".to_owned(),
        kind: "custom-build".to_owned(),
        name: "build-script-build".to_owned(),
        required_features: Vec::new(),
        source: PathBuf::from("/workspace/fixture-a/tools/generate.rs"),
        declared_path_sources: Vec::new(),
        source_bytes: 1,
    });
    inventory.packages[1]
        .dependencies
        .push(PathBuf::from("/workspace/fixture-a"));

    let reached = crate::workspace_tests::selection::cargo_check_selections(
        &inventory,
        Path::new("/workspace"),
        &["fixture-a/tools/generate.rs".to_owned()],
    )
    .expect("custom build check reach resolves");

    assert_eq!(
        reached
            .iter()
            .map(|check| check.name.as_str())
            .collect::<Vec<_>>(),
        ["fixture-a", "fixture-b"]
    );

    let report = selection_report(
        &inventory,
        Path::new("/workspace"),
        wire::Digest::compute(wire::Domain::GateTree, b"custom build reach"),
        &["fixture-a/tools/generate.rs".to_owned()],
    )
    .expect("custom build test reach stays inside its owning package");
    assert_eq!(report.tests(), ["fixture-a::lib:fixture-a"]);
}

fn configuration_target(package: &str, kind: &str, name: &str, source: &str) -> TargetIdentity {
    TargetIdentity {
        package: format!("{package}-id"),
        kind: kind.to_owned(),
        name: name.to_owned(),
        required_features: Vec::new(),
        source: PathBuf::from(source),
        declared_path_sources: Vec::new(),
        source_bytes: 1,
    }
}

fn configuration_slice_inventory() -> Inventory {
    let vendor_lib =
        configuration_target("vendor", "lib", "vendor", "/workspace/vendor/src/lib.rs");
    let vendor_contracts = configuration_target(
        "vendor",
        "test",
        "contracts",
        "/workspace/vendor/tests/contracts.rs",
    );
    let lane_lib = configuration_target("lane", "lib", "lane", "/workspace/lane/src/lib.rs");
    let spice_lib = configuration_target("spice", "lib", "spice", "/workspace/spice/src/lib.rs");
    let policy_config = configuration_target(
        "spice",
        "test",
        "policy_config",
        "/workspace/spice/tests/policy_config.rs",
    );
    let demo_cli = configuration_target(
        "spice",
        "test",
        "demo_cli",
        "/workspace/spice/tests/demo_cli.rs",
    );
    let xtask_lib = configuration_target("xtask", "lib", "xtask", "/workspace/xtask/src/lib.rs");
    Inventory {
        packages: vec![
            PackageInventory {
                id: "vendor-id".to_owned(),
                name: "vendor".to_owned(),
                root: PathBuf::from("/workspace/vendor"),
                dependencies: Vec::new(),
                declared_targets: vec![vendor_lib.clone(), vendor_contracts.clone()],
                targets: vec![vendor_lib, vendor_contracts],
            },
            PackageInventory {
                id: "lane-id".to_owned(),
                name: "lane".to_owned(),
                root: PathBuf::from("/workspace/lane"),
                dependencies: vec![PathBuf::from("/workspace/vendor")],
                declared_targets: vec![lane_lib.clone()],
                targets: vec![lane_lib],
            },
            PackageInventory {
                id: "spice-id".to_owned(),
                name: "spice".to_owned(),
                root: PathBuf::from("/workspace/spice"),
                dependencies: vec![
                    PathBuf::from("/workspace/lane"),
                    PathBuf::from("/workspace/vendor"),
                ],
                declared_targets: vec![spice_lib.clone(), policy_config.clone(), demo_cli.clone()],
                targets: vec![spice_lib, policy_config, demo_cli],
            },
            PackageInventory {
                id: "xtask-id".to_owned(),
                name: "xtask".to_owned(),
                root: PathBuf::from("/workspace/xtask"),
                dependencies: vec![PathBuf::from("/workspace/spice")],
                declared_targets: vec![xtask_lib.clone()],
                targets: vec![xtask_lib],
            },
        ],
        binary_outputs: Vec::new(),
        target_directory: PathBuf::from("/workspace/target"),
        build_directory: PathBuf::from("/workspace/target/build"),
    }
}

#[test]
fn configuration_slice_checks_reverse_dependents_without_running_their_test_binaries() {
    let inventory = configuration_slice_inventory();
    let changes = [
        "vendor/src/config.rs".to_owned(),
        "spice/tests/policy_config.rs".to_owned(),
    ];

    let plan = selection_plan(
        &inventory,
        Path::new("/workspace"),
        wire::Digest::compute(wire::Domain::GateTree, b"configuration slice"),
        &changes,
    )
    .expect("configuration slice selects owning tests");
    assert_eq!(
        plan.report().tests(),
        [
            "spice::test:policy_config",
            "vendor::lib:vendor",
            "vendor::test:contracts",
        ]
    );
    assert!(
        plan.report()
            .tests()
            .iter()
            .all(|test| !test.contains("demo_cli") && !test.starts_with("xtask::"))
    );

    let checks = crate::workspace_tests::selection::cargo_check_selections(
        &inventory,
        Path::new("/workspace"),
        &changes,
    )
    .expect("configuration slice resolves native checks");
    let reach = WorkspacePackageReach {
        roots: checks.iter().map(|check| check.root.clone()).collect(),
        checks,
    };
    assert_eq!(
        reach.cargo_check_arguments(),
        [
            ["check", "--locked", "--all-targets", "--package", "vendor",]
                .map(str::to_owned)
                .to_vec(),
            ["check", "--locked", "--package", "lane", "--lib"]
                .map(str::to_owned)
                .to_vec(),
            [
                "check",
                "--locked",
                "--package",
                "spice",
                "--lib",
                "--test",
                "policy_config",
            ]
            .map(str::to_owned)
            .to_vec(),
            ["check", "--locked", "--package", "xtask", "--lib"]
                .map(str::to_owned)
                .to_vec(),
        ]
    );
}

#[test]
fn metadata_inventory_refuses_an_unowned_local_dependency_path() {
    let root = fixture_root("workspace-unowned-local-dependency");
    fs::create_dir_all(root.join("member/src")).expect("dependency fixture source root creates");
    fs::write(
        root.join("member/Cargo.toml"),
        "[package]\nname = \"member\"\nversion = \"0.0.0\"\n",
    )
    .expect("dependency fixture manifest writes");
    fs::write(root.join("member/src/lib.rs"), "pub fn member() {}\n")
        .expect("dependency fixture source writes");
    let root = fs::canonicalize(root).expect("dependency fixture root canonicalizes");
    let missing = root.join("missing-member");

    let error = inventory_from_metadata(
        Metadata {
            workspace_members: vec!["member-id".to_owned()],
            target_directory: root.join("target"),
            build_directory: root.join("target/cargo-build"),
            packages: vec![MetadataPackage {
                id: "member-id".to_owned(),
                name: "member".to_owned(),
                manifest_path: root.join("member/Cargo.toml"),
                dependencies: vec![MetadataDependency {
                    path: Some(missing.clone()),
                }],
                features: BTreeMap::new(),
                targets: vec![target(&root.join("member"), "member", &["lib"], true)],
            }],
        },
        &root,
    )
    .expect_err("an in-workspace path dependency without a workspace package must refuse")
    .to_string();

    assert!(error.contains("unowned local path"), "{error}");
    assert!(error.contains(&missing.display().to_string()), "{error}");
    fs::remove_dir_all(root).expect("dependency fixture removes");
}

#[test]
fn old_and_new_cross_package_paths_select_both_package_owners() {
    let report = selection_report(
        &fixture_inventory(),
        Path::new("/workspace"),
        wire::Digest::compute(wire::Domain::GateTree, b"cross-package rename"),
        &[
            "fixture-a/src/behavior.rs".to_owned(),
            "fixture-b/src/main.rs".to_owned(),
        ],
    )
    .expect("both rename owners retain canonical test reach");

    assert!(report.admits());
    assert_eq!(
        report.tests(),
        ["fixture-a::lib:fixture-a", "fixture-b::bin:fixture-b"]
    );
    assert_eq!(report.edges().len(), 2);
    for path in report.changed_sources() {
        assert!(report.edges().iter().any(|edge| edge.source() == path));
    }
}

#[test]
fn package_manifest_and_build_script_select_every_test_target_in_their_package() {
    let mut inventory = fixture_inventory();
    inventory.packages[0].targets.push(TargetIdentity {
        package: "fixture-a-id".to_owned(),
        kind: "test".to_owned(),
        name: "behavior".to_owned(),
        required_features: Vec::new(),
        source: PathBuf::from("/workspace/fixture-a/tests/behavior.rs"),
        declared_path_sources: Vec::new(),
        source_bytes: 1,
    });

    for source in ["fixture-a/Cargo.toml", "fixture-a/build.rs"] {
        let plan = selection_plan(
            &inventory,
            Path::new("/workspace"),
            wire::Digest::compute(wire::Domain::GateTree, source.as_bytes()),
            &[source.to_owned()],
        )
        .expect("package-wide build source selects tests");
        let report = plan.report();

        assert!(plan.escalations().is_empty());
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
        let plan = selection_plan(
            &inventory,
            Path::new("/workspace"),
            wire::Digest::compute(wire::Domain::GateTree, source.as_bytes()),
            &[source.to_owned()],
        )
        .expect("workspace authority selects every test target");
        let report = plan.report();

        assert!(plan.escalations().is_empty());
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
fn rust_source_inside_a_package_but_outside_target_roots_stays_with_its_metadata_owner() {
    let plan = selection_plan(
        &fixture_inventory(),
        Path::new("/workspace"),
        wire::Digest::compute(wire::Domain::GateTree, b"unreachable package source"),
        &["fixture-a/tools/unreachable.rs".to_owned()],
    )
    .expect("unreachable package source remains reportable");
    let report = plan.report();

    assert!(report.admits());
    assert!(plan.escalations().is_empty());
    assert_eq!(report.tests(), ["fixture-a::lib:fixture-a"]);
    assert_eq!(report.edges().len(), 1);
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
    let plan = selection_plan(
        &fixture_inventory(),
        Path::new("/workspace"),
        wire::Digest::compute(wire::Domain::GateTree, b"unknown Rust tree"),
        &["tools/outside.rs".to_owned()],
    )
    .expect("unknown selection remains reportable");
    let report = plan.report();

    assert!(report.admits());
    assert_eq!(plan.escalations().len(), 1);
    assert_eq!(plan.escalations()[0].source(), "tools/outside.rs");
    assert_eq!(
        plan.escalations()[0].reason(),
        &CargoReachEscalationReason::OutsidePackage
    );
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
    let plan = selection_plan(
        &fixture_inventory(),
        Path::new("/workspace"),
        wire::Digest::compute(wire::Domain::GateTree, b"incomplete Rust tree"),
        &[
            "tools/outside.rs".to_owned(),
            "fixture-a/src/behavior.rs".to_owned(),
            "fixture-a/tools/unreachable.rs".to_owned(),
            "tools/outside.rs".to_owned(),
        ],
    )
    .expect("incomplete selection remains reportable");
    let report = plan.report();

    assert!(report.admits());
    assert_eq!(
        report.changed_sources(),
        [
            "fixture-a/src/behavior.rs",
            "fixture-a/tools/unreachable.rs",
            "tools/outside.rs",
        ],
        "selected sources are canonical, sorted, and deduplicated"
    );
    assert_eq!(
        plan.escalations()
            .iter()
            .map(|escalation| (
                escalation.source(),
                escalation.reason().kind(),
                escalation.reason().package(),
            ))
            .collect::<Vec<_>>(),
        [("tools/outside.rs", "outside-package", None)],
        "escalations retain typed reasons in stable source order without duplicating inputs"
    );
    let mut diagnostics = Vec::new();
    write_reach_escalations(&mut diagnostics, plan.escalations())
        .expect("render mixed reach escalations");
    assert_eq!(
        String::from_utf8(diagnostics)
            .expect("reach escalation diagnostics are UTF-8")
            .lines()
            .collect::<Vec<_>>(),
        [concat!(
            "[rust-test-reach-escalation] source=\"tools/outside.rs\" ",
            "reason=outside-package"
        )]
    );
    assert_eq!(
        report.tests(),
        ["fixture-a::lib:fixture-a", "fixture-b::bin:fixture-b"]
    );
    assert_eq!(report.edges().len(), 4);
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

#[test]
fn complete_zero_engine_evidence_round_trips_mixed_changed_sources() {
    let source = wire::Digest::compute(wire::Domain::GateTree, b"generic selected tree");
    let report = crate::TestSelectionReport::complete_without_engines(
        source.clone(),
        vec!["README.md".to_owned(), "assets/icon.svg".to_owned()],
    )
    .expect("zero-engine evidence is complete by declaration");

    assert!(report.admits());
    assert_eq!(report.source(), &source);
    assert_eq!(report.changed_sources(), ["README.md", "assets/icon.svg"]);
    assert!(report.engines().is_empty());
    assert!(report.tests().is_empty());
    assert!(report.edges().is_empty());
    assert_eq!(
        crate::TestSelectionReport::decode(&report.encode().expect("encode zero-engine report"))
            .expect("decode zero-engine report"),
        report
    );
}

#[test]
fn selection_report_refuses_an_engine_outside_the_compiled_vocabulary() {
    let report = crate::TestSelectionReport::complete_without_engines(
        wire::Digest::compute(wire::Domain::GateTree, b"unknown engine"),
        vec!["README.md".to_owned()],
    )
    .expect("seed report");
    let mut document = serde_json::to_value(report).expect("encode report value");
    document["engines"] = serde_json::json!(["repository-authored-command"]);

    let error = crate::TestSelectionReport::decode(
        &serde_json::to_vec(&document).expect("encode mutated report"),
    )
    .expect_err("unknown engine identity must refuse");
    assert!(
        error
            .detail()
            .contains("unsupported selected test engine \"repository-authored-command\""),
        "{error}"
    );
}

#[test]
fn cargo_applicability_requires_both_a_workspace_and_relevant_scope() {
    let root = fixture_root("cargo-engine-applicability");
    fs::create_dir_all(&root).expect("create applicability fixture");

    assert!(!cargo_workspace_tests_applicable(
        &root,
        &["src/lib.rs".to_owned()],
    ));

    fs::write(root.join("Cargo.toml"), "[workspace]\nresolver = \"2\"\n")
        .expect("write workspace authority");
    assert!(!cargo_workspace_tests_applicable(
        &root,
        &["README.md".to_owned()],
    ));
    assert!(cargo_workspace_tests_applicable(
        &root,
        &["README.md".to_owned(), "src/lib.rs".to_owned()],
    ));
    assert!(!cargo_workspace_tests_applicable(
        &root,
        &["assets/opaque.bin".to_owned()],
    ));

    fs::remove_dir_all(root).expect("remove applicability fixture");
}
