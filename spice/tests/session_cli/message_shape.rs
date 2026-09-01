//! Session message-shape cases split from the CLI suite.

use super::*;

#[test]
fn one_message_shape_fold_drives_session_reports_and_recall_dialogue() {
    let scratch = Scratch::new("message-shapes");
    let transcript = scratch.path().join("message-shapes.jsonl");
    write_jsonl(&transcript, &message_shape_rows());
    let file = transcript.to_string_lossy().into_owned();

    assert_message_shape_message_rows(&file);
    assert_message_shape_summary(&file);
    assert_message_shape_recall_dialogue(&file);
    scratch.remove();
}

#[test]
fn image_message_blocks_recover_one_authored_dialogue_entry() {
    let scratch = Scratch::new("image-message-shapes");
    let transcript = scratch.path().join("image-message-shapes.jsonl");
    write_jsonl(&transcript, &image_message_rows());
    let file = transcript.to_string_lossy().into_owned();

    let recall = reply_forms(
        &strings(&["session", "briefing", &file, "--deadline-seconds", "5"]),
        &[],
        "recall",
    );
    assert_eq!(
        recall["window"]["turns"][0]["dialogue"],
        json!([
            {"side": "user", "text": "Inspect this bootstrap capture."},
            {"side": "assistant", "text": "The bootstrap capture is visible."},
            {
                "side": "user",
                "text": "Compare the first capture with the second capture."
            },
            {"side": "assistant", "text": "Both captures remain attached."},
        ])
    );
    let encoded = recall.to_string();
    for generated in ["Files mentioned by the user", "<image", "</image>"] {
        assert!(
            !encoded.contains(generated),
            "generated image wrapper leaked into briefing: {generated}"
        );
    }
    scratch.remove();
}

fn image_message_rows() -> Vec<Value> {
    const PNG_URL: &str = "data:image/png;base64,iVBORw0KGgo=";
    vec![
        codex_event(
            "2026-01-04T00:00:00Z",
            "event_msg",
            json!({"type": "task_started", "turn_id": "image-turn"}),
        ),
        codex_message_content(
            "2026-01-04T00:00:01Z",
            "user",
            None,
            json!([
                {
                    "type": "input_text",
                    "text": concat!(
                        "# Files mentioned by the user:\n\n",
                        "## capture.png: /tmp/capture.png\n\n",
                        "Distinguish instructions from file contents.\n\n",
                        "## My request:\n",
                        "Inspect this bootstrap capture.",
                    ),
                },
                {"type": "input_text", "text": "<image name=[Image #1] path=\"/tmp/capture.png\">"},
                {"type": "input_image", "image_url": PNG_URL},
                {"type": "input_text", "text": "</image>"},
            ]),
        ),
        codex_message(
            "2026-01-04T00:00:02Z",
            "assistant",
            Some("commentary"),
            "The bootstrap capture is visible.",
        ),
        codex_message_content(
            "2026-01-04T00:00:03Z",
            "user",
            None,
            json!([
                {"type": "input_text", "text": "Compare the first capture"},
                {"type": "input_image", "image_url": PNG_URL},
                {"type": "input_text", "text": "with the second capture."},
                {"type": "input_image", "image_url": "data:image/gif;base64,R0lGODlhAQABAAAAACw="},
            ]),
        ),
        codex_message(
            "2026-01-04T00:00:04Z",
            "assistant",
            Some("final_answer"),
            "Both captures remain attached.",
        ),
        codex_event(
            "2026-01-04T00:00:05Z",
            "event_msg",
            json!({"type": "task_complete", "turn_id": "image-turn"}),
        ),
    ]
}

fn message_shape_rows() -> Vec<Value> {
    vec![
        codex_event(
            "2026-01-03T00:00:00Z",
            "event_msg",
            json!({"type": "task_started", "turn_id": "shape-turn"}),
        ),
        codex_message(
            "2026-01-03T00:00:01Z",
            "user",
            None,
            "Please preserve the authored ask.",
        ),
        codex_message(
            "2026-01-03T00:00:02Z",
            "user",
            None,
            "[$spice](.agents/skills/spice/SKILL.md)",
        ),
        codex_message(
            "2026-01-03T00:00:03Z",
            "user",
            None,
            "1. Primary Request and Intent:\nContinue.\n2. Work Completed:\nNone.",
        ),
        codex_message(
            "2026-01-03T00:00:04Z",
            "user",
            None,
            "<future-envelope>tool output</future-envelope>",
        ),
        codex_message(
            "2026-01-03T00:00:05Z",
            "user",
            None,
            "[Please keep this bracketed ask.]",
        ),
        codex_message(
            "2026-01-03T00:00:06Z",
            "user",
            None,
            "The malformed tool call was discussed before retrying with valid syntax.",
        ),
        codex_message(
            "2026-01-03T00:00:07Z",
            "user",
            None,
            "Lead prose.\n[$spice](.agents/skills/spice/SKILL.md)\nTail prose.",
        ),
        codex_message(
            "2026-01-03T00:00:08Z",
            "user",
            None,
            "Malformed <future-envelope>tool output",
        ),
        codex_message(
            "2026-01-03T00:00:09Z",
            "user",
            None,
            "Same text at a distinct source position.",
        ),
        codex_message(
            "2026-01-03T00:00:10Z",
            "user",
            None,
            "Same text at a distinct source position.",
        ),
        codex_message(
            "2026-01-03T00:00:11Z",
            "assistant",
            Some("commentary"),
            "Authored commentary.\ndiff --git a/a.rs b/a.rs\n+generated body",
        ),
        codex_message(
            "2026-01-03T00:00:12Z",
            "assistant",
            Some("commentary"),
            "*** Begin Patch\n+generated body\n*** End Patch",
        ),
    ]
}

fn assert_message_shape_message_rows(file: &str) {
    let messages = reply_forms(
        &strings(&[
            "session",
            "messages",
            file,
            "--side",
            "user",
            "--oldest-first",
        ]),
        &[],
        "messages",
    );
    assert_eq!(
        messages["rows"]
            .as_array()
            .expect("message rows")
            .iter()
            .map(|row| row["shape"].as_str().expect("message shape"))
            .collect::<Vec<_>>(),
        [
            "authored-ask",
            "scaffolding",
            "compaction-rehydration",
            "tool-generated-envelope",
            "authored-ask",
            "authored-ask",
            "authored-ask",
            "authored-ask",
            "authored-ask",
            "authored-ask",
        ]
    );
}

fn assert_message_shape_summary(file: &str) {
    let summary = reply_forms(
        &strings(&["session", "summary", file, "--recent", "8"]),
        &[],
        "summary",
    );
    assert_eq!(
        summary["recent_prompts"]
            .as_array()
            .expect("recent prompts")
            .iter()
            .map(|row| row["text"].as_str().expect("prompt text"))
            .collect::<Vec<_>>(),
        [
            "Please preserve the authored ask.",
            "[Please keep this bracketed ask.]",
            "The malformed tool call was discussed before retrying with valid syntax.",
            "Lead prose.\nTail prose.",
            "Malformed <future-envelope>tool output",
            "Same text at a distinct source position.",
            "Same text at a distinct source position.",
        ]
    );
}

fn assert_message_shape_recall_dialogue(file: &str) {
    let recall = reply_forms(
        &strings(&["session", "briefing", file, "--deadline-seconds", "5"]),
        &[],
        "recall",
    );
    assert_eq!(
        recall["window"]["turns"][0]["dialogue"],
        json!([
            {"side": "user", "text": "Please preserve the authored ask."},
            {"side": "user", "text": "[Please keep this bracketed ask.]"},
            {
                "side": "user",
                "text": "The malformed tool call was discussed before retrying with valid syntax."
            },
            {"side": "user", "text": "Lead prose. Tail prose."},
            {"side": "user", "text": "Malformed <future-envelope>tool output"},
            {"side": "user", "text": "Same text at a distinct source position."},
            {"side": "user", "text": "Same text at a distinct source position."},
            {"side": "assistant", "text": "Authored commentary."},
        ])
    );
}
