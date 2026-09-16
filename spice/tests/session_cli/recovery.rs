use super::*;
use rusqlite::{Connection, params};
use serde_json::json;
use std::fs;

fn damaged_fixture(scratch: &Scratch) -> String {
    let damaged_path = scratch.path().join("damaged.jsonl");
    let before = serde_json::to_vec(&json!({
        "timestamp":"2026-01-03T00:00:00Z",
        "type":"event_msg",
        "payload":{"type":"task_started","turn_id":"readable-prefix"}
    }))
    .expect("valid prefix serializes");
    let after = serde_json::to_vec(&json!({
        "timestamp":"2026-01-03T00:00:02Z",
        "type":"event_msg",
        "payload":{
            "type":"task_complete",
            "turn_id":"readable-prefix",
            "last_agent_message":"history after unsupported evidence remains available"
        }
    }))
    .expect("valid suffix serializes");
    let mut damaged = before;
    damaged.extend_from_slice(
        b"\n{\"timestamp\":\"2026-01-03T00:00:01Z\",\"type\":\"future_authority_mutation\",\"payload\":{}}\n{malformed}\n",
    );
    damaged.extend_from_slice(&after);
    damaged.push(b'\n');
    fs::write(&damaged_path, damaged).expect("damaged transcript is written");
    damaged_path.to_string_lossy().into_owned()
}

fn assert_damaged_recovery(damaged: &str) {
    let first = reply_forms(&strings(&["session", "summary", damaged]), &[], "summary");
    let second = reply_forms(&strings(&["session", "summary", damaged]), &[], "summary");
    assert_eq!(first, second, "mixed-record recovery is deterministic");
    assert_eq!(first["turns"], 1);
    assert_eq!(first["completed"], 1);
    assert_eq!(
        first["recent_finals"][0]["text"],
        "history after unsupported evidence remains available"
    );
    assert_eq!(first["residues"].as_array().map(Vec::len), Some(2));
    let canonical = Path::new(damaged)
        .canonicalize()
        .expect("damaged fixture canonicalizes");
    assert_eq!(
        first["residues"][0]["source"],
        canonical.to_string_lossy().as_ref()
    );
    assert_eq!(first["residues"][0]["line"], 2);
    assert_eq!(first["residues"][0]["kind"], "unrecognized-record");
    assert_eq!(first["residues"][0]["detail"], "future_authority_mutation");
    assert_eq!(
        first["residues"][1]["source"],
        canonical.to_string_lossy().as_ref()
    );
    assert_eq!(first["residues"][1]["line"], 3);
    assert_eq!(first["residues"][1]["kind"], "record-field-malformed");
    assert_eq!(
        first["residues"][1]["detail"],
        "field \"$\" must be JSON record"
    );
    assert!(
        first["residues"][1]["recovery"].as_str().is_some_and(
            |recovery| recovery.contains("surrounding typed history remains available")
        )
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

fn command_execution_companion() -> Value {
    codex_event(
        "2026-09-05T07:11:14.001Z",
        "event_msg",
        json!({"type": "item_completed", "item": {
            "type": "CommandExecution",
            "id": "exec-live",
            "command": ["/bin/zsh", "-lc", "cargo test"],
            "cwd": "file:///repo",
            "process_id": 123,
            "status": "completed",
            "parsed_cmd": [],
            "source": "unified_exec_startup",
            "stdout": "Process exited with code 0",
            "stderr": "",
            "aggregated_output": "Process exited with code 0",
            "formatted_output": "Process exited with code 0",
            "exit_code": 0,
            "duration": {"secs": 1, "nanos": 2},
        }}),
    )
}

fn item_completed_rows() -> Vec<Value> {
    vec![
        codex_event(
            "2026-09-05T07:11:10Z",
            "event_msg",
            json!({"type": "task_started", "turn_id": "turn-live"}),
        ),
        codex_message(
            "2026-09-05T07:11:11Z",
            "user",
            None,
            "History before the companion records.",
        ),
        codex_event(
            "2026-09-05T07:11:11.001Z",
            "event_msg",
            json!({"type": "item_completed", "item": {
                "type": "UserMessage",
                "id": "user-live",
                "content": [{"type": "text", "text": "History before the companion records.", "text_elements": []}],
            }}),
        ),
        codex_event(
            "2026-09-05T07:11:12Z",
            "event_msg",
            json!({"type": "item_completed", "item": {
                "type": "Reasoning",
                "id": "reason-live",
                "summary_text": [],
                "raw_content": [],
            }}),
        ),
        codex_event(
            "2026-09-05T07:11:12.001Z",
            "response_item",
            json!({"type": "reasoning", "id": "reason-live", "summary": []}),
        ),
        codex_event(
            "2026-09-05T07:11:13Z",
            "event_msg",
            json!({"type": "item_completed", "item": {
                "type": "AgentMessage",
                "id": "message-live",
                "content": [{"type": "Text", "text": "One commentary line."}],
                "phase": "commentary",
            }}),
        ),
        codex_message(
            "2026-09-05T07:11:13.001Z",
            "assistant",
            Some("commentary"),
            "One commentary line.",
        ),
        codex_event(
            "2026-09-05T07:11:14Z",
            "response_item",
            json!({
                "type": "function_call",
                "name": "exec_command",
                "call_id": "call-live",
                "arguments": r#"{"cmd":"cargo test"}"#,
            }),
        ),
        command_execution_companion(),
        codex_event(
            "2026-09-05T07:11:14.002Z",
            "response_item",
            json!({
                "type": "function_call_output",
                "call_id": "call-live",
                "output": "Process exited with code 0",
            }),
        ),
        codex_event(
            "2026-09-05T07:11:15Z",
            "event_msg",
            json!({"type": "item_completed", "item": {
                "type": "FutureItem",
                "id": "future-live",
            }}),
        ),
        codex_message(
            "2026-09-05T07:11:16Z",
            "assistant",
            Some("final_answer"),
            "History after the unknown companion remains typed.",
        ),
        codex_event(
            "2026-09-05T07:11:16.001Z",
            "event_msg",
            json!({"type": "item_completed", "item": {
                "type": "AgentMessage",
                "id": "final-live",
                "content": [{"type": "Text", "text": "History after the unknown companion remains typed."}],
                "phase": "final_answer",
            }}),
        ),
        codex_event(
            "2026-09-05T07:11:17Z",
            "event_msg",
            json!({"type": "task_complete", "turn_id": "turn-live"}),
        ),
    ]
}

#[test]
fn briefing_reads_item_completed_companions_without_duplicate_history() {
    let scratch = Scratch::new("item-completed");
    let transcript = scratch.path().join("codex-session.jsonl");
    write_jsonl(&transcript, &item_completed_rows());
    let file = transcript.to_string_lossy().into_owned();

    let recall = reply_forms(&strings(&["session", "briefing", &file]), &[], "recall");
    assert_eq!(recall["window"]["turns"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        recall["window"]["turns"][0]["dialogue"],
        json!([
            {"side": "user", "text": "History before the companion records."},
            {"side": "assistant", "text": "One commentary line."},
            {"side": "assistant", "text": "History after the unknown companion remains typed."},
        ])
    );
    assert_eq!(recall["window"]["turns"][0]["commands"], 1);
    assert_eq!(recall["residues"].as_array().map(Vec::len), Some(1));
    assert_eq!(recall["residues"][0]["line"], 11);
    assert_eq!(recall["residues"][0]["kind"], "record-version-drift");
    assert!(
        recall["residues"][0]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("event_msg:item_completed:FutureItem"))
    );

    let commands = reply_forms(
        &strings(&["session", "commands", &file, "--limit", "5"]),
        &[],
        "commands",
    );
    assert_eq!(commands["rows"].as_array().map(Vec::len), Some(1));
    assert_eq!(commands["rows"][0]["command"], "cargo test");
    scratch.remove();
}

#[test]
fn live_thread_briefing_accounts_final_truncated_record_without_blocking() {
    let scratch = Scratch::new("live-tail");
    let codex_home = scratch.path().join("codex-home");
    let sessions = codex_home.join("sessions");
    fs::create_dir_all(&sessions).expect("Codex session root exists");
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
    assert_eq!(recall["sources"][0]["thread"], dashed);
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

    let summary = reply_forms(
        &strings(&["session", "summary", &transcript.to_string_lossy()]),
        &[],
        "summary",
    );
    assert!(summary["residues"].as_array().is_some_and(|residues| {
        residues
            .iter()
            .any(|residue| residue["kind"] == "record-truncated")
    }));
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

fn omp_entry(timestamp: &str, id: &str, message: Value) -> Value {
    json!({"type": "message", "id": id, "parentId": null, "timestamp": timestamp, "message": message})
}

/// omp names its tools in lower case and keys their arguments as `command`
/// and `path`, so a session file's shell calls and file edits count exactly
/// as Claude's and Codex's do.
#[test]
fn briefing_counts_omp_shell_calls_and_file_edits() {
    let scratch = Scratch::new("omp-tools");
    let transcript = scratch
        .path()
        .join("2026-09-16T08-09-36-474Z_01a0a943-9fda-77b0-bf4e-10879cc619e1.jsonl");
    write_jsonl(
        &transcript,
        &[
            json!({"type": "session", "version": 3, "id": "01a0a943-9fda-77b0-bf4e-10879cc619e1", "timestamp": "2026-09-16T08:09:36.474Z", "cwd": "/work/tree"}),
            omp_entry(
                "2026-09-16T08:09:37.189Z",
                "u1",
                json!({"role": "user", "content": [{"type": "text", "text": "run the tests, then fix the seam"}], "attribution": "user"}),
            ),
            omp_entry(
                "2026-09-16T08:09:40.340Z",
                "a1",
                json!({"role": "assistant", "content": [{"type": "toolCall", "id": "call_1", "name": "bash", "arguments": {"command": "cargo test -p spice"}}], "usage": {"input": 4, "output": 12, "cacheRead": 0, "cacheWrite": 0}, "stopReason": "toolUse"}),
            ),
            omp_entry(
                "2026-09-16T08:09:41.000Z",
                "r1",
                json!({"role": "toolResult", "toolCallId": "call_1", "toolName": "bash", "content": [{"type": "text", "text": "test result: ok"}], "details": {"exitCode": 0}, "isError": false}),
            ),
            omp_entry(
                "2026-09-16T08:09:45.000Z",
                "a2",
                json!({"role": "assistant", "content": [{"type": "toolCall", "id": "call_2", "name": "edit", "arguments": {"path": "spice/src/session.rs", "oldText": "a", "newText": "b"}}], "usage": {"input": 4, "output": 12, "cacheRead": 0, "cacheWrite": 0}, "stopReason": "toolUse"}),
            ),
            omp_entry(
                "2026-09-16T08:09:46.000Z",
                "r2",
                json!({"role": "toolResult", "toolCallId": "call_2", "toolName": "edit", "content": [{"type": "text", "text": "edited"}], "isError": false}),
            ),
            omp_entry(
                "2026-09-16T08:09:50.000Z",
                "a3",
                json!({"role": "assistant", "content": [{"type": "text", "text": "The seam is fixed and the tests pass."}], "usage": {"input": 4, "output": 12, "cacheRead": 0, "cacheWrite": 0}, "stopReason": "stop"}),
            ),
        ],
    );
    let file = transcript.to_string_lossy().into_owned();

    let recall = reply_forms(&strings(&["session", "briefing", &file]), &[], "recall");
    let turn = &recall["window"]["turns"][0];
    assert_eq!(turn["vendor"], "omp");
    assert_eq!(turn["completed"], true, "{turn}");
    assert_eq!(turn["commands"], 1, "{turn}");
    assert_eq!(turn["patches"], 1, "{turn}");
    assert_eq!(turn["touched_paths"], json!(["spice/src/session.rs"]));
    assert_eq!(recall["residues"], json!([]));

    let commands = reply_forms(
        &strings(&["session", "commands", &file, "--limit", "5"]),
        &[],
        "commands",
    );
    let row = &commands["rows"][0];
    assert_eq!(row["command"], "cargo test -p spice", "{row}");
    assert_eq!(row["status"], "completed", "{row}");
    assert_eq!(row["exit_code"], 0, "{row}");

    scratch.remove();
}
