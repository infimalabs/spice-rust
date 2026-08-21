//! Multi-owner source reach across physically nested Cargo packages.

use super::*;

const SHARED_CHANGES: [&str; 2] = [
    "outer/src/nested/src/included.rs",
    "outer/src/nested/src/shared/leaf.rs",
];

#[test]
fn path_include_and_conventional_sources_retain_every_cross_package_owner() {
    let fixture = CrossPackageFixture::new();
    let inventory = acquire_inventory(&fixture.root, &fixture.environment, OsStr::new("cargo"))
        .expect("acquire cross-package source claims");
    let shared_changes = shared_changes();

    let shared = selection_plan(
        &inventory,
        &fixture.root,
        wire::Digest::compute(wire::Domain::GateTree, b"cross-package shared sources"),
        &shared_changes,
    )
    .expect("shared sources select every owning target and dependent");

    assert!(shared.escalations().is_empty());
    assert_eq!(
        shared.report().tests(),
        ["nested::lib:nested", "outer::test:bridge"]
    );
    assert_eq!(shared.report().edges().len(), 4);
    for source in &shared_changes {
        assert_eq!(
            shared
                .report()
                .edges()
                .iter()
                .filter(|edge| edge.source() == source)
                .count(),
            2,
            "each cross-package source reaches both direct owners"
        );
    }

    let ordinary = selection_plan(
        &inventory,
        &fixture.root,
        wire::Digest::compute(wire::Domain::GateTree, b"ordinary nested source"),
        &["outer/src/nested/src/only_nested.rs".to_owned()],
    )
    .expect("ordinary nested source keeps its exact package reach");

    assert!(ordinary.escalations().is_empty());
    assert_eq!(ordinary.report().tests(), ["nested::lib:nested"]);
    assert!(
        ordinary
            .report()
            .tests()
            .iter()
            .all(|test| !test.starts_with("outer::")),
        "directory nesting alone cannot manufacture an outer-package owner"
    );
}

#[test]
fn native_checks_selected_tests_and_canvas_consume_one_multi_owner_decision() {
    let fixture = CrossPackageFixture::new();
    let shared_changes = shared_changes();
    let reach = workspace_package_reach_at(
        &fixture.root,
        &shared_changes,
        &fixture.environment,
        OsStr::new("cargo"),
    )
    .expect("native reach resolves every cross-package owner");

    assert_eq!(
        reach.cargo_check_arguments(),
        [
            ["check", "--locked", "--all-targets", "--package", "nested",]
                .map(str::to_owned)
                .to_vec(),
            ["check", "--locked", "--package", "app", "--lib"]
                .map(str::to_owned)
                .to_vec(),
            [
                "check",
                "--locked",
                "--package",
                "outer",
                "--test",
                "bridge",
            ]
            .map(str::to_owned)
            .to_vec(),
        ]
    );
    for package in ["app", "outer", "outer/src/nested"] {
        assert!(
            reach.reaches_root(&fixture.root.join(package)),
            "Canvas applicability must observe reached package {package}"
        );
    }
    let ordinary = workspace_package_reach_at(
        &fixture.root,
        &["outer/src/nested/src/only_nested.rs".to_owned()],
        &fixture.environment,
        OsStr::new("cargo"),
    )
    .expect("ordinary nested native reach resolves");
    assert_eq!(
        ordinary.cargo_check_arguments(),
        [
            ["check", "--locked", "--all-targets", "--package", "nested",]
                .map(str::to_owned)
                .to_vec(),
            ["check", "--locked", "--package", "app", "--lib"]
                .map(str::to_owned)
                .to_vec(),
        ]
    );
    assert!(!ordinary.reaches_root(&fixture.root.join("outer")));

    fs::write(fixture.root.join("commands.log"), "").expect("clear metadata command log");
    let mut output = Vec::new();
    let report = run_selected_at(
        WorkspaceTestRoots::new(&fixture.root, &fixture.root),
        wire::Digest::compute(wire::Domain::GateTree, b"cross-package selected execution"),
        &shared_changes,
        fixture.environment.clone(),
        1,
        &mut output,
    )
    .unwrap_or_else(|error| {
        panic!(
            "cross-package selected execution failed: {error}; output={}",
            String::from_utf8_lossy(&output)
        )
    });

    assert_eq!(report.tests(), ["nested::lib:nested", "outer::test:bridge"]);
    assert!(!String::from_utf8_lossy(&output).contains("reach-escalation"));
    assert_eq!(
        fs::read_to_string(fixture.root.join("commands.log"))
            .expect("read selected multi-owner commands")
            .lines()
            .collect::<Vec<_>>(),
        [
            "metadata --locked --format-version 1 --no-deps",
            concat!(
                "test --locked --package nested-id --package outer-id --all-targets ",
                "--no-run --message-format=json-render-diagnostics",
            ),
            "artifact bridge --test-threads=3",
            "artifact nested --test-threads=3",
        ],
        "selected tests prebuild once and launch their exact compiled artifacts"
    );
}

fn shared_changes() -> Vec<String> {
    SHARED_CHANGES.map(str::to_owned).to_vec()
}

struct CrossPackageFixture {
    root: PathBuf,
    environment: Environment,
}

impl CrossPackageFixture {
    fn new() -> Self {
        let root = fixture_root("workspace-cross-package-ownership");
        write_sources(&root);
        let root = fs::canonicalize(root).expect("canonicalize cross-package fixture");
        write_metadata(&root);
        let programs = root.join("programs");
        write_fake_cargo(&programs);
        let environment = binary_output_environment(&root, &programs);
        Self { root, environment }
    }
}

impl Drop for CrossPackageFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn write_sources(root: &Path) {
    for directory in [
        "app/src",
        "outer/tests",
        "outer/src/nested/src/shared",
        "programs",
    ] {
        fs::create_dir_all(root.join(directory)).expect("create cross-package fixture directory");
    }
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"app\", \"outer\", \"outer/src/nested\"]\n",
    )
    .expect("write cross-package workspace manifest");
    fs::write(
        root.join("app/Cargo.toml"),
        concat!(
            "[package]\nname = \"app\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n",
            "[dependencies]\nnested = { path = \"../outer/src/nested\" }\n",
        ),
    )
    .expect("write dependent app manifest");
    fs::write(
        root.join("outer/Cargo.toml"),
        "[package]\nname = \"outer\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
    )
    .expect("write outer manifest");
    fs::write(
        root.join("outer/src/nested/Cargo.toml"),
        "[package]\nname = \"nested\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
    )
    .expect("write nested manifest");
    fs::write(
        root.join("outer/src/lib.rs"),
        "pub fn outer_only() -> u8 { 1 }\n",
    )
    .expect("write unrelated outer library target");
    fs::write(
        root.join("app/src/lib.rs"),
        "#[test]\nfn app_reaches_nested() { assert_eq!(nested::value(), 3); }\n",
    )
    .expect("write dependent app target");
    fs::write(
        root.join("outer/tests/bridge.rs"),
        concat!(
            "#[path = \"../src/nested/src/shared.rs\"]\nmod shared;\n",
            "include!(\"../src/nested/src/included.rs\");\n",
            "#[test]\nfn bridge() { assert_eq!(shared::value() + included(), 3); }\n",
        ),
    )
    .expect("write outer cross-package target");
    fs::write(
        root.join("outer/src/nested/src/lib.rs"),
        concat!(
            "mod only_nested;\nmod shared;\ninclude!(\"included.rs\");\n",
            "pub fn value() -> u8 { only_nested::one() + shared::value() + included() - 1 }\n",
        ),
    )
    .expect("write nested library target");
    fs::write(
        root.join("outer/src/nested/src/only_nested.rs"),
        "pub fn one() -> u8 { 1 }\n",
    )
    .expect("write ordinary nested source");
    fs::write(
        root.join("outer/src/nested/src/shared.rs"),
        "mod leaf;\npub fn value() -> u8 { leaf::value() }\n",
    )
    .expect("write shared path module");
    fs::write(
        root.join("outer/src/nested/src/shared/leaf.rs"),
        "pub fn value() -> u8 { 1 }\n",
    )
    .expect("write conventionally nested shared source");
    fs::write(
        root.join("outer/src/nested/src/included.rs"),
        "fn included() -> u8 { 2 }\n",
    )
    .expect("write cross-package included source");
}

fn write_metadata(root: &Path) {
    let nested = root.join("outer/src/nested");
    let metadata = serde_json::json!({
        "packages": [
            {
                "id": "app-id",
                "name": "app",
                "manifest_path": root.join("app/Cargo.toml"),
                "dependencies": [{ "path": nested }],
                "targets": [{
                    "name": "app",
                    "kind": ["lib"],
                    "test": true,
                    "src_path": root.join("app/src/lib.rs"),
                }],
            },
            {
                "id": "nested-id",
                "name": "nested",
                "manifest_path": root.join("outer/src/nested/Cargo.toml"),
                "targets": [{
                    "name": "nested",
                    "kind": ["lib"],
                    "test": true,
                    "src_path": root.join("outer/src/nested/src/lib.rs"),
                }],
            },
            {
                "id": "outer-id",
                "name": "outer",
                "manifest_path": root.join("outer/Cargo.toml"),
                "targets": [
                    {
                        "name": "outer",
                        "kind": ["lib"],
                        "test": true,
                        "src_path": root.join("outer/src/lib.rs"),
                    },
                    {
                        "name": "bridge",
                        "kind": ["test"],
                        "test": true,
                        "src_path": root.join("outer/tests/bridge.rs"),
                    },
                ],
            },
        ],
        "workspace_members": ["app-id", "nested-id", "outer-id"],
        "target_directory": root.join("target"),
        "build_directory": root.join("target/cargo-build"),
    });
    fs::write(
        root.join("metadata.json"),
        serde_json::to_vec(&metadata).expect("encode cross-package metadata"),
    )
    .expect("write cross-package metadata");
}

fn write_fake_cargo(programs: &Path) {
    executable(
        programs,
        "nested-test",
        concat!(
            "#!/bin/sh\n",
            "printf 'artifact nested %s\\n' \"$*\" >> \"$FIXTURE_ROOT/commands.log\"\n",
            "printf 'test cross_package::nested ... ok\\n'\n",
        ),
    );
    executable(
        programs,
        "bridge-test",
        concat!(
            "#!/bin/sh\n",
            "printf 'artifact bridge %s\\n' \"$*\" >> \"$FIXTURE_ROOT/commands.log\"\n",
            "printf 'test cross_package::bridge ... ok\\n'\n",
        ),
    );
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
            "for argument in \"$@\"; do\n",
            "  if [ \"$argument\" = --no-run ]; then\n",
            concat!(
                "    printf '{\"reason\":\"compiler-artifact\",",
                "\"package_id\":\"nested-id\",",
                "\"target\":{\"name\":\"nested\",\"kind\":[\"lib\"]},",
                "\"profile\":{\"test\":true},",
                "\"executable\":\"%s/programs/nested-test\"}\\n' \"$FIXTURE_ROOT\"\n",
            ),
            concat!(
                "    printf '{\"reason\":\"compiler-artifact\",",
                "\"package_id\":\"outer-id\",",
                "\"target\":{\"name\":\"bridge\",\"kind\":[\"test\"]},",
                "\"profile\":{\"test\":true},",
                "\"executable\":\"%s/programs/bridge-test\"}\\n' \"$FIXTURE_ROOT\"\n",
            ),
            "    exit 0\n",
            "  fi\n",
            "done\n",
            "printf 'test cross_package::selected ... ok\\n'\n",
        ),
    );
}
