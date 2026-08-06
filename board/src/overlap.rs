//! Predictive warnings for acceptance surfaces shared by live claims.

use std::collections::BTreeSet;

use wire::{CanonicalUuid, ClaimDisposition, TaskHandle};

use crate::Entry;

/// One active claim whose authored acceptance names the same work surface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimOverlap {
    /// Other in-flight task.
    pub task: TaskHandle,
    /// Lane holding the other claim.
    pub lane: CanonicalUuid,
    /// Exact normalized acceptance terms shared by both tasks.
    pub surface: Vec<String>,
}

/// Find active claims whose acceptance shares an exact surface identifier.
///
/// Project names are deliberately absent from this decision. A project is a
/// scheduling route, not evidence that two tasks touch the same product or
/// code surface.
#[must_use]
pub fn claim_overlaps(selected: &Entry, entries: &[Entry]) -> Vec<ClaimOverlap> {
    let selected_surface = acceptance_surface(selected);
    if selected_surface.is_empty() {
        return Vec::new();
    }
    let mut overlaps = entries
        .iter()
        .filter_map(|entry| {
            let claim = entry
                .claim()
                .filter(|claim| claim.disposition == ClaimDisposition::Active)?;
            if entry.statement().handle == selected.statement().handle {
                return None;
            }
            let surface = selected_surface
                .intersection(&acceptance_surface(entry))
                .cloned()
                .collect::<Vec<_>>();
            (!surface.is_empty()).then(|| ClaimOverlap {
                task: entry.statement().handle.clone(),
                lane: claim.lane,
                surface,
            })
        })
        .collect::<Vec<_>>();
    overlaps.sort_by(|left, right| {
        left.task
            .cmp(&right.task)
            .then_with(|| left.lane.cmp(&right.lane))
    });
    overlaps
}

fn acceptance_surface(entry: &Entry) -> BTreeSet<String> {
    entry
        .statement()
        .acceptance
        .iter()
        .flat_map(|criterion| criterion.split(surface_separator))
        .filter_map(surface_term)
        .collect()
}

fn surface_separator(character: char) -> bool {
    !(character.is_ascii_alphanumeric() || matches!(character, '/' | '_' | '.' | ':' | '-'))
}

fn surface_term(raw: &str) -> Option<String> {
    let raw = raw.trim_matches(|character: char| !character.is_ascii_alphanumeric());
    if raw.len() < 3 {
        return None;
    }
    let normalized = raw.to_ascii_lowercase();
    if COMMON_TERMS.contains(&normalized.as_str()) {
        return None;
    }
    let punctuated = raw
        .bytes()
        .any(|byte| matches!(byte, b'/' | b'_' | b'.' | b':' | b'-'));
    let camel = raw
        .as_bytes()
        .windows(2)
        .any(|pair| pair[0].is_ascii_lowercase() && pair[1].is_ascii_uppercase());
    let acronym = raw.len() >= 3 && raw.bytes().all(|byte| byte.is_ascii_uppercase());
    let code_shaped = punctuated || camel || acronym;
    (code_shaped || normalized.len() >= 6).then_some(normalized)
}

// These words describe completion mechanics or generic task relationships,
// not the product/code surface on which parallel implementation would collide.
const COMMON_TERMS: &[&str] = &[
    "acceptance",
    "active",
    "all",
    "also",
    "and",
    "against",
    "another",
    "are",
    "before",
    "both",
    "can",
    "cannot",
    "claim",
    "claims",
    "current",
    "does",
    "each",
    "either",
    "every",
    "existing",
    "fixture",
    "focused",
    "for",
    "from",
    "implemented",
    "implementation",
    "into",
    "instead",
    "its",
    "merely",
    "must",
    "named",
    "none",
    "only",
    "one",
    "operator",
    "other",
    "passes",
    "preserves",
    "rather",
    "reference",
    "rendered",
    "renders",
    "result",
    "returns",
    "should",
    "shows",
    "source",
    "sourced",
    "surface",
    "survives",
    "task",
    "tasks",
    "than",
    "that",
    "the",
    "then",
    "this",
    "through",
    "two",
    "using",
    "validated",
    "validation",
    "verified",
    "where",
    "when",
    "which",
    "whose",
    "will",
    "with",
    "without",
];
