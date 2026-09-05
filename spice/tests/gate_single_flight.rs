#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

#[test]
fn identical_actions_from_different_commits_execute_once() {
    let fixture = Fixture::new();
    let older = fixture.prove(&fixture.older);
    let newer = fixture.prove(&fixture.newer);
    let older = older.wait_with_output().expect("older proof completes");
    let newer = newer.wait_with_output().expect("newer proof completes");
    assert!(
        older.status.success(),
        "older proof failed: {}",
        streams(&older)
    );
    assert!(
        newer.status.success(),
        "newer proof failed: {}",
        streams(&newer)
    );
    let older_body: serde_json::Value = serde_json::from_slice(&older.stdout).unwrap();
    let newer_body: serde_json::Value = serde_json::from_slice(&newer.stdout).unwrap();
    assert_ne!(older_body["body"]["source"], newer_body["body"]["source"]);
    assert_eq!(
        older_body["body"]["actions"][0]["action_key"],
        newer_body["body"]["actions"][0]["action_key"]
    );
    assert_eq!(
        older_body["body"]["actions"][0]["receipt"],
        newer_body["body"]["actions"][0]["receipt"]
    );
    let mut dispositions = [
        older_body["body"]["actions"][0]["disposition"]
            .as_str()
            .unwrap(),
        newer_body["body"]["actions"][0]["disposition"]
            .as_str()
            .unwrap(),
    ];
    dispositions.sort_unstable();
    assert_eq!(dispositions, ["present", "published"]);
    assert_eq!(execution_starts(&fixture.executions), 1);

    let replay = fixture
        .prove(&fixture.older)
        .wait_with_output()
        .expect("replayed proof completes");
    assert!(
        replay.status.success(),
        "replay failed: {}",
        streams(&replay)
    );
    let replay_body: serde_json::Value = serde_json::from_slice(&replay.stdout).unwrap();
    assert_eq!(replay_body["body"]["actions"][0]["disposition"], "present");
    assert_eq!(execution_starts(&fixture.executions), 1);
}

#[test]
fn distinct_action_keys_execute_in_parallel() {
    let fixture = Fixture::new();
    fixture.stage_value(&fixture.older, 3);
    fixture.stage_value(&fixture.newer, 4);
    let older = fixture.prove(&fixture.older);
    let newer = fixture.prove(&fixture.newer);
    let older = older.wait_with_output().expect("older proof completes");
    let newer = newer.wait_with_output().expect("newer proof completes");
    assert!(
        older.status.success(),
        "older proof failed: {}",
        streams(&older)
    );
    assert!(
        newer.status.success(),
        "newer proof failed: {}",
        streams(&newer)
    );
    let lines = fs::read_to_string(&fixture.executions).expect("read formatter executions");
    assert_eq!(
        lines.lines().take(2).collect::<Vec<_>>(),
        ["start", "start"]
    );
    assert_eq!(execution_starts(&fixture.executions), 2);
}

struct Fixture {
    root: PathBuf,
    older: PathBuf,
    newer: PathBuf,
    tools: PathBuf,
    executions: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-gate-single-flight-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple(),
        ));
        let newer = root.join("newer");
        let older = root.join("older");
        let tools = root.join("tools");
        let executions = root.join("executions.log");
        fs::create_dir_all(newer.join("src")).expect("create newer worktree");
        fs::create_dir(&tools).expect("create tool directory");
        git(&newer, &["init", "--initial-branch=main"]);
        git(&newer, &["config", "user.name", "Spice Test"]);
        git(&newer, &["config", "user.email", "spice@example.invalid"]);
        fs::write(
            newer.join("spice-rust.toml"),
            format!("{}[gate.walk]\nskip = []\n", gate::NARROWEST_FILE_CLASSES),
        )
        .expect("write gate policy");
        fs::write(
            newer.join("rust-toolchain.toml"),
            "[toolchain]\nchannel = \"1.97.1\"\ncomponents = [\"rustfmt\"]\n",
        )
        .expect("write toolchain policy");
        fs::write(
            newer.join("Cargo.toml"),
            "[package]\nname = \"flight-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )
        .expect("write Cargo manifest");
        fs::write(newer.join("src/lib.rs"), "pub fn value() -> u8 { 1 }\n")
            .expect("write baseline source");
        git(&newer, &["add", "--all"]);
        git(&newer, &["commit", "-m", "opening"]);
        git(
            &newer,
            &[
                "worktree",
                "add",
                "-b",
                "older",
                older.to_str().expect("older path is UTF-8"),
                "HEAD",
            ],
        );
        fs::write(newer.join("unrelated.txt"), "newer commit\n")
            .expect("write unrelated newer input");
        git(&newer, &["add", "--all"]);
        git(&newer, &["commit", "-m", "newer unrelated input"]);
        for worktree in [&newer, &older] {
            fs::write(worktree.join("src/lib.rs"), "pub fn value() -> u8 { 2 }\n")
                .expect("write identical candidate source");
            git(worktree, &["add", "--all"]);
        }
        let rustfmt = tools.join("rustfmt");
        fs::write(
            &rustfmt,
            format!(
                "#!/bin/sh\nif [ \"${{1-}}\" = --version ]; then\n  printf '%s\\n' 'rustfmt 1.9.0-stable (8bab26f4f6 2026-07-14)'\n  exit 0\nfi\nprintf 'start\\n' >> '{}'\n/bin/sleep 1\nprintf 'end\\n' >> '{}'\n",
                executions.display(),
                executions.display(),
            ),
        )
        .expect("write instrumented rustfmt");
        fs::set_permissions(&rustfmt, fs::Permissions::from_mode(0o700))
            .expect("make instrumented rustfmt executable");
        Self {
            root,
            older,
            newer,
            tools,
            executions,
        }
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "the production-path test rim extends its captured tool search path"
    )]
    fn prove(&self, worktree: &Path) -> std::process::Child {
        let path = std::env::join_paths(std::iter::once(self.tools.clone()).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .expect("join proof PATH");
        Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(["--json", "gate", "prove", "formatters"])
            .current_dir(worktree)
            .env("PATH", path)
            .env_remove(wire::SpiceVariable::Lane.as_str())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn proof request")
    }

    fn stage_value(&self, worktree: &Path, value: u8) {
        fs::write(
            worktree.join("src/lib.rs"),
            format!("pub fn value() -> u8 {{ {value} }}\n"),
        )
        .expect("write distinct candidate source");
        git(worktree, &["add", "--all"]);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove single-flight fixture");
    }
}

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("/usr/bin/git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("run fixture Git command");
    assert!(output.status.success(), "Git failed: {}", streams(&output));
}

fn streams(output: &Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

fn execution_starts(path: &Path) -> usize {
    fs::read_to_string(path)
        .expect("read formatter executions")
        .lines()
        .filter(|line| *line == "start")
        .count()
}
