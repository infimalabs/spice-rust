#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use gate::{CampReport, CampStanding, GateReport, Standing, Verdict};
use spice::placeholder::PlaceholderReason;
use wire::{Outcome, ReplyPosition};

const REFUSAL_EXIT: i32 = 2;
const FAULT_EXIT: i32 = 1;

struct Spoken {
    code: Option<i32>,
    out: Vec<u8>,
    err: Vec<u8>,
}

fn invoke(argv: &[&str], current: &Path) -> Spoken {
    let output = Command::new(env!("CARGO_BIN_EXE_spice"))
        .args(argv)
        .current_dir(current)
        .env_remove(steer::hook::INBOX)
        .output()
        .expect("spice gate process launches");
    Spoken {
        code: output.status.code(),
        out: output.stdout,
        err: output.stderr,
    }
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

#[test]
fn ordinary_gate_uses_only_the_captured_index_and_one_selected_commit() {
    let repository = GateRepository::new();

    let text = invoke(&["gate"], repository.path());
    let json = invoke(&["--json", "gate"], repository.path());
    assert_eq!(text.code, Some(0));
    assert!(text.err.is_empty());
    assert_eq!(newline_count(&text.out), 1);
    let text_report: GateReport = wire::decode(&text.out).expect("text body is the typed report");
    let json_report = reply::<GateReport>(&json);
    assert_eq!(text_report, json_report);
    assert_eq!(json_report.standing(), Standing::Clean);
    assert_eq!(json_report.measurements()[0].value(), 1);
    assert_eq!(json_report.verdict().standing(), Standing::Clean);
    assert_eq!(json_report.findings(), json_report.verdict().findings());
    assert!(json_report.tightening().is_some());

    repository.write_source(2);
    let unstaged = reply::<GateReport>(&invoke(&["gate", "--json"], repository.path()));
    assert_eq!(unstaged.measurements()[0].value(), 1);
    assert_eq!(unstaged.verdict().tree(), json_report.verdict().tree());

    repository.add("src/lib.rs");
    let regressed = invoke(&["--json", "gate"], repository.path());
    assert_eq!(regressed.code, Some(REFUSAL_EXIT));
    assert!(regressed.out.is_empty());
    assert_eq!(newline_count(&regressed.err), 1);
    let regressed: Outcome<GateReport> =
        wire::decode(&regressed.err).expect("regression is one typed outcome");
    let Outcome::Refusal(regressed) = regressed else {
        panic!("regression must not become a green reply: {regressed:?}");
    };
    assert_eq!(regressed.reason, "gate::regressed");
    assert!(
        regressed
            .observed
            .as_deref()
            .is_some_and(|report| report.contains("standing refused"))
    );

    let committed = reply::<GateReport>(&invoke(
        &["gate", "--at", "HEAD", "--json"],
        repository.path(),
    ));
    assert_eq!(committed.standing(), Standing::Clean);
    assert_eq!(committed.measurements()[0].value(), 1);
    assert_eq!(committed.verdict(), json_report.verdict());

    let receipt = reply::<Verdict>(&invoke(&["--json", "gate", "receipt"], repository.path()));
    assert_eq!(receipt.standing(), Standing::Refused);
    assert_ne!(receipt.tree(), committed.verdict().tree());
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
    assert!(!repository.path().join(".git/.spice").exists());

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
    let receipt_text: Verdict =
        wire::decode(&receipt_text.out).expect("receipt text is typed body");
    let receipt_json = reply::<Verdict>(&receipt_json);
    assert_eq!(receipt_text, receipt_json);
    assert_eq!(receipt_json.standing(), Standing::Clean);

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
        !repository.path().join(".git/.spice").exists(),
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
    let missing_repository: Outcome<GateReport> =
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
    let revision: Outcome<GateReport> =
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
    let contract: Outcome<GateReport> =
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
                "[[floor]]\nname = \"rust.long-lines\"\nvalue = 1\n\n",
                "[[floor]]\nname = \"rust.mutation-survivors\"\nvalue = 0\n",
            ),
        )
        .expect("write tracked baseline");
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
            fs::write(self.path().join(path), format!("{}\n", "c".repeat(length)))
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
