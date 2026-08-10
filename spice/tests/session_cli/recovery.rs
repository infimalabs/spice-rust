use super::*;
use serde_json::json;
use std::fs;

fn damaged_fixture(scratch: &Scratch) -> String {
    let damaged_path = scratch.path().join("damaged.jsonl");
    let valid = serde_json::to_vec(&json!({
        "timestamp":"2026-01-03T00:00:00Z",
        "type":"event_msg",
        "payload":{"type":"task_started","turn_id":"readable-prefix"}
    }))
    .expect("valid prefix serializes");
    let mut damaged = valid;
    damaged.extend_from_slice(b"\n{malformed}\n{\"timestamp\":");
    fs::write(&damaged_path, damaged).expect("damaged transcript is written");
    damaged_path.to_string_lossy().into_owned()
}

fn assert_damaged_recovery(damaged: &str) {
    let first = refusal(
        &strings(&["session", "summary", damaged]),
        &[],
        "session::transcript-evidence-invalid",
    );
    let second = refusal(
        &strings(&["session", "summary", damaged]),
        &[],
        "session::transcript-evidence-invalid",
    );
    assert_eq!(first, second, "malformed-evidence refusal is deterministic");
    assert!(
        refusal_value(&first, "observed")
            .is_some_and(|observed| observed.contains(":2: Malformed"))
    );
}

fn assert_invalid_query_arguments(damaged: &str) {
    refusal(
        &strings(&["session", "briefing", damaged, "--deadline-seconds", "0"]),
        &[],
        "session::deadline-invalid",
    );
    refusal(
        &strings(&[
            "session",
            "turns",
            damaged,
            "--start",
            "2026-01-04T00:00:00Z",
            "--end",
            "2026-01-03T00:00:00Z",
        ]),
        &[],
        "session::bounds-reversed",
    );
    refusal(
        &strings(&["session", "timeline", damaged, "--start", "not-a-timestamp"]),
        &[],
        "session::timestamp-invalid",
    );
    refusal(
        &strings(&["session", "summary", damaged, "--recent", "many"]),
        &[],
        "session::bound-invalid",
    );
}

fn assert_unavailable_files(scratch: &Scratch) {
    let corrupt_gzip_path = scratch.path().join("corrupt.jsonl.gz");
    fs::write(&corrupt_gzip_path, b"not a gzip stream")
        .expect("corrupt compressed transcript is written");
    let corrupt_gzip = corrupt_gzip_path.to_string_lossy().into_owned();
    let interrupted = reply_forms(
        &strings(&["session", "summary", &corrupt_gzip]),
        &[],
        "summary",
    );
    assert_eq!(interrupted["residues"][0]["kind"], "read-interrupted");
    assert!(interrupted["residues"][0]["recovery"].is_string());

    let missing_path = scratch.path().join("missing.jsonl");
    let missing = missing_path.to_string_lossy().into_owned();
    let refusal_one = refusal(
        &strings(&["session", "summary", &missing]),
        &[],
        "session::transcript-missing",
    );
    let refusal_two = refusal(
        &strings(&["session", "summary", &missing]),
        &[],
        "session::transcript-missing",
    );
    assert_eq!(refusal_one, refusal_two, "missing-state refusal is stable");

    let directory = scratch.path().join("directory.jsonl");
    fs::create_dir(&directory).expect("non-regular transcript fixture exists");
    refusal(
        &strings(&["session", "summary", &directory.to_string_lossy()]),
        &[],
        "session::transcript-not-regular",
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let real = scratch.path().join("real.jsonl");
        write_jsonl(&real, &codex_rows());
        let link = scratch.path().join("linked.jsonl");
        symlink(&real, &link).expect("transcript symlink fixture exists");
        refusal(
            &strings(&["session", "summary", &link.to_string_lossy()]),
            &[],
            "session::transcript-not-regular",
        );
    }
}

fn assert_unavailable_turn_identity(scratch: &Scratch) {
    let claude_path = scratch.path().join("claude.jsonl");
    let mut rows = claude_rows();
    for row in &mut rows {
        row.as_object_mut()
            .expect("fixture row is an object")
            .remove("promptId");
    }
    write_jsonl(&claude_path, &rows);
    let claude = claude_path.to_string_lossy().into_owned();
    let unavailable_turn = refusal(
        &strings(&["session", "turns", &claude, "--turn-id", "codex-only-id"]),
        &[],
        "session::turn-id-unavailable",
    );
    assert!(
        refusal_value(&unavailable_turn, "remedy").is_some_and(|remedy| remedy.contains("--start"))
    );
}

#[test]
fn recall_and_sweep_deadlines_name_inputs_without_partial_report_data() {
    let scratch = Scratch::new("deadline");
    let transcript = scratch.path().join("private-session.jsonl");
    write_jsonl(&transcript, &codex_rows());
    let input = transcript.to_string_lossy().into_owned();
    for argv in [
        strings(&[
            "session",
            "briefing",
            &input,
            "--deadline-seconds",
            "0.000000001",
        ]),
        strings(&[
            "session",
            "sweep",
            &input,
            "--deadline-seconds",
            "0.000000001",
        ]),
    ] {
        let action = &argv[1];
        let envelope = refusal(&argv, &[], "session::deadline-exceeded");
        let expected_observed = format!(
            "action={action} inputs=requested[{:?}] budget=0.000000001s",
            input
        );
        assert_eq!(
            refusal_value(&envelope, "observed"),
            Some(expected_observed.as_str())
        );
        assert!(
            !envelope
                .to_string()
                .contains("Please implement parity; it must preserve evidence."),
            "a deadline refusal must not carry partial transcript prose"
        );
    }
    scratch.remove();
}

#[test]
fn malformed_truncated_missing_and_unavailable_state_is_accounted_with_recovery() {
    let scratch = Scratch::new("residue");
    let damaged = damaged_fixture(&scratch);
    assert_damaged_recovery(&damaged);
    let valid_path = scratch.path().join("valid.jsonl");
    write_jsonl(&valid_path, &codex_rows());
    assert_invalid_query_arguments(&valid_path.to_string_lossy());
    assert_unavailable_files(&scratch);
    assert_unavailable_turn_identity(&scratch);
    scratch.remove();
}
