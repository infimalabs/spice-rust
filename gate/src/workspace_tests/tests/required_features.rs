//! Required-feature authority from Cargo metadata through real selected execution.

use super::super::selection::cargo_check_selections;
use super::*;

#[test]
fn metadata_refuses_a_target_with_an_undeclared_required_feature() {
    let root = fixture_root("workspace-undeclared-required-feature");
    fs::create_dir_all(root.join("tests")).expect("create undeclared-feature fixture root");
    fs::write(root.join("Cargo.toml"), "[workspace]\n")
        .expect("write undeclared-feature fixture manifest");
    fs::write(root.join("tests/guarded.rs"), "#[test]\nfn guarded() {}\n")
        .expect("write undeclared-feature fixture target");
    let root = fs::canonicalize(root).expect("canonicalize undeclared-feature fixture root");

    let error = package_inventory(
        MetadataPackage {
            id: "guarded-fixture-id".to_owned(),
            name: "guarded-fixture".to_owned(),
            manifest_path: root.join("Cargo.toml"),
            dependencies: Vec::new(),
            features: BTreeMap::new(),
            targets: vec![MetadataTarget {
                name: "guarded".to_owned(),
                kind: vec!["test".to_owned()],
                test: true,
                src_path: root.join("tests/guarded.rs"),
                required_features: vec!["guarded".to_owned()],
            }],
        },
        &root,
    )
    .expect_err("undeclared required feature must refuse inventory")
    .to_string();

    assert!(
        error.contains("requires undeclared feature \"guarded\""),
        "{error}"
    );
    fs::remove_dir_all(root).expect("remove undeclared-feature fixture");
}

#[test]
fn exact_and_exhaustive_check_reach_qualify_only_required_package_features() {
    let inventory = guarded_inventory();

    let exact = WorkspacePackageReach {
        roots: vec![PathBuf::from("/workspace/fixture-a")],
        checks: cargo_check_selections(
            &inventory,
            Path::new("/workspace"),
            &["fixture-a/tests/guarded.rs".to_owned()],
        )
        .expect("guarded target check reach resolves"),
    };
    assert_eq!(
        exact.cargo_check_arguments(),
        vec![
            vec![
                "test",
                "--locked",
                "--package",
                "fixture-a",
                "--test",
                "guarded",
                "--features",
                "fixture-a/guarded",
                "--no-run",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
        ]
    );

    let exhaustive = WorkspacePackageReach {
        roots: inventory
            .packages
            .iter()
            .map(|package| package.root.clone())
            .collect(),
        checks: cargo_check_selections(
            &inventory,
            Path::new("/workspace"),
            &["Cargo.toml".to_owned()],
        )
        .expect("workspace authority check reach resolves"),
    };
    assert_eq!(
        exhaustive.cargo_check_arguments(),
        [
            [
                "test",
                "--locked",
                "--package",
                "fixture-a",
                "--lib",
                "--no-run",
            ]
            .map(str::to_owned)
            .to_vec(),
            [
                "test",
                "--locked",
                "--package",
                "fixture-a",
                "--test",
                "guarded",
                "--features",
                "fixture-a/guarded",
                "--no-run",
            ]
            .map(str::to_owned)
            .to_vec(),
            [
                "test",
                "--locked",
                "--package",
                "fixture-a",
                "--test",
                "unrelated",
                "--features",
                "fixture-a/unrelated",
                "--no-run",
            ]
            .map(str::to_owned)
            .to_vec(),
            [
                "test",
                "--locked",
                "--package",
                "fixture-b",
                "--bin",
                "fixture-b",
                "--no-run",
            ]
            .map(str::to_owned)
            .to_vec(),
        ]
    );
}

#[test]
fn workspace_prebuild_enables_the_complete_test_inventory_feature_closure() {
    let root = fixture_root("workspace-required-feature-prebuild");
    let programs = root.join("programs");
    fs::create_dir_all(&programs).expect("create required-feature prebuild programs");
    let cargo = executable(
        &programs,
        "cargo",
        "#!/bin/sh\nprintf '%s\\n' \"$*\" > \"$FIXTURE_ROOT/commands.log\"\n",
    );
    let environment = binary_output_environment(&root, &programs);
    let mut inventory = guarded_inventory();
    inventory.binary_outputs.clear();
    let mut output = Vec::new();

    prebuild_all_tests(
        &root,
        &environment,
        cargo.as_os_str(),
        &inventory,
        &mut output,
    )
    .expect("workspace prebuild enables every selected test target requirement");

    assert_eq!(
        fs::read_to_string(root.join("commands.log")).expect("read prebuild Cargo arguments"),
        concat!(
            "test --locked --workspace --all-targets --features ",
            "fixture-a/guarded,fixture-a/unrelated --no-run\n",
        ),
    );
    fs::remove_dir_all(root).expect("remove required-feature prebuild fixture");
}

#[test]
fn selected_test_executes_a_required_feature_without_reaching_its_guarded_peer() {
    let root = fixture_root("workspace-required-feature-test");
    fs::create_dir_all(root.join("src")).expect("create guarded fixture library root");
    fs::create_dir_all(root.join("tests")).expect("create guarded fixture test root");
    fs::write(
        root.join("Cargo.toml"),
        concat!(
            "[package]\nname = \"guarded-fixture\"\nversion = \"0.0.0\"\n",
            "edition = \"2024\"\n\n",
            "[features]\nguarded = []\nunrelated = []\n\n",
            "[[test]]\nname = \"guarded\"\npath = \"tests/guarded.rs\"\n",
            "required-features = [\"guarded\"]\n\n",
            "[[test]]\nname = \"unrelated\"\npath = \"tests/unrelated.rs\"\n",
            "required-features = [\"unrelated\"]\n",
        ),
    )
    .expect("write guarded fixture manifest");
    fs::write(
        root.join("Cargo.lock"),
        concat!(
            "version = 4\n\n",
            "[[package]]\nname = \"guarded-fixture\"\nversion = \"0.0.0\"\n",
        ),
    )
    .expect("write guarded fixture lock");
    fs::write(root.join("src/lib.rs"), "pub fn fixture() {}\n")
        .expect("write guarded fixture library");
    fs::write(
        root.join("tests/guarded.rs"),
        concat!(
            "#[cfg(feature = \"guarded\")]\n",
            "#[test]\n",
            "fn guarded_feature_failure_is_observed() {\n",
            "    panic!(\"guarded feature reached real test execution\");\n",
            "}\n",
        ),
    )
    .expect("write guarded fixture target");
    fs::write(
        root.join("tests/unrelated.rs"),
        "compile_error!(\"unrelated guarded target compiled\");\n",
    )
    .expect("write unrelated guarded target");
    let environment = nested_cargo_environment();
    let mut output = Vec::new();

    let error = run_selected_at(
        WorkspaceTestRoots::new(&root, &root),
        wire::Digest::compute(wire::Domain::GateTree, b"required feature fixture"),
        &["tests/guarded.rs".to_owned()],
        environment,
        1,
        &mut output,
    )
    .expect_err("the planted guarded test failure must be observed")
    .to_string();

    assert!(
        error.contains("guarded_feature_failure_is_observed"),
        "{error}"
    );
    assert!(
        error.contains("guarded feature reached real test execution"),
        "{error}"
    );
    assert!(
        !error.contains("unrelated guarded target compiled"),
        "{error}"
    );
    fs::remove_dir_all(root).expect("remove required-feature fixture");
}

fn guarded_inventory() -> Inventory {
    let mut inventory = fixture_inventory();
    for (name, feature) in [("guarded", "guarded"), ("unrelated", "unrelated")] {
        let target = TargetIdentity {
            package: "fixture-a-id".to_owned(),
            kind: "test".to_owned(),
            name: name.to_owned(),
            required_features: vec![feature.to_owned()],
            source: PathBuf::from(format!("/workspace/fixture-a/tests/{name}.rs")),
            declared_path_sources: Vec::new(),
            source_bytes: 1,
        };
        inventory.packages[0].declared_targets.push(target.clone());
        inventory.packages[0].targets.push(target);
    }
    inventory
}

#[expect(
    clippy::disallowed_methods,
    reason = "the real nested Cargo fixture needs the current compiler environment"
)]
fn nested_cargo_environment() -> Environment {
    let entries = std::env::vars_os()
        .filter(|(name, _)| {
            name != OsStr::new(CARGO_TARGET_DIR) && name != OsStr::new(CARGO_BUILD_BUILD_DIR)
        })
        .collect::<BTreeMap<_, _>>();
    Environment::new(entries).expect("capture nested Cargo environment")
}
