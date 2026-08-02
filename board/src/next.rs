use std::collections::BTreeSet;

use wire::{Actor, ClaimDisposition, TaskHandle};

use crate::task::Entry;
use crate::urgency::Urgency;

/// Deterministic answer from the board's sole allocation query.
#[derive(Clone, Copy, Debug)]
pub struct Selection<'a> {
    entry: &'a Entry,
    urgency: Urgency,
}

impl<'a> Selection<'a> {
    /// Selected current head bundle.
    #[must_use]
    pub const fn entry(self) -> &'a Entry {
        self.entry
    }

    /// Scalar that ranked the selection.
    #[must_use]
    pub const fn urgency(self) -> Urgency {
        self.urgency
    }
}

/// Select the highest-ranked eligible task for `actor`.
///
/// This is the only allocation path in `board`. It derives completed
/// dependencies from the same supplied head set, ranks by pure urgency, and
/// breaks ties by ascending handle spelling. Identical heads and actor always
/// return the same task regardless of slice order.
#[must_use]
pub fn select<'a>(entries: &'a [Entry], actor: &Actor) -> Option<Selection<'a>> {
    let completed: BTreeSet<TaskHandle> = entries
        .iter()
        .filter(|entry| entry.workflow().status == "completed")
        .map(|entry| entry.statement().handle.clone())
        .collect();

    let mut selected: Option<Selection<'a>> = None;
    for entry in entries
        .iter()
        .filter(|entry| eligible(entry, actor, &completed))
    {
        let candidate = Selection {
            entry,
            urgency: Urgency::from_heads(entry, &completed),
        };
        let replaces = selected.is_none_or(|current| {
            candidate.urgency > current.urgency
                || (candidate.urgency == current.urgency
                    && candidate.entry.statement().handle < current.entry.statement().handle)
        });
        if replaces {
            selected = Some(candidate);
        }
    }
    selected
}

fn eligible(entry: &Entry, actor: &Actor, completed: &BTreeSet<TaskHandle>) -> bool {
    let workflow = entry.workflow();
    workflow.status == "pending"
        && workflow.completion.is_none()
        && workflow.deferral.is_none()
        && workflow
            .assignment
            .as_ref()
            .is_none_or(|assignment| assignment == actor)
        && entry
            .statement()
            .dependencies
            .iter()
            .all(|dependency| completed.contains(dependency))
        && !entry
            .claim()
            .is_some_and(|claim| claim.disposition == ClaimDisposition::Active)
}

/// Whether one named entry satisfies the same eligibility law as allocation.
#[must_use]
pub fn eligible_entry(entries: &[Entry], entry: &Entry, actor: &Actor) -> bool {
    let completed = entries
        .iter()
        .filter(|candidate| candidate.workflow().status == "completed")
        .map(|candidate| candidate.statement().handle.clone())
        .collect::<BTreeSet<_>>();
    eligible(entry, actor, &completed)
}
