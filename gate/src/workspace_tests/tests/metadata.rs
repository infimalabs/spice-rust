use super::*;

#[test]
fn metadata_inventory_keeps_library_binary_and_integration_targets() {
    let root = fixture_root("workspace-metadata-inventory");
    fs::create_dir_all(root.join("src")).expect("metadata source root creates");
    fs::create_dir_all(root.join("tests")).expect("metadata test root creates");
    fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("metadata manifest fixture writes");
    fs::write(root.join("src/lib.rs"), "pub fn fixture() {}\n")
        .expect("metadata library source writes");
    fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("metadata binary source writes");
    fs::write(root.join("src/default-runner.rs"), "fn main() {}\n")
        .expect("metadata default-feature binary source writes");
    fs::write(root.join("src/disabled-runner.rs"), "fn main() {}\n")
        .expect("metadata disabled-feature binary source writes");
    fs::write(root.join("build.rs"), "fn main() {}\n").expect("metadata build script writes");
    fs::write(
        root.join("tests/resident.rs"),
        "#[test]\nfn resident() {}\n",
    )
    .expect("metadata integration source writes");
    let root = fs::canonicalize(root).expect("metadata fixture root canonicalizes");
    let inventory = inventory_from_metadata(
        Metadata {
            workspace_members: vec!["fixture-id".to_owned()],
            target_directory: root.join("target"),
            build_directory: root.join("target/cargo-build"),
            packages: vec![MetadataPackage {
                id: "fixture-id".to_owned(),
                name: "fixture".to_owned(),
                manifest_path: root.join("Cargo.toml"),
                dependencies: Vec::new(),
                features: BTreeMap::from([
                    ("default".to_owned(), vec!["runner".to_owned()]),
                    ("runner".to_owned(), Vec::new()),
                    ("disabled".to_owned(), Vec::new()),
                ]),
                targets: vec![
                    target(&root, "fixture", &["lib"], true),
                    target(&root, "fixture", &["bin"], true),
                    target(&root, "resident", &["test"], true),
                    target(&root, "build-script-build", &["custom-build"], false),
                    MetadataTarget {
                        name: "default-runner".to_owned(),
                        kind: vec!["bin".to_owned()],
                        test: false,
                        src_path: root.join("src/default-runner.rs"),
                        required_features: vec!["runner".to_owned()],
                    },
                    MetadataTarget {
                        name: "disabled-runner".to_owned(),
                        kind: vec!["bin".to_owned()],
                        test: false,
                        src_path: root.join("src/disabled-runner.rs"),
                        required_features: vec!["disabled".to_owned()],
                    },
                ],
            }],
        },
        &root,
    )
    .expect("fixture metadata admits");

    let package = &inventory.packages[0];
    assert_eq!(package.targets.len(), 3);
    assert_eq!(
        package
            .declared_targets
            .iter()
            .map(TargetIdentity::display)
            .collect::<Vec<_>>(),
        [
            "bin:default-runner",
            "bin:disabled-runner",
            "bin:fixture",
            "custom-build:build-script-build",
            "lib:fixture",
            "test:resident",
        ],
        "ownership retains every declared target independently of default-feature publication",
    );
    assert_eq!(
        inventory.binary_outputs,
        [
            BinaryOutput {
                package_id: "fixture-id".to_owned(),
                package_name: "fixture".to_owned(),
                target_name: "default-runner".to_owned(),
            },
            BinaryOutput {
                package_id: "fixture-id".to_owned(),
                package_name: "fixture".to_owned(),
                target_name: "fixture".to_owned(),
            },
        ],
        "metadata retains unconditional and default-enabled binary publications only"
    );
    fs::remove_dir_all(root).expect("metadata fixture removes");
}
