//! Preparation settlement through ordinary dependency edges.
//!
//! Every pending task downstream of a settling Plan or Design is in its gated
//! closure while the corresponding dependency edges remain.

use std::collections::{BTreeMap, BTreeSet};

use wire::{TaskIntent, TaskStamp};

use crate::Entry;

/// Pending work each Plan or Design gates through ordinary dependency edges.
///
/// Settlement policy decides which controller outcomes consume this closure.
#[must_use]
pub fn controlled_pending_tasks(entries: &[Entry]) -> BTreeMap<TaskStamp, Vec<TaskStamp>> {
    let gated = pending_by_gate(entries);
    entries
        .iter()
        .filter(|entry| {
            entry
                .statement()
                .intent
                .is_some_and(|intent| intent == TaskIntent::Plan || intent.is_design())
        })
        .filter_map(|entry| {
            let tasks = gated_closure(&gated, entry.born());
            (!tasks.is_empty()).then(|| (entry.born().clone(), tasks))
        })
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

fn pending_by_gate(entries: &[Entry]) -> BTreeMap<&TaskStamp, Vec<&TaskStamp>> {
    let mut gated = BTreeMap::<&TaskStamp, Vec<&TaskStamp>>::new();
    for entry in entries.iter().filter(|entry| pending(entry)) {
        for dependency in &entry.statement().dependencies {
            gated.entry(dependency).or_default().push(entry.born());
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
    entry.workflow().status.is_pending()
}
