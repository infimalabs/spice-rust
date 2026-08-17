#![cfg(unix)]

use std::collections::BTreeMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use gate::{
    BATTERY_REFUSAL_FOLD_MAX_BYTES, BatteryReceipt, BatteryReport, BatteryState, CampReport,
    CampStanding, CoverageCompleteness, TestSelectionReport,
};
use sill::{
    Argv, Cancellation, Completion, Deadlines, Environment, Output, ProcessGroup, Redaction, Spec,
    StdinPolicy, StreamPolicy, WaitPolicy,
};
use spice::change_gate::ChangeGateReport;
use spice::placeholder::PlaceholderReason;
use wire::{Digest, Domain, Outcome, Refusal, ReplyPosition};

#[path = "gate_cli/cargo_optional.rs"]
mod cargo_optional;
#[path = "gate_cli/dispatch.rs"]
mod dispatch;
#[path = "gate_cli/fixture.rs"]
mod fixture;
#[path = "gate_cli/refusal.rs"]
mod refusal_artifact;

use fixture::*;

const REFUSAL_EXIT: i32 = 2;
const FAULT_EXIT: i32 = 1;
const PROCESS_PATH: &str = concat!("PA", "TH");
const PRE_COMMIT_PATHS: &str = wire::SpiceVariable::PreCommitPaths.as_str();
const PRE_COMMIT_SOURCE: &str = wire::SpiceVariable::PreCommitSource.as_str();
const COMMIT_WORKER_STARTUP_ALLOWANCE: Duration = Duration::from_secs(6);
const COMMIT_WORKER_CONTAINMENT_RUNTIME: Duration = Duration::from_secs(2);
const COMPACT_CHANGE_REPORT_MAX_BYTES: usize = 128 * 1_024;

#[test]
fn commit_worker_executes_known_targets_and_refuses_unknown_sources_before_selected_work() {
    let marker_scratch = Scratch::new("selected-test-marker");
    let marker = marker_scratch.path().join("executed");
    let repository = GateRepository::new();
    repository.write_selected_test(&marker);
    repository.add("src/lib.rs");

    let known = run_commit_worker(repository.path());
    let Some((kind, document)) = known.terminal.split_first() else {
        panic!("known staged source produced no terminal record");
    };
    assert_eq!(*kind, b'R', "known staged source did not admit: {known:?}");
    let gate_report = packaged_change_report(document);
    assert_eq!(
        gate_report.selection().mode(),
        gate::ChangeSelectionMode::Local,
    );
    assert_eq!(
        fs::read_to_string(&marker).expect("selected target wrote its execution marker"),
        "canonical selected target executed\n",
    );
    let report = selection_report(&known.progress);
    assert!(report.admits());
    assert_eq!(report.changed_sources(), &["src/lib.rs"]);
    assert_eq!(report.engines(), &["cargo-workspace"]);
    assert_eq!(report.tests(), &["gate-fixture::lib:gate_fixture"]);
    assert_eq!(report.edges().len(), 1);
    assert_eq!(report.edges()[0].source(), "src/lib.rs");
    assert_eq!(report.edges()[0].test(), "gate-fixture::lib:gate_fixture");
    assert_eq!(report.coverage(), &CoverageCompleteness::Complete);

    let refused_marker_scratch = Scratch::new("refused-selected-test-marker");
    let refused_marker = refused_marker_scratch.path().join("executed");
    let repository = GateRepository::new();
    repository.write_selected_test(&refused_marker);
    repository.add("src/lib.rs");
    fs::create_dir(repository.path().join("tools")).expect("create unknown-source directory");
    fs::write(
        repository.path().join("tools/outside.rs"),
        "pub fn outside_every_package() {}\n",
    )
    .expect("write unknown staged Rust source");
    repository.add("tools/outside.rs");

    let unknown = run_commit_worker(repository.path());
    assert_selection_refusal(
        &unknown,
        "\"tools/outside.rs\" is Rust source outside every compiled workspace root",
        &refused_marker,
    );
    let authority_marker_scratch = Scratch::new("authority-selected-test-marker");
    let authority_marker = authority_marker_scratch.path().join("executed");
    let repository = GateRepository::new();
    repository.write_selected_test(&authority_marker);
    repository.add("src/lib.rs");
    let policy = repository.path().join("spice-rust.toml");
    let mut policy_text = fs::read_to_string(&policy).expect("read authority fixture policy");
    policy_text.push_str("\n# staged authority change\n");
    fs::write(&policy, policy_text).expect("write valid authority change");
    repository.add("spice-rust.toml");

    let authority = run_commit_worker(repository.path());
    assert_selection_refusal(
        &authority,
        "\"spice-rust.toml\" changes packaged selection policy",
        &authority_marker,
    );
}
#[test]
fn commit_worker_deadline_terminates_the_active_gate_tool_process_group() {
    let repository = GateRepository::new();
    repository.write_source(2);
    repository.add("src/lib.rs");
    let scratch = Scratch::new("deadline-process-tree");
    let programs = scratch.path().join("programs");
    fs::create_dir(&programs).expect("create deadline program directory");
    let tool_pid = scratch.path().join("tool.pid");
    executable(
        &programs.join("cargo"),
        concat!(
            "#!/bin/sh\n",
            "if [ \"$*\" = \"--version\" ]; then\n",
            "  printf '%s\\n' 'cargo 1.97.1 (c980f4866 2026-06-30)'\n",
            "  exit 0\n",
            "fi\n",
            "case \"$1\" in\n",
            "  metadata)\n",
            "    root=$(/bin/pwd -P)\n",
            "    printf '%s\\n' ",
            "'{\"packages\":[{\"id\":\"gate-fixture-id\",\"name\":\"gate-fixture\",",
            "\"manifest_path\":\"'\"$root\"'/Cargo.toml\",\"dependencies\":[],",
            "\"targets\":[{\"name\":\"gate_fixture\",\"kind\":[\"lib\"],",
            "\"test\":true,\"src_path\":\"'\"$root\"'/src/lib.rs\"}]}],",
            "\"workspace_members\":[\"gate-fixture-id\"]}'\n",
            "    exit 0\n",
            "    ;;\n",
            "  test) exit 0 ;;\n",
            "  fmt) exit 0 ;;\n",
            "  check) printf '%s\\n' 'unexpected stale cargo check arm' >&2; exit 97 ;;\n",
            "  *) exit 0 ;;\n",
            "esac\n",
        ),
    );
    executable(
        &programs.join("rustfmt"),
        &format!(
            concat!(
                "#!/bin/sh\n",
                "if [ \"$*\" = \"--version\" ]; then\n",
                "  printf '%s\\n' 'rustfmt 1.9.0-stable (8bab26f4f6 2026-07-14)'\n",
                "  exit 0\n",
                "fi\n",
                "/bin/sleep 30 & child=$!\n",
                "printf '%s\\n' \"$child\" > '{}'\n",
                "wait\n",
            ),
            tool_pid.display(),
        ),
    );
    let progress = scratch.path().join("progress.jsonl");
    let terminal = scratch.path().join("terminal.bin");
    fs::write(&progress, []).expect("create deadline progress file");
    fs::write(&terminal, []).expect("create deadline terminal file");
    let spec = deadline_worker_spec(repository.path(), &programs, &progress, &terminal);

    let waited = sill::spawn(spec)
        .expect("launch hidden commit worker")
        .wait()
        .expect("terminate and reap hidden commit worker group");

    assert!(matches!(waited.completion(), Completion::TimedOut(_)));
    let pid = fs::read_to_string(&tool_pid)
        .expect("the active gate tool published its descendant")
        .trim()
        .parse::<u32>()
        .expect("gate-tool descendant pid is numeric");
    for _ in 0..100 {
        if !process_exists(pid) {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !process_exists(pid),
        "commit worker deadline leaked gate-tool descendant {pid}"
    );
    let progress = fs::read_to_string(progress).expect("read worker phase evidence");
    assert!(
        progress.contains("[change-gate-policy] gate=formatters"),
        "deadline phase evidence omitted the active selected formatter arm: {progress}"
    );
}

#[allow(
    clippy::disallowed_methods,
    reason = "the process-boundary fixture must preserve the invoking test environment"
)]
fn deadline_worker_spec(root: &Path, programs: &Path, progress: &Path, terminal: &Path) -> Spec {
    let mut environment = env::vars_os().collect::<BTreeMap<_, _>>();
    for variable in [
        wire::SpiceVariable::PreCommitPaths,
        wire::SpiceVariable::PreCommitSource,
    ] {
        environment.remove(OsStr::new(variable.as_str()));
    }
    environment.insert(
        OsString::from(PROCESS_PATH),
        programs.as_os_str().to_owned(),
    );
    let capture = StreamPolicy::Capture { limit: 1024 * 1024 };
    Spec {
        argv: Argv::new(
            env!("CARGO_BIN_EXE_spice"),
            [
                OsString::from("__gate-pre-commit-worker"),
                progress.as_os_str().to_owned(),
                terminal.as_os_str().to_owned(),
                OsString::from(r#"{"version":1}"#),
            ],
        )
        .expect("declare hidden commit worker"),
        cwd: root.to_path_buf(),
        environment: Environment::new(environment).expect("capture deadline environment"),
        group: ProcessGroup::New,
        deadlines: Deadlines::new(
            COMMIT_WORKER_STARTUP_ALLOWANCE + COMMIT_WORKER_CONTAINMENT_RUNTIME,
            Duration::from_millis(200),
            Duration::from_millis(200),
        )
        .expect("declare scaled deadline"),
        wait: WaitPolicy::new(
            "commit worker process-tree fixture",
            Cancellation::Never,
            sill::RestartDisposition::Never,
        )
        .expect("declare worker wait"),
        output: Output::new(StdinPolicy::Null, capture, capture),
        redaction: Redaction::All,
        grace: Duration::from_millis(200),
    }
}

struct Spoken {
    code: Option<i32>,
    out: Vec<u8>,
    err: Vec<u8>,
}

#[derive(Debug)]
struct WorkerBoundary {
    progress: String,
    terminal: Vec<u8>,
}

fn run_commit_worker(root: &Path) -> WorkerBoundary {
    run_worker_with_path(root, "__gate-pre-commit-worker", None)
}

fn run_commit_worker_with_path(root: &Path, path: Option<&Path>) -> WorkerBoundary {
    run_worker_with_path(root, "__gate-pre-commit-worker", path)
}

fn run_resolution_worker(root: &Path) -> WorkerBoundary {
    run_worker_with_path(root, "__gate-resolution-worker", None)
}

fn run_worker_with_path(root: &Path, worker: &str, path: Option<&Path>) -> WorkerBoundary {
    let scratch = Scratch::new("commit-worker-boundary");
    let progress = scratch.path().join("progress.jsonl");
    let terminal = scratch.path().join("terminal.bin");
    fs::write(&progress, []).expect("create worker progress file");
    fs::write(&terminal, []).expect("create worker terminal file");
    let mut command = fixture_command(env!("CARGO_BIN_EXE_spice"));
    command.arg(worker).arg(&progress).arg(&terminal);
    if worker == "__gate-pre-commit-worker" {
        command.arg(r#"{"version":1}"#);
    }
    command
        .current_dir(root)
        .env_remove(PRE_COMMIT_PATHS)
        .env_remove(PRE_COMMIT_SOURCE)
        .env_remove(steer::hook::INBOX);
    if let Some(path) = path {
        command.env(PROCESS_PATH, path);
    }
    let output = command
        .output()
        .expect("launch normal commit worker boundary");
    assert!(
        output.status.success(),
        "commit worker failed before publishing its boundary: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    WorkerBoundary {
        progress: fs::read_to_string(progress).expect("read commit worker progress"),
        terminal: fs::read(terminal).expect("read commit worker terminal"),
    }
}

fn assert_selection_refusal(boundary: &WorkerBoundary, detail: &str, test_marker: &Path) {
    let Some((kind, document)) = boundary.terminal.split_first() else {
        panic!("refused staged source produced no terminal record");
    };
    assert_eq!(
        *kind, b'F',
        "staged source did not refuse at selection: {boundary:?}"
    );
    let failure: serde_json::Value =
        serde_json::from_slice(document).expect("decode selected-change refusal boundary");
    assert_eq!(
        failure["reason"], "gate::change-selection-refused",
        "worker surfaced the wrong refusal: {failure}",
    );
    assert_eq!(failure["detail"], detail);
    assert_eq!(failure["fault"], false);
    assert!(!test_marker.exists(), "refused selection executed a test");
    assert!(
        !boundary.progress.contains("[change-gate-policy]")
            && !boundary.progress.contains("[rust-test"),
        "refused selection started policy or workspace tests: {}",
        boundary.progress,
    );
}

fn selection_report(progress: &str) -> TestSelectionReport {
    const PREFIX: &str = "[rust-test-selection-report] ";
    let encoded = progress
        .lines()
        .filter_map(|line| line.strip_prefix(PREFIX))
        .collect::<Vec<_>>();
    assert_eq!(
        encoded.len(),
        1,
        "worker must publish exactly one selection report: {progress}"
    );
    TestSelectionReport::decode(encoded[0].as_bytes()).expect("decode worker selection report")
}

fn packaged_change_report(document: &[u8]) -> ChangeGateReport {
    let report: serde_json::Value =
        serde_json::from_slice(document).expect("worker terminal is a composite pre-commit report");
    let packaged = report
        .get("packaged")
        .expect("default pre-commit report carries the packaged result");
    serde_json::from_value(packaged.clone())
        .expect("composite terminal carries a shaped packaged change-gate report")
}

fn invoke(argv: &[&str], current: &Path) -> Spoken {
    invoke_with_path(argv, current, None)
}

fn invoke_with_path(argv: &[&str], current: &Path, path: Option<&Path>) -> Spoken {
    let mut command = fixture_command(env!("CARGO_BIN_EXE_spice"));
    command
        .args(argv)
        .current_dir(current)
        .env_remove(PRE_COMMIT_PATHS)
        .env_remove(PRE_COMMIT_SOURCE)
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
    assert_eq!(text_report.results().len(), gate::GateId::BATTERY.len());
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
fn camp_receipt_and_kill_are_fresh_direct_leaves_without_checkout_mutation() {
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
    assert_eq!(receipt_text.results().len(), gate::GateId::BATTERY.len());

    let kill = invoke(&["--json", "gate", "kill"], repository.path());
    assert_eq!(kill.code, Some(REFUSAL_EXIT));
    assert!(kill.out.is_empty());
    assert_eq!(newline_count(&kill.err), 1);
    let kill: Outcome<gate::KillReport> =
        wire::decode(&kill.err).expect("unavailable mutation run is typed");
    let Outcome::Refusal(kill) = kill else {
        panic!("fixture mutation tool cannot complete: {kill:?}");
    };
    assert_eq!(kill.reason(), "gate::kill-refused");

    assert_eq!(
        fs::read(repository.path().join("baseline.toml")).expect("reread baseline"),
        baseline
    );
    assert_eq!(
        fs::read(repository.path().join("src/lib.rs")).expect("reread source"),
        selected_source
    );
    let current =
        gate::CurrentState::load(gate::current_state_path(&repository.path().join(".git")))
            .expect("accepted receipt published canonical gate state");
    assert_eq!(
        current.before().version(),
        gate::state::STATE_LEDGER_VERSION
    );
    assert!(
        repository
            .path()
            .join(".git/.spice-rs/data/gate-state.json")
            .is_file(),
        "the accepted battery publishes only its Git-common quality state"
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
    assert_eq!(revision.reason(), "gate::selection-refused");

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
    assert_eq!(contract.reason(), "gate::contract-refused");
    assert!(
        !contract
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::detail)
            .expect("contract refusal carries detail")
            .contains(PlaceholderReason::NotLanded.as_str())
    );
}

#[test]
fn gate_findings_refuse_unregistered_and_cannot_run_before_board_dispatch() {
    let repository = GateRepository::new();
    let unregistered = invoke(
        &["--json", "gate", "findings", "not-a-gate"],
        repository.path(),
    );
    assert_eq!(unregistered.code, Some(REFUSAL_EXIT));
    assert!(unregistered.out.is_empty());
    let unregistered: Outcome<serde_json::Value> =
        wire::decode(&unregistered.err).expect("unregistered refusal is typed");
    let Outcome::Refusal(unregistered) = unregistered else {
        panic!("unregistered focused instrument must refuse: {unregistered:?}");
    };
    assert_eq!(unregistered.reason(), "gate::instrument-unregistered");
    assert!(matches!(unregistered, Refusal::Invocation { .. }));
    assert!(
        !repository.path().join(".git/.spice-rs").exists(),
        "an unregistered instrument must not reach board state"
    );

    let outside = Scratch::new("focused-cannot-run");
    let cannot_run = invoke(
        &["--json", "gate", "findings", "file-shape"],
        outside.path(),
    );
    assert_eq!(cannot_run.code, Some(REFUSAL_EXIT));
    assert!(cannot_run.out.is_empty());
    let cannot_run: Outcome<serde_json::Value> =
        wire::decode(&cannot_run.err).expect("cannot-run refusal is typed");
    let Outcome::Refusal(cannot_run) = cannot_run else {
        panic!("unavailable focused instrument must refuse: {cannot_run:?}");
    };
    assert_eq!(cannot_run.reason(), "gate::instrument-cannot-run");
    assert!(
        cannot_run
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::observed)
            .is_some_and(|observed| observed.contains("gate::repository-unresolved"))
    );
}
