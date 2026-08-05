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
    workable(entry, actor, completed) && unheld(entry)
}

/// Every eligibility clause except possession.
///
/// Allocation and stealing agree on all of this and disagree on possession
/// alone, so possession is the only thing either caller states for itself.
fn workable(entry: &Entry, actor: &Actor, completed: &BTreeSet<TaskHandle>) -> bool {
    let workflow = entry.workflow();
    !entry.project().visibility().is_hidden()
        && workflow.status == "pending"
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
}

/// Whether no claim currently possesses the entry.
fn unheld(entry: &Entry) -> bool {
    !entry
        .claim()
        .is_some_and(|claim| claim.disposition == ClaimDisposition::Active)
}

/// Whether one named entry satisfies the same eligibility law as allocation.
#[must_use]
pub fn eligible_entry(entries: &[Entry], entry: &Entry, actor: &Actor) -> bool {
    workable(entry, actor, &completed_handles(entries)) && unheld(entry)
}

/// Whether one named entry may be taken from whoever currently holds it.
///
/// [`eligible_entry`] without the possession clause, and without any clause put
/// in its place: a steal takes the task at once rather than waiting out a lease
/// or proving the holder gone. Taking immediately is safe rather than reckless
/// because the displaced holder is told: its next renewal refuses as
/// `claim::ownership-refused` and names whoever holds the claim now.
///
/// Every other clause still applies, so a steal can no more take a hidden,
/// completed, deferred or dependency-blocked task than an allocation can.
#[must_use]
pub fn stealable_entry(entries: &[Entry], entry: &Entry, actor: &Actor) -> bool {
    workable(entry, actor, &completed_handles(entries))
}

fn completed_handles(entries: &[Entry]) -> BTreeSet<TaskHandle> {
    entries
        .iter()
        .filter(|candidate| candidate.workflow().status == "completed")
        .map(|candidate| candidate.statement().handle.clone())
        .collect()
}
