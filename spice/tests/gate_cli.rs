#![cfg(unix)]

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use gate::{
    BatteryReceipt, BatteryReport, BatteryState, CampReport, CampStanding, CoverageCompleteness,
    TestSelectionReport,
};
use sill::{
    Argv, Cancellation, Completion, Deadlines, Environment, Output, ProcessGroup, Redaction, Spec,
    StdinPolicy, StreamPolicy, WaitPolicy,
};
use spice::placeholder::PlaceholderReason;
use wire::{Digest, Domain, Outcome, Refusal, ReplyPosition};

const REFUSAL_EXIT: i32 = 2;
const FAULT_EXIT: i32 = 1;
const PROCESS_PATH: &str = concat!("PA", "TH");

#[test]
fn normal_git_commit_with_the_installed_gate_creates_exactly_one_commit_inside_180_seconds() {
    let repository = GateRepository::new();
    repository.write_source(2);
    repository.add("src/lib.rs");
    repository.install_pre_commit();
    let before = git_stdout(repository.path(), &["rev-list", "--count", "HEAD"]);
    let started = Instant::now();

    let output = Command::new("/usr/bin/git")
        .args(["commit", "-qm", "Exercise bounded commit gate"])
        .current_dir(repository.path())
        .output()
        .expect("launch normal fixture commit");

    assert!(
        output.status.success(),
        "normal commit failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        started.elapsed() < Duration::from_secs(180),
        "normal commit crossed the fixed 180-second gate deadline"
    );
    let after = git_stdout(repository.path(), &["rev-list", "--count", "HEAD"]);
    assert_eq!(
        String::from_utf8(before)
            .unwrap()
            .trim()
            .parse::<u64>()
            .unwrap()
            + 1,
        String::from_utf8(after)
            .unwrap()
            .trim()
            .parse::<u64>()
            .unwrap(),
        "one successful gate permits exactly one commit",
    );
    assert_eq!(
        String::from_utf8(git_stdout(
            repository.path(),
            &["show", "-s", "--format=%s", "HEAD"],
        ))
        .unwrap()
        .trim(),
        "Exercise bounded commit gate",
    );
    assert!(
        git_stdout(repository.path(), &["diff", "--cached", "--name-only"]).is_empty(),
        "the successful commit left staged bytes behind"
    );
}

#[test]
fn commit_worker_executes_the_exact_staged_rust_target_and_refuses_unknown_sources() {
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
    BatteryReport::decode(document).expect("known worker terminal is a battery report");
    assert_eq!(
        fs::read_to_string(&marker).expect("selected target wrote its execution marker"),
        "canonical selected target executed",
    );
    let report = selection_report(&known.progress);
    assert!(report.admits());
    assert_eq!(report.changed_sources(), &["src/lib.rs"]);
    assert_eq!(report.tests(), &["gate-fixture::lib:gate_fixture"]);
    assert_eq!(report.edges().len(), 1);
    assert_eq!(report.edges()[0].source(), "src/lib.rs");
    assert_eq!(report.edges()[0].test(), "gate-fixture::lib:gate_fixture");
    assert_eq!(report.coverage(), &CoverageCompleteness::Complete);

    let repository = GateRepository::new();
    fs::create_dir(repository.path().join("tools")).expect("create unknown-source directory");
    fs::write(
        repository.path().join("tools/outside.rs"),
        "pub fn outside_every_package() {}\n",
    )
    .expect("write unknown staged Rust source");
    repository.add("tools/outside.rs");

    let unknown = run_commit_worker(repository.path());
    let Some((kind, document)) = unknown.terminal.split_first() else {
        panic!("unknown staged source produced no terminal record");
    };
    assert_eq!(
        *kind, b'S',
        "unknown staged source did not refuse through selection: {unknown:?}"
    );
    let terminal_report =
        TestSelectionReport::decode(document).expect("unknown terminal is a selection report");
    let progress_report = selection_report(&unknown.progress);
    assert_eq!(terminal_report, progress_report);
    assert!(!terminal_report.admits());
    assert_eq!(terminal_report.changed_sources(), &["tools/outside.rs"]);
    assert!(terminal_report.tests().is_empty());
    assert!(terminal_report.edges().is_empty());
    assert_eq!(
        terminal_report.coverage(),
        &CoverageCompleteness::Unknown {
            reason: "changed Rust source \"tools/outside.rs\" reaches no direct Cargo test target"
                .to_owned(),
        },
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
        &format!(
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
                "'{{\"packages\":[{{\"id\":\"gate-fixture-id\",\"name\":\"gate-fixture\",",
                "\"manifest_path\":\"'\"$root\"'/Cargo.toml\",\"dependencies\":[],",
                "\"targets\":[{{\"name\":\"gate_fixture\",\"kind\":[\"lib\"],",
                "\"test\":true,\"src_path\":\"'\"$root\"'/src/lib.rs\"}}]}}],",
                "\"workspace_members\":[\"gate-fixture-id\"]}}'\n",
                "    exit 0\n",
                "    ;;\n",
                "  test)\n",
                "    /bin/sleep 30 & child=$!\n",
                "    printf '%s\\n' \"$child\" > '{}'\n",
                "    wait\n",
                "    ;;\n",
                "  fmt) exit 0 ;;\n",
                "  check|clippy) exit 0 ;;\n",
                "  *) exit 0 ;;\n",
                "esac\n",
            ),
            tool_pid.display(),
        ),
    );
    executable(
        &programs.join("rustfmt"),
        concat!(
            "#!/bin/sh\n",
            "if [ \"$*\" = \"--version\" ]; then\n",
            "  printf '%s\\n' 'rustfmt 1.9.0-stable (8bab26f4f6 2026-07-14)'\n",
            "fi\n",
            "exit 0\n",
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

    assert!(
        matches!(waited.completion(), Completion::TimedOut(_)),
        "worker completed unexpectedly: completion={:?} stdout={} stderr={}",
        waited.completion(),
        waited
            .stdout()
            .map(|output| String::from_utf8_lossy(output.bytes()))
            .unwrap_or_default(),
        waited
            .stderr()
            .map(|output| String::from_utf8_lossy(output.bytes()))
            .unwrap_or_default(),
    );
    let pid = fs::read_to_string(&tool_pid)
        .expect("the active Cargo tool published its descendant")
        .trim()
        .parse::<u32>()
        .expect("tool descendant pid is numeric");
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
        progress.contains("\"phase\":\"test-selection\""),
        "deadline phase evidence omitted the active test-selection phase: {progress}"
    );
}

#[allow(
    clippy::disallowed_methods,
    reason = "the process-boundary fixture must preserve the invoking test environment"
)]
fn deadline_worker_spec(root: &Path, programs: &Path, progress: &Path, terminal: &Path) -> Spec {
    let mut environment = env::vars_os().collect::<BTreeMap<_, _>>();
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
            ],
        )
        .expect("declare hidden commit worker"),
        cwd: root.to_path_buf(),
        environment: Environment::new(environment).expect("capture deadline environment"),
        group: ProcessGroup::New,
        deadlines: Deadlines::new(
            Duration::from_secs(6),
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
    let scratch = Scratch::new("commit-worker-boundary");
    let progress = scratch.path().join("progress.jsonl");
    let terminal = scratch.path().join("terminal.bin");
    fs::write(&progress, []).expect("create worker progress file");
    fs::write(&terminal, []).expect("create worker terminal file");
    let output = Command::new(env!("CARGO_BIN_EXE_spice"))
        .arg("__gate-pre-commit-worker")
        .arg(&progress)
        .arg(&terminal)
        .current_dir(root)
        .env_remove(steer::hook::INBOX)
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
    // GOV-45 / CASE 250: ordinary `gate` and `gate pre-commit` remain direct
    // observation/enforcement paths; only the explicit `gate findings` leaf
    // enters board dispatch.
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
    std::os::unix::fs::symlink(env!("CARGO"), tripwires.path().join("cargo"))
        .expect("make canonical test runner Cargo available inside the process tripwire path");
    std::os::unix::fs::symlink(
        Path::new(env!("CARGO")).with_file_name("rustc"),
        tripwires.path().join("rustc"),
    )
    .expect("make Cargo's exact Rust compiler available inside the process tripwire path");

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
    let Refusal::Evidence {
        evidence, summary, ..
    } = &refusal
    else {
        panic!("pre-commit gate report must use evidence refusal: {refusal:?}")
    };
    assert!(
        summary.contains("git commit"),
        "the pre-commit refusal names how a merge is recorded: {summary}"
    );
    let pre_commit_report = BatteryReport::decode(evidence.as_bytes())
        .expect("pre-commit refusal carries a canonical report");
    assert_eq!(
        pre_commit_report.refusal_fold().as_str(),
        report.refusal_fold().as_str(),
        "the commit surface renders the same complete failure projection as the canonical report",
    );
    let receipt = receipt_body(&invoke_with_path(
        &["gate", "receipt"],
        repository.path(),
        Some(tripwires.path()),
    ));

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
    assert_eq!(gate::GateId::BATTERY.len(), 27);
    assert_eq!(report.results().len(), 27);
    assert_eq!(
        report
            .results()
            .iter()
            .map(|result| result.gate_id())
            .collect::<Vec<_>>(),
        gate::GateId::BATTERY
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
            scratch.path().join("spice-rust.toml"),
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
        fs::write(scratch.path().join(".gitignore"), "/target/\n")
            .expect("ignore the fixture's persistent Cargo build cache");
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

    fn install_pre_commit(&self) {
        let executable = Path::new(env!("CARGO_BIN_EXE_spice"));
        assert!(!executable.to_string_lossy().contains('\''));
        let path = self.path().join(".git/hooks/pre-commit");
        fs::write(
            &path,
            format!(
                concat!(
                    "#!/bin/sh\n",
                    "set -eu\n",
                    "root=$(git rev-parse --show-toplevel)\n",
                    "git_local_environment=$(git rev-parse --local-env-vars)\n",
                    "for variable in $git_local_environment; do unset \"$variable\"; done\n",
                    "cd \"$root\"\n",
                    "exec '{}' gate pre-commit\n",
                ),
                executable.display(),
            ),
        )
        .expect("install commit-gate hook");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("make commit-gate hook executable");
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

    fn write_selected_test(&self, marker: &Path) {
        let marker = format!("{:?}", marker.to_string_lossy());
        fs::write(
            self.path().join("src/lib.rs"),
            format!(
                concat!(
                    "pub fn fixture() {{}}\n\n",
                    "#[test]\n",
                    "fn canonical_selected_target() {{\n",
                    "    std::fs::write(\n",
                    "        {marker},\n",
                    "        \"canonical selected target executed\",\n",
                    "    )\n",
                    "    .expect(\"write selected-target execution marker\");\n",
                    "}}\n",
                ),
                marker = marker,
            ),
        )
        .expect("write staged source with selected-target marker");
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

fn executable(path: &Path, text: &str) {
    fs::write(path, text).expect("write gate fixture executable");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .expect("make gate fixture executable");
}

fn process_exists(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: signal zero changes no process state and probes this exact pid.
    unsafe { libc::kill(pid, 0) == 0 }
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
        remove_scratch(&self.0)
            .unwrap_or_else(|error| panic!("remove gate scratch {}: {error}", self.0.display()));
    }
}

fn remove_scratch(path: &Path) -> std::io::Result<()> {
    const SCRATCH_REMOVE_ATTEMPTS: usize = 20;
    const SCRATCH_REMOVE_RETRY: Duration = Duration::from_millis(25);
    for attempt in 0..SCRATCH_REMOVE_ATTEMPTS {
        match fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) if attempt + 1 == SCRATCH_REMOVE_ATTEMPTS => return Err(error),
            Err(_) => {
                make_tree_writable(path);
                thread::sleep(SCRATCH_REMOVE_RETRY);
            }
        }
    }
    Ok(())
}

fn make_tree_writable(path: &Path) {
    let Ok(metadata) = fs::metadata(path) else {
        return;
    };
    let mut permissions = metadata.permissions();
    permissions.set_mode(permissions.mode() | 0o700);
    let _ = fs::set_permissions(path, permissions);
    if !metadata.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        make_tree_writable(&entry.path());
    }
}
