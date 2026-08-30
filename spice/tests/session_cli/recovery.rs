use super::*;
use rusqlite::{Connection, params};
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
        refusal_diagnostic(&first)["observed"]
            .as_str()
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
        refusal_diagnostic(&unavailable_turn)["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("--start"))
    );
}

#[test]
fn briefing_retains_an_end_only_mcp_outcome_as_unattributed_residue() {
    let scratch = Scratch::new("end-only-mcp");
    let transcript = scratch.path().join("codex-session.jsonl");
    write_jsonl(
        &transcript,
        &[
            codex_event(
                "2026-01-03T00:00:00Z",
                "event_msg",
                json!({"type": "task_started", "turn_id": "turn-one"}),
            ),
            codex_message(
                "2026-01-03T00:00:01Z",
                "user",
                None,
                "Preserve the independent dialogue.",
            ),
            codex_event(
                "2026-01-03T00:00:02Z",
                "event_msg",
                json!({
                    "type": "mcp_tool_call_end",
                    "call_id": "exec-end-only",
                    "invocation": {
                        "server": "node_repl",
                        "tool": "js",
                        "arguments": {"code": "return 1"},
                    },
                    "result": {"Ok": {
                        "content": [{"type": "text", "text": "one"}],
                        "isError": false,
                    }},
                }),
            ),
            codex_message(
                "2026-01-03T00:00:03Z",
                "assistant",
                Some("final_answer"),
                "Independent dialogue preserved.",
            ),
            codex_event(
                "2026-01-03T00:00:04Z",
                "event_msg",
                json!({"type": "task_complete", "turn_id": "turn-one"}),
            ),
        ],
    );

    let recall = reply_forms(
        &strings(&["session", "briefing", &transcript.to_string_lossy()]),
        &[],
        "recall",
    );
    assert_eq!(recall["window"]["turns"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        recall["window"]["turns"][0]["dialogue"][1]["text"],
        "Independent dialogue preserved."
    );
    assert_eq!(recall["residues"].as_array().map(Vec::len), Some(1));
    assert_eq!(recall["residues"][0]["kind"], "tool-invocation-missing");
    assert_eq!(recall["residues"][0]["line"], 3);
    assert!(
        recall["residues"][0]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("call_id=exec-end-only"))
    );
    assert!(recall["residues"][0]["recovery"].is_string());
    scratch.remove();
}

#[test]
fn live_thread_briefing_accounts_final_truncated_record_without_blocking() {
    let scratch = Scratch::new("live-tail");
    let codex_home = scratch.path().join("codex-home");
    let sessions = codex_home.join("sessions");
    fs::create_dir_all(&sessions).expect("Codex session root exists");
    let thread = "0123456789abcdef0123456789abcdef";
    let dashed = "01234567-89ab-cdef-0123-456789abcdef";
    let transcript = sessions.join("rollout-live-tail.jsonl");
    let mut bytes = write_jsonl(&transcript, &codex_rows());
    bytes.extend_from_slice(b"{\"timestamp\":");
    fs::write(&transcript, bytes).expect("live tail fixture is written");
    let state =
        Connection::open(codex_home.join("state_5.sqlite")).expect("Codex state fixture opens");
    state
        .execute("CREATE TABLE threads (id TEXT, rollout_path TEXT)", [])
        .expect("Codex thread table exists");
    state
        .execute(
            "INSERT INTO threads (id, rollout_path) VALUES (?1, ?2)",
            params![dashed, transcript.to_string_lossy()],
        )
        .expect("Codex thread path is indexed");
    drop(state);

    let environment = [
        ("CODEX_THREAD_ID", dashed.to_owned()), // env-policy: allow
        ("CODEX_HOME", codex_home.to_string_lossy().into_owned()),
    ];
    let recall = reply_forms(
        &strings(&["session", "briefing", "--deadline-seconds", "5"]),
        &environment,
        "recall",
    );
    assert_eq!(recall["sources"][0]["thread"], thread);
    assert!(
        recall["residues"].as_array().is_some_and(|residues| {
            residues.iter().any(|residue| {
                residue["kind"] == "live-tail-truncated"
                    && residue["detail"]
                        .as_str()
                        .is_some_and(|detail| detail.contains("final live transcript row"))
            })
        }),
        "live tail truncation is retained as residue: {recall}"
    );

    refusal(
        &strings(&["session", "summary", &transcript.to_string_lossy()]),
        &[],
        "session::transcript-evidence-invalid",
    );
    scratch.remove();
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
            refusal_diagnostic(&envelope)["observed"].as_str(),
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
