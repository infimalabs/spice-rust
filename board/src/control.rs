//! Preparation-control settlement and stranded-work classification.
//!
//! Every pending construction root waits on exactly one gate: the task whose
//! positive act would release it. A Design gates each output it has not
//! selected; any other preparing parent, a selected Plan included, gates what
//! it prepared until it concludes positively. Settling a gate negatively
//! leaves everything it gates, and everything those gate in turn, with nothing
//! able to release it.

use std::collections::{BTreeMap, BTreeSet};

use wire::{TaskIntent, TaskStamp};

use crate::Entry;

/// Pending work each Plan or Design can no longer release once it settles
/// negatively: every pending task it gates, then every pending task those
/// gate in turn.
#[must_use]
pub fn controlled_pending_tasks(entries: &[Entry]) -> BTreeMap<TaskStamp, Vec<TaskStamp>> {
    let gated = pending_by_gate(entries);
    entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.statement().intent,
                Some(TaskIntent::Plan | TaskIntent::Design)
            )
        })
        .filter_map(|entry| {
            let tasks = gated_closure(&gated, entry.born());
            (!tasks.is_empty()).then(|| (entry.born().clone(), tasks))
        })
        .collect()
}

/// Preparation roots no gate holds: released by their controller's exact
/// selection or by their preparing parent's positive conclusion, and every
/// task inside another task's construction.
pub(crate) fn released_tasks(entries: &[Entry]) -> BTreeSet<TaskStamp> {
    let indexed = index(entries);
    entries
        .iter()
        .filter(|entry| entry.statement().preparation.is_some() && gate(entry, &indexed).is_none())
        .map(|entry| entry.born().clone())
        .collect()
}

/// Pending work the Plan or Design `controller` still gates, directly or
/// through other pending work; empty for any other task.
#[must_use]
pub fn controlled_pending<'a>(entries: &'a [Entry], controller: &TaskStamp) -> Vec<&'a Entry> {
    let Some(tasks) = controlled_pending_tasks(entries).remove(controller) else {
        return Vec::new();
    };
    let tasks = tasks.into_iter().collect::<BTreeSet<_>>();
    entries
        .iter()
        .filter(|entry| tasks.contains(entry.born()))
        .collect()
}

/// Pending tasks a negatively settled Plan or Design still gates, directly or
/// through other pending work, keyed by task identity to that controller.
#[must_use]
pub fn stranded_tasks(entries: &[Entry]) -> BTreeMap<TaskStamp, TaskStamp> {
    let gated = pending_by_gate(entries);
    entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.statement().intent,
                Some(TaskIntent::Plan | TaskIntent::Design)
            ) && settled_negatively(entry)
        })
        .flat_map(|entry| {
            gated_closure(&gated, entry.born())
                .into_iter()
                .map(|task| (task, entry.born().clone()))
        })
        .collect()
}

fn index(entries: &[Entry]) -> BTreeMap<&TaskStamp, &Entry> {
    entries.iter().map(|entry| (entry.born(), entry)).collect()
}

/// The task whose positive act would release `entry`, or `None` when nothing
/// holds it.
fn gate<'a>(entry: &'a Entry, indexed: &BTreeMap<&TaskStamp, &Entry>) -> Option<&'a TaskStamp> {
    let preparation = entry.statement().preparation.as_ref()?;
    if preparation.construction != *entry.born() || preparation.parent == *entry.born() {
        return None;
    }
    if entry
        .statement()
        .authorization
        .as_ref()
        .is_some_and(|decision| decision.controller == preparation.controller)
    {
        return None;
    }
    if preparation.parent == preparation.controller {
        return Some(&preparation.controller);
    }
    let released_by_parent = indexed.get(&preparation.parent).is_some_and(|parent| {
        parent.statement().intent != Some(TaskIntent::Design)
            && parent.workflow().concluded_positively()
    });
    (!released_by_parent).then_some(&preparation.parent)
}

fn pending_by_gate(entries: &[Entry]) -> BTreeMap<&TaskStamp, Vec<&TaskStamp>> {
    let indexed = index(entries);
    let mut gated = BTreeMap::<&TaskStamp, Vec<&TaskStamp>>::new();
    for entry in entries.iter().filter(|entry| pending(entry)) {
        if let Some(gate) = gate(entry, &indexed) {
            gated.entry(gate).or_default().push(entry.born());
        }
    }
    gated
}

/// Every pending task `root` gates, directly or through other pending work.
fn gated_closure(
    gated: &BTreeMap<&TaskStamp, Vec<&TaskStamp>>,
    root: &TaskStamp,
) -> Vec<TaskStamp> {
    let mut seen = BTreeSet::from([root]);
    let mut closure = Vec::new();
    let mut frontier = vec![root];
    while let Some(gate) = frontier.pop() {
        for &task in gated.get(gate).into_iter().flatten() {
            if seen.insert(task) {
                closure.push(task.clone());
                frontier.push(task);
            }
        }
    }
    closure
}

fn pending(entry: &Entry) -> bool {
    entry.workflow().status == "pending" && entry.workflow().completion.is_none()
}

fn settled_negatively(entry: &Entry) -> bool {
    entry.workflow().status == "deleted"
        || (entry.workflow().status == "completed" && entry.workflow().review == "closed")
}
