//! Conditional Rust-module reach cases for selected workspace tests.

use super::*;

#[test]
fn workspace_test_fixture_files_are_discovered_as_test_only_sources() {
    let package = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = package.parent().expect("gate package has a workspace root");
    let library_source =
        fs::canonicalize(package.join("src/lib.rs")).expect("gate library source canonicalizes");
    let sources = declared_path_sources(&package.join("src/lib.rs"))
        .expect("gate library module reach resolves");
    let gate_target = TargetIdentity {
        package: "gate-id".to_owned(),
        kind: "lib".to_owned(),
        name: "gate".to_owned(),
        required_features: Vec::new(),
        source: library_source,
        declared_path_sources: sources.clone(),
        source_bytes: 1,
    };
    let dependent_target = TargetIdentity {
        package: "dependent-id".to_owned(),
        kind: "lib".to_owned(),
        name: "dependent".to_owned(),
        required_features: Vec::new(),
        source: workspace.join("spice/src/lib.rs"),
        declared_path_sources: Vec::new(),
        source_bytes: 1,
    };
    let inventory = Inventory {
        packages: vec![
            PackageInventory {
                id: "gate-id".to_owned(),
                name: "gate".to_owned(),
                root: package.to_path_buf(),
                dependencies: Vec::new(),
                declared_targets: vec![gate_target.clone()],
                targets: vec![gate_target],
            },
            PackageInventory {
                id: "dependent-id".to_owned(),
                name: "dependent".to_owned(),
                root: workspace.join("spice"),
                dependencies: vec![package.to_path_buf()],
                declared_targets: vec![dependent_target.clone()],
                targets: vec![dependent_target],
            },
        ],
        binary_outputs: Vec::new(),
        target_directory: workspace.join("target"),
        build_directory: workspace.join("target/cargo-build"),
    };

    for relative in [
        "src/workspace_tests/tests.rs",
        "src/workspace_tests/tests/execution.rs",
        "src/workspace_tests/tests/reporting.rs",
    ] {
        let expected = fs::canonicalize(package.join(relative))
            .expect("workspace test fixture source canonicalizes");
        let source = sources
            .iter()
            .find(|source| source.path == expected)
            .unwrap_or_else(|| panic!("gate library did not declare {relative}"));
        assert!(source.test_only, "{relative} must remain test-only");

        let changed = format!("gate/{relative}");
        let report = selection_report(
            &inventory,
            workspace,
            wire::Digest::compute(wire::Domain::GateTree, changed.as_bytes()),
            std::slice::from_ref(&changed),
        )
        .unwrap_or_else(|error| panic!("select {changed}: {error}"));
        assert_eq!(
            report.tests(),
            ["gate::lib:gate"],
            "{changed} must not reach the dependent package"
        );
    }
}

#[test]
fn exact_cfg_test_subtree_selects_only_its_owning_test_unit() {
    let (root, inventory) = cfg_test_reach_fixture("exact");
    let changes = [
        "fixture-a/src/tests.rs".to_owned(),
        "fixture-a/src/tests/nested.rs".to_owned(),
    ];

    let report = selection_report(
        &inventory,
        &root,
        wire::Digest::compute(wire::Domain::GateTree, b"exact cfg test reach"),
        &changes,
    )
    .expect("exact cfg(test) subtree selects its owning test unit");

    assert_eq!(report.tests(), ["fixture-a::lib:fixture-a"]);
    assert_eq!(report.edges().len(), 2);
    assert!(
        report
            .edges()
            .iter()
            .all(|edge| edge.test() == "fixture-a::lib:fixture-a")
    );

    let checks =
        crate::workspace_tests::selection::cargo_check_selections(&inventory, &root, &changes)
            .expect("exact cfg(test) subtree selects its owning native check");
    assert_eq!(
        checks
            .iter()
            .map(|check| check.name.as_str())
            .collect::<Vec<_>>(),
        ["fixture-a"]
    );
    let reach = WorkspacePackageReach {
        roots: checks.iter().map(|check| check.root.clone()).collect(),
        checks,
    };
    assert_eq!(
        reach.cargo_check_arguments(),
        [[
            "test",
            "--locked",
            "--package",
            "fixture-a",
            "--lib",
            "--no-run",
        ]
        .map(str::to_owned)
        .to_vec()]
    );
    fs::remove_dir_all(root).expect("remove exact cfg(test) fixture");
}

#[test]
fn production_mixed_ambiguous_and_removed_sources_retain_reverse_dependents() {
    let (root, inventory) = cfg_test_reach_fixture("conservative");
    let changes = [
        "fixture-a/src/production.rs".to_owned(),
        "fixture-a/src/shared.rs".to_owned(),
        "fixture-a/src/ambiguous.rs".to_owned(),
    ];

    let report = selection_report(
        &inventory,
        &root,
        wire::Digest::compute(wire::Domain::GateTree, b"conservative cfg reach"),
        &changes,
    )
    .expect("production and ambiguous reach retains reverse dependents");
    assert_eq!(
        report.tests(),
        ["fixture-a::lib:fixture-a", "fixture-b::lib:fixture-b"]
    );
    assert_eq!(report.edges().len(), 6);

    fs::remove_file(root.join("fixture-a/src/tests/nested.rs"))
        .expect("remove formerly test-only source");
    let removed = selection_report(
        &inventory,
        &root,
        wire::Digest::compute(wire::Domain::GateTree, b"removed cfg test reach"),
        &["fixture-a/src/tests/nested.rs".to_owned()],
    )
    .expect("removed test-only source retains conservative reverse reach");
    assert_eq!(
        removed.tests(),
        ["fixture-a::lib:fixture-a", "fixture-b::lib:fixture-b"]
    );
    fs::remove_dir_all(root).expect("remove conservative cfg fixture");
}

fn cfg_test_reach_fixture(label: &str) -> (PathBuf, Inventory) {
    let root = fixture_root(&format!("workspace-cfg-test-reach-{label}"));
    for package in ["fixture-a", "fixture-b"] {
        fs::create_dir_all(root.join(package).join("src"))
            .expect("create cfg reach package source directory");
        fs::write(
            root.join(package).join("Cargo.toml"),
            format!("[package]\nname = \"{package}\"\nversion = \"0.0.0\"\n"),
        )
        .expect("write cfg reach package manifest");
    }
    fs::create_dir_all(root.join("fixture-a/src/tests"))
        .expect("create nested cfg(test) module directory");
    fs::write(
        root.join("fixture-a/src/lib.rs"),
        concat!(
            "#[cfg(test)]\nmod tests;\n",
            "mod production;\n",
            "#[cfg(test)]\n#[path = \"shared.rs\"]\nmod test_shared;\n",
            "#[path = \"shared.rs\"]\nmod production_shared;\n",
            "#[cfg(any(test, feature = \"dev\"))]\nmod ambiguous;\n",
        ),
    )
    .expect("write cfg reach library root");
    fs::write(root.join("fixture-a/src/tests.rs"), "mod nested;\n")
        .expect("write cfg(test) module root");
    fs::write(
        root.join("fixture-a/src/tests/nested.rs"),
        "#[test]\nfn nested() {}\n",
    )
    .expect("write nested cfg(test) module");
    for source in ["production.rs", "shared.rs", "ambiguous.rs"] {
        fs::write(
            root.join("fixture-a/src").join(source),
            "pub fn reachable() {}\n",
        )
        .expect("write conservative module source");
    }
    fs::write(root.join("fixture-b/src/lib.rs"), "pub fn dependent() {}\n")
        .expect("write dependent library source");
    let root = fs::canonicalize(root).expect("canonical cfg reach fixture root");
    let inventory = inventory_from_metadata(
        Metadata {
            workspace_members: vec!["fixture-a-id".to_owned(), "fixture-b-id".to_owned()],
            target_directory: root.join("target"),
            build_directory: root.join("target/cargo-build"),
            packages: vec![
                cfg_test_package(&root, "fixture-a", Vec::new()),
                cfg_test_package(
                    &root,
                    "fixture-b",
                    vec![MetadataDependency {
                        path: Some(root.join("fixture-a")),
                    }],
                ),
            ],
        },
        &root,
    )
    .expect("acquire cfg reach inventory");
    (root, inventory)
}

fn cfg_test_package(
    root: &Path,
    name: &str,
    dependencies: Vec<MetadataDependency>,
) -> MetadataPackage {
    MetadataPackage {
        id: format!("{name}-id"),
        name: name.to_owned(),
        manifest_path: root.join(name).join("Cargo.toml"),
        dependencies,
        targets: vec![target(&root.join(name), name, &["lib"], true)],
        features: BTreeMap::new(),
    }
}
