//! Preparation-control settlement and stranded-work classification.

use std::collections::{BTreeMap, BTreeSet};

use wire::{TaskIntent, TaskStamp};

use crate::Entry;

/// Pending work still gated by each Plan or Design, including every pending
/// task prepared beneath one of its unreleased construction roots.
#[must_use]
pub fn controlled_pending_tasks(entries: &[Entry]) -> BTreeMap<TaskStamp, Vec<TaskStamp>> {
    let controllers = entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.statement().intent,
                Some(TaskIntent::Plan | TaskIntent::Design)
            )
        })
        .map(|entry| entry.born().clone())
        .collect::<BTreeSet<_>>();
    let released = released_tasks(entries);
    let mut controlled = BTreeMap::<TaskStamp, Vec<TaskStamp>>::new();
    for entry in entries
        .iter()
        .filter(|entry| pending(entry))
        .filter(|entry| !released.contains(entry.born()))
    {
        let Some(preparation) = &entry.statement().preparation else {
            continue;
        };
        if controllers.contains(&preparation.controller) {
            controlled
                .entry(preparation.controller.clone())
                .or_default()
                .push(entry.born().clone());
        }
    }
    controlled
}

/// Pending or completed preparation branches permanently released from their
/// outer controller by an exact Design selection or a positive inner
/// preparation conclusion.
pub(crate) fn released_tasks(entries: &[Entry]) -> BTreeSet<TaskStamp> {
    let indexed = entries
        .iter()
        .map(|entry| (entry.born(), entry))
        .collect::<BTreeMap<_, _>>();
    entries
        .iter()
        .filter(|entry| released_from_controller(entry, &indexed))
        .map(|entry| entry.born().clone())
        .collect()
}

fn released_from_controller(entry: &Entry, indexed: &BTreeMap<&TaskStamp, &Entry>) -> bool {
    let Some(preparation) = &entry.statement().preparation else {
        return false;
    };
    if preparation.construction != *entry.born() {
        return true;
    }
    let controller = &preparation.controller;
    let mut current = entry;
    let mut visited = BTreeSet::new();
    while visited.insert(current.born()) {
        if current
            .statement()
            .authorization
            .as_ref()
            .is_some_and(|decision| decision.controller == *controller)
        {
            return true;
        }
        let Some(preparation) = &current.statement().preparation else {
            return false;
        };
        if preparation.controller != *controller || preparation.parent == *controller {
            return false;
        }
        let parent = if preparation.parent == *current.born() {
            indexed.get(&preparation.construction)
        } else {
            indexed.get(&preparation.parent)
        };
        let Some(parent) = parent.copied() else {
            return false;
        };
        if parent.statement().intent != Some(TaskIntent::Design)
            && parent.workflow().concluded_positively()
        {
            return true;
        }
        current = parent;
    }
    false
}

/// Pending work still gated by `controller`, including every pending task
/// prepared beneath one of its unreleased construction roots.
///
/// A Design releases one construction root through an exact authorization.
/// That authorization releases the whole preparation subtree rooted there. A
/// Plan has no authorization record; while it is open, each construction root
/// it owns remains gated until the Plan completes positively.
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

/// Pending tasks whose preparation controller has settled negatively and can
/// no longer release their construction root, keyed by task identity.
#[must_use]
pub fn stranded_tasks(entries: &[Entry]) -> BTreeMap<TaskStamp, TaskStamp> {
    let controllers = entries
        .iter()
        .filter(|entry| controller_settled_negatively(entry))
        .map(|entry| entry.born().clone())
        .collect::<BTreeSet<_>>();
    controlled_pending_tasks(entries)
        .into_iter()
        .filter(|(controller, _)| controllers.contains(controller))
        .flat_map(|(controller, tasks)| {
            tasks
                .into_iter()
                .map(move |task| (task, controller.clone()))
        })
        .collect()
}

fn pending(entry: &Entry) -> bool {
    entry.workflow().status == "pending" && entry.workflow().completion.is_none()
}

fn controller_settled_negatively(entry: &Entry) -> bool {
    entry.workflow().status == "deleted"
        || (entry.workflow().status == "completed" && entry.workflow().review == "closed")
}
