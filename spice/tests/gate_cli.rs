#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use gate::{ChangeReachMode, ChangeReachObject, ChangeSelectionReport, CoverageCompleteness};
use spice::change_gate::ChangeGateReport;
use spice::placeholder::PlaceholderReason;
use wire::{Outcome, Refusal};

#[path = "gate_cli/fixture.rs"]
mod fixture;
#[path = "support/verify_claim.rs"]
mod verify_claim_fixture;

use fixture::*;

const REFUSAL_EXIT: i32 = 2;
const FAULT_EXIT: i32 = 1;
const PRE_COMMIT_PATHS: &str = wire::SpiceVariable::PreCommitPaths.as_str();
const PRE_COMMIT_SOURCE: &str = wire::SpiceVariable::PreCommitSource.as_str();

/// One process-boundary smoke spanning admitted, refused, and authority-wide
/// selected-source execution. Policy details stay in the faster library tests.
#[test]
fn commit_worker_executes_known_and_authority_targets_but_refuses_unknown_sources() {
    let marker_scratch = Scratch::new("selected-test-marker");
    let marker = marker_scratch.path().join("executed");
    let repository = GateRepository::new();
    repository.write_selected_test(&marker);
    repository.add("src/lib.rs");

    let known = run_commit_worker(repository.path());
    assert_worker_process_cap(&known);
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
    let report = gate_report.tests();
    assert!(report.admits());
    assert_eq!(report.changed_sources(), &["src/lib.rs"]);
    assert_eq!(report.engines(), &["cargo-workspace"]);
    assert_eq!(report.tests(), &["gate-fixture::lib:gate_fixture"]);
    assert_eq!(report.edges().len(), 1);
    assert_eq!(report.coverage(), &CoverageCompleteness::Complete);
    assert_execution_matches_reach(&known);

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

    assert_selection_refusal(
        &run_commit_worker(repository.path()),
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
    let (selection, _) = selected_execution_signature(&authority);
    assert_eq!(selection.gate_ids(), selection.applicable_gate_ids());
    assert!(selection.reach().iter().all(|reach| {
        matches!(reach.mode(), ChangeReachMode::Exhaustive { .. })
            && reach.objects() == [ChangeReachObject::Repository]
    }));
    assert_eq!(
        fs::read_to_string(&authority_marker).expect("read authority selected-test marker"),
        "canonical selected target executed\n",
    );
}

fn assert_worker_process_cap(boundary: &WorkerBoundary) {
    let budget = boundary
        .progress
        .lines()
        .find(|line| line.starts_with("[change-gate-budget]"))
        .expect("executable boundary publishes its worker budget");
    let cap = budget
        .split_whitespace()
        .find_map(|field| field.strip_prefix("worker_process_cap="))
        .expect("budget names direct worker cap")
        .parse::<usize>()
        .expect("worker cap is numeric");
    assert!(
        (1..=3).contains(&cap),
        "one gate admits at most three roots: {budget}"
    );
    assert!(budget.contains("policy_jobs=1 test_concurrency=1 shard_jobs=1 test_threads=1"));
    assert!(budget.contains("worker_scope=direct-roots nested_children=inherit-parent-slot"));
}

#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "the process-boundary test replaces Cargo at the inherited PATH rim"
)]
fn policy_screen_refuses_before_selected_tests_start() {
    let marker_scratch = Scratch::new("screen-refusal-marker");
    let marker = marker_scratch.path().join("executed");
    let repository = GateRepository::new();
    repository.write_selected_test(&marker);
    repository.add("src/lib.rs");

    let programs = Scratch::new("screen-refusal-programs");
    let cargo = programs.path().join("cargo");
    fs::write(
        &cargo,
        "#!/bin/sh\nprintf '%s\\n' 'cargo 0.0.0 (screen-refusal fixture)'\n",
    )
    .expect("write drifted Cargo fixture");
    let mut permissions = fs::metadata(&cargo)
        .expect("inspect drifted Cargo fixture")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&cargo, permissions).expect("make drifted Cargo fixture executable");
    let ambient_path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(
        std::iter::once(programs.path().to_path_buf()).chain(std::env::split_paths(&ambient_path)),
    )
    .expect("prepend drifted Cargo fixture to PATH");

    let boundary = run_commit_worker_with(repository.path(), |command| {
        command.env("PATH", path);
    });
    let Some((kind, document)) = boundary.terminal.split_first() else {
        panic!("screened staged source produced no terminal record");
    };
    assert_eq!(*kind, b'F', "screened staged source did not refuse");
    let failure: serde_json::Value =
        serde_json::from_slice(document).expect("decode policy-screen refusal boundary");
    assert_eq!(failure["reason"], "gate::change-gate-policy-regressed");
    assert!(!marker.exists(), "policy-screen refusal executed a test");
    assert!(
        !boundary.progress.contains("[rust-test"),
        "policy-screen refusal started selected tests: {}",
        boundary.progress,
    );
}

fn selected_execution_signature(boundary: &WorkerBoundary) -> (ChangeSelectionReport, Vec<String>) {
    assert_eq!(
        boundary.terminal.first(),
        Some(&b'R'),
        "selected execution refused: {boundary:?}",
    );
    assert_execution_matches_reach(boundary);
    let execution = boundary
        .progress
        .lines()
        .filter(|line| {
            line.starts_with("[change-gate-policy]") || line.starts_with("[change-gate-execution]")
        })
        .map(str::to_owned)
        .collect();
    (change_selection_report(boundary), execution)
}

#[derive(Debug)]
struct WorkerBoundary {
    progress: String,
    terminal: Vec<u8>,
}

fn run_commit_worker(root: &Path) -> WorkerBoundary {
    run_commit_worker_with(root, |_| {})
}

fn run_commit_worker_with(
    root: &Path,
    configure: impl FnOnce(&mut std::process::Command),
) -> WorkerBoundary {
    let scratch = Scratch::new("commit-worker-boundary");
    let progress = scratch.path().join("progress.jsonl");
    let terminal = scratch.path().join("terminal.bin");
    fs::write(&progress, []).expect("create worker progress file");
    fs::write(&terminal, []).expect("create worker terminal file");
    let mut command = fixture_command(env!("CARGO_BIN_EXE_spice"));
    command
        .arg("__gate-pre-commit-worker")
        .arg(&progress)
        .arg(&terminal)
        .arg(r#"{"version":1}"#)
        .current_dir(root)
        .env_remove(PRE_COMMIT_PATHS)
        .env_remove(PRE_COMMIT_SOURCE)
        .env_remove(steer::hook::INBOX);
    configure(&mut command);
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
    assert_eq!(*kind, b'F', "staged source did not refuse: {boundary:?}");
    let failure: serde_json::Value =
        serde_json::from_slice(document).expect("decode selected-change refusal boundary");
    assert_eq!(failure["reason"], "gate::change-selection-refused");
    assert!(
        failure["detail"]
            .as_str()
            .expect("selection refusal detail is text")
            .ends_with(detail),
        "worker replaced the selection cause: {failure}",
    );
    assert!(!test_marker.exists(), "refused selection executed a test");
    assert!(
        !boundary.progress.contains("[change-gate-policy]")
            && !boundary.progress.contains("[change-gate-execution]")
            && !boundary.progress.contains("[rust-test"),
        "refused selection started work: {}",
        boundary.progress,
    );
}

fn assert_execution_matches_reach(boundary: &WorkerBoundary) {
    const EXECUTION_PREFIX: &str = "[change-gate-execution] gate=";
    let expected = change_selection_report(boundary)
        .reach()
        .iter()
        .map(|reach| {
            (
                reach.gate_id().as_str().to_owned(),
                reach.objects().to_vec(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut executed = BTreeMap::new();
    for line in boundary
        .progress
        .lines()
        .filter_map(|line| line.strip_prefix(EXECUTION_PREFIX))
    {
        let (gate, objects) = line
            .split_once(" objects=")
            .expect("execution scope separates gate and objects");
        let objects = serde_json::from_str::<Vec<ChangeReachObject>>(objects)
            .expect("execution scope carries typed objects");
        assert!(executed.insert(gate.to_owned(), objects).is_none());
    }
    assert_eq!(executed, expected);
}

fn change_selection_report(boundary: &WorkerBoundary) -> ChangeSelectionReport {
    assert_eq!(boundary.terminal.first(), Some(&b'R'));
    packaged_change_report(&boundary.terminal[1..])
        .selection()
        .clone()
}

fn packaged_change_report(document: &[u8]) -> ChangeGateReport {
    let report: serde_json::Value =
        serde_json::from_slice(document).expect("worker terminal is a composite report");
    let packaged = report
        .get("packaged")
        .expect("pre-commit report carries the packaged result");
    if packaged.get("coverage").and_then(serde_json::Value::as_str) == Some("selected") {
        let encoded: Vec<u8> = serde_json::from_value(
            packaged
                .get("report")
                .expect("selected package carries report bytes")
                .clone(),
        )
        .expect("selected report bytes decode");
        return ChangeGateReport::decode(&encoded).expect("selected report is canonical");
    }
    serde_json::from_value(packaged.clone()).expect("packaged change-gate report is shaped")
}

struct Spoken {
    code: Option<i32>,
    out: Vec<u8>,
    err: Vec<u8>,
}

fn invoke(argv: &[&str], current: &Path) -> Spoken {
    let selects_full_gate = verify_claim_fixture::selects_full_gate(argv);
    if selects_full_gate && spice::repository::Repository::discover(current).is_ok() {
        verify_claim_fixture::install(current);
    }
    let mut command = fixture_command(env!("CARGO_BIN_EXE_spice"));
    command
        .args(argv)
        .current_dir(current)
        .env_remove(PRE_COMMIT_PATHS)
        .env_remove(PRE_COMMIT_SOURCE)
        .env_remove(steer::hook::INBOX);
    if selects_full_gate {
        command.env(
            wire::SpiceVariable::Lane.as_str(),
            verify_claim_fixture::LANE,
        );
    }
    let output = command.output().expect("spice gate process launches");
    Spoken {
        code: output.status.code(),
        out: output.stdout,
        err: output.stderr,
    }
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
    let Outcome::Refusal(revision) = wire::decode::<Outcome<serde_json::Value>>(&revision.err)
        .expect("selection refusal is typed")
    else {
        panic!("missing revision must refuse")
    };
    assert_eq!(revision.reason(), "gate::selection-refused");

    fs::write(repository.path().join("baseline.toml"), "version = 77\n")
        .expect("write malformed current baseline");
    repository.add("baseline.toml");
    let contract = invoke(&["gate", "--json"], repository.path());
    assert_eq!(contract.code, Some(REFUSAL_EXIT));
    let Outcome::Refusal(contract) = wire::decode::<Outcome<serde_json::Value>>(&contract.err)
        .expect("contract refusal is typed")
    else {
        panic!("foreign contract must refuse")
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
    let Outcome::Refusal(unregistered) =
        wire::decode::<Outcome<serde_json::Value>>(&unregistered.err)
            .expect("unregistered refusal is typed")
    else {
        panic!("unregistered focused instrument must refuse")
    };
    assert_eq!(unregistered.reason(), "gate::instrument-unregistered");
    assert!(matches!(unregistered, Refusal::Invocation { .. }));

    let outside = Scratch::new("focused-cannot-run");
    let cannot_run = invoke(
        &["--json", "gate", "findings", "file-shape"],
        outside.path(),
    );
    assert_eq!(cannot_run.code, Some(REFUSAL_EXIT));
    let Outcome::Refusal(cannot_run) = wire::decode::<Outcome<serde_json::Value>>(&cannot_run.err)
        .expect("cannot-run refusal is typed")
    else {
        panic!("unavailable focused instrument must refuse")
    };
    assert_eq!(cannot_run.reason(), "gate::instrument-cannot-run");
}
