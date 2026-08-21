//! Process-boundary proof for fail-closed Cargo source ownership.

use super::*;

#[test]
fn unknown_rust_ownership_refuses_before_any_cargo_execution_launches() {
    let fixture = UnknownOwnershipFixture::new();
    let mut output = Vec::new();

    let error = run_selected_at(
        WorkspaceTestRoots::new(&fixture.root, &fixture.root),
        wire::Digest::compute(wire::Domain::GateTree, b"unknown ownership fixture"),
        &["tools/outside.rs".to_owned()],
        fixture.environment.clone(),
        1,
        &mut output,
    )
    .expect_err("unknown Rust ownership must refuse")
    .to_string();

    assert!(
        error.contains("changed Rust source \"tools/outside.rs\" is outside every Cargo package"),
        "{error}",
    );
    let output = String::from_utf8(output).expect("selection diagnostics are UTF-8");
    assert!(output.contains("stage=selection-terminal"), "{output}");
    assert!(output.contains("disposition=failed"), "{output}");
    assert!(!output.contains("[rust-test-unit]"), "{output}");
    assert!(!output.contains("[rust-test-selection]"), "{output}");
    assert_eq!(
        fs::read_to_string(fixture.root.join("commands.log"))
            .expect("read ownership-refusal Cargo commands")
            .lines()
            .collect::<Vec<_>>(),
        ["metadata --locked --format-version 1 --no-deps"],
        "metadata establishes ownership authority and no compiler follows the refusal"
    );
}

struct UnknownOwnershipFixture {
    root: PathBuf,
    environment: Environment,
}

impl UnknownOwnershipFixture {
    fn new() -> Self {
        let root = fixture_root("workspace-unknown-ownership-refusal");
        let programs = root.join("programs");
        let package = root.join("fixture");
        fs::create_dir_all(package.join("src")).expect("create fixture library source root");
        fs::create_dir_all(root.join("tools")).expect("create unowned source root");
        fs::create_dir_all(&programs).expect("create fixture program root");
        fs::write(root.join("Cargo.toml"), "[workspace]\nresolver = \"2\"\n")
            .expect("write workspace manifest");
        fs::write(
            package.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\n",
        )
        .expect("write package manifest");
        fs::write(package.join("src/lib.rs"), "pub fn fixture() {}\n")
            .expect("write library source");
        fs::write(root.join("tools/outside.rs"), "pub fn outside() {}\n")
            .expect("write unowned Rust source");
        let root = fs::canonicalize(root).expect("canonicalize ownership fixture");
        write_fixture_metadata(&root);
        write_fake_cargo(&programs);
        let environment = binary_output_environment(&root, &programs);
        Self { root, environment }
    }
}

impl Drop for UnknownOwnershipFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn write_fixture_metadata(root: &Path) {
    let package = root.join("fixture");
    let metadata = serde_json::json!({
        "packages": [{
            "id": "fixture-id",
            "name": "fixture",
            "manifest_path": package.join("Cargo.toml"),
            "targets": [{
                "name": "fixture",
                "kind": ["lib"],
                "test": true,
                "src_path": package.join("src/lib.rs"),
            }],
        }],
        "workspace_members": ["fixture-id"],
        "target_directory": root.join("target"),
        "build_directory": root.join("target/cargo-build"),
    });
    fs::write(
        root.join("metadata.json"),
        serde_json::to_vec(&metadata).expect("encode fixture metadata"),
    )
    .expect("write fixture metadata");
}

fn write_fake_cargo(programs: &Path) {
    executable(
        programs,
        "cargo",
        concat!(
            "#!/bin/sh\n",
            "printf '%s\\n' \"$*\" >> \"$FIXTURE_ROOT/commands.log\"\n",
            "if [ \"$1\" = metadata ]; then\n",
            "  /bin/cat \"$FIXTURE_ROOT/metadata.json\"\n",
            "  exit 0\n",
            "fi\n",
            "printf 'unexpected Cargo execution after ownership refusal\\n' >&2\n",
            "exit 70\n",
        ),
    );
}
