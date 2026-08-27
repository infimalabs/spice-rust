#![cfg(unix)]

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use gate::{BatteryReport, GateId};

const PROCESS_PATH: &str = concat!("PA", "TH");
const FORMAT_ACTION: &str =
    "rustfmt --edition <workspace> --check --files-with-diff <gate::formatter_sources>";
const WORKSPACE_TEST_ACTION: &str = "deterministic sharded cargo test --locked --workspace";

#[test]
fn standalone_compiler_arms_run_their_declared_analyses_without_verifier_evidence() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.repository.join("app")).expect("create fixture app directory");
    fs::write(
        fixture.repository.join("app/index.html"),
        "<!doctype html><title>fixture</title>\n",
    )
    .expect("write fixture app entry point");
    fs::write(
        fixture.repository.join("src/lib.rs"),
        "pub fn fixture() { assert_eq!(2 + 2, 4); }\n",
    )
    .expect("write fixture Rust source");
    git(
        &fixture.repository,
        &["add", "app/index.html", "src/lib.rs"],
    );

    let programs = fixture.root.join("programs");
    fs::create_dir(&programs).expect("create standalone compiler programs");
    let cargo_log = fixture.root.join("standalone-cargo-actions");
    install_cargo_with_metadata(&programs, &cargo_log);
    let trunk_log = fixture.root.join("standalone-trunk-actions");
    executable(
        &programs.join("trunk"),
        &format!(
            concat!(
                "#!/bin/sh\n",
                "if [ \"$*\" = \"--version\" ]; then\n",
                "  printf '%s\\n' 'trunk 0.21.14'\n",
                "  exit 0\n",
                "fi\n",
                "printf '%s\\n' \"$*\" >> '{}'\n",
                "exit 0\n",
            ),
            trunk_log.display()
        ),
    );

    for gate_id in ["rust-typecheck", "canvas-web-typecheck"] {
        let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args(["gate", gate_id])
            .current_dir(&fixture.repository)
            .env(PROCESS_PATH, &programs)
            .output()
            .expect("launch standalone compiler arm");
        assert!(output.status.success(), "{}", streams(&output));
    }

    let cargo = fs::read_to_string(&cargo_log).expect("read standalone Cargo trace");
    assert!(
        cargo.lines().any(|line| {
            line == "test --locked --package verify-process-fixture --lib --no-run"
        }),
        "native standalone analysis did not run: {cargo}"
    );
    assert!(
        cargo.lines().any(|line| {
            line == concat!(
                "clippy --locked --package app --target wasm32-unknown-unknown ",
                "-- -D warnings",
            )
        }),
        "canvas standalone Cargo analysis did not run: {cargo}"
    );
    let trunk = fs::read_to_string(&trunk_log).expect("read standalone Trunk trace");
    assert_eq!(
        trunk.trim(),
        "build index.html --release --locked --filehash=false"
    );
}

#[test]
fn scoped_verifier_runs_one_package_without_any_full_only_action() {
    let fixture = Fixture::new();
    fs::write(
        fixture.repository.join("src/lib.rs"),
        "pub fn fixture() { assert_eq!(2 + 2, 4); }\n",
    )
    .expect("write one-package staged change");
    git(&fixture.repository, &["add", "src/lib.rs"]);

    let programs = fixture.root.join("programs");
    fs::create_dir(&programs).expect("create scoped verifier program directory");
    install_real_git(&programs);
    let cargo_log = fixture.root.join("cargo-actions");
    executable(
        &programs.join("cargo"),
        &format!(
            "#!/bin/sh\nprintf 'jobs=%s %s\\n' \"$CARGO_BUILD_JOBS\" \"$*\" >> '{}'\nexit 0\n",
            cargo_log.display()
        ),
    );
    let browser_marker = fixture.root.join("browser-invoked");
    executable(
        &programs.join("npm"),
        &format!(
            "#!/bin/sh\nprintf 'invoked\\n' > '{}'\nexit 99\n",
            browser_marker.display()
        ),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["verify", "--package", "verify-process-fixture"])
        .current_dir(&fixture.repository)
        .env(PROCESS_PATH, &programs)
        .output()
        .expect("launch scoped verifier process");

    assert!(output.status.success(), "{}", streams(&output));
    assert_eq!(
        fs::read_to_string(cargo_log)
            .expect("read scoped Cargo actions")
            .lines()
            .collect::<Vec<_>>(),
        [
            concat!(
                "jobs=1 clippy --locked --package verify-process-fixture --all-targets ",
                "-- -D warnings"
            ),
            "jobs=1 test --locked --package verify-process-fixture --all-targets",
            "jobs=1 test --locked --package verify-process-fixture --doc",
        ]
    );
    assert!(
        !browser_marker.exists(),
        "scoped verification launched the full browser suite"
    );

    let stdout = String::from_utf8(output.stdout.clone()).expect("scoped stdout is UTF-8");
    assert!(stdout.contains(concat!(
        "[verify] scoped package verify-process-fixture; slice-local: ",
        "add named downstream packages or run staged selection before completion\n"
    )));
    assert!(stdout.contains("[verify] scoped gate arms run: none\n"));
    let skipped = GateId::BATTERY
        .into_iter()
        .map(GateId::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    assert!(stdout.contains(&format!("[verify] scoped gate arms skipped: {skipped}\n")));
    for full_only in [
        FORMAT_ACTION,
        "cargo clippy --locked --workspace --all-targets -- -D warnings",
        "cargo check --locked --workspace --target wasm32-unknown-unknown",
        "npm run test:canvas (BROWSER-1kKSWhZk guard)",
        WORKSPACE_TEST_ACTION,
        "canonical Rust gate battery",
        "contract landing",
    ] {
        assert!(
            !stdout.lines().any(|line| {
                line.contains("stage=") && line.contains(&format!("identity={full_only:?}"))
            }),
            "scoped verifier executed full-only action {full_only}: {stdout}"
        );
    }
    assert!(
        output
            .stdout
            .split(|byte| *byte == b'\n')
            .all(|line| BatteryReport::decode(line).is_err()),
        "scoped verifier emitted a complete full-battery report"
    );
}

#[test]
fn bare_verify_refuses_before_any_repository_wide_action_starts() {
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("verify")
        .output()
        .expect("launch retired bare verifier");

    assert!(!output.status.success(), "{}", streams(&output));
    assert!(output.stdout.is_empty(), "{}", streams(&output));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("repository-wide verification is disabled"),
        "{}",
        streams(&output),
    );
    assert!(!stderr.contains("stage="), "{}", streams(&output));
}

#[test]
fn retired_verifier_app_publication_refuses_before_compiler_work() {
    let destination = std::env::temp_dir().join("retired-verifier-app-publication");
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["app", gate::VERIFIER_APP_PUBLISH_FLAG])
        .arg(&destination)
        .output()
        .expect("launch retired verifier app publication");

    assert!(!output.status.success(), "{}", streams(&output));
    assert!(output.stdout.is_empty(), "{}", streams(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("verifier app publication is retired"),
        "{}",
        streams(&output),
    );
    assert!(
        !destination.exists(),
        "retired app publication wrote output"
    );
}

#[test]
fn command_help_enumerates_every_single_gate_arm_spelling() {
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("help")
        .output()
        .expect("launch xtask help");

    assert!(output.status.success(), "{}", streams(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(concat!(
            "\"$(/usr/bin/git rev-parse --path-format=absolute --git-common-dir)",
            "/.spice-install/bin/spice\" dev xtask -- <help|verify --package PACKAGE|",
            "gate [--report] ARM|"
        )),
        "{}",
        streams(&output)
    );
    assert!(stdout.contains("gate --report ARM emits one canonical machine report"));
    for gate_id in GateId::BATTERY {
        assert!(
            stdout
                .lines()
                .any(|line| line == format!("  {}", gate_id.as_str())),
            "help omits {}:\n{}",
            gate_id.as_str(),
            streams(&output)
        );
    }
}

#[test]
fn unknown_gate_arm_points_to_the_canonical_inventory() {
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["gate", "not-a-gate"])
        .output()
        .expect("launch unknown gate arm");

    assert_eq!(output.status.code(), Some(2), "{}", streams(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("run installed Spice `dev xtask -- help` for accepted gate arms"),
        "{}",
        streams(&output)
    );
}

struct Fixture {
    root: PathBuf,
    repository: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "spice-xtask-verify-process-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let repository = root.join("repository");
        fs::create_dir_all(repository.join("src")).expect("create verifier fixture repository");
        git(&repository, &["init", "-q"]);
        git(
            &repository,
            &["config", "user.email", "verify-process@example.invalid"],
        );
        git(
            &repository,
            &["config", "user.name", "Verifier Process Fixture"],
        );
        fs::write(
            repository.join("spice-rust.toml"),
            format!("{}[gate.walk]\nskip = []\n", gate::NARROWEST_FILE_CLASSES),
        )
        .expect("write verifier fixture policy");
        fs::write(
            repository.join("baseline.toml"),
            concat!(
                "version = 1\n\n",
                "[[floor]]\nname = \"rust.mutation-survivors\"\nvalue = 0\n",
            ),
        )
        .expect("write verifier fixture baseline");
        fs::write(
            repository.join("Cargo.toml"),
            concat!(
                "[package]\n",
                "name = \"verify-process-fixture\"\n",
                "version = \"0.0.0\"\n",
                "edition = \"2024\"\n",
            ),
        )
        .expect("write verifier fixture Cargo manifest");
        fs::write(repository.join(".gitignore"), "/dist/\n/target/\n")
            .expect("ignore verifier-owned fixture build outputs");
        fs::create_dir(repository.join("app")).expect("create verifier fixture app directory");
        fs::write(repository.join("app/index.html"), "<!doctype html>\n")
            .expect("write verifier fixture Canvas entry point");
        fs::write(repository.join("src/lib.rs"), "pub fn fixture() {}\n")
            .expect("write verifier fixture source");
        git(&repository, &["add", "."]);
        git(&repository, &["commit", "-qm", "fixture"]);

        let binaries = root.join("current-build");
        fs::create_dir(&binaries).expect("create fixture current-build directory");
        let current_spice = binaries.join(format!("spice{}", env::consts::EXE_SUFFIX));
        fs::copy(env!("CARGO_BIN_EXE_xtask"), &current_spice)
            .expect("copy fixture current Spice image");
        let discovered = spice::repository::Repository::discover(&repository)
            .expect("discover verifier fixture repository");
        let plan = spice::install::plan(&discovered, Some(&current_spice))
            .expect("plan fixture installed appliance");
        spice::install::apply(&discovered, Some(&current_spice), &plan.digest().unwrap())
            .expect("publish fixture installed appliance");
        let hooks = discovered.common_dir().join(".spice-install/git-hooks");
        git(
            &repository,
            &[
                "config",
                "--local",
                "core.hooksPath",
                hooks.to_str().expect("fixture hook path is UTF-8"),
            ],
        );
        Self { root, repository }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove verifier process fixture");
    }
}

fn install_cargo_with_metadata(programs: &Path, cargo_log: &Path) {
    let metadata = serde_json::json!({
        "packages": [{
            "id": "verify-process-fixture-id",
            "name": "verify-process-fixture",
            "manifest_path": "$PWD/Cargo.toml",
            "targets": [{
                "name": "verify_process_fixture",
                "kind": ["lib"],
                "src_path": "$PWD/src/lib.rs",
                "test": true,
            }],
        }],
        "workspace_members": ["verify-process-fixture-id"],
    })
    .to_string()
    .replace('"', "\\\"");
    executable(
        &programs.join("cargo"),
        &format!(
            concat!(
                "#!/bin/sh\n",
                "if [ \"$*\" = \"--version\" ]; then\n",
                "  printf '%s\\n' 'cargo 1.97.1 (c980f4866 2026-06-30)'\n",
                "  exit 0\n",
                "fi\n",
                "if [ \"$1\" = metadata ]; then\n",
                "  printf '%s\\n' \"{}\"\n",
                "  exit 0\n",
                "fi\n",
                "printf '%s\\n' \"$*\" >> '{}'\n",
                "if [ \"$1\" = xtask ] && [ \"$2\" = app ]; then\n",
                "  printf '[app] compiler-ready\\n'\n",
                "fi\n",
                "exit 0\n",
            ),
            metadata,
            cargo_log.display(),
        ),
    );
}

fn executable(path: &Path, text: &str) {
    fs::write(path, text).expect("write process fixture executable");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .expect("make process fixture executable");
}

fn install_real_git(programs: &Path) {
    std::os::unix::fs::symlink("/usr/bin/git", programs.join("git"))
        .expect("install real Git in the verifier fixture rim");
}

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("/usr/bin/git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("fixture Git launches");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn streams(output: &std::process::Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
