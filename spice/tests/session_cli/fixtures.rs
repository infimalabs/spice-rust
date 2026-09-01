use serde_json::{Value, json};

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
