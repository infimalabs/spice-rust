//! Focused regressions for the bounded pre-commit worker and supervisor.

use super::*;
use std::cell::Cell;
use std::os::unix::fs::PermissionsExt as _;
use std::thread;
use std::time::Instant;

#[test]
fn fixed_envelope_reserves_cleanup_inside_180_seconds() {
    let bounds = ProcessBounds::production();
    let envelope = bounds.runtime + bounds.grace + bounds.reap + bounds.output_drain;
    assert_eq!(envelope, Duration::from_secs(179));
    assert_eq!(HARD_DEADLINE - envelope, Duration::from_secs(1));
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
        assert!(!files.report.exists());
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
fn selected_test_refusal_renders_its_stage_and_output_evidence() {
    let evidence = concat!(
        "package \"spice\" ended as exit code=101 at binary \"gate_cli\", ",
        "last test \"change selection fixture\""
    );
    let outcome = worker_failure(WorkerFailure {
        reason: "gate::change-gate-tests-refused".to_owned(),
        detail: evidence.to_owned(),
        fault: false,
    });

    let Outcome::Refusal(refusal) = &outcome else {
        panic!("failed selected test execution must refuse");
    };
    assert_eq!(refusal.reason(), "gate::change-gate-tests-refused");
    let rendering = crate::render::render_commit_gate(&outcome, crate::render::Form::Text)
        .expect("selected-test refusal renders")
        .expect("selected-test refusal is visible");
    let text = std::str::from_utf8(rendering.document()).expect("selected-test refusal is UTF-8");
    assert!(text.contains("package \"spice\""), "{text}");
    assert!(text.contains("exit code=101"), "{text}");
    assert!(text.contains("binary \"gate_cli\""), "{text}");
    assert!(text.contains("last test"), "{text}");
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
    assert!(!files.report.exists());
    files.cleanup().expect("remove private worker files");
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
    let rendered = crate::render::render_commit_gate(
        &Outcome::<Report>::Refusal(refusal),
        crate::render::Form::Text,
    )
    .expect("deadline refusal renders")
    .expect("deadline refusal is visible");
    assert!(
        std::str::from_utf8(rendered.document())
            .expect("deadline rendering is UTF-8")
            .contains("phase=family family=toolchain")
    );
    files.cleanup().expect("remove deadline worker files");
}

#[test]
fn last_phase_prefers_later_selected_test_progress() {
    let files = WorkerFiles::create().expect("create progress worker files");
    fs::write(
        &files.progress,
        concat!(
            "[change-gate-policy] gate=repo-docs\n",
            "[rust-test] stage=progress shard=2/4 package=\"spice\" last_test=\"fixture\"\n",
        ),
    )
    .expect("write ordered progress evidence");
    assert_eq!(
        last_phase(&files.progress),
        "selected-tests stage=progress shard=2/4 package=\"spice\" last_test=\"fixture\""
    );
    files.cleanup().expect("remove progress worker files");
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

#[test]
fn configured_only_report_is_validated_and_silent_when_every_step_admits() {
    let source = Digest::compute(wire::Domain::GateTree, b"configured-only source");
    let document = serde_json::json!({
        "version": PRE_COMMIT_REPORT_VERSION,
        "source": source,
        "configured": {
            "source": source,
            "steps": [{
                "label": "configured fixture",
                "argv": ["/bin/true"],
                "selected_paths": ["src/lib.rs"],
                "enabled": true,
                "findings": [],
            }],
        },
    });
    let report = Report::decode(&serde_json::to_vec(&document).expect("encode fixture report"))
        .expect("configured-only report is valid");

    assert!(report.packaged().is_none());
    let configured = report.configured().expect("configured report retained");
    assert_eq!(configured.steps().len(), 1);
    assert_eq!(configured.steps()[0].label(), "configured fixture");
    assert_eq!(configured.steps()[0].selected_paths(), ["src/lib.rs"]);
    assert!(configured.steps()[0].enabled());
    assert!(configured.steps()[0].findings().is_empty());
    assert!(report.admits());
    let outcome = report_outcome(report, Path::new("unused-on-admission"));
    assert!(matches!(outcome, Outcome::Reply(_)));
    assert!(
        crate::render::render_commit_gate(&outcome, crate::render::Form::Text)
            .expect("configured admission renders")
            .is_none(),
        "clean configured-only admission must remain silent",
    );
}

#[test]
fn composite_report_refuses_an_empty_battery_or_mismatched_source() {
    let source = Digest::compute(wire::Domain::GateTree, b"empty battery");
    let empty = serde_json::json!({
        "version": PRE_COMMIT_REPORT_VERSION,
        "source": source,
    });
    assert!(
        Report::decode(&serde_json::to_vec(&empty).expect("encode empty report"))
            .expect_err("empty battery must refuse")
            .contains("empty effective battery")
    );

    let other = Digest::compute(wire::Domain::GateTree, b"other configured source");
    let mismatch = serde_json::json!({
        "version": PRE_COMMIT_REPORT_VERSION,
        "source": source,
        "configured": {
            "source": other,
            "steps": [{
                "label": "configured fixture",
                "argv": ["/bin/true"],
                "selected_paths": [],
                "enabled": true,
                "findings": [],
            }],
        },
    });
    assert!(
        Report::decode(&serde_json::to_vec(&mismatch).expect("encode mismatched report"))
            .expect_err("mismatched configured source must refuse")
            .contains("different staged source")
    );
}

#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: signal zero changes no process state and probes this exact pid.
    unsafe { libc::kill(pid, 0) == 0 }
}
