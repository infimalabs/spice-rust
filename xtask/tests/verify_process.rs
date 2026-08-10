#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use gate::metrics::{EvidenceState, MetricId, MetricKey};
use gate::{ArmEvidence, BATTERY_REFUSAL_FOLD_MAX_BYTES, BatteryArmResult, BatteryReport, GateId};

const PROCESS_PATH: &str = concat!("PA", "TH");
const FORMAT_ACTION: &str =
    "rustfmt --edition <workspace> --check --files-with-diff <gate::formatter_sources>";
const WORKSPACE_TEST_ACTION: &str =
    "deterministic sharded cargo test --locked --workspace --all-targets";

#[test]
fn verifier_refuses_a_formatting_defect_reached_only_through_include() {
    let fixture = Fixture::new();
    fs::write(
        fixture.repository.join("src/lib.rs"),
        "include!(\"mutation.rs\");\n",
    )
    .expect("write include-only fixture root");
    fs::write(
        fixture.repository.join("src/mutation.rs"),
        "pub fn mutation( ) { let value=1; assert_eq!(value,1); }\n",
    )
    .expect("write unformatted included source");
    git(
        &fixture.repository,
        &["add", "src/lib.rs", "src/mutation.rs"],
    );

    let programs = fixture.root.join("programs");
    fs::create_dir(&programs).expect("create formatter fixture program directory");
    install_unset_hooks_git(&programs);
    install_real_rustfmt(&programs);
    let cargo_marker = fixture.root.join("cargo-invoked");
    executable(
        &programs.join("cargo"),
        &format!(
            "#!/bin/sh\nprintf 'invoked\\n' > '{}'\nexit 0\n",
            cargo_marker.display()
        ),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("verify")
        .current_dir(&fixture.repository)
        .env(PROCESS_PATH, &programs)
        .output()
        .expect("launch verifier against an include-only formatting defect");

    assert_eq!(output.status.code(), Some(1), "{}", streams(&output));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("src/mutation.rs"),
        "formatter refusal does not name the included source: {}",
        streams(&output)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(FORMAT_ACTION),
        "verifier did not attribute the refusal to its formatter action: {}",
        streams(&output)
    );
    assert!(
        !cargo_marker.exists(),
        "the verifier crossed its formatter refusal"
    );
}

#[test]
fn verifier_process_projects_one_bounded_actionable_file_shape_refusal() {
    let fixture = Fixture::new();
    let programs = install_full_verifier_programs(&fixture);

    install_file_shape_overrun(&fixture);
    let selected_result = run_selected_gate(
        &fixture,
        &programs.directory,
        &programs.cargo_log,
        &programs.browser_log,
    );
    let refused_result = run_refusing_selected_gate(&fixture, &programs.directory);

    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("verify")
        .current_dir(&fixture.repository)
        .env(PROCESS_PATH, &programs.directory)
        .output()
        .expect("launch the real verifier process");

    assert_eq!(output.status.code(), Some(1), "{}", streams(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("battery refused publication"),
        "{}",
        streams(&output)
    );
    assert!(
        !programs.forbidden_marker.exists(),
        "a forbidden program crossed the verifier rim: {}",
        programs.forbidden_marker.display()
    );
    assert_full_verifier_action_trace(&programs, &fixture.repository, &output);

    assert_bounded_refusal(&output, &refused_result);
    assert_eq!(selected_result.gate_id(), GateId::EnvNameLedger);
}

struct FullVerifierPrograms {
    directory: PathBuf,
    forbidden_marker: PathBuf,
    cargo_log: PathBuf,
    trunk_log: PathBuf,
    browser_log: PathBuf,
}

fn assert_full_verifier_action_trace(
    programs: &FullVerifierPrograms,
    repository: &Path,
    output: &std::process::Output,
) {
    assert_workspace_test_execution(&programs.cargo_log, repository, output);
    let browser = fs::read_to_string(&programs.browser_log).expect("read verifier browser action");
    assert!(
        browser.starts_with("run test:canvas -- --spice-coverage-receipt "),
        "unexpected browser action: {browser:?}"
    );
    assert!(browser.contains("--spice-prebuilt-app"));
    assert!(
        browser.contains(&format!(
            "binary={}/target/debug/spice\n",
            repository
                .canonicalize()
                .expect("canonical browser repository")
                .display()
        )),
        "browser action did not consume the prerequisite resident binary: {browser}"
    );
    let compiler_actions =
        fs::read_to_string(&programs.cargo_log).expect("read compiler process trace");
    assert_eq!(
        compiler_actions
            .lines()
            .filter(|line| *line == "clippy --locked --workspace --all-targets -- -D warnings")
            .count(),
        1,
        "native compiler analysis repeated: {compiler_actions}"
    );
    assert_eq!(
        compiler_actions
            .lines()
            .filter(|line| {
                *line == "check --locked --workspace --target wasm32-unknown-unknown"
            })
            .count(),
        1,
        "Wasm compiler analysis repeated: {compiler_actions}"
    );
    assert!(
        !compiler_actions.contains("check --locked --workspace --all-targets"),
        "battery repeated the native workspace analysis: {compiler_actions}"
    );
    assert!(
        !programs.trunk_log.exists(),
        "battery repeated the canvas Trunk analysis: {}",
        fs::read_to_string(&programs.trunk_log).unwrap_or_default()
    );
}

fn install_full_verifier_programs(fixture: &Fixture) -> FullVerifierPrograms {
    let directory = fixture.root.join("programs");
    fs::create_dir(&directory).expect("create process fixture directory");
    install_unset_hooks_git(&directory);
    install_real_rustfmt(&directory);
    let forbidden_marker = fixture.root.join("forbidden-program-invoked");
    for program in ["python", "python3", "taskwarrior", "task"] {
        executable(
            &directory.join(program),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' '{}' >> '{}'\nexit 97\n",
                program,
                forbidden_marker.display()
            ),
        );
    }
    let cargo_log = fixture.root.join("cargo-actions");
    install_cargo_with_metadata(&directory, &cargo_log);
    let trunk_log = fixture.root.join("trunk-actions");
    executable(
        &directory.join("trunk"),
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
    let browser_log = fixture.root.join("browser-action");
    executable(
        &directory.join("npm"),
        &format!(
            concat!(
                "#!/bin/sh\n",
                "printf '%s\\n' \"$*\" > '{}'\n",
                "printf 'binary=%s\\n' \"$SPICE_CANVAS_BINARY\" >> '{}'\n",
                "while [ \"$#\" -gt 0 ]; do\n",
                "  if [ \"$1\" = \"--spice-coverage-receipt\" ]; then\n",
                "    shift\n",
                "    printf 'BROWSER-1kKSWhZk browser coverage complete\\n' > \"$1\"\n",
                "    exit 0\n",
                "  fi\n",
                "  shift\n",
                "done\n",
                "exit 96\n",
            ),
            browser_log.display(),
            browser_log.display(),
        ),
    );
    FullVerifierPrograms {
        directory,
        forbidden_marker,
        cargo_log,
        trunk_log,
        browser_log,
    }
}

fn install_cargo_with_metadata(programs: &Path, cargo_log: &Path) {
    let metadata = serde_json::json!({
        "packages": [{
            "id": "verify-process-fixture-id",
            "name": "verify-process-fixture",
            "targets": [{
                "name": "verify_process_fixture",
                "kind": ["lib"],
                "test": true,
            }],
        }],
        "workspace_members": ["verify-process-fixture-id"],
    })
    .to_string();
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
                "  printf '%s\\n' '{}'\n",
                "  exit 0\n",
                "fi\n",
                "printf '%s\\n' \"$*\" >> '{}'\n",
                "exit 0\n",
            ),
            metadata,
            cargo_log.display(),
        ),
    );
}

fn assert_workspace_test_execution(
    cargo_log: &Path,
    repository: &Path,
    output: &std::process::Output,
) {
    let app_action = format!(
        "xtask app --spice-verifier-publish-root {}",
        repository
            .canonicalize()
            .expect("canonical verifier repository")
            .display()
    );
    assert_eq!(
        fs::read_to_string(cargo_log)
            .expect("read verifier Cargo actions")
            .lines()
            .filter(|line| *line != "--version")
            .take(8)
            .collect::<Vec<_>>(),
        [
            "clippy --locked --workspace --all-targets -- -D warnings",
            "clippy --locked -p app --target wasm32-unknown-unknown -- -D warnings",
            "check --locked --workspace --target wasm32-unknown-unknown",
            app_action.as_str(),
            "build --locked -p spice --bin spice",
            "test --locked --package verify-process-fixture-id --lib",
            "doc --locked --workspace --lib --bins --no-deps",
            "test --locked --workspace --doc",
        ]
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(concat!(
            "[rust-test] stage=start shard=1/1 ",
            "package=\"verify-process-fixture\" targets=\"lib:verify_process_fixture\""
        )),
        "workspace shard inventory was not emitted: {}",
        streams(output)
    );
    assert!(
        stdout.contains("[rust-test] coordinator disposition=success"),
        "workspace shard coordinator did not complete: {}",
        streams(output)
    );
}

fn run_selected_gate(
    fixture: &Fixture,
    programs: &Path,
    cargo_log: &Path,
    browser_log: &Path,
) -> BatteryArmResult {
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["gate", "env-name-ledger"])
        .current_dir(&fixture.repository)
        .env(PROCESS_PATH, programs)
        .output()
        .expect("launch one real gate arm");
    assert!(output.status.success(), "{}", streams(&output));
    let result: BatteryArmResult =
        serde_json::from_slice(&output.stdout).expect("selected arm emits one JSON result");
    assert_eq!(result.gate_id(), GateId::EnvNameLedger);
    assert!(!cargo_log.exists(), "selected arm launched a Cargo stage");
    assert!(
        !browser_log.exists(),
        "selected arm launched the browser stage"
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("[verify]"));
    result
}

fn run_refusing_selected_gate(fixture: &Fixture, programs: &Path) -> BatteryArmResult {
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["gate", "file-shape"])
        .current_dir(&fixture.repository)
        .env(PROCESS_PATH, programs)
        .output()
        .expect("launch the refusing file-shape gate arm");
    assert_eq!(output.status.code(), Some(1), "{}", streams(&output));
    let lines = output
        .stdout
        .split(|byte| *byte == b'\n')
        .collect::<Vec<_>>();
    assert_eq!(
        lines.iter().filter(|line| !line.is_empty()).count(),
        1,
        "a refusing selected arm still emits exactly one machine row: {}",
        streams(&output)
    );
    let result: BatteryArmResult =
        serde_json::from_slice(lines[0]).expect("refusing selected arm emits canonical JSON");
    assert_eq!(result.gate_id(), GateId::FileShape);
    assert!(!result.state().admits());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("[verify]"));
    result
}

#[test]
fn standalone_compiler_arms_run_their_declared_analyses_without_verifier_evidence() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.repository.join("app")).expect("create fixture app directory");
    fs::write(
        fixture.repository.join("app/index.html"),
        "<!doctype html><title>fixture</title>\n",
    )
    .expect("write fixture app entry point");
    git(&fixture.repository, &["add", "."]);

    let programs = fixture.root.join("programs");
    fs::create_dir(&programs).expect("create standalone compiler programs");
    let cargo_log = fixture.root.join("standalone-cargo-actions");
    executable(
        &programs.join("cargo"),
        &format!(
            concat!(
                "#!/bin/sh\n",
                "if [ \"$*\" = \"--version\" ]; then\n",
                "  printf '%s\\n' 'cargo 1.97.1 (c980f4866 2026-06-30)'\n",
                "  exit 0\n",
                "fi\n",
                "printf '%s\\n' \"$*\" >> '{}'\n",
                "exit 0\n",
            ),
            cargo_log.display()
        ),
    );
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
        cargo
            .lines()
            .any(|line| line == "check --locked --workspace --all-targets"),
        "native standalone analysis did not run: {cargo}"
    );
    assert!(
        cargo
            .lines()
            .any(|line| { line == "check --locked --workspace --target wasm32-unknown-unknown" }),
        "canvas standalone Cargo analysis did not run: {cargo}"
    );
    let trunk = fs::read_to_string(&trunk_log).expect("read standalone Trunk trace");
    assert_eq!(
        trunk.trim(),
        "build index.html --release --locked --filehash=false"
    );
}

fn assert_bounded_refusal(output: &std::process::Output, selected: &BatteryArmResult) {
    let stdout = String::from_utf8(output.stdout.clone()).expect("verifier stdout is UTF-8");
    let fold_start = stdout
        .find("arm position=")
        .expect("verifier leads its refusal fold with arm evidence");
    let summary_start = stdout
        .rfind("battery refusal summary:")
        .expect("verifier closes its refusal fold with a summary");
    let summary_end = stdout[summary_start..]
        .find('\n')
        .map_or(stdout.len(), |end| summary_start + end + 1);
    let fold = &stdout.as_bytes()[fold_start..summary_end];
    assert!(
        fold.len() <= BATTERY_REFUSAL_FOLD_MAX_BYTES,
        "battery diagnostic is {} bytes, limit is {}",
        fold.len(),
        BATTERY_REFUSAL_FOLD_MAX_BYTES
    );
    assert!(
        stdout.contains("full-evidence: cargo xtask gate file-shape"),
        "refusal omits the exact selected-arm recovery command: {}",
        streams(output)
    );
    assert!(
        !stdout.contains("[verify] contract landing"),
        "a refused battery crossed contract landing: {}",
        streams(output)
    );

    let ArmEvidence::Metrics { report } = selected.evidence() else {
        panic!("the selected file-shape arm carries structural metrics")
    };
    let metric = report
        .metrics()
        .iter()
        .find(|metric| {
            metric.metric() == MetricId::FileLines
                && metric.key() == &MetricKey::path("src/overrun.rs").expect("fixture overrun path")
        })
        .expect("selected file-shape evidence names the overrun path");
    let EvidenceState::Measured { measurement } = metric.state() else {
        panic!("the overrun is a measured file-lines value")
    };
    let latch = measurement
        .latch()
        .expect("file lines carry their active latch");
    let exact_finding = format!(
        concat!(
            "finding gate=file-shape metric=file.lines key=path=\"src/overrun.rs\" ",
            "state=failed unit=source-lines value={} base_limit={} ",
            "configured_ratchet_limit={} ratchet_limit={} active_limit={} ",
            "latch_transition={}"
        ),
        measurement.value(),
        latch.base_limit(),
        latch.configured_ratchet_limit(),
        latch.ratchet_limit(),
        latch.active_limit(),
        latch.transition(),
    );
    assert!(
        stdout.contains(&exact_finding),
        "refusal omits the exact actionable file-shape finding {exact_finding:?}: {}",
        streams(output)
    );

    let reports = output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter_map(|line| BatteryReport::decode(line).ok())
        .collect::<Vec<_>>();
    assert!(
        reports.is_empty(),
        "a refusing human verifier emitted a complete canonical BatteryReport: {}",
        streams(output)
    );
    let lifecycle = String::from_utf8_lossy(&output.stdout);
    for identity in [
        "canonical Rust gate battery",
        "npm run test:canvas (BROWSER-1kKSWhZk guard)",
    ] {
        assert_eq!(
            lifecycle
                .lines()
                .filter(|line| {
                    line.contains("stage=start") && line.contains(&format!("identity={identity:?}"))
                })
                .count(),
            1,
            "action {identity:?} did not start exactly once: {}",
            streams(output)
        );
    }
}

fn install_file_shape_overrun(fixture: &Fixture) {
    const OVERLONG_LINES: usize = 4_000;
    let path = fixture.repository.join("src/overrun.rs");
    fs::write(&path, "// measured overrun\n".repeat(OVERLONG_LINES))
        .expect("write real file-shape overrun");
    git(&fixture.repository, &["add", "src/overrun.rs"]);
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
    install_unset_hooks_git(&programs);
    let cargo_log = fixture.root.join("cargo-actions");
    executable(
        &programs.join("cargo"),
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
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
                "clippy --locked --package verify-process-fixture --all-targets ",
                "-- -D warnings"
            ),
            "test --locked --package verify-process-fixture --all-targets",
            "test --locked --package verify-process-fixture --doc",
        ]
    );
    assert!(
        !browser_marker.exists(),
        "scoped verification launched the full browser suite"
    );

    let stdout = String::from_utf8(output.stdout.clone()).expect("scoped stdout is UTF-8");
    assert!(stdout.contains(concat!(
        "[verify] scoped package verify-process-fixture; feedback only: ",
        "run `cargo xtask verify` before task completion\n"
    )));
    assert!(stdout.contains("[verify] scoped gate arms run: none\n"));
    let skipped = GateId::ALL
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
fn command_help_enumerates_every_single_gate_arm_spelling() {
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("help")
        .output()
        .expect("launch xtask help");

    assert!(output.status.success(), "{}", streams(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(concat!(
            "cargo xtask <help|verify [--package PACKAGE]|",
            "verify-action battery|gate ARM|"
        )),
        "{}",
        streams(&output)
    );
    assert!(stdout.contains("not a completion verifier"));
    for gate_id in GateId::ALL {
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
            .contains("run `cargo xtask help` for accepted gate arms"),
        "{}",
        streams(&output)
    );
}

#[test]
fn zero_exit_browser_process_without_a_receipt_refuses_and_never_lands() {
    let fixture = Fixture::new();
    let programs = fixture.root.join("programs");
    fs::create_dir(&programs).expect("create process fixture directory");
    install_unset_hooks_git(&programs);
    install_real_rustfmt(&programs);
    let cargo_log = fixture.root.join("cargo-actions");
    executable(
        &programs.join("cargo"),
        &format!(
            concat!(
                "#!/bin/sh\n",
                "printf '%s\\n' \"$*\" >> '{}'\n",
                "if [ \"$*\" = \"--version\" ]; then\n",
                "  printf '%s\\n' 'cargo 1.97.1 (c980f4866 2026-06-30)'\n",
                "fi\n",
                "exit 0\n",
            ),
            cargo_log.display()
        ),
    );
    executable(
        &programs.join("trunk"),
        concat!(
            "#!/bin/sh\n",
            "if [ \"$*\" = \"--version\" ]; then\n",
            "  printf '%s\\n' 'trunk 0.21.14'\n",
            "fi\n",
            "exit 0\n",
        ),
    );
    executable(&programs.join("npm"), "#!/bin/sh\nexit 0\n");

    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("verify")
        .current_dir(&fixture.repository)
        .env(PROCESS_PATH, &programs)
        .output()
        .expect("launch the real verifier process");

    assert_eq!(output.status.code(), Some(1), "{}", streams(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("browser coverage did not complete"),
        "{}",
        streams(&output)
    );
    let cargo_actions = fs::read_to_string(cargo_log).expect("read verifier Cargo actions");
    let cargo_actions = cargo_actions.lines().collect::<Vec<_>>();
    let chain = cargo_actions
        .iter()
        .copied()
        .filter(|action| *action != "--version")
        .collect::<Vec<_>>();
    let app_action = format!(
        "xtask app --spice-verifier-publish-root {}",
        fixture
            .repository
            .canonicalize()
            .expect("canonical verifier repository")
            .display()
    );
    assert_eq!(
        chain.get(..5),
        Some(
            &[
                "clippy --locked --workspace --all-targets -- -D warnings",
                "clippy --locked -p app --target wasm32-unknown-unknown -- -D warnings",
                "check --locked --workspace --target wasm32-unknown-unknown",
                app_action.as_str(),
                "build --locked -p spice --bin spice",
            ][..]
        )
    );
    assert!(
        chain.iter().all(|action| [
            "clippy --locked --workspace --all-targets -- -D warnings",
            "clippy --locked -p app --target wasm32-unknown-unknown -- -D warnings",
            "check --locked --workspace --target wasm32-unknown-unknown",
            app_action.as_str(),
            "build --locked -p spice --bin spice",
            "metadata --locked --format-version 1 --no-deps",
        ]
        .contains(action)),
        "unexpected Cargo action under browser refusal: {cargo_actions:?}",
    );
    let mut unique_chain = chain.clone();
    unique_chain.sort_unstable();
    unique_chain.dedup();
    assert_eq!(
        unique_chain.len(),
        chain.len(),
        "duplicate Cargo action: {chain:?}"
    );
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("ordinal=13/13"),
        "{}",
        streams(&output)
    );
}

#[test]
fn verifier_wasm_clippy_rejects_a_lint_hidden_from_the_native_target() {
    let fixture = Fixture::new();
    wasm_lint_fixture(&fixture.repository);
    let programs = fixture.root.join("programs");
    fs::create_dir(&programs).expect("create wasm lint fixture programs");
    install_real_rustfmt(&programs);
    let cargo_log = fixture.root.join("cargo-actions");
    executable(
        &programs.join("cargo"),
        &format!(
            concat!(
                "#!/bin/sh\n",
                "printf '%s\\n' \"$*\" >> '{cargo_log}'\n",
                "if [ \"$*\" = \"--version\" ]; then\n",
                "  printf '%s\\n' 'cargo 1.97.1 (c980f4866 2026-06-30)'\n",
                "  exit 0\n",
                "fi\n",
                "if [ \"$*\" = \"clippy --locked -p app --target ",
                "wasm32-unknown-unknown -- -D warnings\" ]; then\n",
                "  {path_name}=${{{path_name}#*:}}\n",
                "  export {path_name}\n",
                "  exec cargo \"$@\"\n",
                "fi\n",
                "exit 0\n",
            ),
            cargo_log = cargo_log.display(),
            path_name = PROCESS_PATH,
        ),
    );
    let launcher = programs.join("verify");
    executable(
        &launcher,
        &format!(
            concat!(
                "#!/bin/sh\n",
                "{path_name}='{programs}':${{{path_name}}}\n",
                "export {path_name}\n",
                "exec '{xtask}' verify\n",
            ),
            path_name = PROCESS_PATH,
            programs = programs.display(),
            xtask = env!("CARGO_BIN_EXE_xtask"),
        ),
    );

    let output = Command::new(launcher)
        .current_dir(&fixture.repository)
        .output()
        .expect("launch verifier against the wasm lint fixture");

    assert_eq!(output.status.code(), Some(1), "{}", streams(&output));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("variable does not need to be mutable"),
        "{}",
        streams(&output),
    );
    let cargo_actions = fs::read_to_string(cargo_log).expect("read wasm lint Cargo actions");
    let cargo_actions = cargo_actions.lines().collect::<Vec<_>>();
    assert_eq!(
        cargo_actions
            .iter()
            .copied()
            .filter(|action| action.starts_with("clippy "))
            .collect::<Vec<_>>(),
        [
            "clippy --locked --workspace --all-targets -- -D warnings",
            "clippy --locked -p app --target wasm32-unknown-unknown -- -D warnings",
        ],
    );
    assert!(
        cargo_actions
            .iter()
            .all(|action| *action == "--version" || action.starts_with("clippy ")),
        "unexpected Cargo action while the battery ran concurrently: {cargo_actions:?}",
    );
    assert!(
        cargo_actions
            .iter()
            .copied()
            .filter(|action| *action == "--version")
            .count()
            <= 1,
        "the concurrent battery repeated its Cargo probe: {cargo_actions:?}",
    );
}

fn wasm_lint_fixture(repository: &Path) {
    fs::write(
        repository.join("Cargo.toml"),
        concat!(
            "[package]\n",
            "name = \"app\"\n",
            "version = \"0.0.0\"\n",
            "edition = \"2024\"\n",
        ),
    )
    .expect("write wasm lint manifest");
    fs::write(
        repository.join("Cargo.lock"),
        concat!(
            "version = 4\n\n",
            "[[package]]\n",
            "name = \"app\"\n",
            "version = \"0.0.0\"\n",
        ),
    )
    .expect("write wasm lint lock");
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask manifest has a workspace parent");
    fs::copy(
        workspace.join("rust-toolchain.toml"),
        repository.join("rust-toolchain.toml"),
    )
    .expect("copy the workspace wasm lint toolchain");
    fs::write(
        repository.join("src/lib.rs"),
        concat!(
            "#[cfg(target_arch = \"wasm32\")]\n",
            "pub fn wasm_only_lint() {\n",
            "    let mut value = 7;\n",
            "    assert_eq!(value, 7);\n",
            "}\n",
        ),
    )
    .expect("write wasm-only lint source");
    git(
        repository,
        &[
            "add",
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
            "src/lib.rs",
        ],
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
            repository.join("spice.toml"),
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
        fs::create_dir(repository.join("app")).expect("create verifier fixture app directory");
        fs::write(repository.join("app/index.html"), "<!doctype html>\n")
            .expect("write verifier fixture Canvas entry point");
        fs::write(repository.join("src/lib.rs"), "pub fn fixture() {}\n")
            .expect("write verifier fixture source");
        git(&repository, &["add", "."]);
        git(&repository, &["commit", "-qm", "fixture"]);
        Self { root, repository }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove verifier process fixture");
    }
}

fn executable(path: &Path, text: &str) {
    fs::write(path, text).expect("write process fixture executable");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .expect("make process fixture executable");
}

fn install_unset_hooks_git(programs: &Path) {
    executable(
        &programs.join("git"),
        concat!(
            "#!/bin/sh\n",
            "if [ \"$*\" = \"config --get core.hooksPath\" ]; then\n",
            "  exit 1\n",
            "fi\n",
            "exit 98\n",
        ),
    );
}

fn install_real_rustfmt(programs: &Path) {
    let output = Command::new("rustup")
        .args(["which", "rustfmt"])
        .output()
        .expect("query the active Rust toolchain for rustfmt");
    assert!(output.status.success(), "{}", streams(&output));
    let rustfmt = PathBuf::from(
        String::from_utf8(output.stdout)
            .expect("rustfmt path is UTF-8")
            .trim(),
    );
    assert!(
        rustfmt.is_file(),
        "rustfmt path exists: {}",
        rustfmt.display()
    );
    std::os::unix::fs::symlink(rustfmt, programs.join("rustfmt"))
        .expect("install the real rustfmt in the verifier fixture rim");
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
