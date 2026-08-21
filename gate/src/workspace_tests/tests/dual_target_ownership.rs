//! Exact ownership precedence inside one conventional library-and-binary package.

use super::*;

#[test]
fn binary_roots_and_modules_outrank_the_library_directory_fallback() {
    let root = fixture_root("workspace-dual-target-ownership");
    let inventory = dual_target_inventory(&root);

    for (changed, identity, arguments) in [
        (
            "dual/src/main.rs",
            "dual::bin:dual",
            [
                "test",
                "--locked",
                "--package",
                "dual",
                "--bin",
                "dual",
                "--no-run",
            ],
        ),
        (
            "dual/src/bin/tool.rs",
            "dual::bin:tool",
            [
                "test",
                "--locked",
                "--package",
                "dual",
                "--bin",
                "tool",
                "--no-run",
            ],
        ),
        (
            "dual/src/bin/tool/helper.rs",
            "dual::bin:tool",
            [
                "test",
                "--locked",
                "--package",
                "dual",
                "--bin",
                "tool",
                "--no-run",
            ],
        ),
    ] {
        let report = selection_report(
            &inventory,
            &root,
            wire::Digest::compute(wire::Domain::GateTree, changed.as_bytes()),
            &[changed.to_owned()],
        )
        .expect("an exact binary-owned source selects one test unit");
        assert_eq!(report.tests(), [identity], "changed source {changed}");

        let reach = native_reach(&inventory, &root, changed);
        let units = reach.cargo_check_units();
        let [unit] = units.as_slice() else {
            panic!("changed source {changed} must select exactly one native unit");
        };
        assert_eq!(unit.identity(), identity, "changed source {changed}");
        assert_eq!(
            unit.arguments(),
            arguments.map(str::to_owned),
            "changed source {changed}",
        );
        assert!(
            !reach.reaches_root(&root.join("consumer")),
            "binary-only change {changed} must not reach the library's dependent",
        );
    }

    for changed in ["dual/src/library.rs", "dual/src/shared.rs"] {
        let report = selection_report(
            &inventory,
            &root,
            wire::Digest::compute(wire::Domain::GateTree, changed.as_bytes()),
            &[changed.to_owned()],
        )
        .expect("library and explicitly shared modules retain downstream reach");
        assert_eq!(
            report.tests(),
            [
                "consumer::lib:consumer",
                "dual::bin:dual",
                "dual::bin:tool",
                "dual::lib:dual",
            ],
            "changed source {changed}",
        );
        assert_eq!(
            native_reach(&inventory, &root, changed)
                .cargo_check_units()
                .iter()
                .map(|unit| unit.identity())
                .collect::<Vec<_>>(),
            [
                "consumer::lib:consumer",
                "dual::bin:dual",
                "dual::bin:tool",
                "dual::lib:dual",
            ],
            "changed source {changed}",
        );
    }

    fs::remove_dir_all(root).expect("dual-target fixture removes");
}

fn native_reach(inventory: &Inventory, root: &Path, changed: &str) -> WorkspacePackageReach {
    let checks = crate::workspace_tests::selection::cargo_check_selections(
        inventory,
        root,
        &[changed.to_owned()],
    )
    .expect("dual-target native reach resolves");
    WorkspacePackageReach {
        roots: checks.iter().map(|check| check.root.clone()).collect(),
        checks,
    }
}

fn dual_target_inventory(root: &Path) -> Inventory {
    let dual = root.join("dual");
    let consumer = root.join("consumer");
    fs::create_dir_all(dual.join("src/bin/tool")).expect("dual source roots create");
    fs::create_dir_all(consumer.join("src")).expect("consumer source root creates");
    fs::write(
        dual.join("Cargo.toml"),
        "[package]\nname = \"dual\"\nversion = \"0.0.0\"\n",
    )
    .expect("dual manifest writes");
    fs::write(
        consumer.join("Cargo.toml"),
        "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\n",
    )
    .expect("consumer manifest writes");
    fs::write(
        dual.join("src/lib.rs"),
        "mod library;\n#[path = \"shared.rs\"]\nmod shared;\npub fn library() {}\n",
    )
    .expect("dual library root writes");
    fs::write(dual.join("src/library.rs"), "pub fn owned() {}\n")
        .expect("dual library module writes");
    fs::write(
        dual.join("src/main.rs"),
        "#[path = \"shared.rs\"]\nmod shared;\nfn main() {}\n",
    )
    .expect("dual main root writes");
    fs::write(dual.join("src/shared.rs"), "pub fn shared() {}\n")
        .expect("dual shared module writes");
    fs::write(
        dual.join("src/bin/tool.rs"),
        "#[path = \"tool/helper.rs\"]\nmod helper;\nfn main() { helper::run(); }\n",
    )
    .expect("dual tool root writes");
    fs::write(dual.join("src/bin/tool/helper.rs"), "pub fn run() {}\n")
        .expect("dual tool module writes");
    fs::write(consumer.join("src/lib.rs"), "pub fn consumer() {}\n")
        .expect("consumer library writes");
    let root = fs::canonicalize(root).expect("dual-target fixture canonicalizes");
    let dual = root.join("dual");
    let consumer = root.join("consumer");

    inventory_from_metadata(
        Metadata {
            workspace_members: vec!["dual-id".to_owned(), "consumer-id".to_owned()],
            target_directory: root.join("target"),
            build_directory: root.join("target/cargo-build"),
            packages: vec![
                MetadataPackage {
                    id: "dual-id".to_owned(),
                    name: "dual".to_owned(),
                    manifest_path: dual.join("Cargo.toml"),
                    dependencies: Vec::new(),
                    features: BTreeMap::new(),
                    targets: vec![
                        MetadataTarget {
                            name: "dual".to_owned(),
                            kind: vec!["lib".to_owned()],
                            test: true,
                            src_path: dual.join("src/lib.rs"),
                            required_features: Vec::new(),
                        },
                        MetadataTarget {
                            name: "dual".to_owned(),
                            kind: vec!["bin".to_owned()],
                            test: true,
                            src_path: dual.join("src/main.rs"),
                            required_features: Vec::new(),
                        },
                        MetadataTarget {
                            name: "tool".to_owned(),
                            kind: vec!["bin".to_owned()],
                            test: true,
                            src_path: dual.join("src/bin/tool.rs"),
                            required_features: Vec::new(),
                        },
                    ],
                },
                MetadataPackage {
                    id: "consumer-id".to_owned(),
                    name: "consumer".to_owned(),
                    manifest_path: consumer.join("Cargo.toml"),
                    dependencies: vec![MetadataDependency { path: Some(dual) }],
                    features: BTreeMap::new(),
                    targets: vec![MetadataTarget {
                        name: "consumer".to_owned(),
                        kind: vec!["lib".to_owned()],
                        test: true,
                        src_path: consumer.join("src/lib.rs"),
                        required_features: Vec::new(),
                    }],
                },
            ],
        },
        &root,
    )
    .expect("dual-target inventory admits")
}
