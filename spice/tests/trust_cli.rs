#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use serde::de::DeserializeOwned;
use wire::intent::TrustAnswer;

#[path = "support/fixture_root.rs"]
mod fixture_root;
#[path = "support/git_repository.rs"]
mod git_repository;
#[path = "support/readiness.rs"]
mod readiness;
#[path = "support/staged_app.rs"]
mod staged_app;

use readiness::Readiness;

struct Fixture {
    root: PathBuf,
    main: PathBuf,
    linked: PathBuf,
    canvas: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = fixture_root::claim(&std::env::temp_dir(), "trust-worktree-process");
        let main = root.join("main");
        let linked = root.join("linked");
        fs::create_dir(&main).expect("create main fixture worktree");
        git_repository::link(&main, &linked, "linked");
        staged_app::seed(&main);
        fs::write(main.join("spice-rust.toml"), "# main executable policy\n")
            .expect("write main policy");
        fs::write(
            linked.join("spice-rust.toml"),
            "# linked executable policy\n",
        )
        .expect("write linked policy");
        let main = fs::canonicalize(main).expect("canonicalize main worktree");
        let linked = fs::canonicalize(linked).expect("canonicalize linked worktree");
        let canvas = main.join(".git/.spice-rs/canvas");
        Self {
            root,
            main,
            linked,
            canvas,
        }
    }

    fn invoke(&self, worktree: &Path, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(arguments)
            .current_dir(worktree)
            .env_remove(spice::agent_launch::SPICE_RS_LANE)
            .output()
            .expect("spice trust command launches")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove trust fixture");
    }
}

struct Resident {
    child: Option<Child>,
}

impl Resident {
    fn start(fixture: &Fixture) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(["--json", "up", "--foreground", "--port", "0"])
            .current_dir(&fixture.main)
            .env_remove(spice::agent_launch::SPICE_RS_LANE)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("shared resident starts");
        let mut resident = Self { child: Some(child) };
        let watched = fixture
            .canvas
            .parent()
            .expect("canvas has a runtime parent");
        fs::create_dir_all(watched).expect("runtime directory exists");
        match readiness::wait_until_ready(
            resident.child.as_mut().expect("resident child"),
            watched,
            readiness::ARRIVAL_HORIZON,
            "shared resident discovery",
            || fixture.canvas.is_file(),
        ) {
            Readiness::Ready => resident,
            Readiness::Exited => panic!("shared resident exited before readiness"),
        }
    }

    fn stop(mut self, fixture: &Fixture) {
        let down = fixture.invoke(&fixture.main, &["--json", "down"]);
        assert!(down.status.success(), "down failed: {}", spoken(&down));
        let output = self
            .child
            .take()
            .expect("resident child")
            .wait_with_output()
            .expect("resident exits");
        assert!(
            output.status.success(),
            "resident failed: {}",
            spoken(&output)
        );
    }
}

impl Drop for Resident {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn linked_worktree_rulings_use_the_producer_across_resident_restart() {
    let fixture = Fixture::new();
    let resident = Resident::start(&fixture);

    let pended: TrustAnswer = reply(&fixture.invoke(&fixture.linked, &["trust", "pend"]));
    assert_eq!(pended.standing, wire::TrustState::Pended);
    let digest = pended.digest.to_string();
    let granted: TrustAnswer =
        reply(&fixture.invoke(&fixture.linked, &["trust", "grant", &digest]));
    assert_eq!(granted.digest, pended.digest);
    assert_eq!(granted.standing, wire::TrustState::Granted);

    resident.stop(&fixture);
    let restarted = Resident::start(&fixture);
    let revoked: TrustAnswer =
        reply(&fixture.invoke(&fixture.linked, &["trust", "revoke", &digest]));
    assert_eq!(revoked.digest, pended.digest);
    assert_eq!(revoked.standing, wire::TrustState::Revoked);
    let report: serde_json::Value = reply(&fixture.invoke(&fixture.linked, &["trust"]));
    assert_eq!(report["digest"], digest);
    assert_eq!(report["standing"], "revoked");
    restarted.stop(&fixture);
}

fn reply<T: DeserializeOwned>(output: &Output) -> T {
    assert!(
        output.status.success(),
        "command failed: {}",
        spoken(output)
    );
    wire::decode(&output.stdout).expect("command reply is typed")
}

fn spoken(output: &Output) -> String {
    format!(
        "status={:?} stdout={} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}
