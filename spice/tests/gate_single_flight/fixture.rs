use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::tool::write_tool;

pub(super) struct Fixture {
    root: PathBuf,
    pub(super) older: PathBuf,
    pub(super) newer: PathBuf,
    tools: PathBuf,
    pub(super) executions: PathBuf,
    release: PathBuf,
    pub(super) move_index: PathBuf,
    pub(super) move_tool: PathBuf,
    pub(super) fail_execution: PathBuf,
}

impl Fixture {
    pub(super) fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-gate-single-flight-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple(),
        ));
        let newer = root.join("newer");
        let older = root.join("older");
        let tools = root.join("tools");
        let executions = root.join("executions.log");
        let release = root.join("release");
        let move_index = root.join("move-index");
        let move_tool = root.join("move-tool");
        let fail_execution = root.join("fail-execution");
        fs::create_dir_all(newer.join("src")).expect("create newer worktree");
        fs::create_dir(&tools).expect("create tool directory");
        initialize_repository(&newer);
        add_linked_candidate(&newer, &older);
        let fixture = Self {
            root,
            older,
            newer,
            tools,
            executions,
            release,
            move_index,
            move_tool,
            fail_execution,
        };
        fixture.rewrite_tool("");
        fixture
    }

    pub(super) fn prove(&self, worktree: &Path) -> std::process::Child {
        self.prove_action(worktree, "formatters")
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "the production-path test rim extends its captured tool search path"
    )]
    fn prove_action(&self, worktree: &Path, action: &str) -> std::process::Child {
        let path = std::env::join_paths(std::iter::once(self.tools.clone()).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .expect("join proof PATH");
        Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(["--json", "gate", "prove", action])
            .current_dir(worktree)
            .env("PATH", path)
            .env_remove(wire::SpiceVariable::Lane.as_str())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn proof request")
    }

    pub(super) fn prove_output(&self, worktree: &Path) -> Output {
        self.prove(worktree)
            .wait_with_output()
            .expect("proof request completes")
    }

    pub(super) fn prove_action_output(&self, worktree: &Path, action: &str) -> Output {
        self.prove_action(worktree, action)
            .wait_with_output()
            .expect("proof request completes")
    }

    pub(super) fn stage_value(&self, worktree: &Path, value: u8) {
        fs::write(
            worktree.join("src/lib.rs"),
            format!("pub fn value() -> u8 {{ {value} }}\n"),
        )
        .expect("write distinct candidate source");
        git(worktree, &["add", "--all"]);
    }

    pub(super) fn release(&self) {
        fs::write(&self.release, b"release\n").expect("release instrumented formatter");
    }

    pub(super) fn wait_for_execution_starts(&self, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while execution_starts(&self.executions) < expected {
            assert!(
                Instant::now() < deadline,
                "formatter did not reach deterministic start barrier {expected}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    pub(super) fn rewrite_tool(&self, suffix: &str) {
        let rustfmt = self.tools.join("rustfmt");
        write_tool(
            &rustfmt,
            &self.executions,
            &self.move_index,
            &self.newer,
            &self.move_tool,
            &self.release,
            &self.fail_execution,
            suffix,
        );
    }

    pub(super) fn action_index_count(&self) -> usize {
        let directory = self.newer.join(".git/.spice-rs/gate-evidence/v1/actions");
        match fs::read_dir(directory) {
            Ok(entries) => entries.count(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => panic!("read evidence action indexes: {error}"),
        }
    }
}

fn initialize_repository(newer: &Path) {
    git(newer, &["init", "--initial-branch=main"]);
    git(newer, &["config", "user.name", "Spice Test"]);
    git(newer, &["config", "user.email", "spice@example.invalid"]);
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
    git(newer, &["add", "--all"]);
    git(newer, &["commit", "-m", "opening"]);
}

fn add_linked_candidate(newer: &Path, older: &Path) {
    git(
        newer,
        &[
            "worktree",
            "add",
            "-b",
            "older",
            older.to_str().expect("older path is UTF-8"),
            "HEAD",
        ],
    );
    fs::write(newer.join("unrelated.txt"), "newer commit\n").expect("write unrelated newer input");
    git(newer, &["add", "--all"]);
    git(newer, &["commit", "-m", "newer unrelated input"]);
    for worktree in [newer, older] {
        fs::write(worktree.join("src/lib.rs"), "pub fn value() -> u8 { 2 }\n")
            .expect("write identical candidate source");
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

pub(super) fn streams(output: &Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

pub(super) fn execution_starts(path: &Path) -> usize {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| *line == "start")
        .count()
}

pub(super) fn proof_action(output: &Output) -> serde_json::Value {
    let body: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("proof output is JSON");
    body["body"]["actions"][0].clone()
}
