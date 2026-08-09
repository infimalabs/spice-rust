#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use gate::{BatteryReceipt, BatteryReport, BatteryState, CampReport, CampStanding};
use spice::placeholder::PlaceholderReason;
use wire::{Digest, Domain, Outcome, ReplyPosition};

const REFUSAL_EXIT: i32 = 2;
const FAULT_EXIT: i32 = 1;
const PROCESS_PATH: &str = concat!("PA", "TH");

struct Spoken {
    code: Option<i32>,
    out: Vec<u8>,
    err: Vec<u8>,
}

fn invoke(argv: &[&str], current: &Path) -> Spoken {
    invoke_with_path(argv, current, None)
}

fn invoke_with_path(argv: &[&str], current: &Path, path: Option<&Path>) -> Spoken {
    let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
    command
        .args(argv)
        .current_dir(current)
        .env_remove(steer::hook::INBOX);
    if let Some(path) = path {
        command.env(PROCESS_PATH, path);
    }
    let output = command.output().expect("spice gate process launches");
    Spoken {
        code: output.status.code(),
        out: output.stdout,
        err: output.stderr,
    }
}

#[test]
fn every_gate_leaf_uses_one_rust_vector_without_interpreter_or_taskwarrior_processes() {
    let repository = GateRepository::new();
    let tripwires = Scratch::new("tripwires");
    let marker = tripwires.path().join("invoked");
    for program in ["python", "python3", "taskwarrior", "task"] {
        let executable = tripwires.path().join(program);
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\nprintf '%s\\n' '{}' >> '{}'\nexit 97\n",
                program,
                marker.display()
            ),
        )
        .expect("write process tripwire");
        let mut permissions = fs::metadata(&executable)
            .expect("read process tripwire mode")
            .permissions();
        use std::os::unix::fs::PermissionsExt as _;
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).expect("make process tripwire executable");
    }

    let gate = invoke_with_path(&["gate"], repository.path(), Some(tripwires.path()));
    let report = report_body(&gate);
    let pre_commit = invoke_with_path(
        &["--json", "gate", "pre-commit"],
        repository.path(),
        Some(tripwires.path()),
    );
    assert_eq!(pre_commit.code, Some(REFUSAL_EXIT));
    assert!(pre_commit.out.is_empty());
    let outcome: Outcome<serde_json::Value> =
        wire::decode(&pre_commit.err).expect("typed pre-commit refusal");
    let Outcome::Refusal(refusal) = outcome else {
        panic!("non-admitting tripwire vector must refuse: {outcome:?}");
    };
    // An operator reaching this refusal mid-merge needs to know that a merge is
    // recorded like anything else, because the alternative they otherwise reach
    // for is bypassing the hook. The remedy names that command rather than the
    // gate growing a second way to record one, and it is asserted here, on the
    // shipped binary's own output, rather than beside the string that builds it.
    assert!(
        refusal.remedy.contains("git commit"),
        "the pre-commit refusal names how a merge is recorded: {}",
        refusal.remedy
    );
    let pre_commit_report = BatteryReport::decode(
        refusal
            .observed
            .as_deref()
            .expect("pre-commit retains its complete report")
            .as_bytes(),
    )
    .expect("pre-commit refusal carries a canonical report");
    let receipt = receipt_body(&invoke_with_path(
        &["gate", "receipt"],
        repository.path(),
        Some(tripwires.path()),
    ));

    assert_equivalent_except_rust_elapsed(&pre_commit_report, &report);
    assert_eq!(receipt.tree(), report.context().source());
    assert_eq!(
        receipt.manifest(),
        &Digest::compute(
            Domain::GateBatteryManifest,
            gate::CANONICAL_BATTERY_MANIFEST.as_bytes(),
        )
    );
    assert_eq!(receipt.gate_set(), report.gate_set());
    assert_equivalent_except_rust_elapsed(receipt.results(), report.results());
    assert_eq!(gate::GateId::ALL.len(), 27);
    assert_eq!(report.results().len(), 27);
    assert_eq!(
        report
            .results()
            .iter()
            .map(|result| result.gate_id())
            .collect::<Vec<_>>(),
        gate::GateId::ALL
    );
    assert!(
        !marker.exists(),
        "a forbidden interpreter or Taskwarrior process crossed the Rust rim: {}",
        marker.display()
    );
}

fn reply<B>(spoken: &Spoken) -> B
where
    B: serde::de::DeserializeOwned + std::fmt::Debug,
{
    assert_eq!(spoken.code, Some(0));
    assert!(spoken.err.is_empty());
    assert_eq!(newline_count(&spoken.out), 1, "one terminated document");
    let outcome: Outcome<B> = wire::decode(&spoken.out).expect("typed gate outcome decodes");
    let Outcome::Reply(reply) = outcome else {
        panic!("gate leaf did not reply: {outcome:?}");
    };
    assert_eq!(reply.position, ReplyPosition::Direct {});
    reply.body
}

fn newline_count(bytes: &[u8]) -> usize {
    bytes.iter().filter(|byte| **byte == b'\n').count()
}

fn report_body(spoken: &Spoken) -> BatteryReport {
    assert_eq!(spoken.code, Some(0));
    assert!(spoken.err.is_empty());
    assert_eq!(newline_count(&spoken.out), 1);
    let bytes = spoken.out.strip_suffix(b"\n").unwrap().to_vec();
    BatteryReport::decode(&bytes).expect("complete canonical battery report")
}

fn receipt_body(spoken: &Spoken) -> BatteryReceipt {
    assert_eq!(spoken.code, Some(0));
    assert!(spoken.err.is_empty());
    let bytes = spoken.out.strip_suffix(b"\n").unwrap().to_vec();
    BatteryReceipt::verify(&bytes).expect("complete canonical battery receipt")
}

fn assert_structured_body(spoken: &Spoken, expected: &impl serde::Serialize) {
    assert_eq!(spoken.code, Some(0));
    assert!(spoken.err.is_empty());
    let value: serde_json::Value =
        serde_json::from_slice(&spoken.out).expect("structured gate outcome");
    assert_eq!(value["kind"], "reply");
    assert_equivalent_except_rust_elapsed(&value["body"], expected);
}

fn assert_equivalent_except_rust_elapsed<Left, Right>(left: &Left, right: &Right)
where
    Left: serde::Serialize + ?Sized,
    Right: serde::Serialize + ?Sized,
{
    let mut left = serde_json::to_value(left).expect("left body encodes");
    let mut right = serde_json::to_value(right).expect("right body encodes");
    let left_timings = normalize_rust_elapsed(&mut left);
    let right_timings = normalize_rust_elapsed(&mut right);
    assert!(left_timings <= 1);
    assert_eq!(left_timings, right_timings);
    assert_eq!(left, right);
}

fn normalize_rust_elapsed(value: &mut serde_json::Value) -> usize {
    match value {
        serde_json::Value::Array(values) => values.iter_mut().map(normalize_rust_elapsed).sum(),
        serde_json::Value::Object(fields) => {
            if fields.contains_key("run") && fields.contains_key("baseline") {
                let report = fields
                    .get_mut("report")
                    .expect("a receipt carries its report digest");
                assert!(report.is_string(), "a receipt report digest is a string");
                *report =
                    serde_json::Value::String("normalized-run-specific-report-digest".to_owned());
            }
            let is_rust = fields.get("gate_id").and_then(serde_json::Value::as_str)
                == Some(gate::GateId::RustTypecheck.as_str());
            let mut normalized = 0;
            if is_rust && let Some(timing) = fields.get_mut("timing") {
                let elapsed = timing["elapsed_ms"]
                    .as_u64()
                    .expect("rust timing has typed elapsed milliseconds");
                let bound = timing["observation_bound_ms"]
                    .as_u64()
                    .expect("rust timing has a typed observation bound");
                assert!(bound > 0);
                assert!(elapsed <= bound);
                timing["elapsed_ms"] = serde_json::Value::from(0);
                normalized += 1;
            }
            normalized
                + fields
                    .values_mut()
                    .map(normalize_rust_elapsed)
                    .sum::<usize>()
        }
        _ => 0,
    }
}

#[test]
fn ordinary_gate_uses_only_the_captured_index_and_one_selected_commit() {
    let repository = GateRepository::new();

    let text = invoke(&["gate"], repository.path());
    let json = invoke(&["--json", "gate"], repository.path());
    let text_report = report_body(&text);
    assert_structured_body(&json, &text_report);
    assert_eq!(text_report.results().len(), gate::GateId::ALL.len());
    assert!(
        text_report.admits(),
        "fixture battery must admit publication: {:?}",
        text_report
            .results()
            .iter()
            .filter(|result| !result.state().admits())
            .map(|result| (result.gate_id(), result.state(), result.reason()))
            .collect::<Vec<_>>()
    );
    assert!(text_report.results().iter().all(|result| {
        !matches!(
            result.state(),
            BatteryState::Unsupported | BatteryState::Unmeasured
        )
    }));

    repository.write_source(2);
    let unstaged = report_body(&invoke(&["gate"], repository.path()));
    assert_eq!(unstaged.context().source(), text_report.context().source());

    repository.add("src/lib.rs");
    let staged = report_body(&invoke(&["gate"], repository.path()));
    assert_ne!(staged.context().source(), text_report.context().source());

    let committed = report_body(&invoke(&["gate", "--at", "HEAD"], repository.path()));
    assert_eq!(committed.context().source(), text_report.context().source());
    assert_eq!(committed.gate_set(), text_report.gate_set());

    let receipt = receipt_body(&invoke(&["gate", "receipt"], repository.path()));
    assert_eq!(receipt.tree(), staged.context().source());
    assert_eq!(receipt.gate_set(), staged.gate_set());
    assert_eq!(
        receipt
            .results()
            .iter()
            .map(|result| result.gate_id())
            .collect::<Vec<_>>(),
        staged
            .results()
            .iter()
            .map(|result| result.gate_id())
            .collect::<Vec<_>>()
    );
}

#[test]
fn camp_receipt_and_kill_are_fresh_direct_leaves_without_checkout_state() {
    let repository = GateRepository::new();
    repository.write_camp_lines(100);
    repository.add("gate/src/camp.rs");
    repository.add("gate/src/input.rs");
    repository.add("gate/src/kill.rs");
    repository.add("gate/src/verdict.rs");

    let baseline = fs::read(repository.path().join("baseline.toml")).expect("read baseline");
    let selected_source = fs::read(repository.path().join("src/lib.rs")).expect("read source");
    let git_state = git_stdout(repository.path(), &["status", "--porcelain=v1", "-z"]);
    let index = fs::read(repository.path().join(".git/index")).expect("read captured index");
    assert!(!repository.path().join(".git/.spice-rs").exists());

    let camp_text = invoke(&["gate", "camp"], repository.path());
    let camp_json = invoke(&["gate", "camp", "--json"], repository.path());
    assert_eq!(camp_text.code, Some(0));
    assert!(camp_text.err.is_empty());
    assert_eq!(newline_count(&camp_text.out), 1);
    let camp_text: CampReport = wire::decode(&camp_text.out).expect("camp text is typed body");
    let camp_json = reply::<CampReport>(&camp_json);
    assert_eq!(camp_text, camp_json);
    assert_eq!(camp_json.standing, CampStanding::Camped);

    let receipt_text = invoke(&["gate", "receipt"], repository.path());
    let receipt_json = invoke(&["--json", "gate", "receipt"], repository.path());
    assert_eq!(receipt_text.code, Some(0));
    assert!(receipt_text.err.is_empty());
    let receipt_text = receipt_body(&receipt_text);
    assert_structured_body(&receipt_json, &receipt_text);
    assert_eq!(receipt_text.results().len(), gate::GateId::ALL.len());

    let kill = invoke(&["--json", "gate", "kill"], repository.path());
    assert_eq!(kill.code, Some(REFUSAL_EXIT));
    assert!(kill.out.is_empty());
    assert_eq!(newline_count(&kill.err), 1);
    let kill: Outcome<gate::KillReport> =
        wire::decode(&kill.err).expect("unavailable mutation run is typed");
    let Outcome::Refusal(kill) = kill else {
        panic!("fixture mutation tool cannot complete: {kill:?}");
    };
    assert_eq!(kill.reason, "gate::kill-refused");

    assert_eq!(
        fs::read(repository.path().join("baseline.toml")).expect("reread baseline"),
        baseline
    );
    assert_eq!(
        fs::read(repository.path().join("src/lib.rs")).expect("reread source"),
        selected_source
    );
    assert!(
        !repository.path().join(".git/.spice-rs").exists(),
        "gate leaves never opened a journal or resident state"
    );
    assert!(
        !repository.path().join(".spice-gate-mutants").exists(),
        "mutation output never reached the caller tree"
    );
    assert_eq!(
        fs::read(repository.path().join(".git/index")).expect("reread captured index"),
        index,
        "gate leaves never changed the captured index"
    );
    assert_eq!(
        git_stdout(repository.path(), &["status", "--porcelain=v1", "-z"]),
        git_state,
        "the caller worktree and captured index stayed unchanged"
    );
}

#[test]
fn repository_selection_and_contract_failures_are_named_and_never_fall_through() {
    let outside = Scratch::new("outside");
    let missing_repository = invoke(&["--json", "gate"], outside.path());
    assert_eq!(missing_repository.code, Some(FAULT_EXIT));
    assert!(missing_repository.out.is_empty());
    let missing_repository: Outcome<serde_json::Value> =
        wire::decode(&missing_repository.err).expect("repository fault is typed");
    let Outcome::Fault(missing_repository) = missing_repository else {
        panic!("missing repository must fault: {missing_repository:?}");
    };
    assert_eq!(missing_repository.reason, "gate::repository-unresolved");

    let repository = GateRepository::new();
    let revision = invoke(
        &["--json", "gate", "--at", "does-not-exist"],
        repository.path(),
    );
    assert_eq!(revision.code, Some(REFUSAL_EXIT));
    let revision: Outcome<serde_json::Value> =
        wire::decode(&revision.err).expect("selection refusal is typed");
    let Outcome::Refusal(revision) = revision else {
        panic!("missing revision must refuse: {revision:?}");
    };
    assert_eq!(revision.reason, "gate::selection-refused");

    fs::write(repository.path().join("baseline.toml"), "version = 77\n")
        .expect("write malformed current baseline");
    repository.add("baseline.toml");
    let contract = invoke(&["gate", "--json"], repository.path());
    assert_eq!(contract.code, Some(REFUSAL_EXIT));
    let contract: Outcome<serde_json::Value> =
        wire::decode(&contract.err).expect("contract refusal is typed");
    let Outcome::Refusal(contract) = contract else {
        panic!("foreign contract must refuse: {contract:?}");
    };
    assert_eq!(contract.reason, "gate::contract-refused");
    assert!(
        !contract
            .remedy
            .contains(PlaceholderReason::NotLanded.as_str())
    );
}

struct GateRepository {
    scratch: Scratch,
}

impl GateRepository {
    fn new() -> Self {
        let scratch = Scratch::new("repository");
        git(scratch.path(), &["init", "-q"]);
        git(
            scratch.path(),
            &["config", "user.email", "gate@example.invalid"],
        );
        git(scratch.path(), &["config", "user.name", "Gate Fixture"]);

        fs::create_dir_all(scratch.path().join("src")).expect("create source directory");
        fs::create_dir_all(scratch.path().join("gate/src")).expect("create camp loci");
        fs::write(
            scratch.path().join("spice.toml"),
            format!("{}[gate.walk]\nskip = []\n", gate::NARROWEST_FILE_CLASSES),
        )
        .expect("write tracked policy");
        fs::write(
            scratch.path().join("baseline.toml"),
            concat!(
                "version = 1\n\n",
                "[[floor]]\nname = \"rust.mutation-survivors\"\nvalue = 0\n",
            ),
        )
        .expect("write tracked baseline");
        fs::write(
            scratch.path().join("Cargo.toml"),
            "[package]\nname = \"gate-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )
        .expect("write fixture Cargo manifest");
        fs::write(
            scratch.path().join("Cargo.lock"),
            concat!(
                "# This file is automatically @generated by Cargo.\n",
                "# It is not intended for manual editing.\n",
                "version = 4\n\n",
                "[[package]]\n",
                "name = \"gate-fixture\"\n",
                "version = \"0.0.0\"\n",
            ),
        )
        .expect("write fixture Cargo lockfile");
        let repository = Self { scratch };
        repository.write_source(1);
        repository.write_camp_lines(20);
        git(repository.path(), &["add", "."]);
        git(repository.path(), &["commit", "-qm", "fixture"]);
        repository
    }

    fn path(&self) -> &Path {
        self.scratch.path()
    }

    fn add(&self, path: &str) {
        git(self.path(), &["add", path]);
    }

    fn write_source(&self, long_lines: usize) {
        let mut source = String::from("pub fn fixture() {}\n");
        for _ in 0..long_lines {
            source.push_str("// ");
            source.push_str(&"x".repeat(101));
            source.push('\n');
        }
        fs::write(self.path().join("src/lib.rs"), source).expect("write fixture source");
    }

    fn write_camp_lines(&self, length: usize) {
        for path in [
            "gate/src/camp.rs",
            "gate/src/input.rs",
            "gate/src/kill.rs",
            "gate/src/verdict.rs",
        ] {
            fs::write(
                self.path().join(path),
                format!("//{}\n", "c".repeat(length.saturating_sub(2))),
            )
            .expect("write camp locus");
        }
    }
}

fn git(root: &Path, arguments: &[&str]) {
    let _ = git_output(root, arguments);
}

fn git_stdout(root: &Path, arguments: &[&str]) -> Vec<u8> {
    git_output(root, arguments).stdout
}

fn git_output(root: &Path, arguments: &[&str]) -> std::process::Output {
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
    output
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "spice-gate-cli-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create unique gate scratch");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove gate scratch");
    }
}
