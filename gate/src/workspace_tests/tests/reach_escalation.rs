//! Process-boundary proof for explicit Cargo reach escalation diagnostics.

use super::*;

#[test]
fn reach_escalation_is_flushed_before_exhaustive_cargo_test_selectors_launch() {
    let fixture = ReachEscalationFixture::new();
    let mut output = EscalationFlushOutput::new(fixture.marker.clone());

    let report = run_selected_at(
        WorkspaceTestRoots::new(&fixture.root, &fixture.root),
        wire::Digest::compute(wire::Domain::GateTree, b"escalated selected fixture"),
        &["tools/outside.rs".to_owned()],
        fixture.environment.clone(),
        1,
        &mut output,
    )
    .unwrap_or_else(|error| panic!("escalated selected runner failed: {error}"));

    assert_eq!(
        report.tests(),
        ["fixture::lib:fixture", "fixture::test:resident"],
        "outside-package reach remains fail-closed over every workspace target"
    );
    assert_diagnostic_precedes_selection(&output.text());
    assert_eq!(
        fs::read_to_string(fixture.root.join("commands.log"))
            .expect("read escalation Cargo commands")
            .lines()
            .collect::<Vec<_>>(),
        [
            "metadata --locked --format-version 1 --no-deps",
            "test --locked --package fixture-id --lib --test resident",
        ],
        "metadata establishes authority before one exhaustive selected-target Cargo shard"
    );
}

fn assert_diagnostic_precedes_selection(output: &str) {
    let escalation = output
        .find(concat!(
            "[rust-test-reach-escalation] source=\"tools/outside.rs\" ",
            "reason=outside-package"
        ))
        .expect("canonical outside-package escalation is observable");
    let selection = output
        .find("[rust-test-selection] detail=canonical-report sources=1 tests=2 edges=2")
        .expect("exhaustive selection summary remains observable");
    assert!(escalation < selection);
}

struct ReachEscalationFixture {
    root: PathBuf,
    environment: Environment,
    marker: PathBuf,
}

impl ReachEscalationFixture {
    fn new() -> Self {
        let root = fixture_root("workspace-reach-escalation-ordering");
        write_fixture_sources(&root);
        let root = fs::canonicalize(root).expect("canonicalize escalation fixture");
        write_fixture_metadata(&root);
        let programs = root.join("programs");
        write_fake_cargo(&programs);
        let marker = root.join("escalation-flushed");
        let environment = binary_output_environment(&root, &programs);
        Self {
            root,
            environment,
            marker,
        }
    }
}

impl Drop for ReachEscalationFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn write_fixture_sources(root: &Path) {
    let programs = root.join("programs");
    let package = root.join("fixture");
    fs::create_dir_all(package.join("src")).expect("create escalation library source root");
    fs::create_dir_all(package.join("tests")).expect("create escalation test source root");
    fs::create_dir_all(root.join("tools")).expect("create escalation source root");
    fs::create_dir_all(programs).expect("create escalation program root");
    fs::write(root.join("Cargo.toml"), "[workspace]\nresolver = \"2\"\n")
        .expect("write escalation workspace manifest");
    fs::write(
        package.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\n",
    )
    .expect("write escalation package manifest");
    fs::write(package.join("src/lib.rs"), "pub fn fixture() {}\n")
        .expect("write escalation library source");
    fs::write(
        package.join("tests/resident.rs"),
        "#[test]\nfn resident() {}\n",
    )
    .expect("write escalation integration source");
    fs::write(root.join("tools/outside.rs"), "pub fn outside() {}\n")
        .expect("write escalation outside-package source");
}

fn write_fixture_metadata(root: &Path) {
    let package = root.join("fixture");
    let metadata = serde_json::json!({
        "packages": [{
            "id": "fixture-id",
            "name": "fixture",
            "manifest_path": package.join("Cargo.toml"),
            "targets": [
                {
                    "name": "fixture",
                    "kind": ["lib"],
                    "test": true,
                    "src_path": package.join("src/lib.rs"),
                },
                {
                    "name": "resident",
                    "kind": ["test"],
                    "test": true,
                    "src_path": package.join("tests/resident.rs"),
                },
            ],
        }],
        "workspace_members": ["fixture-id"],
        "target_directory": root.join("target"),
        "build_directory": root.join("target/cargo-build"),
    });
    fs::write(
        root.join("metadata.json"),
        serde_json::to_vec(&metadata).expect("encode escalation metadata"),
    )
    .expect("write escalation metadata");
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
            "if [ ! -f \"$FIXTURE_ROOT/escalation-flushed\" ]; then\n",
            "  printf 'Cargo test launched before reach escalation was flushed\\n' >&2\n",
            "  exit 70\n",
            "fi\n",
            "printf 'test fixture::done ... ok\\n'\n",
        ),
    );
}

struct EscalationFlushOutput {
    marker: PathBuf,
    bytes: Vec<u8>,
}

impl EscalationFlushOutput {
    fn new(marker: PathBuf) -> Self {
        Self {
            marker,
            bytes: Vec::new(),
        }
    }

    fn text(&self) -> String {
        String::from_utf8(self.bytes.clone()).expect("escalation output is UTF-8")
    }
}

impl Write for EscalationFlushOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if String::from_utf8_lossy(&self.bytes).contains("[rust-test-reach-escalation]") {
            fs::write(&self.marker, b"flushed\n")?;
        }
        Ok(())
    }
}
