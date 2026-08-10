use super::*;
use serde_json::{Value, json};

fn planner_tail_rows() -> Vec<Value> {
    vec![
        codex_event("2026-02-01T00:10:00Z", "compacted", json!({})),
        codex_event(
            "2026-02-01T00:19:00Z",
            "event_msg",
            json!({"type": "task_started", "turn_id": "straddle"}),
        ),
        codex_message(
            "2026-02-01T00:19:10Z",
            "user",
            None,
            "This turn starts before the second boundary.",
        ),
        codex_event("2026-02-01T00:20:00Z", "compacted", json!({})),
        codex_message(
            "2026-02-01T00:20:10Z",
            "assistant",
            Some("commentary"),
            "The same turn continues after the boundary.",
        ),
        codex_event(
            "2026-02-01T00:21:00Z",
            "event_msg",
            json!({"type": "task_complete", "turn_id": "straddle"}),
        ),
        codex_event(
            "2026-02-01T00:22:00Z",
            "event_msg",
            json!({"type": "task_started", "turn_id": "after"}),
        ),
        codex_message(
            "2026-02-01T00:22:10Z",
            "user",
            None,
            "This turn belongs only to the open tail window.",
        ),
        codex_event(
            "2026-02-01T00:23:00Z",
            "event_msg",
            json!({"type": "task_complete", "turn_id": "after"}),
        ),
    ]
}

fn planner_surrounding_rows() -> Vec<Value> {
    let mut rows = vec![
        codex_event(
            "2026-02-01T00:00:00Z",
            "event_msg",
            json!({"type": "task_started", "turn_id": "ignored-before"}),
        ),
        codex_message(
            "2026-02-01T00:00:10Z",
            "assistant",
            Some("final_answer"),
            "Older assistant text must not become the selected compaction predecessor.",
        ),
        codex_event(
            "2026-02-01T00:01:00Z",
            "event_msg",
            json!({"type": "task_complete", "turn_id": "ignored-before"}),
        ),
        codex_event(
            "2026-02-01T00:09:59Z",
            "session_meta",
            json!({"type": "session_meta"}),
        ),
    ];
    rows.extend(planner_tail_rows());
    rows
}

fn strip_sources(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("source");
            for value in object.values_mut() {
                strip_sources(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                strip_sources(value);
            }
        }
        _ => {}
    }
}

fn without_sources(mut value: Value) -> Value {
    strip_sources(&mut value);
    value
}

fn window_turn_ids(window: &Value) -> Vec<String> {
    window["turns"]
        .as_array()
        .expect("window turns are typed")
        .iter()
        .map(|turn| {
            turn["turn_id"]
                .as_str()
                .expect("fixture turns carry ids")
                .to_owned()
        })
        .collect()
}

fn window_compaction_timestamps(window: &Value) -> Vec<String> {
    window["compactions"]
        .as_array()
        .expect("window compactions are typed")
        .iter()
        .map(|row| {
            row["ts"]
                .as_str()
                .expect("fixture compactions carry timestamps")
                .to_owned()
        })
        .collect()
}

#[test]
fn sweep_windows_share_boundary_straddling_turns() {
    let scratch = Scratch::new("session-window-straddle");
    let transcript = scratch.path().join("planner.jsonl");
    write_jsonl(&transcript, &planner_tail_rows());
    let input = transcript.to_string_lossy().into_owned();

    let sweep = reply_forms(
        &strings(&["session", "sweep", &input, "--count", "2"]),
        &[],
        "sweep",
    );
    let windows = sweep["windows"].as_array().expect("windows are typed");

    assert_eq!(windows.len(), 2);
    assert_eq!(windows[0]["index"], 1);
    assert_eq!(windows[1]["index"], 2);
    assert_eq!(
        window_compaction_timestamps(&windows[0]),
        ["2026-02-01T00:10:00.000Z"]
    );
    assert_eq!(
        window_compaction_timestamps(&windows[1]),
        ["2026-02-01T00:20:00.000Z"]
    );
    assert_eq!(window_turn_ids(&windows[0]), ["straddle"]);
    assert_eq!(window_turn_ids(&windows[1]), ["straddle", "after"]);

    scratch.remove();
}

#[test]
fn sweep_does_not_copy_exact_boundary_starts_backward() {
    let scratch = Scratch::new("session-window-boundary-start");
    let transcript = scratch.path().join("boundary.jsonl");
    write_jsonl(
        &transcript,
        &[
            codex_event("2026-02-01T00:10:00Z", "compacted", json!({})),
            codex_event(
                "2026-02-01T00:19:00Z",
                "event_msg",
                json!({"type": "task_started", "turn_id": "before"}),
            ),
            codex_message(
                "2026-02-01T00:19:10Z",
                "user",
                None,
                "This turn stays before the boundary.",
            ),
            codex_event(
                "2026-02-01T00:19:30Z",
                "event_msg",
                json!({"type": "task_complete", "turn_id": "before"}),
            ),
            codex_event("2026-02-01T00:20:00Z", "compacted", json!({})),
            codex_event(
                "2026-02-01T00:20:00Z",
                "event_msg",
                json!({"type": "task_started", "turn_id": "boundary-start"}),
            ),
            codex_message(
                "2026-02-01T00:20:05Z",
                "user",
                None,
                "This turn starts exactly at the boundary.",
            ),
            codex_event(
                "2026-02-01T00:20:30Z",
                "event_msg",
                json!({"type": "task_complete", "turn_id": "boundary-start"}),
            ),
        ],
    );
    let input = transcript.to_string_lossy().into_owned();

    let sweep = reply_forms(
        &strings(&["session", "sweep", &input, "--count", "2"]),
        &[],
        "sweep",
    );
    let windows = sweep["windows"].as_array().expect("windows are typed");

    assert_eq!(windows.len(), 2);
    assert_eq!(window_turn_ids(&windows[0]), ["before"]);
    assert_eq!(window_turn_ids(&windows[1]), ["boundary-start"]);

    scratch.remove();
}

#[test]
fn recall_and_sweep_ignore_surrounding_input_before_the_recent_horizon() {
    let scratch = Scratch::new("session-window-horizon");
    let base = scratch.path().join("base.jsonl");
    let surrounded = scratch.path().join("surrounded.jsonl");
    write_jsonl(&base, &planner_tail_rows());
    write_jsonl(&surrounded, &planner_surrounding_rows());
    let base = base.to_string_lossy().into_owned();
    let surrounded = surrounded.to_string_lossy().into_owned();

    let base_briefing = reply_forms(
        &strings(&[
            "session",
            "briefing",
            &base,
            "--max-lines",
            "100",
            "--max-bytes",
            "10000",
        ]),
        &[],
        "recall",
    );
    let surrounded_briefing = reply_forms(
        &strings(&[
            "session",
            "briefing",
            &surrounded,
            "--max-lines",
            "100",
            "--max-bytes",
            "10000",
        ]),
        &[],
        "recall",
    );
    assert_eq!(
        without_sources(base_briefing["window"].clone()),
        without_sources(surrounded_briefing["window"].clone())
    );
    assert_eq!(
        surrounded_briefing["residues"].as_array().map(Vec::len),
        Some(0)
    );

    let base_sweep = reply_forms(
        &strings(&["session", "sweep", &base, "--count", "2"]),
        &[],
        "sweep",
    );
    let surrounded_sweep = reply_forms(
        &strings(&["session", "sweep", &surrounded, "--count", "2"]),
        &[],
        "sweep",
    );
    assert_eq!(
        without_sources(base_sweep["windows"].clone()),
        without_sources(surrounded_sweep["windows"].clone())
    );
    assert_eq!(
        surrounded_sweep["residues"].as_array().map(Vec::len),
        Some(0)
    );

    scratch.remove();
}

#[test]
fn sweep_keeps_one_labelled_window_without_severing_compactions() {
    let scratch = Scratch::new("session-window-full-history");
    let transcript = scratch.path().join("plain.jsonl");
    write_jsonl(
        &transcript,
        &[
            codex_event(
                "2026-02-02T00:00:00Z",
                "event_msg",
                json!({"type": "task_started", "turn_id": "first"}),
            ),
            codex_message("2026-02-02T00:00:10Z", "user", None, "First prompt."),
            codex_event(
                "2026-02-02T00:01:00Z",
                "event_msg",
                json!({"type": "task_complete", "turn_id": "first"}),
            ),
            codex_event(
                "2026-02-02T00:02:00Z",
                "event_msg",
                json!({"type": "task_started", "turn_id": "second"}),
            ),
            codex_message("2026-02-02T00:02:10Z", "user", None, "Second prompt."),
            codex_event(
                "2026-02-02T00:03:00Z",
                "event_msg",
                json!({"type": "task_complete", "turn_id": "second"}),
            ),
        ],
    );
    let input = transcript.to_string_lossy().into_owned();

    let sweep = reply_forms(
        &strings(&["session", "sweep", &input, "--count", "3"]),
        &[],
        "sweep",
    );
    let windows = sweep["windows"].as_array().expect("windows are typed");

    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0]["index"], 1);
    assert_eq!(window_turn_ids(&windows[0]), ["first", "second"]);
    assert_eq!(windows[0]["compactions"].as_array().map(Vec::len), Some(0));

    scratch.remove();
}
