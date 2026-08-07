use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

use flate2::Compression;
use flate2::write::GzEncoder;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

const REFUSAL_EXIT: i32 = 2;
const CODEX_THREAD: &str = "0123456789abcdef0123456789abcdef";
const CODEX_THREAD_DASHED: &str = "01234567-89ab-cdef-0123-456789abcdef";
const CLAUDE_THREAD: &str = "fedcba9876543210fedcba9876543210";
const CLAUDE_THREAD_DASHED: &str = "fedcba98-7654-3210-fedc-ba9876543210";

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

#[cfg(unix)]
struct PermissionRestore {
    path: PathBuf,
    permissions: Option<fs::Permissions>,
}

#[cfg(unix)]
impl PermissionRestore {
    fn new(path: &Path, permissions: fs::Permissions) -> Self {
        Self {
            path: path.to_path_buf(),
            permissions: Some(permissions),
        }
    }

    fn restore(mut self) {
        let permissions = self
            .permissions
            .take()
            .expect("permission restoration remains armed");
        fs::set_permissions(&self.path, permissions).expect("restore fixture permissions");
    }
}

#[cfg(unix)]
impl Drop for PermissionRestore {
    fn drop(&mut self) {
        if let Some(permissions) = self.permissions.take() {
            let _ = fs::set_permissions(&self.path, permissions);
        }
    }
}

struct Spoken {
    code: Option<i32>,
    out: Vec<u8>,
    err: Vec<u8>,
}

fn invoke(argv: &[String], environment: &[(&str, String)]) -> Spoken {
    let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
    command
        .args(argv)
        .env_remove("CODEX_THREAD_ID") // env-policy: allow
        .env_remove("CLAUDE_CODE_SESSION_ID") // env-policy: allow
        .env_remove("CODEX_HOME")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("SPICE_AGENT_VENDOR") // env-policy: allow
        .env_remove(steer::hook::INBOX);
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
    let text = invoke(argv, environment);
    assert_eq!(text.code, Some(0), "text stderr: {}", utf8(&text.err));
    assert!(text.err.is_empty(), "successful text reply has no stderr");
    let body: Value = serde_json::from_slice(&text.out).expect("text form is the typed body");
    assert_eq!(body["session"], expected);

    let mut structured_argv = vec!["--json".to_owned()];
    structured_argv.extend_from_slice(argv);
    let structured = invoke(&structured_argv, environment);
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
    assert_eq!(envelope["kind"], "refusal");
    assert_eq!(envelope["reason"], expected_reason);
    assert!(
        envelope["remedy"]
            .as_str()
            .is_some_and(|value| !value.is_empty()),
        "every refusal carries explicit recovery guidance"
    );
    envelope
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

fn write_gzip(path: &Path, bytes: &[u8]) {
    let file = File::create(path).expect("gzip fixture is created");
    let mut encoder = GzEncoder::new(file, Compression::default());
    encoder
        .write_all(bytes)
        .expect("gzip fixture payload is written");
    encoder.finish().expect("gzip fixture is finalized");
}

fn transcript_event(timestamp: &str, kind: &str, fields: Value) -> Value {
    let Value::Object(mut fields) = fields else {
        panic!("transcript fixture fields must be an object");
    };
    fields.insert("timestamp".to_owned(), Value::String(timestamp.to_owned()));
    fields.insert("type".to_owned(), Value::String(kind.to_owned()));
    Value::Object(fields)
}

fn codex_event(timestamp: &str, kind: &str, payload: Value) -> Value {
    transcript_event(timestamp, kind, json!({"payload": payload}))
}

fn codex_message(timestamp: &str, role: &str, phase: Option<&str>, text: &str) -> Value {
    let mut payload = json!({
        "type": "message",
        "role": role,
        "content": [{"type": "output_text", "text": text}],
    });
    if let Some(phase) = phase {
        payload["phase"] = Value::String(phase.to_owned());
    }
    codex_event(timestamp, "response_item", payload)
}

fn token_payload(input: u64, cached: u64, output: u64, reasoning: u64, total: u64) -> Value {
    json!({
        "type": "token_count",
        "info": {"last_token_usage": {
            "input_tokens": input,
            "cached_input_tokens": cached,
            "output_tokens": output,
            "reasoning_output_tokens": reasoning,
            "total_tokens": total,
        }},
    })
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
                "command": "spice agent run -- cargo test | spice agent run -- rg ok",
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

fn codex_rows() -> Vec<Value> {
    let mut rows = codex_first_turn_rows();
    rows.extend(codex_second_turn_rows());
    rows
}

fn claude_rows() -> Vec<Value> {
    vec![
        transcript_event(
            "2026-01-02T00:00:01Z",
            "user",
            json!({
                "promptId": "prompt-one",
                "message": {"content": [{
                    "type": "text",
                    "text": "Can you inspect Claude evidence?",
                }]},
            }),
        ),
        transcript_event(
            "2026-01-02T00:00:02Z",
            "assistant",
            json!({"message": {
                "content": [
                    {"type": "text", "text": "I will inspect it."},
                    {
                        "type": "tool_use",
                        "id": "claude-command",
                        "name": "Bash",
                        "input": {
                            "command": "spice agent run -- cargo check",
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
                "message": {"content": [{
                    "type": "tool_result",
                    "tool_use_id": "claude-command",
                    "content": "Process exited with code 0",
                }]},
            }),
        ),
        transcript_event(
            "2026-01-02T00:00:04Z",
            "assistant",
            json!({"message": {
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
                "message": {"content": [{
                    "type": "text",
                    "text": "Finish Claude reconstruction.",
                }]},
            }),
        ),
        transcript_event(
            "2026-01-02T00:00:07Z",
            "assistant",
            json!({"message": {
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

    let summary = reply_forms(
        &strings(&["session", "summary", file, "--recent", "1"]),
        &[],
        "summary",
    );
    assert_eq!(summary["turns"], 2);
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
    assert_eq!(all_commands["rows"][1]["pipeline"], "canonical");
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
    assert_eq!(codex_turns.len(), 2);
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
fn provider_turn_identity_and_final_deduplication_cross_query_projections() {
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
fn root_thread_discovery_gzip_and_deduplication_preserve_identity() {
    let scratch = Scratch::new("discovery");
    let codex_home = scratch.path().join("codex-home");
    fs::create_dir_all(codex_home.join("sessions")).expect("Codex session root exists");
    let codex_path = codex_home.join("sessions/codex-thread.jsonl");
    let codex_bytes = write_jsonl(&codex_path, &codex_rows());
    let state =
        Connection::open(codex_home.join("state_5.sqlite")).expect("Codex state fixture opens");
    state
        .execute("CREATE TABLE threads (id TEXT, rollout_path TEXT)", [])
        .expect("Codex thread table exists");
    state
        .execute(
            "INSERT INTO threads (id, rollout_path) VALUES (?1, ?2)",
            params![CODEX_THREAD_DASHED, codex_path.to_string_lossy()],
        )
        .expect("Codex thread path is indexed");
    drop(state);
    let codex_env = [
        ("CODEX_THREAD_ID", CODEX_THREAD_DASHED.to_owned()), // env-policy: allow
        ("CODEX_HOME", codex_home.to_string_lossy().into_owned()),
    ];

    let root = reply_forms(&strings(&["session"]), &codex_env, "recall");
    assert_eq!(root["sources"][0]["thread"], CODEX_THREAD);
    assert_eq!(root["sources"][0]["vendor"], "codex");

    let codex_thread = reply_forms(
        &strings(&["session", "thread", CODEX_THREAD_DASHED]),
        &[("CODEX_HOME", codex_home.to_string_lossy().into_owned())],
        "thread",
    );
    assert_eq!(codex_thread["thread"]["id"], CODEX_THREAD);
    assert_eq!(codex_thread["thread"]["vendor"], "codex");

    let claude_home = scratch.path().join("claude-home");
    let claude_project = claude_home.join("projects/project-a");
    fs::create_dir_all(&claude_project).expect("Claude project root exists");
    let claude_path = claude_project.join(format!("session-{CLAUDE_THREAD}.jsonl"));
    write_jsonl(&claude_path, &claude_rows());
    let claude_thread = reply_forms(
        &strings(&["session", "thread", CLAUDE_THREAD_DASHED]),
        &[
            (
                "CLAUDE_CONFIG_DIR",
                claude_home.to_string_lossy().into_owned(),
            ),
            ("SPICE_AGENT_VENDOR", "claude".to_owned()), // env-policy: allow
        ],
        "thread",
    );
    assert_eq!(claude_thread["thread"]["id"], CLAUDE_THREAD);
    assert_eq!(claude_thread["thread"]["vendor"], "claude");
    assert_eq!(
        claude_thread["thread"]["latest_turn"]["turn_id"],
        "prompt-two"
    );

    let resolved_as_input = reply_forms(
        &strings(&["session", "summary", CLAUDE_THREAD_DASHED]),
        &[
            (
                "CLAUDE_CONFIG_DIR",
                claude_home.to_string_lossy().into_owned(),
            ),
            ("SPICE_AGENT_VENDOR", "claude".to_owned()), // env-policy: allow
        ],
        "summary",
    );
    assert_eq!(resolved_as_input["sources"][0]["thread"], CLAUDE_THREAD);

    let gzip_path = scratch.path().join("codex-session.jsonl.gz");
    write_gzip(&gzip_path, &codex_bytes);
    let gzip = gzip_path.to_string_lossy().into_owned();
    let deduped = reply_forms(
        &strings(&["session", "summary", &gzip, &gzip, "--recent", "2"]),
        &[],
        "summary",
    );
    assert_eq!(deduped["sources"].as_array().map(Vec::len), Some(1));
    assert_eq!(deduped["sources"][0]["compressed"], true);
    assert_eq!(deduped["turns"], 2);

    scratch.remove();
}

#[cfg(unix)]
#[test]
fn public_thread_refusal_preserves_an_incomplete_claude_search_before_codex() {
    let scratch = Scratch::new("unreadable-thread");
    let claude_home = scratch.path().join("claude-home");
    let projects = claude_home.join("projects");
    let denied = projects.join("denied");
    fs::create_dir_all(&denied).expect("denied Claude project branch exists");
    let original = fs::metadata(&denied)
        .expect("denied branch metadata exists")
        .permissions();
    let restore = PermissionRestore::new(&denied, original.clone());
    let mut blocked = original;
    blocked.set_mode(0o0);
    fs::set_permissions(&denied, blocked).expect("deny Claude project branch reads");

    let read_error = fs::read_dir(&denied)
        .expect_err("fixture must force a real read_dir failure")
        .to_string();
    let codex_home = scratch.path().join("codex-home");
    fs::create_dir_all(codex_home.join("sessions")).expect("empty Codex session storage exists");
    let spoken = invoke(
        &strings(&["--json", "session", "thread", CLAUDE_THREAD_DASHED]),
        &[
            (
                "CLAUDE_CONFIG_DIR",
                claude_home.to_string_lossy().into_owned(),
            ),
            ("CODEX_HOME", codex_home.to_string_lossy().into_owned()),
            ("SPICE_AGENT_VENDOR", "claude".to_owned()), // env-policy: allow
        ],
    );
    restore.restore();

    assert_eq!(
        spoken.code,
        Some(REFUSAL_EXIT),
        "refusal stdout: {} stderr: {}",
        utf8(&spoken.out),
        utf8(&spoken.err)
    );
    assert!(
        spoken.out.is_empty(),
        "refusal does not impersonate a reply"
    );
    assert_eq!(
        spoken.err.iter().filter(|byte| **byte == b'\n').count(),
        1,
        "public refusal is one JSON outcome"
    );
    let envelope: Value =
        serde_json::from_slice(&spoken.err).expect("refusal is a structured outcome DTO");
    assert_eq!(envelope["kind"], "refusal");
    assert_eq!(envelope["reason"], "session::thread-unresolved");
    let remedy = envelope["remedy"]
        .as_str()
        .expect("thread refusal carries a remedy");
    let claude_failure = format!(
        "transcript search was incomplete: could not read directory {}: {read_error}",
        denied.display()
    );
    let codex_failure = format!(
        "missing Codex state database {}",
        codex_home.join("state_5.sqlite").display()
    );
    assert_eq!(
        remedy,
        format!(
            "restore the owning Codex state index or Claude project transcript, then retry; \
             {claude_failure}; {codex_failure}"
        ),
        "selected Claude failure precedes the prepared Codex failure"
    );
    let former_absence = format!(
        "Claude thread {CLAUDE_THREAD} is absent beneath {}",
        projects.display()
    );
    assert!(!remedy.contains(&former_absence), "{remedy}");

    scratch.remove();
}

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
    let first = reply_forms(&strings(&["session", "summary", damaged]), &[], "summary");
    let second = reply_forms(&strings(&["session", "summary", damaged]), &[], "summary");
    assert_eq!(first, second, "residue projection is deterministic");
    let residues = first["residues"].as_array().expect("residues are typed");
    let kinds = residues
        .iter()
        .filter_map(|row| row["kind"].as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(kinds, BTreeSet::from(["malformed-json", "truncated-json"]));
    assert!(residues.iter().all(|row| {
        row["recovery"]
            .as_str()
            .is_some_and(|recovery| !recovery.is_empty())
    }));
    let full_history_slice = reply_forms(
        &strings(&["session", "slices", damaged, "--limit", "1"]),
        &[],
        "slices",
    );
    assert_eq!(
        full_history_slice["rows"][0]["basis"],
        "full_history_no_compactions"
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
        unavailable_turn["remedy"]
            .as_str()
            .is_some_and(|remedy| remedy.contains("--start"))
    );
}

#[test]
fn malformed_truncated_missing_and_unavailable_state_is_accounted_with_recovery() {
    let scratch = Scratch::new("residue");
    let damaged = damaged_fixture(&scratch);
    assert_damaged_recovery(&damaged);
    assert_invalid_query_arguments(&damaged);
    assert_unavailable_files(&scratch);
    assert_unavailable_turn_identity(&scratch);
    scratch.remove();
}
