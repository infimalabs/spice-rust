use std::fs;

use serde_json::Value;

pub(super) fn assert_commit_rows(commits: &Value, file: &str, peer: &str) {
    let source = fs::canonicalize(file).expect("commit fixture path is canonical");
    let source = source.to_string_lossy();
    let peer_source = fs::canonicalize(peer).expect("peer commit fixture path is canonical");
    let peer_source = peer_source.to_string_lossy();
    let rows = commits["rows"].as_array().expect("commit rows are typed");
    assert_eq!(rows.len(), 3);
    let row = |sha: &str| {
        rows.iter()
            .find(|row| row["sha"] == sha)
            .unwrap_or_else(|| panic!("commit {sha} is listed"))
    };
    for (sha, user, source) in [
        (
            "abcdef1",
            "Please implement parity; it must preserve evidence.",
            source.as_ref(),
        ),
        (
            "1234abc",
            "Finish the second window without losing identity.",
            source.as_ref(),
        ),
        (
            "fedcba9",
            "Finish the peer reconstruction.",
            peer_source.as_ref(),
        ),
    ] {
        assert_eq!(row(sha)["user"]["text"], user);
        assert_eq!(row(sha)["source"], source);
    }
    let stamps = rows
        .iter()
        .map(|row| row["ts"].as_str().expect("commit rows are timestamped"))
        .collect::<Vec<_>>();
    assert!(
        stamps.windows(2).all(|pair| pair[0] >= pair[1]),
        "commits list newest first"
    );
}

pub(super) fn assert_filtered_timeline(file: &str, peer: &str) {
    let timeline = super::reply_forms(
        &super::strings(&[
            "session",
            "timeline",
            file,
            peer,
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
        timeline["rows"][0]["text"]["text"],
        "assistant_before=Committed abcdef1 after verification. summary_after=1. Primary Request and Intent - preserve session forensics 2. Work Completed - first turn intent=- preserve session forensics user_after=Finish the second window without losing identity."
    );
    assert!(
        timeline["rows"][0]["source"]
            .as_str()
            .unwrap()
            .ends_with("primary.jsonl")
    );
}
