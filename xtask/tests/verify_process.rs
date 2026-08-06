#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use gate::{BatteryArmResult, BatteryReport, GateId};

const PROCESS_PATH: &str = concat!("PA", "TH");

#[test]
fn verifier_process_reaches_one_rust_battery_without_forbidden_programs() {
    let fixture = Fixture::new();
    let programs = fixture.root.join("programs");
    fs::create_dir(&programs).expect("create process fixture directory");
    let forbidden_marker = fixture.root.join("forbidden-program-invoked");
    for program in ["python", "python3", "taskwarrior", "task"] {
        executable(
            &programs.join(program),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' '{}' >> '{}'\nexit 97\n",
                program,
                forbidden_marker.display()
            ),
        );
    }

    // The process under test is the real verifier binary. Its five Cargo build
    // and test actions and browser receipt are satisfied by logged fixtures so
    // this integration test does not recursively launch either toolchain from
    // inside Cargo. The following battery action is not replaceable and
    // executes through SystemRunner.
    let cargo_log = fixture.root.join("cargo-actions");
    executable(
        &programs.join("cargo"),
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
            cargo_log.display()
        ),
    );
    let browser_log = fixture.root.join("browser-action");
    executable(
        &programs.join("npm"),
        &format!(
            concat!(
                "#!/bin/sh\n",
                "printf '%s\\n' \"$*\" > '{}'\n",
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
        ),
    );

    let selected_result = run_selected_gate(&fixture, &programs, &cargo_log, &browser_log);

    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("verify")
        .current_dir(&fixture.repository)
        .env(PROCESS_PATH, &programs)
        .output()
        .expect("launch the real verifier process");

    assert_eq!(output.status.code(), Some(1), "{}", streams(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("battery refused publication"),
        "{}",
        streams(&output)
    );
    assert!(
        !forbidden_marker.exists(),
        "a forbidden program crossed the verifier rim: {}",
        forbidden_marker.display()
    );
    assert_eq!(
        fs::read_to_string(&cargo_log)
            .expect("read verifier Cargo actions")
            .lines()
            .take(5)
            .collect::<Vec<_>>(),
        [
            "fmt --all -- --check",
            "clippy --locked --workspace --all-targets -- -D warnings",
            "check --locked --workspace --target wasm32-unknown-unknown",
            "test --locked --workspace --all-targets",
            "test --locked --workspace --doc",
        ]
    );
    let browser = fs::read_to_string(&browser_log).expect("read verifier browser action");
    assert!(
        browser.starts_with("run test:canvas -- --spice-coverage-receipt "),
        "unexpected browser action: {browser:?}"
    );

    assert_full_battery(&output, &selected_result);
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

fn assert_full_battery(output: &std::process::Output, selected: &BatteryArmResult) {
    let reports = output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter_map(|line| BatteryReport::decode(line).ok())
        .collect::<Vec<_>>();
    assert_eq!(reports.len(), 1, "{}", streams(output));
    let report = &reports[0];
    assert_eq!(GateId::ALL.len(), 27);
    assert_eq!(report.results().len(), 27);
    assert_eq!(
        selected,
        report
            .results()
            .iter()
            .find(|result| result.gate_id() == GateId::EnvNameLedger)
            .expect("full battery contains env-name-ledger")
    );
    assert_eq!(
        report
            .results()
            .iter()
            .map(|result| result.gate_id())
            .collect::<Vec<_>>(),
        GateId::ALL
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout)
            .matches("[verify] canonical Rust gate battery")
            .count(),
        1
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout)
            .matches("[verify] npm run test:canvas (BROWSER-1kKSWhZk guard)")
            .count(),
        1
    );
}

#[test]
fn command_usage_names_the_single_gate_arm_surface() {
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .output()
        .expect("launch xtask usage");

    assert_eq!(output.status.code(), Some(2), "{}", streams(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("cargo xtask <verify|gate ARM|"),
        "{}",
        streams(&output)
    );
}

#[test]
fn zero_exit_browser_process_without_a_receipt_refuses_before_workspace_tests() {
    let fixture = Fixture::new();
    let programs = fixture.root.join("programs");
    fs::create_dir(&programs).expect("create process fixture directory");
    let cargo_log = fixture.root.join("cargo-actions");
    executable(
        &programs.join("cargo"),
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
            cargo_log.display()
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
    assert_eq!(
        fs::read_to_string(cargo_log)
            .expect("read verifier Cargo actions")
            .lines()
            .collect::<Vec<_>>(),
        [
            "fmt --all -- --check",
            "clippy --locked --workspace --all-targets -- -D warnings",
            "check --locked --workspace --target wasm32-unknown-unknown",
        ]
    );
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("canonical Rust gate battery"),
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
