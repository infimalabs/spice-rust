//! Replay-visible ordinary dependency edges, including predecessor gates.

use std::collections::BTreeMap;

use wire::{AnyFact, TaskStamp};

use super::Authority;

/// Each task's replay-visible ordinary dependencies, beside the predecessor
/// preparation gate its birth implied rather than listed. That gate persists
/// until an edit lists it or authority resets the task's edges.
#[derive(Default)]
pub(super) struct ReplayDependencies {
    edges: BTreeMap<String, Vec<TaskStamp>>,
    implicit_gates: BTreeMap<String, TaskStamp>,
}

impl ReplayDependencies {
    pub(super) fn get(&self, task: &TaskStamp) -> Option<Vec<TaskStamp>> {
        self.edges.get(task.as_str()).cloned()
    }

    pub(super) fn set(&mut self, task: &TaskStamp, dependencies: Vec<TaskStamp>) {
        self.implicit_gates.remove(task.as_str());
        self.edges.insert(task.to_string(), dependencies);
    }

    /// Restore a preparation gate that a predecessor crossing wrote directly
    /// into the statement head.
    pub(super) fn birth(&mut self, added: &wire::body::TaskAdded) {
        let mut dependencies = added.depends.clone();
        let gate = added.preparation.as_ref().filter(|preparation| {
            preparation.construction == added.task && !dependencies.contains(&preparation.parent)
        });
        if let Some(preparation) = gate {
            dependencies.push(preparation.parent.clone());
        }
        self.set(&added.task, dependencies);
        if let Some(preparation) = gate {
            self.implicit_gates
                .insert(added.task.to_string(), preparation.parent.clone());
        }
    }

    /// Replace a task's edges, keeping an implicit gate the edit omits: a
    /// predecessor edit never listed the gate its preparation kept.
    pub(super) fn edit(&mut self, task: &TaskStamp, mut dependencies: Vec<TaskStamp>) {
        let gate = self
            .implicit_gates
            .get(task.as_str())
            .filter(|parent| !dependencies.contains(parent))
            .cloned();
        if let Some(parent) = &gate {
            dependencies.push(parent.clone());
        }
        self.set(task, dependencies);
        if let Some(parent) = gate {
            self.implicit_gates.insert(task.to_string(), parent);
        }
    }
}

/// Fold one task birth or dependency edit into `authority`'s edges.
pub(super) fn track(authority: &mut impl Authority, fact: &AnyFact) {
    if let Some(added) = fact.read::<wire::body::TaskAdded>() {
        authority.set_task_birth_dependencies(&added);
        return;
    }
    if let Some(edited) = fact.read::<wire::body::TaskEdited>()
        && let Some(dependencies) = edited.dependencies
    {
        authority.set_task_edit_dependencies(&edited.task, dependencies);
    }
}
