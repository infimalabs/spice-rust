#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

#[test]
fn selection_is_one_typed_public_leaf_with_exact_arity() {
    let contract = spice::cli::contracts()
        .iter()
        .find(|contract| contract.path == ["gate", "selection"])
        .expect("selection contract is registered");
    assert_eq!(contract.syntax, "gate selection");

    let help = invoke(Path::new("."), &["gate", "selection", "--help"]);
    assert!(help.status.success(), "{}", streams(&help));
    assert!(
        String::from_utf8_lossy(&help.stdout).contains("Usage: spice gate selection"),
        "{}",
        streams(&help),
    );
    let extra = invoke(Path::new("."), &["gate", "selection", "extra"]);
    assert_eq!(extra.status.code(), Some(2), "{}", streams(&extra));
}

#[test]
fn selection_requires_each_strict_tracked_input_before_selected_work() {
    for tracked_input in ["spice-rust.toml", "baseline.toml"] {
        let fixture = Fixture::new();
        fs::remove_file(fixture.repository.join(tracked_input)).expect("remove strict input");
        git(&fixture.repository, &["add", "--update", tracked_input]);
        let programs = fixture.root.join("programs");
        fs::create_dir(&programs).expect("create fixture program directory");
        let cargo_marker = fixture.root.join("cargo-invoked");
        executable(
            &programs.join("cargo"),
            &format!(
                "#!/bin/sh\nprintf invoked > '{}'\nexit 91\n",
                cargo_marker.display(),
            ),
        );
        let path =
            std::env::join_paths([programs, PathBuf::from("/usr/bin"), PathBuf::from("/bin")])
                .expect("compose fixture path");

        let output = Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(["gate", "selection"])
            .current_dir(&fixture.repository)
            .env("PATH", path)
            .output()
            .expect("run strict source selection");

        assert_eq!(output.status.code(), Some(2), "{}", streams(&output));
        assert!(output.stdout.is_empty(), "{}", streams(&output));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("gate::contract-refused"), "{stderr}");
        assert!(stderr.contains(tracked_input), "{stderr}");
        assert!(
            !cargo_marker.exists(),
            "missing input reached selected work"
        );
    }
}

struct Fixture {
    root: PathBuf,
    repository: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "spice-gate-selection-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        let repository = root.join("repository");
        fs::create_dir_all(repository.join("src")).expect("create fixture repository");
        git(&repository, &["init", "-q"]);
        git(
            &repository,
            &["config", "user.email", "selection@example.invalid"],
        );
        git(&repository, &["config", "user.name", "Selection Fixture"]);
        fs::write(
            repository.join("spice-rust.toml"),
            format!("{}[gate.walk]\nskip = []\n", gate::NARROWEST_FILE_CLASSES),
        )
        .expect("write fixture policy");
        fs::write(
            repository.join("baseline.toml"),
            "version = 1\n\n[[floor]]\nname = \"rust.mutation-survivors\"\nvalue = 0\n",
        )
        .expect("write fixture baseline");
        fs::write(
            repository.join("Cargo.toml"),
            "[package]\nname = \"selection-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )
        .expect("write fixture manifest");
        fs::write(repository.join("src/lib.rs"), "pub fn fixture() {}\n")
            .expect("write fixture source");
        git(&repository, &["add", "."]);
        git(&repository, &["commit", "-qm", "fixture"]);
        Self { root, repository }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove selection fixture");
    }
}

fn invoke(root: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_spice"))
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("run Spice selection CLI")
}

fn executable(path: &Path, text: &str) {
    fs::write(path, text).expect("write fixture executable");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("make fixture executable");
}

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("/usr/bin/git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("run fixture Git");
    assert!(output.status.success(), "{}", streams(&output));
}

fn streams(output: &Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}
