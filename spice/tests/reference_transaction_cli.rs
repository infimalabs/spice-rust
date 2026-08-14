#![cfg(unix)]

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use spice::install::{apply, installed_git_hooks, plan};
use spice::repository::Repository;

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);
const USAGE: &str = "usage: spice gate reference-transaction <prepared|committed|aborted>\n";

#[test]
fn committed_and_aborted_are_inert_before_repository_access() {
    let scratch = Scratch::directory("inert-states");
    let marker = scratch.join("git-entered");
    let git_script = format!("#!/bin/sh\n: >'{}'\nexit 91\n", marker.display());
    let git = scratch.executable("bin/git", git_script.as_bytes());
    let path = git.parent().expect("sentinel Git has a parent");
    for state in ["committed", "aborted"] {
        let output = Command::new(spice())
            .args(["gate", "reference-transaction", state])
            .current_dir(scratch.path())
            .env("PATH", path)
            .output()
            .expect("run inert transaction state");
        assert_success(&output, state);
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
    assert!(!marker.exists(), "an inert state launched Git");
}

#[test]
fn every_parser_admitted_json_scope_still_uses_the_early_hook_path() {
    let scratch = Scratch::directory("json-inert-state");
    let marker = scratch.join("git-entered");
    let git_script = format!("#!/bin/sh\n: >'{}'\nexit 91\n", marker.display());
    let git = scratch.executable("bin/git", git_script.as_bytes());
    for arguments in [
        &["--json", "gate", "reference-transaction", "committed"][..],
        &["gate", "--json", "reference-transaction", "aborted"],
        &["gate", "reference-transaction", "committed", "--json"],
        &[
            "--json",
            "gate",
            "--json",
            "reference-transaction",
            "aborted",
            "--json",
        ],
    ] {
        let output = Command::new(spice())
            .args(arguments)
            .current_dir(scratch.path())
            .env("PATH", git.parent().expect("sentinel Git has a parent"))
            .output()
            .expect("run JSON inert transaction state");
        assert_success(&output, "JSON inert state");
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
    assert!(!marker.exists(), "a JSON inert state launched Git");
}

#[test]
fn parser_rejected_duplicate_json_never_reaches_hook_or_dispatch_gap() {
    let scratch = Scratch::directory("duplicate-json");
    let output = Command::new(spice())
        .args([
            "--json",
            "--json",
            "gate",
            "reference-transaction",
            "committed",
        ])
        .current_dir(scratch.path())
        .output()
        .expect("run parser-rejected duplicate JSON flags");

    assert_eq!(output.status.code(), Some(2), "{}", streams(&output));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("parser diagnostic is UTF-8");
    assert!(stderr.contains("the argument '--json' cannot be used multiple times"));
    assert!(!stderr.contains("release::dispatch-gap"));
}

#[test]
fn generated_gate_help_exposes_the_hook_only_leaf() {
    let scratch = Scratch::repository("generated-help");
    let output = Command::new(spice())
        .args(["gate", "--help"])
        .current_dir(scratch.path())
        .output()
        .expect("render generated gate help");
    assert_success(&output, "generated gate help");
    assert!(output.stderr.is_empty());
    let help = String::from_utf8(output.stdout).expect("gate help is UTF-8");
    assert!(help.contains("reference-transaction"), "{help}");
}

#[test]
fn prepared_queue_naming_no_branch_launches_no_git() {
    let scratch = Scratch::directory("non-branch-queue");
    let marker = scratch.join("git-entered");
    let git_script = format!("#!/bin/sh\n: >'{}'\nexit 91\n", marker.display());
    let git = scratch.executable("bin/git", git_script.as_bytes());
    let path = git.parent().expect("sentinel Git has a parent");
    for arguments in [
        &["gate", "reference-transaction", "prepared"][..],
        &["--json", "gate", "reference-transaction", "prepared"],
        &["gate", "reference-transaction", "prepared", "--json"],
        &[
            "--json",
            "gate",
            "--json",
            "reference-transaction",
            "prepared",
            "--json",
        ],
    ] {
        let output = invoke_prepared_arguments(
            scratch.path(),
            "old new AUTO_MERGE\n",
            Some(path),
            arguments,
        );
        assert_success(&output, "non-branch prepared queue");
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
    assert!(!marker.exists(), "a non-branch prepared queue launched Git");
}

#[test]
fn exact_cli_rejects_unknown_missing_and_extra_state_arguments() {
    let scratch = Scratch::directory("exact-cli");
    for arguments in [
        vec!["gate", "reference-transaction"],
        vec!["gate", "reference-transaction", "unknown"],
        vec!["gate", "reference-transaction", "prepared", "extra"],
    ] {
        let output = Command::new(spice())
            .args(&arguments)
            .current_dir(scratch.path())
            .output()
            .expect("run malformed reference-transaction CLI");
        assert_eq!(output.status.code(), Some(2), "{}", streams(&output));
        assert!(output.stdout.is_empty());
        assert_eq!(String::from_utf8(output.stderr).unwrap(), USAGE);
    }
}

#[test]
fn prepared_reads_stdin_and_names_the_first_malformed_line() {
    let scratch = Scratch::directory("malformed-input");
    let output = invoke_prepared(scratch.path(), "old new refs/heads/main\nmalformed\n");
    assert_eq!(output.status.code(), Some(1), "{}", streams(&output));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(
        diagnostic.contains("malformed reference-transaction line 2"),
        "{diagnostic}"
    );
}

#[test]
fn installed_hook_reaches_installed_spice_and_refuses_protected_history() {
    let scratch = Scratch::repository("installed-product");
    let (base, protected) = scratch.protected_history();
    scratch.install_real_hook_target();

    let output = Command::new("git")
        .args(["update-ref", "refs/heads/main", &base, &protected])
        .current_dir(scratch.path())
        .output()
        .expect("rewind protected branch through installed hook");
    assert_ne!(output.status.code(), Some(0), "{}", streams(&output));
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(
        diagnostic.contains("refused to abandon upstream-merged commits"),
        "{diagnostic}"
    );
    assert_eq!(scratch.rev_parse("refs/heads/main"), protected);
}

#[test]
fn linked_worktree_reaches_the_same_common_installed_product() {
    let scratch = Scratch::repository("linked-worktree");
    let (base, protected) = scratch.protected_history();
    scratch.install_real_hook_target();
    let linked = scratch.join("linked");
    scratch.git(&[
        "worktree",
        "add",
        "--quiet",
        "-b",
        "linked",
        linked.to_str().expect("linked path is UTF-8"),
        &protected,
    ]);
    scratch.git(&["config", "branch.linked.remote", "origin"]);
    scratch.git(&["config", "branch.linked.merge", "refs/heads/main"]);

    let output = Command::new("git")
        .args(["update-ref", "refs/heads/linked", &base, &protected])
        .current_dir(&linked)
        .output()
        .expect("rewind linked branch through common installed hook");
    assert_ne!(output.status.code(), Some(0), "{}", streams(&output));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("refused to abandon upstream-merged commits")
    );
    assert!(
        !linked.join(".spice-install").exists(),
        "linked worktree must resolve the Git-common installation"
    );
}

#[test]
fn hook_launcher_has_one_direct_product_path_and_never_enters_cargo() {
    let scratch = Scratch::repository("no-cold-build");
    let log = scratch.join("spice-argv");
    let cargo_marker = scratch.join("cargo-entered");
    let spice_script = format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >'{}'\n", log.display());
    let source = scratch.executable("fixture-spice", spice_script.as_bytes());
    let hook = scratch.install_hook_target(&source);
    let bytes = fs::read(&hook).expect("read installed hook launcher");
    let text = std::str::from_utf8(&bytes).expect("hook launcher is UTF-8");

    assert_eq!(text.lines().count(), 2);
    assert_eq!(text.matches("exec ").count(), 1);
    assert!(text.contains("/.spice-install/bin/spice' gate reference-transaction \"$@\""));
    assert!(!text.contains("cargo"));
    assert!(!text.contains("xtask"));

    let bin = scratch.join("bin");
    fs::create_dir(&bin).expect("create sentinel PATH directory");
    let cargo_script = format!("#!/bin/sh\n: >'{}'\nexit 91\n", cargo_marker.display());
    scratch.executable("bin/cargo", cargo_script.as_bytes());
    let path = std::env::join_paths([bin, PathBuf::from("/usr/bin"), PathBuf::from("/bin")])
        .expect("compose sentinel PATH");

    let output = Command::new(&hook)
        .arg("committed")
        .current_dir(scratch.path())
        .env("PATH", path)
        .output()
        .expect("run installed hook template");
    assert_success(&output, "direct installed-product launcher");
    assert_eq!(
        fs::read_to_string(log).unwrap(),
        "gate reference-transaction committed\n"
    );
    assert!(
        !cargo_marker.exists(),
        "hook entered Cargo under the ref lock"
    );
}

fn invoke_prepared(root: &Path, input: &str) -> Output {
    invoke_prepared_command(root, input, None)
}

fn invoke_prepared_command(root: &Path, input: &str, path: Option<&Path>) -> Output {
    invoke_prepared_arguments(
        root,
        input,
        path,
        &["gate", "reference-transaction", "prepared"],
    )
}

fn invoke_prepared_arguments(
    root: &Path,
    input: &str,
    path: Option<&Path>,
    arguments: &[&str],
) -> Output {
    let mut command = Command::new(spice());
    command
        .args(arguments)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = path {
        command.env("PATH", path);
    }
    let mut child = command.spawn().expect("run prepared transaction");
    child
        .stdin
        .take()
        .expect("prepared stdin is piped")
        .write_all(input.as_bytes())
        .expect("write prepared queue");
    child.wait_with_output().expect("collect prepared output")
}

fn spice() -> &'static str {
    env!("CARGO_BIN_EXE_spice")
}

fn assert_success(output: &Output, label: &str) {
    assert!(output.status.success(), "{label}: {}", streams(output));
}

fn streams(output: &Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

struct Scratch(PathBuf);

impl Scratch {
    fn directory(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "spice-reference-transaction-{label}-{}-{}",
            std::process::id(),
            NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create scratch directory");
        Self(path)
    }

    fn repository(label: &str) -> Self {
        let scratch = Self::directory(label);
        scratch.git(&["init", "--quiet", "--initial-branch=main", "."]);
        scratch.git(&["config", "user.name", "Spice Hook Test"]);
        scratch.git(&["config", "user.email", "hooks@example.invalid"]);
        scratch
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn join(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.0.join(relative)
    }

    fn executable(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.join(relative);
        fs::create_dir_all(path.parent().expect("executable has parent"))
            .expect("create executable parent");
        fs::write(&path, bytes).expect("write executable");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("make executable");
        path
    }

    fn git(&self, arguments: &[&str]) -> Output {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(self.path())
            .output()
            .expect("run Git fixture command");
        assert_success(&output, &format!("git {}", arguments.join(" ")));
        output
    }

    fn rev_parse(&self, revision: &str) -> String {
        String::from_utf8(self.git(&["rev-parse", revision]).stdout)
            .unwrap()
            .trim()
            .to_owned()
    }

    fn protected_history(&self) -> (String, String) {
        let story = self.join("story.txt");
        fs::write(&story, "base\n").expect("write base story");
        self.git(&["add", "story.txt"]);
        self.git(&["commit", "--quiet", "-m", "Create base history"]);
        let base = self.rev_parse("HEAD");

        fs::write(&story, "base\nprotected\n").expect("write protected story");
        self.git(&["commit", "--quiet", "-am", "Publish protected history"]);
        let protected = self.rev_parse("HEAD");

        fs::write(&story, "base\nprotected\nlocal\n").expect("write local story");
        self.git(&["commit", "--quiet", "-am", "Keep local history"]);
        let local = self.rev_parse("HEAD");
        self.git(&["remote", "add", "origin", "."]);
        self.git(&["update-ref", "refs/remotes/origin/main", &protected]);
        self.git(&["update-ref", "refs/heads/main", &protected, &local]);
        self.git(&["config", "branch.main.remote", "origin"]);
        self.git(&["config", "branch.main.merge", "refs/heads/main"]);
        (base, protected)
    }

    fn install_real_hook_target(&self) {
        self.install_hook_target(Path::new(spice()));
    }

    fn install_hook_target(&self, source: &Path) -> PathBuf {
        let repository = Repository::discover(self.path()).expect("discover test repository");
        let planned = plan(&repository, Some(source)).expect("plan test installation");
        apply(
            &repository,
            Some(source),
            &planned.digest().expect("digest test installation"),
        )
        .expect("apply test installation");
        let hook = installed_git_hooks(&repository).join("reference-transaction");
        self.git(&[
            "config",
            "core.hooksPath",
            hook.parent().unwrap().to_str().expect("hook path is UTF-8"),
        ]);
        hook
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove scratch directory");
    }
}
