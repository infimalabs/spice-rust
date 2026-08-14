#![cfg(unix)]

use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use gate::{BatteryArmResult, BatteryReport, FocusedArmReport, GateId};

const PROCESS_PATH: &str = concat!("PA", "TH");
const FORMAT_ACTION: &str =
    "rustfmt --edition <workspace> --check --files-with-diff <gate::formatter_sources>";
const WORKSPACE_TEST_ACTION: &str =
    "deterministic sharded cargo test --locked --workspace --all-targets";
const STREAMING_FIXTURE_TIMEOUT: Duration = Duration::from_secs(15);

#[path = "verify_process/browser_refusal.rs"]
mod browser_refusal;
#[path = "verify_process/structural_preflight.rs"]
mod structural_preflight;
#[path = "verify_process/wasm_clippy.rs"]
mod wasm_clippy;

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
    install_real_git(&programs);
    install_real_rustfmt(&programs);
    let cargo_marker = fixture.root.join("cargo-invoked");
    executable(
        &programs.join("cargo"),
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\nexit 0\n",
            cargo_marker.display()
        ),
    );

    let output = Command::new(fixture.xtask())
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
        fs::read_to_string(&cargo_marker)
            .is_ok_and(|action| action == "build --locked -p spice --bin spice\n"),
        "the verifier crossed its formatter refusal into another Cargo action"
    );
}

#[test]
fn verifier_streams_progress_before_exit_and_emits_every_final_byte_once() {
    let fixture = Fixture::new();
    let programs = install_full_verifier_programs(&fixture);
    let entered = fixture.root.join("streaming-rustfmt-entered");
    let release = fixture.root.join("streaming-rustfmt-release");
    install_held_rustfmt(&programs.directory, &entered, &release);

    let mut child = Command::new(fixture.xtask())
        .arg("verify")
        .current_dir(&fixture.repository)
        .env(PROCESS_PATH, &programs.directory)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("launch verifier with observable output pipes");
    let stdout = child.stdout.take().expect("verifier stdout is piped");
    let stderr = child.stderr.take().expect("verifier stderr is piped");
    let (first_record, observed_first_record) = mpsc::sync_channel(1);
    let stdout_reader = thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut first = Vec::new();
        reader
            .read_until(b'\n', &mut first)
            .expect("read first live verifier record");
        first_record
            .send(first.clone())
            .expect("publish first live verifier record");
        let mut complete = first;
        reader
            .read_to_end(&mut complete)
            .expect("drain complete verifier stdout");
        complete
    });
    let stderr_reader = thread::spawn(move || {
        let mut complete = Vec::new();
        BufReader::new(stderr)
            .read_to_end(&mut complete)
            .expect("drain complete verifier stderr");
        complete
    });

    let first = observed_first_record
        .recv_timeout(STREAMING_FIXTURE_TIMEOUT)
        .expect("the scheduler record is visible before the held action exits");
    wait_for_file(&entered);
    let was_running = child
        .try_wait()
        .expect("observe held verifier process")
        .is_none();
    fs::write(&release, b"release\n").expect("release the held formatter action");
    let status = child.wait().expect("wait for streamed verifier completion");
    let stdout = stdout_reader.join().expect("stdout reader joins");
    let stderr = stderr_reader.join().expect("stderr reader joins");
    let process_output = std::process::Output {
        status,
        stdout,
        stderr,
    };
    assert_streamed_verifier(&first, was_running, &process_output);
    assert_full_verifier_action_trace(&programs, &fixture.repository, &process_output);
}

fn install_held_rustfmt(programs: &Path, entered: &Path, release: &Path) {
    fs::remove_file(programs.join("rustfmt")).expect("remove the real rustfmt fixture symlink");
    executable(
        &programs.join("rustfmt"),
        &format!(
            concat!(
                "#!/bin/sh\n",
                "if [ \"$*\" = \"--version\" ]; then\n",
                "  printf '%s\\n' 'rustfmt 1.9.0-stable (8bab26f4f6 2026-07-14)'\n",
                "  exit 0\n",
                "fi\n",
                "printf 'entered\\n' > '{}'\n",
                "attempt=0\n",
                "while [ ! -e '{}' ]; do\n",
                "  if [ \"$attempt\" -ge 500 ]; then exit 95; fi\n",
                "  attempt=$((attempt + 1))\n",
                "  /bin/sleep 0.01\n",
                "done\n",
                "printf 'fixture-action-output\\n'\n",
                "exit 0\n",
            ),
            entered.display(),
            release.display(),
        ),
    );
}

fn assert_streamed_verifier(first: &[u8], was_running: bool, output: &std::process::Output) {
    let first = String::from_utf8_lossy(first);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        first.starts_with("[verify] scheduler workers="),
        "unexpected first live record: {first:?}"
    );
    assert!(
        was_running,
        "the first record was buffered until process exit"
    );
    assert!(output.status.success(), "{}", streams(output));
    assert_eq!(stdout.matches("[verify] scheduler workers=").count(), 1);
    assert_eq!(stdout.matches("fixture-action-output").count(), 1);
    assert_eq!(
        stdout
            .lines()
            .filter(|line| line.contains("stage=terminal ordinal=13/13"))
            .count(),
        1,
        "contract landing is absent or duplicated: {stdout}"
    );
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
    let expected_binary = expected_resident_binary(repository);
    assert!(
        browser.contains(&format!("binary={}\n", expected_binary.display())),
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

#[expect(
    clippy::disallowed_methods,
    reason = "the process fixture mirrors Cargo's test-binary target directory environment"
)]
fn expected_resident_binary(repository: &Path) -> PathBuf {
    let repository = repository
        .canonicalize()
        .expect("canonical browser repository");
    let target = match env::var_os("CARGO_TARGET_DIR") {
        Some(target) if Path::new(&target).is_absolute() => PathBuf::from(target),
        Some(target) => repository.join(target),
        None => repository.join("target"),
    };
    target
        .join("debug")
        .join(format!("spice{}", env::consts::EXE_SUFFIX))
}

fn install_full_verifier_programs(fixture: &Fixture) -> FullVerifierPrograms {
    let directory = fixture.root.join("programs");
    fs::create_dir(&directory).expect("create process fixture directory");
    install_real_git(&directory);
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
    install_cargo_with_metadata(&directory, &cargo_log, &fixture.repository);
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

fn install_cargo_with_metadata(programs: &Path, cargo_log: &Path, _repository: &Path) {
    let metadata = serde_json::json!({
        "packages": [{
            "id": "verify-process-fixture-id",
            "name": "verify-process-fixture",
            "manifest_path": "Cargo.toml",
            "targets": [{
                "name": "verify_process_fixture",
                "kind": ["lib"],
                "src_path": "src/lib.rs",
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
            .take(9)
            .collect::<Vec<_>>(),
        [
            "build --locked -p spice --bin spice",
            "clippy --locked --workspace --all-targets -- -D warnings",
            "clippy --locked -p app --target wasm32-unknown-unknown -- -D warnings",
            "check --locked --workspace --target wasm32-unknown-unknown",
            app_action.as_str(),
            "test --locked --workspace --all-targets --no-run",
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
    let default_output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["gate", "env-name-ledger"])
        .current_dir(&fixture.repository)
        .env(PROCESS_PATH, programs)
        .output()
        .expect("launch one real gate arm with human output");
    assert!(
        default_output.status.success(),
        "{}",
        streams(&default_output)
    );
    assert!(
        default_output.stdout.is_empty(),
        "an admitting arm needs no human diagnostic: {}",
        streams(&default_output)
    );

    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["gate", "--report", "env-name-ledger"])
        .current_dir(&fixture.repository)
        .env(PROCESS_PATH, programs)
        .output()
        .expect("launch one real gate arm with machine output");
    assert!(output.status.success(), "{}", streams(&output));
    let document = output
        .stdout
        .strip_suffix(b"\n")
        .expect("machine report carries one terminal newline");
    let report = FocusedArmReport::decode(document)
        .expect("explicit report mode emits one canonical focused report");
    let result = report.result().clone();
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
    let default_output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["gate", "file-shape"])
        .current_dir(&fixture.repository)
        .env(PROCESS_PATH, programs)
        .output()
        .expect("launch the refusing file-shape gate arm with human output");
    assert_eq!(
        default_output.status.code(),
        Some(1),
        "{}",
        streams(&default_output)
    );
    assert!(
        default_output.stdout.len() <= gate::BATTERY_REFUSAL_FOLD_MAX_BYTES,
        "focused diagnostic exceeded its bound: {}",
        streams(&default_output)
    );
    let default_stdout = String::from_utf8_lossy(&default_output.stdout);
    assert!(default_stdout.contains("gate=file-shape state=failed"));
    assert!(default_stdout.contains("full-evidence: cargo xtask gate --report file-shape"));
    assert!(
        FocusedArmReport::decode(
            default_output
                .stdout
                .strip_suffix(b"\n")
                .unwrap_or_default()
        )
        .is_err(),
        "ordinary refusal unexpectedly emitted its canonical machine report"
    );

    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["gate", "--report", "file-shape"])
        .current_dir(&fixture.repository)
        .env(PROCESS_PATH, programs)
        .output()
        .expect("launch the refusing file-shape gate arm with machine output");
    assert_eq!(output.status.code(), Some(1), "{}", streams(&output));
    let lines = output
        .stdout
        .split(|byte| *byte == b'\n')
        .collect::<Vec<_>>();
    assert_eq!(
        lines.iter().filter(|line| !line.is_empty()).count(),
        1,
        "explicit report mode emits exactly one machine row: {}",
        streams(&output)
    );
    let report = FocusedArmReport::decode(lines[0])
        .expect("refusing selected arm emits a canonical focused report");
    let result = report.result().clone();
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
            .any(|line| { line == "check --locked --package app --target wasm32-unknown-unknown" }),
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
            .contains("run `cargo xtask help` for accepted gate arms"),
        "{}",
        streams(&output)
    );
}

struct Fixture {
    root: PathBuf,
    repository: PathBuf,
    xtask: PathBuf,
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
        fs::create_dir(repository.join("app")).expect("create verifier fixture app directory");
        fs::write(repository.join("app/index.html"), "<!doctype html>\n")
            .expect("write verifier fixture Canvas entry point");
        fs::write(repository.join("src/lib.rs"), "pub fn fixture() {}\n")
            .expect("write verifier fixture source");
        git(&repository, &["add", "."]);
        git(&repository, &["commit", "-qm", "fixture"]);
        let binaries = root.join("current-build");
        fs::create_dir(&binaries).expect("create fixture current-build directory");
        let xtask = binaries.join(format!("xtask{}", env::consts::EXE_SUFFIX));
        let current_spice = binaries.join(format!("spice{}", env::consts::EXE_SUFFIX));
        fs::copy(env!("CARGO_BIN_EXE_xtask"), &xtask).expect("copy fixture xtask executable");
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
        Self {
            root,
            repository,
            xtask,
        }
    }

    fn xtask(&self) -> &Path {
        &self.xtask
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

fn install_real_git(programs: &Path) {
    std::os::unix::fs::symlink("/usr/bin/git", programs.join("git"))
        .expect("install real Git in the verifier fixture rim");
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

fn wait_for_file(path: &Path) {
    let started = Instant::now();
    while !path.is_file() {
        assert!(
            started.elapsed() < STREAMING_FIXTURE_TIMEOUT,
            "fixture did not publish {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(5));
    }
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
