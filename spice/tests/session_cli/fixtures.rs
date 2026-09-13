use std::fs;

use serde_json::{Value, json};

pub(super) fn assert_commit_rows(commits: &Value, file: &str, peer: &str) {
    let source = fs::canonicalize(file).expect("commit fixture path is canonical");
    let source = source.to_string_lossy();
    let peer_source = fs::canonicalize(peer).expect("peer commit fixture path is canonical");
    let peer_source = peer_source.to_string_lossy();
    assert_eq!(commits["rows"][0]["sha"], "abcdef1");
    assert_eq!(commits["rows"][0]["turn_id"], "turn-one");
    assert_eq!(
        commits["rows"][0]["user"],
        "Please implement parity; it must preserve evidence."
    );
    assert_eq!(commits["rows"][0]["source"], source.as_ref());
    assert_eq!(commits["rows"][1]["sha"], "1234abc");
    assert_eq!(commits["rows"][1]["turn_id"], "turn-two");
    assert_eq!(
        commits["rows"][1]["user"],
        "Finish the second window without losing identity."
    );
    assert_eq!(commits["rows"][1]["source"], source.as_ref());
    assert_eq!(commits["rows"][2]["sha"], "fedcba9");
    assert_eq!(commits["rows"][2]["turn_id"], "prompt-two");
    assert_eq!(commits["rows"][2]["user"], "Finish Claude reconstruction.");
    assert_eq!(commits["rows"][2]["source"], peer_source.as_ref());
    assert_eq!(commits["rows"].as_array().map(Vec::len), Some(3));
}

pub(super) fn assert_filtered_timeline(codex: &str, claude: &str) {
    let timeline = super::reply_forms(
        &super::strings(&[
            "session",
            "timeline",
            codex,
            claude,
            "--start",
            "2026-01-01T00:00:09.500Z",
            "--contains",
            "after verification",
            "--limit",
            "1",
            "--max-text",
            "500",
        ]),
        &[],
        "timeline",
    );
    assert_eq!(timeline["rows"].as_array().map(Vec::len), Some(1));
    assert_eq!(timeline["rows"][0]["kind"], "compaction");
    assert_eq!(
        timeline["rows"][0]["text"],
        "assistant_before=Committed abcdef1 after verification. summary_after=This session is being continued from a previous conversation 1. Primary Request and Intent - preserve session forensics 2. Work Completed - first turn intent=- preserve session forensics user_after=Finish the second window without losing identity."
    );
    assert!(
        timeline["rows"][0]["source"]
            .as_str()
            .unwrap()
            .ends_with("codex.jsonl")
    );
}

pub(super) fn transcript_event(timestamp: &str, kind: &str, fields: Value) -> Value {
    let Value::Object(mut fields) = fields else {
        panic!("transcript fixture fields must be an object");
    };
    fields.insert("timestamp".to_owned(), Value::String(timestamp.to_owned()));
    fields.insert("type".to_owned(), Value::String(kind.to_owned()));
    Value::Object(fields)
}

pub(super) fn codex_event(timestamp: &str, kind: &str, payload: Value) -> Value {
    transcript_event(timestamp, kind, json!({"payload": payload}))
}

pub(super) fn codex_message(timestamp: &str, role: &str, phase: Option<&str>, text: &str) -> Value {
    codex_message_content(
        timestamp,
        role,
        phase,
        json!([{"type": "output_text", "text": text}]),
    )
}

pub(super) fn codex_message_content(
    timestamp: &str,
    role: &str,
    phase: Option<&str>,
    content: Value,
) -> Value {
    let mut payload = json!({
        "type": "message",
        "role": role,
        "content": content,
    });
    if let Some(phase) = phase {
        payload["phase"] = Value::String(phase.to_owned());
    }
    codex_event(timestamp, "response_item", payload)
}

pub(super) fn token_payload(
    input: u64,
    cached: u64,
    output: u64,
    reasoning: u64,
    total: u64,
) -> Value {
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
