#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use gate::{BatteryReport, GateId};

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

    // The process under test is the real verifier binary. Its six Cargo build
    // and test actions are satisfied by this logged fixture so this integration
    // test does not recursively launch Cargo from inside Cargo. The following
    // battery action is not replaceable and executes through SystemRunner.
    let cargo_log = fixture.root.join("cargo-actions");
    executable(
        &programs.join("cargo"),
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
            cargo_log.display()
        ),
    );

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

    let reports = output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter_map(|line| BatteryReport::decode(line).ok())
        .collect::<Vec<_>>();
    assert_eq!(reports.len(), 1, "{}", streams(&output));
    let report = &reports[0];
    assert_eq!(GateId::ALL.len(), 26);
    assert_eq!(report.results().len(), 26);
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
                "[[floor]]\nname = \"rust.long-lines\"\nvalue = 0\n\n",
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
