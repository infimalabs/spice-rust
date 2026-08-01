use std::collections::BTreeSet;

use wire::{ClaimDisposition, TaskHandle};

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
    pub fn from_heads(entry: &Entry, completed: &BTreeSet<TaskHandle>) -> Self {
        let workflow = entry.workflow();
        let mut score = if workflow.status == "pending" {
            10_000_i64
        } else {
            -10_000_i64
        };

        score = score.saturating_add(priority(entry));
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

fn priority(entry: &Entry) -> i64 {
    entry
        .statement()
        .tags
        .iter()
        .filter_map(|tag| match tag.as_str() {
            "priority:critical" => Some(5_000),
            "priority:high" => Some(4_000),
            "priority:medium" => Some(3_000),
            "priority:low" => Some(2_000),
            "priority:none" => Some(1_000),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}
