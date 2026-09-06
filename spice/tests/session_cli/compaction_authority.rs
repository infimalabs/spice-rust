use std::fs::OpenOptions;
use std::io::Write as _;

use super::*;

const REPORT_TEXT_LIMIT: usize = 180;

struct BoundaryCase {
    label: &'static str,
    rows: Vec<Value>,
    marker: Value,
    after: Vec<Value>,
    marker_ts: &'static str,
    summary_after: &'static str,
    intent: &'static str,
    user_after: &'static str,
    model_context_window: Option<u64>,
}

#[test]
fn every_persisted_boundary_generation_counts_once_and_preserves_continuation_context() {
    for case in cases() {
        let scratch = Scratch::new(case.label);
        let transcript = scratch.path().join(format!("{}.jsonl", case.label));
        write_jsonl(&transcript, &case.rows);
        assert_no_boundary(&transcript);

        append_rows(&transcript, std::slice::from_ref(&case.marker));
        append_rows(&transcript, &case.after);
        assert_one_boundary(&transcript, &case);
        scratch.remove();
    }
}

fn assert_no_boundary(transcript: &Path) {
    let file = transcript.to_string_lossy();
    let briefing = reply_forms(
        &strings(&["session", "briefing", &file, "--deadline-seconds", "5"]),
        &[],
        "recall",
    );
    assert_eq!(briefing["horizon_basis"], "full-history-no-compactions");
    assert_eq!(briefing["selected_compactions"], 0);

    let compactions = reply_forms(
        &strings(&["session", "compactions", &file]),
        &[],
        "compactions",
    );
    assert_eq!(compactions["rows"], json!([]));

    let sweep = reply_forms(
        &strings(&[
            "session",
            "sweep",
            &file,
            "--count",
            "3",
            "--deadline-seconds",
            "5",
        ]),
        &[],
        "sweep",
    );
    assert_eq!(sweep["windows"].as_array().map(Vec::len), Some(1));
    assert_eq!(sweep["windows"][0]["compactions"], json!([]));
}

fn assert_one_boundary(transcript: &Path, case: &BoundaryCase) {
    let file = transcript.to_string_lossy();
    let compactions = reply_forms(
        &strings(&["session", "compactions", &file]),
        &[],
        "compactions",
    );
    assert_eq!(compactions["rows"].as_array().map(Vec::len), Some(1));
    assert_eq!(compactions["rows"][0]["ts"], case.marker_ts);
    assert_summary(&compactions["rows"][0]["summary_after"], case.summary_after);
    assert_eq!(compactions["rows"][0]["intent"], case.intent);
    assert_eq!(compactions["rows"][0]["user_after"], case.user_after);

    let briefing = reply_forms(
        &strings(&["session", "briefing", &file, "--deadline-seconds", "5"]),
        &[],
        "recall",
    );
    assert_eq!(
        briefing["horizon_basis"], "compaction-count",
        "{} reverse horizon basis",
        case.label
    );
    assert_eq!(
        briefing["selected_compactions"], 1,
        "{} reverse horizon count",
        case.label
    );
    assert_eq!(
        briefing["window"]["compactions"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(briefing["window"]["compactions"][0]["ts"], case.marker_ts);
    assert_summary(
        &briefing["window"]["compactions"][0]["summary_after"],
        case.summary_after,
    );

    let sweep = reply_forms(
        &strings(&[
            "session",
            "sweep",
            &file,
            "--count",
            "3",
            "--deadline-seconds",
            "5",
        ]),
        &[],
        "sweep",
    );
    assert_eq!(sweep["windows"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        sweep["windows"][0]["compactions"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(sweep["windows"][0]["compactions"][0]["ts"], case.marker_ts);

    let tokens = reply_forms(
        &strings(&["session", "tokens", &file, "--by-file"]),
        &[],
        "tokens",
    );
    assert_eq!(
        tokens["usage"][0]["model_context_window"],
        case.model_context_window.map_or(Value::Null, Value::from),
        "{} metering must retain the observed full context window",
        case.label
    );
}

fn assert_summary(actual: &Value, expected: &str) {
    let actual = actual.as_str().expect("compaction summary is text");
    let normalized = expected.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.len() <= REPORT_TEXT_LIMIT {
        assert_eq!(actual, normalized);
    } else {
        assert!(
            normalized.starts_with(actual.trim_end_matches('…')) && actual.ends_with('…'),
            "the report carries the bounded prefix of the recovered continuation summary",
        );
    }
}

fn append_rows(path: &Path, rows: &[Value]) {
    let mut file = OpenOptions::new()
        .append(true)
        .open(path)
        .expect("transcript opens for append");
    for row in rows {
        serde_json::to_writer(&mut file, row).expect("appended row serializes");
        file.write_all(b"\n").expect("appended row terminates");
    }
}

fn cases() -> [BoundaryCase; 4] {
    [
        codex_current_case(),
        codex_legacy_case(),
        claude_current_case(),
        claude_legacy_case(),
    ]
}

fn codex_current_case() -> BoundaryCase {
    BoundaryCase {
        label: "codex-current-boundary-authority",
        rows: codex_before_rows(),
        marker: codex_event(
            "2026-02-01T00:00:10Z",
            "compacted",
            json!({
                "first_window_id": "window-0",
                "message": "",
                "previous_window_id": "window-0",
                "replacement_history": [{"type": "compaction", "id": "cmp-1"}],
                "window_id": "window-1",
                "window_number": 1,
            }),
        ),
        after: vec![
            codex_event(
                "2026-02-01T00:00:10.001Z",
                "event_msg",
                json!({"type": "context_compacted"}),
            ),
            codex_message(
                "2026-02-01T00:00:11Z",
                "user",
                None,
                "Continue after the advertised boundary.",
            ),
            codex_message(
                "2026-02-01T00:00:12Z",
                "assistant",
                Some("final_answer"),
                "The observed boundary now owns the horizon.",
            ),
        ],
        marker_ts: "2026-02-01T00:00:10.000Z",
        summary_after: "",
        intent: "[unparseable compaction-summary intent]",
        user_after: "Continue after the advertised boundary.",
        model_context_window: Some(vendor::DEFAULT_CODEX_MODEL_CONTEXT_WINDOW),
    }
}

fn codex_legacy_case() -> BoundaryCase {
    BoundaryCase {
        label: "codex-legacy-boundary-authority",
        rows: codex_before_rows(),
        marker: codex_event(
            "2026-02-01T00:00:10Z",
            "compacted",
            json!({
                "message": "",
                "replacement_history": [{"type": "compaction", "id": "cmp-legacy"}],
            }),
        ),
        after: codex_after_rows(),
        marker_ts: "2026-02-01T00:00:10.000Z",
        summary_after: "",
        intent: "[unparseable compaction-summary intent]",
        user_after: "Continue after the advertised boundary.",
        model_context_window: Some(vendor::DEFAULT_CODEX_MODEL_CONTEXT_WINDOW),
    }
}

fn codex_before_rows() -> Vec<Value> {
    vec![
        codex_event(
            "2026-02-01T00:00:01Z",
            "event_msg",
            json!({"type": "task_started", "turn_id": "before"}),
        ),
        codex_message(
            "2026-02-01T00:00:02Z",
            "user",
            None,
            "[$spice] repeated bootstrap text does not advertise compaction",
        ),
        codex_event(
            "2026-02-01T00:00:03Z",
            "response_item",
            json!({
                "type": "token_count",
                "info": {
                    "last_token_usage": {"input_tokens": 997_499, "total_tokens": 997_499},
                    "model_context_window": vendor::DEFAULT_CODEX_MODEL_CONTEXT_WINDOW,
                },
            }),
        ),
        codex_message(
            "2026-02-01T00:00:04Z",
            "assistant",
            Some("final_answer"),
            "This compaction-like prose and rehydration summary are ordinary text.",
        ),
        codex_event(
            "2026-02-01T00:00:04.500Z",
            "event_msg",
            json!({"type": "task_complete", "turn_id": "before"}),
        ),
        codex_event(
            "2026-02-01T00:00:04.750Z",
            "event_msg",
            json!({"type": "task_started", "turn_id": "fresh-bootstrap"}),
        ),
        codex_message(
            "2026-02-01T00:00:04.800Z",
            "user",
            None,
            "[$spice] this fresh user turn is not a compaction",
        ),
        codex_event(
            "2026-02-01T00:00:05Z",
            "event_msg",
            json!({"type": "context_compacted"}),
        ),
    ]
}

fn codex_after_rows() -> Vec<Value> {
    vec![
        codex_message(
            "2026-02-01T00:00:11Z",
            "user",
            None,
            "Continue after the advertised boundary.",
        ),
        codex_message(
            "2026-02-01T00:00:12Z",
            "assistant",
            Some("final_answer"),
            "The observed boundary now owns the horizon.",
        ),
    ]
}

const CLAUDE_REHYDRATION: &str = concat!(
    "This session is being continued from a previous conversation that ran out of context. ",
    "The summary below covers the earlier portion of the conversation.\n\n",
    "1. Primary Request and Intent\n",
    "Preserve the continuation context.\n",
    "2. Work Completed\n",
    "The boundary was recorded."
);

fn claude_current_case() -> BoundaryCase {
    BoundaryCase {
        label: "claude-current-boundary-authority",
        rows: claude_before_rows(),
        marker: transcript_event(
            "2026-02-02T00:00:10.004Z",
            "system",
            json!({
                "subtype": "compact_boundary",
                "content": "Conversation compacted",
                "compactMetadata": {"trigger": "auto"},
            }),
        ),
        after: vec![
            transcript_event(
                "2026-02-02T00:00:10.003Z",
                "user",
                json!({
                    "promptId": "synthetic-continuation",
                    "message": {"role": "user", "content": CLAUDE_REHYDRATION},
                }),
            ),
            transcript_event(
                "2026-02-02T00:00:11Z",
                "user",
                json!({
                    "promptId": "after",
                    "message": {"role": "user", "content": "Continue after the advertised boundary."},
                }),
            ),
            claude_final(),
        ],
        marker_ts: "2026-02-02T00:00:10.004Z",
        summary_after: CLAUDE_REHYDRATION,
        intent: "Preserve the continuation context.",
        user_after: "Continue after the advertised boundary.",
        model_context_window: None,
    }
}

fn claude_legacy_case() -> BoundaryCase {
    const SUMMARY: &str = concat!(
        "1. Primary Request and Intent\n",
        "Preserve the legacy continuation.\n",
        "2. Work Completed\n",
        "None."
    );
    BoundaryCase {
        label: "claude-legacy-boundary-authority",
        rows: claude_before_rows(),
        marker: transcript_event(
            "2026-02-02T00:00:10Z",
            "summary",
            json!({"summary": SUMMARY}),
        ),
        after: vec![
            transcript_event(
                "2026-02-02T00:00:11Z",
                "user",
                json!({
                    "promptId": "after",
                    "message": {"role": "user", "content": "Continue after the advertised boundary."},
                }),
            ),
            claude_final(),
        ],
        marker_ts: "2026-02-02T00:00:10.000Z",
        summary_after: SUMMARY,
        intent: "Preserve the legacy continuation.",
        user_after: "Continue after the advertised boundary.",
        model_context_window: None,
    }
}

fn claude_before_rows() -> Vec<Value> {
    vec![
        transcript_event(
            "2026-02-02T00:00:01Z",
            "user",
            json!({
                "promptId": "before",
                "message": {"role": "user", "content": "[$spice] repeated bootstrap text"},
            }),
        ),
        transcript_event(
            "2026-02-02T00:00:02Z",
            "assistant",
            json!({
                "message": {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "text", "text": "Summary-shaped compaction prose is still prose."}],
                    "stop_reason": "end_turn",
                    "model_context_window": 200_000,
                    "usage": {"input_tokens": 199_000, "output_tokens": 1},
                },
            }),
        ),
        transcript_event(
            "2026-02-02T00:00:03Z",
            "user",
            json!({
                "promptId": "fresh-bootstrap",
                "message": {
                    "role": "user",
                    "content": "[$spice] this fresh user turn is not a compaction",
                },
            }),
        ),
    ]
}

fn claude_final() -> Value {
    transcript_event(
        "2026-02-02T00:00:12Z",
        "assistant",
        json!({
            "message": {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "text", "text": "The observed boundary now owns the horizon."}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 2},
            },
        }),
    )
}
