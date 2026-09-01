use std::fs::OpenOptions;
use std::io::Write as _;

use super::*;

struct BoundaryCase {
    label: &'static str,
    rows: Vec<Value>,
    marker: Value,
    after: Vec<Value>,
    marker_ts: &'static str,
}

#[test]
fn only_transcript_advertised_boundaries_partition_codex_and_claude_sessions() {
    for case in cases() {
        let scratch = Scratch::new(case.label);
        let transcript = scratch.path().join(format!("{}.jsonl", case.label));
        write_jsonl(&transcript, &case.rows);
        assert_no_boundary(&transcript);

        append_rows(&transcript, std::slice::from_ref(&case.marker));
        append_rows(&transcript, &case.after);
        assert_one_boundary(&transcript, case.marker_ts);
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

fn assert_one_boundary(transcript: &Path, marker_ts: &str) {
    let file = transcript.to_string_lossy();
    let compactions = reply_forms(
        &strings(&["session", "compactions", &file]),
        &[],
        "compactions",
    );
    assert_eq!(compactions["rows"].as_array().map(Vec::len), Some(1));
    assert_eq!(compactions["rows"][0]["ts"], marker_ts);

    let briefing = reply_forms(
        &strings(&["session", "briefing", &file, "--deadline-seconds", "5"]),
        &[],
        "recall",
    );
    assert_eq!(briefing["horizon_basis"], "compaction-count");
    assert_eq!(briefing["selected_compactions"], 1);
    assert_eq!(
        briefing["window"]["compactions"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(briefing["window"]["compactions"][0]["ts"], marker_ts);

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
    assert_eq!(sweep["windows"][0]["compactions"][0]["ts"], marker_ts);
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

fn cases() -> [BoundaryCase; 2] {
    [codex_case(), claude_case()]
}

fn codex_case() -> BoundaryCase {
    BoundaryCase {
        label: "codex-boundary-authority",
        rows: vec![
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
                        "last_token_usage": {"input_tokens": 199_000, "total_tokens": 199_000},
                        "model_context_window": 200_000,
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
                "2026-02-01T00:00:05Z",
                "event_msg",
                json!({"type": "context_compacted"}),
            ),
        ],
        marker: codex_event("2026-02-01T00:00:10Z", "compacted", json!({})),
        after: vec![
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
    }
}

fn claude_case() -> BoundaryCase {
    BoundaryCase {
        label: "claude-boundary-authority",
        rows: vec![
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
        ],
        marker: transcript_event(
            "2026-02-02T00:00:10Z",
            "summary",
            json!({"summary": "Canonical Claude summary boundary"}),
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
            ),
        ],
        marker_ts: "2026-02-02T00:00:10.000Z",
    }
}
