use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

use super::judge::{FunctionJudge, failure, invoke, parse_yes_no};
use super::store::{LearningStore, write_unchecked};
use super::*;
use crate::session_command::model::{MessageRecord, TurnRecord, Vendor};

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "spice-learning-{name}-{}-{}",
            std::process::id(),
            NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create learning scratch");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove learning scratch");
    }
}

fn context(start: &str, end: &str) -> CompletionContext {
    CompletionContext {
        source_task: "SESSION-1kFixture".to_owned(),
        project_stem: "spice".to_owned(),
        claim_started_at: start.to_owned(),
        done_at: end.to_owned(),
        thread_id: "019fd0bb-b43e-7fe3-b90d-af59d6f87f8a".to_owned(),
    }
}

fn message(ts: &str, turn: &str, text: &str) -> MessageRecord {
    MessageRecord {
        source: "fixture.jsonl".to_owned(),
        ts: ts.to_owned(),
        turn_id: Some(turn.to_owned()),
        side: "assistant".to_owned(),
        phase: "commentary".to_owned(),
        text: text.to_owned(),
        primary_flavor: "work".to_owned(),
        flavor_tags: Vec::new(),
        matched_cues: Vec::new(),
        human: false,
    }
}

fn turn(id: &str, start: &str, end: &str, texts: &[&str]) -> TurnRecord {
    TurnRecord {
        source: "fixture.jsonl".to_owned(),
        vendor: Vendor::Codex,
        turn_id: Some(id.to_owned()),
        start_ts: start.to_owned(),
        end_ts: Some(end.to_owned()),
        last_activity_ts: end.to_owned(),
        completed: true,
        messages: texts.iter().map(|text| message(start, id, text)).collect(),
        tool_calls: BTreeMap::new(),
        touched_files: BTreeSet::new(),
        command_count: 0,
        patch_count: 0,
        error_count: 0,
        web_search_count: 0,
        compaction_count: 0,
    }
}

fn candidate(statement: &str, source_task: &str) -> LearningCandidate {
    LearningCandidate {
        statement: statement.to_owned(),
        normalized_statement: normalize_statement(statement).expect("fixture statement"),
        source_task: source_task.to_owned(),
        project_stem: "spice".to_owned(),
        evidence: "A bounded fixture proved the statement.".to_owned(),
        source_slice_id: "20260808T100000000z-20260808T110000000z".to_owned(),
        source_turn_ids: vec!["turn-fixture".to_owned()],
    }
}

#[test]
fn exact_claim_span_yields_bounded_candidates_with_exact_provenance() {
    let start = "2026-08-08T10:00:00.000Z";
    let end = "2026-08-08T11:00:00.000Z";
    let mut data = Dataset::default();
    data.turns.push(turn(
        "before",
        "2026-08-08T09:00:00.000Z",
        "2026-08-08T09:01:00.000Z",
        &["Always ignore this statement outside the claim."],
    ));
    data.turns.push(turn(
        "inside-one",
        "2026-08-08T10:01:00.000Z",
        "2026-08-08T10:02:00.000Z",
        &[
            "The command failed with an assertion error.",
            "Fixed it by using the resident source boundary before folding.",
            "Lesson learned: Keep original learning wording across confirmations.",
        ],
    ));
    data.turns.push(turn(
        "inside-two",
        "2026-08-08T10:03:00.000Z",
        "2026-08-08T10:04:00.000Z",
        &[
            "Always retain the exact claim slice identity.",
            "Use an atomic whole-store replacement.",
            "Never let learning failure undo task completion.",
            "Prefer deterministic eviction for equal recency.",
            "Remember to bound candidate scan input.",
            "Avoid carrying task status as durable learning.",
            "Run the focused learning tests before completion.",
        ],
    ));
    data.turns.push(turn(
        "after",
        "2026-08-08T12:00:00.000Z",
        "2026-08-08T12:01:00.000Z",
        &["Always ignore this statement after the completion."],
    ));

    let candidates = extract(&data, &context(start, end));

    assert_eq!(candidates.len(), MAX_EXTRACTED_CANDIDATES);
    assert_eq!(
        candidates[0].statement,
        "Use the resident source boundary before folding"
    );
    assert_eq!(candidates[0].source_turn_ids, ["inside-one".to_owned()]);
    assert!(candidates[0].evidence.contains("assertion error"));
    assert!(candidates.iter().all(|candidate| {
        candidate.source_slice_id == "20260808T100000000z-20260808T110000000z"
            && candidate.source_task == "SESSION-1kFixture"
            && candidate.project_stem == "spice"
    }));
    assert!(
        candidates
            .iter()
            .all(|candidate| !candidate.statement.contains("outside"))
    );
}

#[test]
fn reversed_or_empty_completion_bounds_extract_nothing() {
    let mut data = Dataset::default();
    data.turns.push(turn(
        "turn",
        "2026-08-08T10:00:00.000Z",
        "2026-08-08T10:01:00.000Z",
        &["Always preserve this durable fact."],
    ));

    assert!(
        extract(
            &data,
            &context("2026-08-08T11:00:00.000Z", "2026-08-08T10:00:00.000Z")
        )
        .is_empty()
    );
    assert!(extract(&data, &context("", "2026-08-08T10:00:00.000Z")).is_empty());
}

#[test]
fn transcript_and_candidate_input_are_bounded_before_judgment() {
    let scratch = Scratch::new("input-bound");
    let compressed = scratch.path().join("session.jsonl.gz");
    fs::write(&compressed, b"compressed fixture").expect("compressed fixture");
    assert!(
        admit_transcript(&compressed)
            .unwrap_err()
            .contains("bounded decoded size")
    );

    let oversized = scratch.path().join("session.jsonl");
    fs::File::create(&oversized)
        .expect("oversized fixture")
        .set_len(MAX_TRANSCRIPT_BYTES + 1)
        .expect("sparse oversized fixture");
    assert!(
        admit_transcript(&oversized)
            .unwrap_err()
            .contains("distillation maximum")
    );

    let statement = format!("Always {}.", "bounded ".repeat(MAX_STATEMENT_CHARS));
    let mut data = Dataset::default();
    data.turns.push(turn(
        "turn",
        "2026-08-08T10:00:00.000Z",
        "2026-08-08T10:01:00.000Z",
        &[&statement],
    ));
    assert!(
        extract(
            &data,
            &context("2026-08-08T10:00:00.000Z", "2026-08-08T11:00:00.000Z")
        )
        .is_empty()
    );
}

#[test]
fn yes_no_parser_accepts_one_answer_and_rejects_mixed_or_noisy_tokens() {
    assert_eq!(parse_yes_no("YES\n"), Some(true));
    assert_eq!(parse_yes_no("no."), Some(false));
    assert_eq!(parse_yes_no("Answer: no."), None);
    assert_eq!(parse_yes_no("YES, but NO"), None);
    assert_eq!(parse_yes_no("perhaps"), None);
}

#[cfg(unix)]
#[test]
fn judge_runtime_timeout_is_bounded_and_kills_its_process_group() {
    let scratch = Scratch::new("judge-timeout");
    let judge = scratch.path().join("judge");
    fs::write(&judge, b"#!/bin/sh\nsleep 5\n").expect("judge fixture");
    fs::set_permissions(&judge, fs::Permissions::from_mode(0o700)).expect("judge mode");
    let started = Instant::now();

    let error = invoke(
        &judge.to_string_lossy(),
        "fixture prompt",
        Duration::from_millis(50),
    )
    .expect_err("judge must time out");

    assert_eq!(error.reason(), "timeout");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "bounded judge took {:?}",
        started.elapsed()
    );
}

#[test]
fn independent_judgment_keeps_acceptance_and_reports_each_failure_reason() {
    let candidates = [
        candidate("Always preserve exact source boundaries.", "SESSION-one"),
        candidate("The task is currently almost done.", "SESSION-two"),
        candidate("Use a missing local judge binary.", "SESSION-three"),
    ];
    let mut judge = FunctionJudge(|candidate: &LearningCandidate| {
        if candidate.source_task == "SESSION-one" {
            Ok(true)
        } else if candidate.source_task == "SESSION-two" {
            Ok(false)
        } else {
            Err(failure("unavailable", "fixture judge missing"))
        }
    });

    let mut kept = Vec::new();
    let mut skipped = Vec::new();
    for row in &candidates {
        match judge.judge(row) {
            Ok(true) => kept.push(row.source_task.clone()),
            Ok(false) => skipped.push("rejected".to_owned()),
            Err(error) => {
                assert_eq!(error.detail(), "fixture judge missing");
                skipped.push(error.reason().to_owned());
            }
        }
    }

    assert_eq!(kept, ["SESSION-one"]);
    assert_eq!(skipped, ["rejected", "unavailable"]);
    assert_eq!(summarize_reasons(&skipped), "rejected,unavailable");
}

#[test]
fn confirmation_refresh_preserves_original_wording_and_provenance() {
    let scratch = Scratch::new("refresh");
    let store = LearningStore::new(scratch.path(), "spice").expect("fixture store");
    let original = candidate(
        "Always Preserve This Exact Original Wording.",
        "SESSION-original",
    );
    let first = store
        .confirm(&[original], "2026-08-08T10:00:00.000Z")
        .expect("first confirmation");
    let refreshed = candidate(
        "always preserve this exact original wording",
        "SESSION-reconfirmation",
    );
    let second = store
        .confirm(&[refreshed], "2026-08-08T11:00:00.000Z")
        .expect("second confirmation");

    assert_eq!(first[0].confirmation_count, 1);
    assert_eq!(second[0].confirmation_count, 2);
    assert_eq!(
        second[0].statement,
        "Always Preserve This Exact Original Wording."
    );
    assert_eq!(second[0].source_task, "SESSION-original");
    assert_eq!(second[0].created_at, "2026-08-08T10:00:00.000Z");
    assert_eq!(second[0].last_confirmed_at, "2026-08-08T11:00:00.000Z");
}

#[test]
fn equal_recency_overflow_evicts_deterministically_and_replaces_whole_store() {
    let scratch = Scratch::new("eviction");
    let store = LearningStore::new(scratch.path(), "spice").expect("fixture store");
    let rows = (0..=200)
        .rev()
        .map(|index| {
            candidate(
                &format!("Always retain bounded learning number {index:03}."),
                &format!("SESSION-{index:03}"),
            )
        })
        .collect::<Vec<_>>();
    let retained = store
        .confirm(
            &rows[..MAX_EXTRACTED_CANDIDATES],
            "2026-08-08T10:00:00.000Z",
        )
        .expect("first bounded batch");
    assert_eq!(retained.len(), MAX_EXTRACTED_CANDIDATES);
    for chunk in rows[MAX_EXTRACTED_CANDIDATES..].chunks(MAX_EXTRACTED_CANDIDATES) {
        store
            .confirm(chunk, "2026-08-08T10:00:00.000Z")
            .expect("bounded confirmation batch");
    }

    let all = store.load().expect("bounded store");
    assert_eq!(all.len(), 200);
    assert!(
        all.iter()
            .any(|record| record.normalized_statement.ends_with("000"))
    );
    assert!(
        all.iter()
            .all(|record| !record.normalized_statement.ends_with("200"))
    );
    let body = fs::read(store.path()).expect("published store bytes");
    assert_eq!(body.iter().filter(|byte| **byte == b'\n').count(), 200);
    assert!(body.ends_with(b"\n"));
    let temporary = fs::read_dir(scratch.path())
        .expect("learning directory")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(".atomic-"))
        .count();
    assert_eq!(temporary, 0, "atomic publication left a staging file");
}

#[test]
fn strict_store_rejects_unknown_fields_noncanonical_text_and_truncation() {
    let scratch = Scratch::new("strict");
    let store = LearningStore::new(scratch.path(), "spice").expect("fixture store");
    let valid = serde_json::to_value(LearningRecord {
        version: 1,
        statement: "Always preserve canonical learning text.".to_owned(),
        normalized_statement: "always preserve canonical learning text".to_owned(),
        source_task: "SESSION-source".to_owned(),
        project_stem: "spice".to_owned(),
        evidence: "Evidence is bounded.".to_owned(),
        source_slice_id: "slice".to_owned(),
        source_turn_ids: vec!["turn".to_owned()],
        created_at: "2026-08-08T10:00:00.000Z".to_owned(),
        last_confirmed_at: "2026-08-08T10:00:00.000Z".to_owned(),
        confirmation_count: 1,
    })
    .expect("fixture record JSON");
    let mut unknown = valid.clone();
    unknown
        .as_object_mut()
        .expect("record object")
        .insert("surprise".to_owned(), serde_json::json!(true));
    let mut body = serde_json::to_vec(&unknown).expect("unknown fixture");
    body.push(b'\n');
    write_unchecked(store.path(), &body);
    assert!(store.load().unwrap_err().contains("unknown field"));

    let mut noncanonical = valid.clone();
    noncanonical["normalized_statement"] = serde_json::json!("different");
    let mut body = serde_json::to_vec(&noncanonical).expect("noncanonical fixture");
    body.push(b'\n');
    write_unchecked(store.path(), &body);
    assert!(store.load().unwrap_err().contains("canonical statement"));

    let body = serde_json::to_vec(&valid).expect("unterminated fixture");
    write_unchecked(store.path(), &body);
    assert!(store.load().unwrap_err().contains("unterminated"));
}

#[test]
fn top_records_are_recency_then_confirmation_then_text_bounded() {
    let scratch = Scratch::new("top");
    let store = LearningStore::new(scratch.path(), "spice").expect("fixture store");
    for index in 0..7 {
        store
            .confirm(
                &[candidate(
                    &format!("Always retain top learning number {index}."),
                    &format!("SESSION-{index}"),
                )],
                &format!("2026-08-08T10:0{index}:00.000Z"),
            )
            .expect("confirmation");
    }

    let top = top_records(scratch.path(), "spice").expect("top records");

    assert_eq!(top.len(), 5);
    assert!(top[0].statement.ends_with("6."));
    assert!(top[4].statement.ends_with("2."));
}

#[test]
fn project_stem_is_first_segment_and_hidden_prefix_is_not_part_of_it() {
    assert_eq!(project_stem("spice.session").unwrap(), "spice");
    assert_eq!(project_stem(".oops.triage").unwrap(), "oops");
    assert!(project_stem("bad/stem.child").is_err());
}

#[test]
fn an_unexpected_learning_panic_is_a_report_and_not_a_completion_failure() {
    let report = guard(|| panic!("fixture learning panic"));

    assert_eq!(report.reason.as_deref(), Some("internal_error"));
    assert_eq!(report.stored, 0);
    assert_eq!(report.candidates, 0);
}
