use std::collections::BTreeSet;

use dialect::Priority;
use wire::{ClaimDisposition, TaskStamp};

use crate::task::Entry;

/// Deterministic allocation scalar derived only from authoritative heads.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Urgency(i64);

impl Urgency {
    /// Score one entry against the current completed-task head set.
    ///
    /// No clock, environment value, or iteration order participates. Deferral
    /// is handled as eligibility by `next`; priority is a statement tag, and
    /// every other contribution comes from workflow, dependency, assignment,
    /// and claim heads. No priority tag contributes zero; if malformed input
    /// carries several priority tags, the strongest declared priority wins.
    #[must_use]
    pub fn from_heads(entry: &Entry, completed: &BTreeSet<TaskStamp>) -> Self {
        Self::from_heads_with_effective_priority(entry, completed, declared_priority(entry))
    }

    /// Score one entry after allocation has resolved graph-inherited priority.
    pub(crate) fn from_heads_with_effective_priority(
        entry: &Entry,
        completed: &BTreeSet<TaskStamp>,
        effective_priority: i64,
    ) -> Self {
        let workflow = entry.workflow();
        let mut score = if workflow.status == "pending" {
            10_000_i64
        } else {
            -10_000_i64
        };

        score = score.saturating_add(effective_priority);
        // `next::select` calls this only after `workable` has proved every
        // dependency satisfied, so this term is always zero for allocation.
        // It remains observable through `from_heads` for diagnostics that
        // deliberately score an arbitrary, possibly blocked head bundle.
        let missing = entry
            .statement()
            .dependencies
            .iter()
            .filter(|dependency| !completed.contains(*dependency))
            .count() as i64;
        score = score.saturating_sub(missing.saturating_mul(1_000));

        if workflow.assignment.is_none() {
            score = score.saturating_add(100);
        }
        if workflow.review == "unassigned" {
            score = score.saturating_add(10);
        }
        if entry
            .claim()
            .is_some_and(|claim| claim.disposition == ClaimDisposition::Active)
        {
            score = score.saturating_sub(20_000);
        }

        let remaining = workflow
            .flow
            .iter()
            .position(|phase| phase == &workflow.phase)
            .expect("Entry admits only a current phase inside its flow");
        let remaining = workflow.flow.len().saturating_sub(remaining);
        score = score.saturating_add(remaining as i64);
        Self(score)
    }

    /// Numeric score for rendering and diagnostics.
    #[must_use]
    pub const fn value(self) -> i64 {
        self.0
    }
}

pub(crate) fn declared_priority(entry: &Entry) -> i64 {
    entry
        .statement()
        .tags
        .iter()
        .filter_map(|tag| priority_from_tag(tag))
        .map(priority_score)
        .max()
        .unwrap_or(0)
}

/// The prefix every priority tag carries on the statement.
pub const PRIORITY_TAG_PREFIX: &str = "priority:";

/// Read one declared priority off a statement tag, which may not be one.
///
/// Priority reaches a task as a statement tag rather than a workflow field, so
/// a spelling this refuses is not an error anyone sees: it scores zero and the
/// task ranks below every task that declared a priority. Matching is exact
/// because this reads stored tags rather than authored input -- a door that
/// accepts a caller's word normalizes it with [`Priority::parse`] first, so
/// what reaches a tag is already the canonical spelling.
#[must_use]
pub fn priority_from_tag(tag: &str) -> Option<Priority> {
    priority_from_wire(tag.strip_prefix(PRIORITY_TAG_PREFIX)?)
}

/// Read one declared priority off the word a stored tag carries.
///
/// Every reader of a stored priority goes through here, so what the board
/// renders and what the allocator scores cannot disagree about which spellings
/// count.
#[must_use]
pub fn priority_from_wire(value: &str) -> Option<Priority> {
    Priority::ALL
        .into_iter()
        .find(|priority| priority.as_str() == value)
}

/// Read one priority a caller wrote, in either form this build uses.
///
/// The board renders a priority as a single letter, so an agent reading a row
/// and correcting what it just read types that letter back. Accepting the word
/// alone would refuse the exact spelling the board printed, and the predecessor
/// takes both. Case and surrounding space are the caller's business either way.
#[must_use]
pub fn priority_from_authored(value: &str) -> Option<Priority> {
    let value = value.trim();
    Priority::ALL
        .into_iter()
        .find(|priority| priority_label(*priority).eq_ignore_ascii_case(value))
        .or_else(|| Priority::parse(value))
}

/// The whole statement tag that declares one priority.
#[must_use]
pub fn priority_tag(priority: Priority) -> String {
    format!("{PRIORITY_TAG_PREFIX}{}", priority.as_str())
}

/// What one declared priority contributes to the allocation scalar.
#[must_use]
pub const fn priority_score(priority: Priority) -> i64 {
    match priority {
        Priority::Critical => 5_000,
        Priority::High => 4_000,
        Priority::Medium => 3_000,
        Priority::Low => 2_000,
        Priority::None => 1_000,
    }
}

/// The single-letter form the board renders for one declared priority.
#[must_use]
pub const fn priority_label(priority: Priority) -> &'static str {
    match priority {
        Priority::Critical => "C",
        Priority::High => "H",
        Priority::Medium => "M",
        Priority::Low => "L",
        Priority::None => "N",
    }
}
