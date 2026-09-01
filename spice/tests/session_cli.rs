use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

#[path = "session_cli/authority.rs"]
mod authority;
#[path = "session_cli/discovery.rs"]
mod discovery;
#[path = "session_cli/fixtures.rs"]
mod fixtures;
#[path = "session_cli/message_shape.rs"]
mod message_shape;
#[path = "session_cli/recovery.rs"]
mod recovery;
#[path = "session_cli/window_planner.rs"]
mod window_planner;

use fixtures::{
    codex_event, codex_message, codex_message_content, token_payload, transcript_event,
};

const REFUSAL_EXIT: i32 = 2;

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "spice-session-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("scratch root is exclusively created");
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn remove(self) {
        fs::remove_dir_all(&self.0).expect("scratch root is explicitly removed");
        assert!(!self.0.exists(), "scratch root outlived removal");
        std::mem::forget(self);
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Spoken {
    code: Option<i32>,
    out: Vec<u8>,
    err: Vec<u8>,
}

fn invoke(argv: &[String], environment: &[(&str, String)]) -> Spoken {
    invoke_in(None, argv, environment)
}

fn invoke_in(
    current_dir: Option<&Path>,
    argv: &[String],
    environment: &[(&str, String)],
) -> Spoken {
    let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
    command
        .args(argv)
        .env_remove("CODEX_THREAD_ID") // env-policy: allow
        .env_remove("CLAUDE_CODE_SESSION_ID") // env-policy: allow
        .env_remove("CODEX_HOME")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("SPICE_AGENT_VENDOR") // env-policy: allow
        .env_remove(steer::hook::INBOX);
    if let Some(current_dir) = current_dir {
        command.current_dir(current_dir);
    }
    for (name, value) in environment {
        command.env(name, value);
    }
    let output = command.output().expect("spice session process launches");
    Spoken {
        code: output.status.code(),
        out: output.stdout,
        err: output.stderr,
    }
}

fn reply_forms(argv: &[String], environment: &[(&str, String)], expected: &str) -> Value {
    reply_forms_in(None, argv, environment, expected)
}

fn reply_forms_in(
    current_dir: Option<&Path>,
    argv: &[String],
    environment: &[(&str, String)],
    expected: &str,
) -> Value {
    let text = invoke_in(current_dir, argv, environment);
    assert_eq!(text.code, Some(0), "text stderr: {}", utf8(&text.err));
    assert!(text.err.is_empty(), "successful text reply has no stderr");
    let body: Value = serde_json::from_slice(&text.out).expect("text form is the typed body");
    assert_eq!(body["session"], expected);

    let mut structured_argv = vec!["--json".to_owned()];
    structured_argv.extend_from_slice(argv);
    let structured = invoke_in(current_dir, &structured_argv, environment);
    assert_eq!(
        structured.code,
        Some(0),
        "structured stderr: {}",
        utf8(&structured.err)
    );
    assert!(
        structured.err.is_empty(),
        "successful structured reply has no stderr"
    );
    assert_eq!(
        structured.out.iter().filter(|byte| **byte == b'\n').count(),
        1,
        "structured reply is one DTO record"
    );
    let envelope: Value =
        serde_json::from_slice(&structured.out).expect("structured form is an outcome DTO");
    assert_eq!(envelope["kind"], "reply");
    assert_eq!(envelope["body"], body, "both forms cross the same DTO");
    body
}

fn refusal(argv: &[String], environment: &[(&str, String)], expected_reason: &str) -> Value {
    let mut structured_argv = vec!["--json".to_owned()];
    structured_argv.extend_from_slice(argv);
    let spoken = invoke(&structured_argv, environment);
    assert_eq!(
        spoken.code,
        Some(REFUSAL_EXIT),
        "refusal stdout: {} stderr: {}",
        utf8(&spoken.out),
        utf8(&spoken.err)
    );
    assert!(spoken.out.is_empty(), "refusals do not impersonate replies");
    let envelope: Value =
        serde_json::from_slice(&spoken.err).expect("refusal is a structured outcome DTO");
    let diagnostic = refusal_diagnostic(&envelope);
    assert_eq!(diagnostic["reason"], expected_reason);
    assert!(
        diagnostic["detail"]
            .as_str()
            .is_some_and(|value| !value.is_empty()),
        "every refusal carries explicit recovery guidance"
    );
    envelope
}

fn refusal_diagnostic(envelope: &Value) -> &Value {
    assert_eq!(envelope["kind"], "refusal");
    assert_eq!(envelope["form"], "bare");
    &envelope["diagnostic"]
}

fn utf8(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn write_jsonl(path: &Path, rows: &[Value]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for row in rows {
        serde_json::to_writer(&mut bytes, row).expect("fixture row serializes");
        bytes.push(b'\n');
    }
    fs::write(path, &bytes).expect("fixture transcript is written");
    bytes
}

fn codex_first_turn_rows() -> Vec<Value> {
    let command_arguments = json!({
        "cmd": "cargo check | rg error",
        "workdir": "/repo",
    })
    .to_string();
    let patch_arguments = json!({
        "input": concat!(
            "*** Begin Patch\n",
            "*** Update File: spice/src/session.rs\n",
            "@@\n-old\n+new\n",
            "*** End Patch",
        ),
    })
    .to_string();
    vec![
        codex_event(
            "2026-01-01T00:00:00Z",
            "session_meta",
            json!({"type": "session_meta"}),
        ),
        codex_event(
            "2026-01-01T00:00:01Z",
            "event_msg",
            json!({"type": "task_started", "turn_id": "turn-one"}),
        ),
        codex_message(
            "2026-01-01T00:00:02Z",
            "user",
            None,
            "Please implement parity; it must preserve evidence.",
        ),
        codex_message(
            "2026-01-01T00:00:03Z",
            "assistant",
            Some("commentary"),
            "I am inspecting the transcript.",
        ),
        codex_event(
            "2026-01-01T00:00:04Z",
            "response_item",
            json!({
                "type": "function_call",
                "name": "exec_command",
                "call_id": "command-one",
                "arguments": command_arguments,
            }),
        ),
        codex_event(
            "2026-01-01T00:00:05Z",
            "response_item",
            json!({
                "type": "function_call_output",
                "call_id": "command-one",
                "output": "Process exited with code 1\nfinal output",
            }),
        ),
        codex_event(
            "2026-01-01T00:00:06Z",
            "response_item",
            json!({
                "type": "function_call",
                "name": "apply_patch",
                "call_id": "patch-one",
                "arguments": patch_arguments,
            }),
        ),
        codex_event(
            "2026-01-01T00:00:07Z",
            "response_item",
            token_payload(100, 40, 20, 5, 125),
        ),
        codex_message(
            "2026-01-01T00:00:08Z",
            "assistant",
            Some("final_answer"),
            "Committed abcdef1 after verification.",
        ),
        codex_event(
            "2026-01-01T00:00:09Z",
            "event_msg",
            json!({
                "type": "task_complete",
                "turn_id": "turn-one",
                "last_agent_message": "Committed abcdef1 after verification.",
            }),
        ),
        codex_event("2026-01-01T00:00:10Z", "compacted", json!({})),
    ]
}

fn codex_second_turn_rows() -> Vec<Value> {
    vec![
        codex_event(
            "2026-01-01T00:00:11Z",
            "event_msg",
            json!({"type": "task_started", "turn_id": "turn-two"}),
        ),
        codex_message(
            "2026-01-01T00:00:12Z",
            "user",
            None,
            concat!(
                "This session is being continued from a previous conversation\n",
                "1. Primary Request and Intent\n",
                "- preserve session forensics\n",
                "2. Work Completed\n",
                "- first turn",
            ),
        ),
        codex_message(
            "2026-01-01T00:00:13Z",
            "user",
            None,
            "Finish the second window without losing identity.",
        ),
        codex_event(
            "2026-01-01T00:00:14Z",
            "event_msg",
            json!({
                "type": "exec_command_end",
                "turn_id": "turn-two",
                "command": "spice run -- cargo test | spice run -- rg ok",
                "cwd": "/repo",
                "exit_code": 0,
                "status": "completed",
            }),
        ),
        codex_event(
            "2026-01-01T00:00:15Z",
            "event_msg",
            json!({"type": "web_search", "query": "session parity"}),
        ),
        codex_event(
            "2026-01-01T00:00:16Z",
            "response_item",
            token_payload(140, 60, 30, 10, 180),
        ),
        codex_message(
            "2026-01-01T00:00:17Z",
            "assistant",
            Some("final_answer"),
            "Commit 1234abc completes the port.",
        ),
        codex_event(
            "2026-01-01T00:00:18Z",
            "event_msg",
            json!({
                "type": "task_complete",
                "turn_id": "turn-two",
                "last_agent_message": concat!(
                    "Commit 1234abc remains visible in a distinct ",
                    "task-complete final.",
                ),
            }),
        ),
    ]
}

fn codex_aborted_turn_rows() -> Vec<Value> {
    vec![
        codex_event(
            "2026-01-01T00:00:19Z",
            "event_msg",
            json!({"type": "task_started", "turn_id": "turn-three"}),
        ),
        codex_message(
            "2026-01-01T00:00:20Z",
            "user",
            None,
            "Interrupt this turn cleanly.",
        ),
        codex_event(
            "2026-01-01T00:00:21Z",
            "event_msg",
            json!({
                "type": "turn_aborted",
                "turn_id": "turn-three",
                "reason": "interrupted",
            }),
        ),
    ]
}

fn codex_rows() -> Vec<Value> {
    let mut rows = codex_first_turn_rows();
    rows.extend(codex_second_turn_rows());
    rows.extend(codex_aborted_turn_rows());
    rows
}

fn claude_rows() -> Vec<Value> {
    vec![
        transcript_event(
            "2026-01-02T00:00:01Z",
            "user",
            json!({
                "promptId": "prompt-one",
                "message": {"type": "message", "role": "user", "content": [{
                    "type": "text",
                    "text": "Can you inspect Claude evidence?",
                }]},
            }),
        ),
        transcript_event(
            "2026-01-02T00:00:02Z",
            "assistant",
            json!({"message": {"type": "message", "role": "assistant",
                "content": [
                    {"type": "text", "text": "I will inspect it."},
                    {
                        "type": "tool_use",
                        "id": "claude-command",
                        "name": "Bash",
                        "input": {
                            "command": "spice run -- cargo check",
                            "cwd": "/repo",
                        },
                    },
                ],
                "usage": {
                    "input_tokens": 20,
                    "cache_read_input_tokens": 5,
                    "output_tokens": 8,
                },
            }}),
        ),
        transcript_event(
            "2026-01-02T00:00:03Z",
            "user",
            json!({
                "promptId": "prompt-one",
                "message": {"type": "message", "role": "user", "content": [{
                    "type": "tool_result",
                    "tool_use_id": "claude-command",
                    "content": "Process exited with code 0",
                }]},
            }),
        ),
        transcript_event(
            "2026-01-02T00:00:04Z",
            "assistant",
            json!({"message": {"type": "message", "role": "assistant",
                "content": [{"type": "text", "text": "Claude evidence is intact."}],
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": 25,
                    "cache_read_input_tokens": 6,
                    "output_tokens": 9,
                },
            }}),
        ),
        transcript_event(
            "2026-01-02T00:00:05Z",
            "summary",
            json!({"summary": concat!(
                "This session is being continued from a previous conversation\n",
                "1. Primary Request and Intent\n",
                "- preserve Claude discovery\n",
                "2. Work Completed\n",
                "- inspection",
            )}),
        ),
        transcript_event(
            "2026-01-02T00:00:06Z",
            "user",
            json!({
                "promptId": "prompt-two",
                "message": {"type": "message", "role": "user", "content": [{
                    "type": "text",
                    "text": "Finish Claude reconstruction.",
                }]},
            }),
        ),
        transcript_event(
            "2026-01-02T00:00:07Z",
            "assistant",
            json!({"message": {"type": "message", "role": "assistant",
                "content": [{"type": "text", "text": "Claude reconstruction is done."}],
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": 30,
                    "cache_read_input_tokens": 8,
                    "output_tokens": 10,
                },
            }}),
        ),
    ]
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn assert_recall_and_commands(file: &str, start: &str, end: &str) {
    let recall = reply_forms(
        &strings(&[
            "session",
            "briefing",
            file,
            "--start",
            start,
            "--end",
            end,
            "--contains",
            "preserve evidence",
            "--turn-id",
            "turn-one",
            "--tool",
            "apply_patch",
            "--max-lines",
            "40",
            "--max-bytes",
            "4000",
            "--explain-pruning",
            "--deadline-seconds",
            "5",
        ]),
        &[],
        "recall",
    );
    assert_eq!(recall["window"]["turns"].as_array().map(Vec::len), Some(1));
    assert!(recall["pruning"]["explanation"].is_string());
    assert_eq!(
        recall["window"]["turns"][0]["touched_paths"],
        json!(["spice/src/session.rs"])
    );

    let commands = reply_forms(
        &strings(&[
            "session",
            "commands",
            file,
            "--limit",
            "5",
            "--newest-first",
            "--since-compaction",
            "--failed",
            "--pipelines",
            "--noncanonical-pipelines",
            "--summary",
            "--max-text",
            "90",
        ]),
        &[],
        "commands",
    );
    assert_eq!(
        commands["audit"]["filters"],
        json!([
            "since-compaction",
            "failed",
            "pipelines",
            "noncanonical-pipelines"
        ])
    );
}

fn assert_commits_compactions_and_messages(file: &str, start: &str, end: &str) {
    let commits = reply_forms(
        &strings(&["session", "commits", file, "--limit", "2"]),
        &[],
        "commits",
    );
    assert_eq!(commits["rows"].as_array().map(Vec::len), Some(2));

    let compactions = reply_forms(
        &strings(&["session", "compactions", file, "--limit", "1"]),
        &[],
        "compactions",
    );
    assert_eq!(compactions["rows"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        compactions["rows"][0]["intent"],
        "- preserve session forensics"
    );

    let messages = reply_forms(
        &strings(&[
            "session",
            "messages",
            file,
            "--start",
            start,
            "--end",
            end,
            "--contains",
            "must preserve",
            "--turn-id",
            "turn-one",
            "--side",
            "user",
            "--side",
            "assistant",
            "--phase-kind",
            "prompt",
            "--phase-kind",
            "commentary",
            "--flavor",
            "constraint_like",
            "--limit",
            "4",
            "--oldest-first",
            "--max-text",
            "100",
        ]),
        &[],
        "messages",
    );
    assert_eq!(messages["rows"].as_array().map(Vec::len), Some(1));
    assert_eq!(messages["rows"][0]["flavor"], "constraint_like");
}

fn assert_phases_slices_and_summary(file: &str, start: &str, end: &str) {
    let phases = reply_forms(
        &strings(&[
            "session",
            "phases",
            file,
            "--start",
            start,
            "--end",
            end,
            "--contains",
            "preserve evidence",
            "--turn-id",
            "turn-one",
            "--tool",
            "apply_patch",
            "--limit",
            "2",
            "--examples",
            "1",
            "--max-text",
            "90",
        ]),
        &[],
        "phases",
    );
    assert_eq!(phases["rows"].as_array().map(Vec::len), Some(1));

    let slices = reply_forms(
        &strings(&[
            "session",
            "slices",
            file,
            "--limit",
            "2",
            "--slice-id",
            "compaction-1",
            "--view",
            "full",
            "--max-text",
            "100",
        ]),
        &[],
        "slices",
    );
    assert_eq!(slices["rows"].as_array().map(Vec::len), Some(1));
    assert_eq!(slices["rows"][0]["crossing_files"], json!(["session.rs"]));

    let summary = reply_forms(
        &strings(&["session", "summary", file, "--recent", "1"]),
        &[],
        "summary",
    );
    assert_eq!(summary["turns"], 3);
    assert_eq!(summary["recent_prompts"].as_array().map(Vec::len), Some(1));
}

fn assert_sweep_and_timeline(file: &str, start: &str, end: &str) {
    let sweep = reply_forms(
        &strings(&[
            "session",
            "sweep",
            file,
            "--start",
            start,
            "--end",
            end,
            "--contains",
            "preserve evidence",
            "--turn-id",
            "turn-one",
            "--tool",
            "apply_patch",
            "--count",
            "2",
            "--deadline-seconds",
            "5",
        ]),
        &[],
        "sweep",
    );
    assert!(
        !sweep["windows"]
            .as_array()
            .expect("windows are typed")
            .is_empty()
    );

    let timeline = reply_forms(
        &strings(&[
            "session",
            "timeline",
            file,
            "--start",
            start,
            "--end",
            end,
            "--contains",
            "preserve evidence",
            "--turn-id",
            "turn-one",
            "--tool",
            "apply_patch",
            "--limit",
            "2",
            "--max-text",
            "80",
        ]),
        &[],
        "timeline",
    );
    assert_eq!(timeline["rows"].as_array().map(Vec::len), Some(1));
}

fn assert_tokens_turns_and_pipelines(file: &str, start: &str, end: &str) {
    let tokens = reply_forms(
        &strings(&["session", "tokens", file, "--by-file"]),
        &[],
        "tokens",
    );
    assert_eq!(tokens["usage"].as_array().map(Vec::len), Some(2));
    assert_eq!(tokens["usage"][1]["source"], "TOTAL");
    assert_eq!(tokens["usage"][0]["model_context_window"], Value::Null);
    assert_eq!(tokens["usage"][1]["model_context_window"], Value::Null);

    let turns = reply_forms(
        &strings(&[
            "session",
            "turns",
            file,
            "--start",
            start,
            "--end",
            end,
            "--contains",
            "preserve evidence",
            "--turn-id",
            "turn-one",
            "--tool",
            "apply_patch",
            "--limit",
            "2",
            "--view",
            "full",
        ]),
        &[],
        "turns",
    );
    assert_eq!(turns["rows"].as_array().map(Vec::len), Some(1));
    assert_eq!(turns["rows"][0]["turn_id"], "turn-one");

    let all_commands = reply_forms(
        &strings(&["session", "commands", file, "--limit", "5"]),
        &[],
        "commands",
    );
    assert_eq!(all_commands["rows"].as_array().map(Vec::len), Some(2));
    assert_eq!(all_commands["rows"][0]["pipeline"], "noncanonical");
    assert_eq!(all_commands["rows"][0]["exit_code"], 1);
    assert_eq!(all_commands["rows"][0]["status"], "completed");
    assert_eq!(all_commands["rows"][1]["pipeline"], "canonical");
    assert_eq!(all_commands["rows"][1]["exit_code"], 0);
}

#[test]
fn every_session_query_accepts_current_filters_and_crosses_both_forms() {
    let scratch = Scratch::new("query-family");
    let transcript = scratch.path().join("codex-session.jsonl");
    write_jsonl(&transcript, &codex_rows());
    let file = transcript.to_string_lossy().into_owned();
    let start = "2026-01-01T00:00:00Z";
    let end = "2026-01-01T00:00:18Z";
    assert_recall_and_commands(&file, start, end);
    assert_commits_compactions_and_messages(&file, start, end);
    assert_phases_slices_and_summary(&file, start, end);
    assert_sweep_and_timeline(&file, start, end);
    assert_tokens_turns_and_pipelines(&file, start, end);
    scratch.remove();
}

fn assert_codex_final_rows(codex: &str) {
    let finals = reply_forms(
        &strings(&[
            "session",
            "messages",
            codex,
            "--side",
            "assistant",
            "--phase-kind",
            "final_answer",
            "--oldest-first",
        ]),
        &[],
        "messages",
    );
    assert_eq!(finals["rows"].as_array().map(Vec::len), Some(3));
    assert_eq!(
        finals["rows"][0]["text"],
        "Committed abcdef1 after verification."
    );
    assert_eq!(
        finals["rows"][1]["text"],
        "Commit 1234abc completes the port."
    );
    assert_eq!(
        finals["rows"][2]["text"],
        "Commit 1234abc remains visible in a distinct task-complete final."
    );

    let summary = reply_forms(
        &strings(&["session", "summary", codex, "--recent", "8"]),
        &[],
        "summary",
    );
    assert_eq!(summary["recent_finals"].as_array().map(Vec::len), Some(3));
}

fn assert_codex_turn_and_recall_rows(codex: &str) {
    let codex_turns = reply_forms(
        &strings(&["session", "turns", codex, "--view", "full"]),
        &[],
        "turns",
    );
    let codex_turns = codex_turns["rows"]
        .as_array()
        .expect("Codex turns are typed");
    assert_eq!(codex_turns.len(), 3);
    let first_final_count = codex_turns[0]["dialogue"]
        .as_array()
        .expect("turn dialogue is typed")
        .iter()
        .filter(|message| message["text"] == "Committed abcdef1 after verification.")
        .count();
    assert_eq!(
        first_final_count, 1,
        "the repeated Codex final crosses once"
    );
    let second_assistant = codex_turns[1]["dialogue"]
        .as_array()
        .expect("turn dialogue is typed")
        .iter()
        .filter(|message| message["side"] == "assistant")
        .map(|message| message["text"].as_str().expect("dialogue text is typed"))
        .collect::<Vec<_>>();
    assert_eq!(
        second_assistant,
        [
            "Commit 1234abc completes the port.",
            "Commit 1234abc remains visible in a distinct task-complete final.",
        ]
    );
    assert_eq!(codex_turns[2]["turn_id"], "turn-three");
    assert_eq!(codex_turns[2]["completed"], true);

    let codex_recall = reply_forms(
        &strings(&[
            "session",
            "briefing",
            codex,
            "--start",
            "2026-01-01T00:00:00Z",
            "--turn-id",
            "turn-one",
        ]),
        &[],
        "recall",
    );
    let recall_dialogue = codex_recall["window"]["turns"][0]["dialogue"]
        .as_array()
        .expect("recall dialogue is typed");
    assert_eq!(
        recall_dialogue
            .iter()
            .filter(|message| message["text"] == "Committed abcdef1 after verification.")
            .count(),
        1,
        "recall projects the repeated Codex final once"
    );
}

fn assert_claude_identity_rows(claude: &str) {
    let tokens = reply_forms(&strings(&["session", "tokens", claude]), &[], "tokens");
    assert_eq!(tokens["usage"][0]["snapshots"], 3);
    assert_eq!(tokens["usage"][0]["input"], 75);
    assert_eq!(tokens["usage"][0]["cached"], 19);
    assert_eq!(tokens["usage"][0]["output"], 27);

    let claude_messages = reply_forms(
        &strings(&[
            "session",
            "messages",
            claude,
            "--turn-id",
            "prompt-one",
            "--side",
            "user",
            "--oldest-first",
        ]),
        &[],
        "messages",
    );
    assert_eq!(claude_messages["rows"].as_array().map(Vec::len), Some(1));
    assert_eq!(claude_messages["rows"][0]["turn_id"], "prompt-one");

    let claude_turns = reply_forms(
        &strings(&[
            "session",
            "turns",
            claude,
            "--turn-id",
            "prompt-one",
            "--view",
            "full",
        ]),
        &[],
        "turns",
    );
    assert_eq!(claude_turns["rows"].as_array().map(Vec::len), Some(1));
    assert_eq!(claude_turns["rows"][0]["turn_id"], "prompt-one");

    let timeline = reply_forms(
        &strings(&["session", "timeline", claude, "--turn-id", "prompt-two"]),
        &[],
        "timeline",
    );
    assert_eq!(timeline["rows"].as_array().map(Vec::len), Some(1));
    assert_eq!(timeline["rows"][0]["turn_id"], "prompt-two");

    let recall = reply_forms(
        &strings(&["session", "briefing", claude, "--turn-id", "prompt-two"]),
        &[],
        "recall",
    );
    assert_eq!(recall["window"]["turns"].as_array().map(Vec::len), Some(1));
    assert_eq!(recall["window"]["turns"][0]["turn_id"], "prompt-two");
}

#[test]
fn provider_turn_identity_and_final_deduplication_cross_query_folds() {
    let scratch = Scratch::new("provider-identity");
    let codex_path = scratch.path().join("codex.jsonl");
    let claude_path = scratch.path().join("claude.jsonl");
    write_jsonl(&codex_path, &codex_rows());
    write_jsonl(&claude_path, &claude_rows());

    assert_codex_final_rows(&codex_path.to_string_lossy());
    assert_codex_turn_and_recall_rows(&codex_path.to_string_lossy());
    assert_claude_identity_rows(&claude_path.to_string_lossy());

    scratch.remove();
}

#[test]
fn token_reporting_refuses_invalid_time_and_keeps_an_unknown_context_unknown() {
    let scratch = Scratch::new("token-admission");
    let path = scratch.path().join("usage.jsonl");
    let invalid = codex_event(
        "not-a-timestamp",
        "response_item",
        json!({
            "type": "token_count",
            "info": {
                "last_token_usage": {
                    "input_tokens": 999,
                    "total_tokens": 999,
                },
                "model_context_window": 200_000,
            },
        }),
    );
    let valid_unknown = codex_event(
        "2026-01-01T00:00:01Z",
        "response_item",
        token_payload(7, 0, 2, 0, 9),
    );
    write_jsonl(&path, &[invalid]);

    refusal(
        &strings(&["session", "tokens", &path.to_string_lossy()]),
        &[],
        "session::transcript-timestamp-invalid",
    );
    write_jsonl(&path, &[valid_unknown]);

    let tokens = reply_forms(
        &strings(&["session", "tokens", &path.to_string_lossy()]),
        &[],
        "tokens",
    );

    assert_eq!(tokens["usage"][0]["input"], 7);
    assert_eq!(tokens["usage"][0]["snapshots"], 1);
    assert_eq!(tokens["usage"][0]["model_context_window"], Value::Null);
    assert_eq!(tokens["residues"], json!([]));
    scratch.remove();
}
