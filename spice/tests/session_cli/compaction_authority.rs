use std::fs::OpenOptions;
use std::io::Write as _;

use super::*;

const REPORT_TEXT_LIMIT: usize = 180;

const REHYDRATION: &str = concat!(
    "1. Primary Request and Intent\n",
    "Preserve the continuation context.\n",
    "2. Work Completed\n",
    "The boundary was recorded."
);
const MARKER_TS: &str = "2026-02-02T00:00:10.004Z";
const ASSISTANT_BEFORE: &str = "Summary-shaped compaction prose is still prose.";
const USER_AFTER: &str = "Continue after the advertised boundary.";

#[test]
fn a_persisted_boundary_counts_once_and_preserves_continuation_context() {
    let scratch = Scratch::new("omp-boundary-authority");
    let transcript = scratch.path().join("omp-boundary-authority.jsonl");
    write_jsonl(&transcript, &before_rows());
    assert_no_boundary(&transcript);

    append_rows(
        &transcript,
        &[
            omp_compaction(MARKER_TS, "c1", REHYDRATION, "after"),
            omp_user("2026-02-02T00:00:11Z", "after", USER_AFTER),
            omp_text(
                "2026-02-02T00:00:12Z",
                "a2",
                "The observed boundary now owns the horizon.",
                "stop",
            ),
        ],
    );
    assert_one_boundary(&transcript);
    scratch.remove();
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

fn assert_one_boundary(transcript: &Path) {
    let file = transcript.to_string_lossy();
    let compactions = reply_forms(
        &strings(&["session", "compactions", &file]),
        &[],
        "compactions",
    );
    assert_eq!(compactions["rows"].as_array().map(Vec::len), Some(1));
    assert_eq!(compactions["rows"][0]["ts"], MARKER_TS);
    assert_eq!(compactions["rows"][0]["assistant_before"], ASSISTANT_BEFORE);
    assert_summary(&compactions["rows"][0]["summary_after"], REHYDRATION);
    assert_eq!(
        compactions["rows"][0]["intent"],
        "Preserve the continuation context."
    );
    assert_eq!(compactions["rows"][0]["user_after"], USER_AFTER);
    assert_eq!(compactions["residues"], json!([]));

    let briefing = reply_forms(
        &strings(&["session", "briefing", &file, "--deadline-seconds", "5"]),
        &[],
        "recall",
    );
    assert_eq!(
        briefing["horizon_basis"], "compaction-count",
        "reverse horizon basis"
    );
    assert_eq!(briefing["selected_compactions"], 1, "reverse horizon count");
    assert_eq!(
        briefing["window"]["compactions"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(briefing["window"]["compactions"][0]["ts"], MARKER_TS);
    assert_summary(
        &briefing["window"]["compactions"][0]["summary_after"],
        REHYDRATION,
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
    assert_eq!(sweep["windows"][0]["compactions"][0]["ts"], MARKER_TS);

    let tokens = reply_forms(
        &strings(&["session", "tokens", &file, "--by-file"]),
        &[],
        "tokens",
    );
    assert_eq!(
        tokens["usage"][0]["model_context_window"],
        Value::Null,
        "a session file states no context window"
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

fn before_rows() -> Vec<Value> {
    vec![
        omp_header(THREAD, "2026-02-02T00:00:00.000Z"),
        omp_user(
            "2026-02-02T00:00:01Z",
            "u1",
            "[$spice] repeated bootstrap text",
        ),
        with_usage(
            omp_text("2026-02-02T00:00:02Z", "a1", ASSISTANT_BEFORE, "stop"),
            199_000,
            0,
            1,
        ),
        omp_user(
            "2026-02-02T00:00:03Z",
            "u2",
            "[$spice] this fresh user turn is not a compaction",
        ),
    ]
}
