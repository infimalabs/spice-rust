#![cfg(unix)]

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use spice::install::{apply, installed_git_hooks, plan};
use spice::repository::Repository;
use wire::{Actor, Digest, Domain};

#[path = "support/git_repository.rs"]
mod git_repository;

#[path = "reference_transaction_cli/movement.rs"]
mod movement;

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);
const USAGE: &str =
    "usage: spice gate reference-transaction <preparing|prepared|committed|aborted>\n";

#[test]
fn preparing_is_inert_before_input_or_repository_access() {
    let scratch = Scratch::directory("inert-states");
    let marker = scratch.join("git-entered");
    let git_script = format!("#!/bin/sh\n: >'{}'\nexit 91\n", marker.display());
    let git = scratch.executable("bin/git", git_script.as_bytes());
    let path = git.parent().expect("sentinel Git has a parent");
    let mut child = Command::new(spice())
        .args(["gate", "reference-transaction", "preparing"])
        .current_dir(scratch.path())
        .env("PATH", path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run inert transaction state");
    child
        .stdin
        .take()
        .expect("inert stdin is piped")
        .write_all(b"malformed input that prepared would reject\n")
        .expect("write malformed inert input");
    let output = child.wait_with_output().expect("collect inert output");
    assert_success(&output, "preparing");
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert!(!marker.exists(), "an inert state launched Git");
}

#[test]
fn malformed_terminal_states_refuse_before_repository_access() {
    let scratch = Scratch::directory("malformed-terminal-states");
    let marker = scratch.join("git-entered");
    let git_script = format!("#!/bin/sh\n: >'{}'\nexit 91\n", marker.display());
    let git = scratch.executable("bin/git", git_script.as_bytes());
    for state in ["committed", "aborted"] {
        let output = invoke_prepared_arguments(
            scratch.path(),
            "malformed\n",
            git.parent(),
            &["gate", "reference-transaction", state],
        );
        assert_eq!(output.status.code(), Some(1), "{}", streams(&output));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("malformed reference-transaction")
        );
    }
    assert!(!marker.exists(), "malformed terminal input launched Git");
}

#[test]
fn prepared_commit_and_abort_consume_exact_admissions_across_linked_worktrees() {
    let main = Scratch::repository("credit-main");
    configure_identity(main.path());
    fs::write(main.join("tracked"), "base\n").unwrap();
    git(main.path(), &["add", "tracked"]);
    git(main.path(), &["commit", "--no-verify", "-m", "base"]);
    let head = git(main.path(), &["rev-parse", "HEAD"]);
    fs::write(main.join("tracked"), "candidate\n").unwrap();
    git(main.path(), &["add", "tracked"]);
    let tree = git(main.path(), &["write-tree"]);
    let candidate = git(
        main.path(),
        &["commit-tree", &tree, "-p", &head, "-m", "candidate"],
    );
    let linked = Scratch::directory("credit-linked");
    git_repository::link(main.path(), linked.path(), "linked");
    git(linked.path(), &["update-ref", "refs/heads/linked", &head]);
    publish_admission(main.path(), &tree, "refs/heads/linked", Some(&head));
    let queue = format!("{head} {candidate} refs/heads/linked\n");
    let transaction = transaction_id(&head, &candidate, "refs/heads/linked");
    let cargo_marker = linked.join("cargo-entered");
    let cargo_script = format!("#!/bin/sh\n: >'{}'\nexit 91\n", cargo_marker.display());
    let cargo = linked.executable("bin/cargo", cargo_script.as_bytes());
    let path = PathBuf::from(
        std::env::join_paths([
            cargo.parent().expect("sentinel Cargo has a parent"),
            Path::new("/usr/bin"),
            Path::new("/bin"),
        ])
        .expect("compose prepared hook PATH"),
    );

    let prepared = invoke_prepared_command(linked.path(), &queue, Some(&path));
    assert_success(&prepared, "linked prepared consumption");
    assert!(
        !cargo_marker.exists(),
        "prepared consumption launched Cargo"
    );
    let store = evidence_store(main.path());
    assert!(store.admission_credit(&transaction).unwrap().is_none());

    let committed = invoke_prepared_arguments(
        main.path(),
        &queue,
        None,
        &["gate", "reference-transaction", "committed"],
    );
    assert_success(&committed, "cross-worktree committed consumption");
    let credit = store
        .admission_credit(&transaction)
        .unwrap()
        .expect("committed state publishes durable credit");
    assert_eq!(credit.commits().collect::<Vec<_>>(), [candidate.as_str()]);

    let retry = invoke_prepared_arguments(
        linked.path(),
        &queue,
        None,
        &["gate", "reference-transaction", "committed"],
    );
    assert_success(&retry, "committed retry");
    let aborted_after_commit = invoke_prepared_arguments(
        linked.path(),
        &queue,
        None,
        &["gate", "reference-transaction", "aborted"],
    );
    assert_success(&aborted_after_commit, "abort after committed retry");
    assert_eq!(store.admission_credit(&transaction).unwrap(), Some(credit));

    let abort_queue = format!("{head} {candidate} refs/heads/main\n");
    publish_admission(main.path(), &tree, "refs/heads/main", Some(&head));
    assert_success(
        &invoke_prepared(main.path(), &abort_queue),
        "main prepared consumption",
    );
    assert_success(
        &invoke_prepared_arguments(
            main.path(),
            &abort_queue,
            None,
            &["gate", "reference-transaction", "aborted"],
        ),
        "main aborted consumption",
    );
    assert!(
        store
            .admission_credit(&transaction_id(&head, &candidate, "refs/heads/main"))
            .unwrap()
            .is_none()
    );
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

fn configure_identity(root: &Path) {
    git(root, &["config", "user.email", "fixture@example.invalid"]);
    git(root, &["config", "user.name", "Fixture"]);
}

fn git(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("run fixture git");
    assert!(
        output.status.success(),
        "git {:?}: {}",
        arguments,
        streams(&output)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn evidence_store(root: &Path) -> gate::EvidenceStore {
    let repository = Repository::discover(root).expect("discover evidence repository");
    gate::EvidenceStore::open(repository.common_dir()).expect("open shared evidence store")
}

fn publish_admission(root: &Path, tree: &str, branch: &str, head: Option<&str>) {
    let store = evidence_store(root);
    let action_key = gate::derive_action_key(branch.as_bytes());
    let now = jiff::Timestamp::now();
    let started_at = wire::Instant::new(
        now.checked_sub(jiff::SignedDuration::from_secs(60))
            .expect("fixture start is representable"),
    )
    .unwrap();
    let finished_at = wire::Instant::new(now).unwrap();
    let receipt = gate::PassReceipt::new(gate::PassReceiptFields {
        action_key: action_key.clone(),
        action_spec: Digest::compute(Domain::GatePolicy, b"reference transaction action"),
        input_manifest: Digest::compute(Domain::GateInventory, tree.as_bytes()),
        typed_result: Digest::compute(Domain::GateMeasurement, b"passing result"),
        started_at,
        finished_at,
        duration_millis: 60_000,
        artifacts: Vec::new(),
        producer: gate::EvidenceProducerRecord::new(None, None, None, Some(Actor::Spice), None),
    })
    .unwrap();
    store.publish(&receipt).unwrap();
    let expectation = gate::ReceiptExpectation::new(
        "formatters",
        action_key,
        receipt.action_spec().clone(),
        receipt.input_manifest().clone(),
    )
    .unwrap();
    let action = store
        .lookup_batch_exact(&[expectation])
        .unwrap()
        .remove(0)
        .unwrap();
    let bundle = gate::AdmissionBundle::new(gate::AdmissionBundleFields {
        candidate_tree: tree.to_owned(),
        branch: branch.to_owned(),
        head: head.map(str::to_owned),
        selection: Digest::compute(Domain::GateSelection, b"selection"),
        policy: Digest::compute(Domain::GatePolicy, b"policy"),
        gate_set: Digest::compute(Domain::GateSet, b"gate set"),
        actions: vec![action],
        live_checks: Vec::new(),
        claim: Digest::compute(Domain::GateRepositoryAuthority, b"claim"),
        created_at: started_at,
        expires_at: wire::Instant::new(
            now.checked_add(jiff::SignedDuration::from_secs(600))
                .expect("fixture expiry is representable"),
        )
        .unwrap(),
    })
    .unwrap();
    store.publish_admission(&bundle).unwrap();
}

fn transaction_id(old: &str, new: &str, reference: &str) -> gate::AdmissionTransactionId {
    let bytes = wire::encode(&[(reference, old, new)]).unwrap();
    gate::AdmissionTransactionId::derive(&bytes)
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
        git_repository::initialize(scratch.path());
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

    fn install_hook_target(&self, source: &Path) -> PathBuf {
        let repository = Repository::discover(self.path()).expect("discover test repository");
        let planned = plan(&repository, Some(source)).expect("plan test installation");
        apply(
            &repository,
            Some(source),
            &planned.digest().expect("digest test installation"),
        )
        .expect("apply test installation");
        installed_git_hooks(&repository).join("reference-transaction")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove scratch directory");
    }
}
