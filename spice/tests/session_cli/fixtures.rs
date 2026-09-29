use std::fs;

use serde_json::Value;

pub(super) fn assert_commit_rows(commits: &Value, file: &str, peer: &str) {
    let source = fs::canonicalize(file).expect("commit fixture path is canonical");
    let source = source.to_string_lossy();
    let peer_source = fs::canonicalize(peer).expect("peer commit fixture path is canonical");
    let peer_source = peer_source.to_string_lossy();
    assert_eq!(commits["rows"][0]["sha"], "abcdef1");
    assert_eq!(
        commits["rows"][0]["user"],
        "Please implement parity; it must preserve evidence."
    );
    assert_eq!(commits["rows"][0]["source"], source.as_ref());
    assert_eq!(commits["rows"][1]["sha"], "1234abc");
    assert_eq!(
        commits["rows"][1]["user"],
        "Finish the second window without losing identity."
    );
    assert_eq!(commits["rows"][1]["source"], source.as_ref());
    assert_eq!(commits["rows"][2]["sha"], "fedcba9");
    assert_eq!(
        commits["rows"][2]["user"],
        "Finish the peer reconstruction."
    );
    assert_eq!(commits["rows"][2]["source"], peer_source.as_ref());
    assert_eq!(commits["rows"].as_array().map(Vec::len), Some(3));
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
        timeline["rows"][0]["text"],
        "assistant_before=Committed abcdef1 after verification. summary_after=1. Primary Request and Intent - preserve session forensics 2. Work Completed - first turn intent=- preserve session forensics user_after=Finish the second window without losing identity."
    );
    assert!(
        timeline["rows"][0]["source"]
            .as_str()
            .unwrap()
            .ends_with("primary.jsonl")
    );
}
