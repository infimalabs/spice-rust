//! Focused regressions for the bounded pre-commit worker and supervisor.

use super::*;
use gate::CoverageCompleteness;
use std::cell::Cell;
use std::os::unix::fs::PermissionsExt as _;
use std::thread;
use std::time::Instant;

#[test]
fn fixed_envelope_reserves_cleanup_inside_180_seconds() {
    let bounds = ProcessBounds::production();
    let envelope = bounds.runtime + bounds.grace + bounds.reap + bounds.output_drain;
    assert_eq!(envelope, Duration::from_secs(175));
    assert_eq!(HARD_DEADLINE - envelope, bounds.grace);
    assert!(envelope < HARD_DEADLINE);
}

#[test]
fn run_enters_supervision_with_only_private_worker_artifacts() {
    let phase = Cell::new(0);
    let answer = run_with(|files| {
        assert_eq!(phase.get(), 0);
        for path in [&files.progress, &files.terminal] {
            assert_eq!(
                fs::metadata(path)
                    .expect("worker artifact metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        phase.set(1);
        fault("gate::supervisor-fixture", "bounded supervisor entered")
    });

    assert_eq!(phase.get(), 1);
    let Outcome::Fault(fault) = answer else {
        panic!("fixture supervisor returns its typed marker fault");
    };
    assert_eq!(fault.reason, "gate::supervisor-fixture");
}

#[test]
fn verifier_compiler_refusal_renders_its_stage_and_output_evidence() {
    let evidence = concat!(
        "stage=NativeClippy; completion=exited; code=Some(101); ",
        "stdout=compiler stdout witness; stderr=compiler stderr witness"
    );
    let outcome = worker_failure(WorkerFailure {
        reason: "gate::verifier-compiler-refused".to_owned(),
        detail: evidence.to_owned(),
        fault: false,
    });

    let Outcome::Refusal(refusal) = &outcome else {
        panic!("failed compiler preparation must refuse");
    };
    assert_eq!(
        refusal.diagnostic().and_then(RefusalDiagnostic::observed),
        Some(evidence)
    );
    let rendering = crate::render::render_commit_gate(&outcome, crate::render::Form::Text)
        .expect("compiler refusal renders")
        .expect("compiler refusal is visible");
    let text = std::str::from_utf8(rendering.document()).expect("compiler refusal is UTF-8");
    assert!(text.contains("stage=NativeClippy"), "{text}");
    assert!(text.contains("completion=exited"), "{text}");
    assert!(text.contains("code=Some(101)"), "{text}");
    assert!(text.contains("stdout=compiler stdout witness"), "{text}");
    assert!(text.contains("stderr=compiler stderr witness"), "{text}");
}

#[test]
fn worker_artifact_directory_is_private() {
    let files = WorkerFiles::create().expect("create private worker files");
    assert_eq!(
        fs::metadata(&files.root)
            .expect("worker root metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    for path in [&files.progress, &files.terminal] {
        assert_eq!(
            fs::metadata(path)
                .expect("worker artifact metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    files.cleanup().expect("remove private worker files");
}

#[test]
fn unknown_staged_source_coverage_refuses_with_the_exact_selection_report() {
    let report = gate::TestEvidenceGraph::select_tests(
        wire::Digest::compute(wire::Domain::GateTree, b"unknown staged selection"),
        vec!["tools/outside.rs".to_owned()],
        Vec::new(),
        Vec::new(),
        CoverageCompleteness::unknown("changed Rust source is outside every workspace package")
            .expect("unknown reason"),
    )
    .expect("unknown selection remains typed");

    let outcome = selection_refusal(report.clone());
    let Outcome::Refusal(refusal) = &outcome else {
        panic!("unknown source-to-test coverage must refuse");
    };
    assert_eq!(refusal.reason(), "gate::test-selection-coverage-unknown");
    let rendering = crate::render::render_commit_gate(&outcome, crate::render::Form::Text)
        .expect("selection refusal renders")
        .expect("a selection refusal is not silent");
    assert_eq!(rendering.stream(), crate::render::Stream::Err);
    let evidence = std::str::from_utf8(rendering.document())
        .expect("selection evidence is UTF-8")
        .lines()
        .next()
        .expect("selection evidence precedes its summary");
    assert_eq!(
        TestSelectionReport::decode(evidence.as_bytes()).expect("decode refusal evidence"),
        report
    );
}

#[test]
fn deadline_terminates_the_worker_group_and_names_its_last_phase() {
    let files = WorkerFiles::create().expect("create deadline worker files");
    let pid = files.root.join("descendant.pid");
    let script = concat!(
        "printf '%s\\n' '{\"phase\":\"family\",\"family\":\"toolchain\"}' >> \"$1\"; ",
        "sleep 30 & child=$!; printf '%s\\n' \"$child\" > \"$2\"; wait"
    );
    let bounds = ProcessBounds {
        runtime: Duration::from_millis(100),
        output_drain: Duration::from_millis(100),
        reap: Duration::from_millis(100),
        grace: Duration::from_millis(100),
    };
    let spec = worker_spec(
        "/bin/sh",
        [
            OsString::from("-c"),
            OsString::from(script),
            OsString::from("commit-gate-deadline-fixture"),
            files.progress.as_os_str().to_owned(),
            pid.as_os_str().to_owned(),
        ],
        bounds,
    )
    .expect("declare deadline worker");
    let started = Instant::now();
    let waited = sill::spawn(spec)
        .expect("launch deadline worker")
        .wait()
        .expect("terminate and reap deadline worker group");
    assert!(matches!(waited.completion(), Completion::TimedOut(_)));
    assert!(started.elapsed() < Duration::from_secs(2));

    let pid = fs::read_to_string(pid)
        .expect("read descendant pid")
        .trim()
        .parse::<u32>()
        .expect("descendant pid is numeric");
    for _ in 0..100 {
        if !process_exists(pid) {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(!process_exists(pid), "deadline leaked descendant {pid}");

    let Outcome::Refusal(refusal) = deadline(&files.progress) else {
        panic!("deadline is a typed refusal");
    };
    assert_eq!(refusal.reason(), "gate::pre-commit-deadline-elapsed");
    let observed = refusal
        .diagnostic()
        .and_then(RefusalDiagnostic::observed)
        .expect("deadline retains phase evidence");
    assert!(observed.contains("phase=family family=toolchain"));
    assert!(observed.contains("findings=incomplete"));
    files.cleanup().expect("remove deadline worker files");
}

#[test]
fn abnormal_worker_exit_is_typed_and_retains_the_last_exact_phase() {
    let files = WorkerFiles::create().expect("create lost-worker files");
    let script = concat!(
        "printf '%s\\n' ",
        "'{\"phase\":\"arm\",\"ordinal\":12,\"gate_id\":\"rust-typecheck\",",
        "\"family\":\"toolchain\"}' >> \"$1\"; exit 17"
    );
    let spec = worker_spec(
        "/bin/sh",
        [
            OsString::from("-c"),
            OsString::from(script),
            OsString::from("commit-gate-lost-worker-fixture"),
            files.progress.as_os_str().to_owned(),
        ],
        ProcessBounds {
            runtime: Duration::from_secs(1),
            output_drain: Duration::from_millis(100),
            reap: Duration::from_millis(100),
            grace: Duration::from_millis(100),
        },
    )
    .expect("declare lost worker");
    let waited = sill::spawn(spec)
        .expect("launch lost worker")
        .wait()
        .expect("reap lost worker");

    let Outcome::Fault(fault) = classify(&files, &waited) else {
        panic!("worker loss is a typed fault");
    };
    assert_eq!(fault.reason, "gate::pre-commit-worker-lost");
    assert!(
        fault.detail.contains(concat!(
            "phase=arm ordinal=12 gate=rust-typecheck family=toolchain; ",
            "findings=incomplete"
        )),
        "worker loss omitted exact phase evidence: {}",
        fault.detail,
    );
    assert!(fault.detail.contains("worker exited Some(17)"));
    files.cleanup().expect("remove lost-worker files");
}

#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: signal zero changes no process state and probes this exact pid.
    unsafe { libc::kill(pid, 0) == 0 }
}
