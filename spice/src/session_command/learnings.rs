//! Bounded task-completion learning distillation for successor sessions.
//!
//! Completion is the commit point. Every operation here is a best-effort
//! continuation after that point: transcript resolution, extraction, judging,
//! and store publication report their own skip without changing the completed
//! task outcome.

use std::collections::BTreeMap;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use serde::{Deserialize, Serialize};
use wire::intent::TaskLearning;

use super::model::{Dataset, TurnRecord};
use super::{Environment, dataset, source};
use spice::repository::Repository;

mod judge;
mod store;

#[cfg(test)]
mod tests;

use judge::{Judge, ProcessJudge};
use store::LearningStore;

const MAX_EXTRACTED_CANDIDATES: usize = 8;
const MAX_SCAN_MESSAGES: usize = 80;
const MAX_EVIDENCE_CHARS: usize = 240;
const MIN_STATEMENT_WORDS: usize = 4;
const DIAGNOSTIC_CHARS: usize = 160;
const MAX_STATEMENT_CHARS: usize = 1_000;
const MAX_TRANSCRIPT_BYTES: u64 = 128 * 1024 * 1024;

const ERROR_CUES: [&str; 6] = [
    "assertionerror",
    "error",
    "exception",
    "failed",
    "failure",
    "traceback",
];
const FIX_CUES: [&str; 6] = [
    "changed", "fixed", "passes", "resolved", "reran", "switched",
];
const MARKER_PREFIXES: [&str; 7] = [
    "lesson learned",
    "lesson",
    "learning",
    "learned that",
    "learned",
    "going forward",
    "next time",
];
const IMPERATIVE_PREFIXES: [&str; 9] = [
    "always ",
    "avoid ",
    "do not ",
    "don't ",
    "never ",
    "prefer ",
    "remember to ",
    "run ",
    "use ",
];

/// Exact completion inputs acquired before the task mutation is sent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CompletionContext {
    pub(crate) source_task: String,
    pub(crate) project_stem: String,
    pub(crate) claim_started_at: String,
    pub(crate) done_at: String,
    pub(crate) thread_id: String,
}

/// One candidate whose original wording and exact claim slice are retained.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LearningCandidate {
    pub(crate) statement: String,
    pub(crate) normalized_statement: String,
    pub(crate) source_task: String,
    pub(crate) project_stem: String,
    pub(crate) evidence: String,
    pub(crate) source_slice_id: String,
    pub(crate) source_turn_ids: Vec<String>,
}

/// Strict version-one record stored as one JSONL object.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LearningRecord {
    pub(crate) version: u8,
    pub(crate) statement: String,
    pub(crate) normalized_statement: String,
    pub(crate) source_task: String,
    pub(crate) project_stem: String,
    pub(crate) evidence: String,
    pub(crate) source_slice_id: String,
    pub(crate) source_turn_ids: Vec<String>,
    pub(crate) created_at: String,
    pub(crate) last_confirmed_at: String,
    pub(crate) confirmation_count: u64,
}

/// Run distillation without allowing any failure, including an unexpected
/// panic, to invalidate an already-durable task completion.
pub(crate) fn best_effort(
    repository: &Repository,
    environment: &Environment,
    context: CompletionContext,
) -> TaskLearning {
    guard(|| {
        let mut judge = ProcessJudge::default();
        distill(repository, environment, &context, &mut judge)
    })
}

fn guard(operation: impl FnOnce() -> TaskLearning) -> TaskLearning {
    let attempted = catch_unwind(AssertUnwindSafe(operation));
    match attempted {
        Ok(report) => report,
        Err(_) => skipped("internal_error", "learning continuation panicked"),
    }
}

fn distill(
    repository: &Repository,
    environment: &Environment,
    context: &CompletionContext,
    judge: &mut impl Judge,
) -> TaskLearning {
    let resolved = match source::resolve_thread(&context.thread_id, environment) {
        Ok(resolved) => resolved,
        Err(error) => {
            return skipped(
                "missing_transcript",
                error.observed.as_deref().unwrap_or(""),
            );
        }
    };
    if let Err(error) = admit_transcript(&resolved.path) {
        return skipped("input_bound", &error);
    }
    let data = match dataset::load(&[resolved]) {
        Ok(data) => data,
        Err(error) => return skipped("extract_error", error.observed.as_deref().unwrap_or("")),
    };
    let extracted = extract(&data, context);
    if extracted.is_empty() {
        return skipped("no_candidates", "");
    }

    let mut kept = Vec::new();
    let mut skipped_reasons = Vec::new();
    let mut skipped_details = Vec::new();
    for candidate in &extracted {
        match judge.judge(candidate) {
            Ok(true) => kept.push(candidate.clone()),
            Ok(false) => skipped_reasons.push("rejected".to_owned()),
            Err(error) => {
                skipped_reasons.push(error.reason().to_owned());
                skipped_details.push(error.detail().to_owned());
            }
        }
    }
    if kept.is_empty() {
        return TaskLearning {
            stored: 0,
            candidates: extracted.len() as u64,
            skipped: skipped_reasons.clone(),
            reason: Some(summarize_reasons(&skipped_reasons)),
            detail: clipped_detail(&skipped_details.join(" | ")),
        };
    }

    let state = match repository.admit_state() {
        Ok(state) => state,
        Err(error) => {
            return skipped_with_counts(
                "store_error",
                &error.to_string(),
                &extracted,
                skipped_reasons,
            );
        }
    };
    let store = match LearningStore::new(state.learnings(), &context.project_stem) {
        Ok(store) => store,
        Err(error) => {
            return skipped_with_counts("store_error", &error, &extracted, skipped_reasons);
        }
    };
    let confirmed = match store.confirm(&kept, &context.done_at) {
        Ok(confirmed) => confirmed,
        Err(error) => {
            return skipped_with_counts("store_error", &error, &extracted, skipped_reasons);
        }
    };
    if let Err(error) = top_records(state.learnings(), &context.project_stem) {
        return skipped_with_counts("store_error", &error, &extracted, skipped_reasons);
    }
    TaskLearning {
        stored: confirmed.len() as u64,
        candidates: extracted.len() as u64,
        skipped: skipped_reasons,
        reason: None,
        detail: None,
    }
}

fn skipped_with_counts(
    reason: &str,
    detail: &str,
    extracted: &[LearningCandidate],
    skipped_reasons: Vec<String>,
) -> TaskLearning {
    TaskLearning {
        stored: 0,
        candidates: extracted.len() as u64,
        skipped: skipped_reasons,
        reason: Some(reason.to_owned()),
        detail: clipped_detail(detail),
    }
}

fn skipped(reason: &str, detail: &str) -> TaskLearning {
    TaskLearning {
        stored: 0,
        candidates: 0,
        skipped: Vec::new(),
        reason: Some(reason.to_owned()),
        detail: clipped_detail(detail),
    }
}

pub(crate) fn unavailable(reason: &str, detail: &str) -> TaskLearning {
    skipped(reason, detail)
}

fn clipped_detail(detail: &str) -> Option<String> {
    let compact = compact(detail);
    (!compact.is_empty()).then(|| compact.chars().take(DIAGNOSTIC_CHARS).collect())
}

fn summarize_reasons(reasons: &[String]) -> String {
    let mut counts = BTreeMap::new();
    for reason in reasons {
        *counts.entry(reason.as_str()).or_insert(0_u64) += 1;
    }
    counts
        .into_iter()
        .map(|(reason, count)| {
            if count == 1 {
                reason.to_owned()
            } else {
                format!("{reason}x{count}")
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn extract(data: &Dataset, context: &CompletionContext) -> Vec<LearningCandidate> {
    if context.claim_started_at.is_empty()
        || context.done_at.is_empty()
        || context.claim_started_at > context.done_at
    {
        return Vec::new();
    }
    let mut turns = data
        .turns
        .iter()
        .filter(|turn| overlaps(turn, &context.claim_started_at, &context.done_at))
        .collect::<Vec<_>>();
    turns.sort_by(|left, right| {
        (&left.start_ts, &left.turn_id).cmp(&(&right.start_ts, &right.turn_id))
    });
    let slice_id = format!(
        "{}-{}",
        compact_timestamp(&context.claim_started_at),
        compact_timestamp(&context.done_at)
    );
    let messages = turns
        .iter()
        .flat_map(|turn| {
            turn.messages.iter().map(move |message| {
                (
                    compact(&message.text),
                    turn.turn_id.as_deref().unwrap_or(""),
                )
            })
        })
        .filter(|(text, _)| !text.is_empty())
        .take(MAX_SCAN_MESSAGES);

    let mut candidates = Vec::new();
    let mut seen = Vec::new();
    let mut pending_error: Option<(String, String)> = None;
    for (text, turn_id) in messages {
        if let Some((error, error_turn)) = &pending_error
            && is_fix(&text)
        {
            append_candidate(
                &mut candidates,
                &mut seen,
                fix_statement(&text),
                evidence(&[error, &text]),
                turn_ids(&[error_turn, turn_id]),
                context,
                &slice_id,
            );
        }
        for statement in explicit_statements(&text) {
            append_candidate(
                &mut candidates,
                &mut seen,
                statement,
                evidence(&[&text]),
                turn_ids(&[turn_id]),
                context,
                &slice_id,
            );
            if candidates.len() == MAX_EXTRACTED_CANDIDATES {
                return candidates;
            }
        }
        if candidates.len() == MAX_EXTRACTED_CANDIDATES {
            return candidates;
        }
        if contains_cue(&text, &ERROR_CUES) {
            pending_error = Some((text, turn_id.to_owned()));
        }
    }
    candidates
}

fn overlaps(turn: &TurnRecord, start: &str, end: &str) -> bool {
    let activity = turn.end_ts.as_deref().unwrap_or(&turn.last_activity_ts);
    activity >= start && turn.start_ts.as_str() <= end
}

fn append_candidate(
    candidates: &mut Vec<LearningCandidate>,
    seen: &mut Vec<String>,
    statement: String,
    evidence: String,
    source_turn_ids: Vec<String>,
    context: &CompletionContext,
    slice_id: &str,
) {
    if candidates.len() >= MAX_EXTRACTED_CANDIDATES {
        return;
    }
    let statement = clean_statement(&statement);
    if statement.split_whitespace().count() < MIN_STATEMENT_WORDS
        || statement.chars().count() > MAX_STATEMENT_CHARS
    {
        return;
    }
    let Ok(normalized_statement) = normalize_statement(&statement) else {
        return;
    };
    if seen.contains(&normalized_statement) {
        return;
    }
    seen.push(normalized_statement.clone());
    candidates.push(LearningCandidate {
        statement,
        normalized_statement,
        source_task: context.source_task.clone(),
        project_stem: context.project_stem.clone(),
        evidence,
        source_slice_id: slice_id.to_owned(),
        source_turn_ids,
    });
}

fn admit_transcript(path: &Path) -> Result<(), String> {
    if path.to_string_lossy().ends_with(".gz") {
        return Err("compressed transcript has no bounded decoded size".to_owned());
    }
    let bytes = fs::metadata(path)
        .map_err(|error| format!("measure {}: {error}", path.display()))?
        .len();
    if bytes > MAX_TRANSCRIPT_BYTES {
        return Err(format!(
            "transcript is {bytes} bytes; distillation maximum is {MAX_TRANSCRIPT_BYTES}"
        ));
    }
    Ok(())
}

fn explicit_statements(text: &str) -> Vec<String> {
    sentences(text)
        .into_iter()
        .filter_map(|sentence| {
            let lower = sentence.to_lowercase();
            for prefix in MARKER_PREFIXES {
                if lower.starts_with(prefix) {
                    return Some(
                        sentence[prefix.len()..]
                            .trim_start_matches([' ', ':', '-', ','])
                            .to_owned(),
                    );
                }
            }
            IMPERATIVE_PREFIXES
                .iter()
                .any(|prefix| lower.starts_with(prefix))
                .then_some(sentence)
        })
        .collect()
}

fn fix_statement(text: &str) -> String {
    for sentence in sentences(text) {
        let lower = sentence.to_lowercase();
        for prefix in ["fixed it by using ", "fixed this by using "] {
            if let Some(start) = lower.find(prefix) {
                return format!("Use {}", &sentence[start + prefix.len()..]);
            }
        }
        if lower.starts_with("switched to ") {
            return format!("Use {}", &sentence["switched to ".len()..]);
        }
        if contains_cue(&lower, &FIX_CUES) {
            return sentence;
        }
    }
    text.to_owned()
}

fn sentences(text: &str) -> Vec<String> {
    let mut rows = Vec::new();
    let mut start = 0;
    for (index, character) in text.char_indices() {
        if matches!(character, '.' | '!' | '?' | '\n') {
            let end = index + character.len_utf8();
            let statement = clean_statement(&text[start..end]);
            if !statement.is_empty() {
                rows.push(statement);
            }
            start = end;
        }
    }
    if start < text.len() {
        let statement = clean_statement(&text[start..]);
        if !statement.is_empty() {
            rows.push(statement);
        }
    }
    rows
}

fn is_fix(text: &str) -> bool {
    contains_cue(text, &FIX_CUES)
}

fn contains_cue(text: &str, cues: &[&str]) -> bool {
    let lower = text.to_lowercase();
    cues.iter().any(|cue| lower.contains(cue))
}

fn evidence(parts: &[&str]) -> String {
    let joined = parts
        .iter()
        .map(|part| compact(part))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" -> ");
    if joined.chars().count() <= MAX_EVIDENCE_CHARS {
        return joined;
    }
    let mut clipped = joined
        .chars()
        .take(MAX_EVIDENCE_CHARS.saturating_sub(3))
        .collect::<String>();
    clipped.push_str("...");
    clipped
}

fn turn_ids(values: &[&str]) -> Vec<String> {
    let mut ids = Vec::new();
    for value in values {
        let value = value.trim();
        if !value.is_empty() && !ids.iter().any(|existing| existing == value) {
            ids.push(value.to_owned());
        }
    }
    ids
}

pub(super) fn normalize_statement(statement: &str) -> Result<String, String> {
    let normalized = compact(statement)
        .trim_end_matches(|character: char| {
            character.is_ascii_punctuation() || character.is_whitespace()
        })
        .to_lowercase();
    if normalized.is_empty() {
        return Err("learning statement must be non-empty".to_owned());
    }
    Ok(normalized)
}

fn clean_statement(statement: &str) -> String {
    compact(statement)
        .trim_matches([' ', ':', '-', ','])
        .trim_end_matches(|character: char| {
            character.is_ascii_punctuation() || character.is_whitespace()
        })
        .to_owned()
}

fn compact(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn compact_timestamp(value: &str) -> String {
    value
        .chars()
        .filter_map(|character| match character {
            'Z' => Some('z'),
            '-' | ':' | '.' | '+' => None,
            other => Some(other),
        })
        .collect()
}

pub(crate) fn project_stem(project: &str) -> Result<String, String> {
    let body = project.trim().trim_start_matches('.');
    let stem = body.split('.').next().unwrap_or("");
    store::validate_stem(stem)?;
    Ok(stem.to_owned())
}

pub(crate) fn top_records(directory: &Path, stem: &str) -> Result<Vec<LearningRecord>, String> {
    LearningStore::new(directory, stem)?.top()
}
