#![cfg(unix)]

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use wire::Outcome;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

#[test]
fn retired_release_one_root_refuses_before_current_spice_starts() {
    let root = std::env::temp_dir().join(format!(
        "spice-retired-state-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
    ));
    fs::create_dir(&root).expect("create exclusive fixture");
    let initialized = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&root)
        .status()
        .expect("Git launches for fixture");
    assert!(initialized.success(), "Git initializes fixture");

    let retired = root.join(".git/.spice-rust");
    fs::create_dir(&retired).expect("create retired release-one root");
    fs::write(retired.join("install.json"), b"release-one evidence\n")
        .expect("plant retired evidence");

    let output = Command::new(env!("CARGO_BIN_EXE_spice"))
        .args(["--json", "version"])
        .current_dir(&root)
        .output()
        .expect("current Spice launches");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let outcome: Outcome<serde_json::Value> =
        wire::decode(&output.stderr).expect("state refusal is one typed outcome");
    let Outcome::Refusal(refusal) = outcome else {
        panic!("retired state did not emit a refusal: {outcome:?}");
    };
    assert_eq!(refusal.reason, "repository::state-path-refused");
    let observed = refusal.observed.expect("refusal names observation");
    assert!(observed.contains("release=release-one"));
    assert!(observed.contains(&retired.display().to_string()));
    assert!(observed.contains("archive or remove"));
    assert_eq!(
        fs::read(retired.join("install.json")).unwrap(),
        b"release-one evidence\n"
    );
    assert!(!root.join(".git/.spice").exists());

    fs::remove_dir_all(&root).expect("remove fixture");
}
