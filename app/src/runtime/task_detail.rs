//! On-demand task detail: which tasks the reader's surfaces show, what each
//! detail reads as, and which shown tasks still owe a read. Demand never
//! sends; the connection's bounded pump takes owed reads as mailbox capacity
//! allows, so any number of shown tasks hydrates without saturating it.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use wire::intent::TaskDetailAnswer;
use wire::{Outcome, Response, TaskStamp};

use super::AppRuntime;
use crate::socket::Admitted;

/// Capability-scoped state for one on-demand task detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskDetailState {
    /// Shown, and not yet answered: its read is owed or in flight.
    Loading,
    /// Current task evidence and its ordered durable notes.
    Available(Rc<TaskDetailAnswer>),
    /// Last admitted evidence retained while a changed task is refreshed.
    Refreshing(Rc<TaskDetailAnswer>),
    /// The resident refused or could not serve the selected task.
    Unavailable {
        /// Stable resident reason.
        reason: String,
        /// Human diagnostic detail, when the resident supplied it.
        detail: Option<String>,
    },
}

impl TaskDetailState {
    /// Last admitted detail, retained during an in-flight refresh.
    #[must_use]
    pub fn answer(&self) -> Option<&TaskDetailAnswer> {
        match self {
            Self::Available(answer) | Self::Refreshing(answer) => Some(answer),
            Self::Loading | Self::Unavailable { .. } => None,
        }
    }

    /// Whether this settled state proves the durable task exists: an admitted
    /// answer does, and the resident's unknown-task refusal proves it does
    /// not. Loading and other refusals settle nothing.
    #[must_use]
    pub fn known(&self) -> Option<bool> {
        match self {
            Self::Available(_) | Self::Refreshing(_) => Some(true),
            Self::Unavailable { reason, .. } if reason == "task::detail-unknown" => Some(false),
            Self::Loading | Self::Unavailable { .. } => None,
        }
    }

    /// Keep a stale answer on screen while its replacement is read.
    fn refresh(&mut self) {
        *self = match std::mem::replace(self, Self::Loading) {
            Self::Available(answer) | Self::Refreshing(answer) => Self::Refreshing(answer),
            Self::Loading | Self::Unavailable { .. } => Self::Loading,
        };
    }
}

/// Detail states, the tasks surfaces show, and the reads those tasks owe.
#[derive(Default, PartialEq, Eq)]
pub(super) struct TaskDetails {
    states: BTreeMap<TaskStamp, TaskDetailState>,
    /// Tasks some surface currently shows.
    shown: BTreeSet<TaskStamp>,
    /// Tasks whose next read is owed and not yet sent.
    owed: BTreeSet<TaskStamp>,
}

impl AppRuntime {
    /// On-demand detail state retained only for this admitted capability scope.
    #[must_use]
    pub fn task_detail(&self, task: &TaskStamp) -> Option<&TaskDetailState> {
        self.task_details.states.get(task)
    }

    /// Replace the tasks the reader's surfaces show. A shown task without an
    /// admitted answer reads as Loading and owes a read unless one is already
    /// owed or in flight; a refused one is asked again. A task no longer shown
    /// draws no further reads, and its unanswered placeholder is dropped.
    /// Returns whether any read is owed.
    pub fn show_task_details(&mut self, tasks: impl IntoIterator<Item = TaskStamp>) -> bool {
        let TaskDetails {
            states,
            shown,
            owed,
        } = &mut self.task_details;
        *shown = tasks.into_iter().collect();
        owed.retain(|task| shown.contains(task));
        states.retain(|task, state| shown.contains(task) || *state != TaskDetailState::Loading);
        for task in shown.iter() {
            if matches!(
                states.get(task),
                None | Some(TaskDetailState::Unavailable { .. })
            ) {
                states.insert(task.clone(), TaskDetailState::Loading);
                owed.insert(task.clone());
            }
        }
        !owed.is_empty()
    }

    /// Ask again for one shown task whose read did not produce an answer.
    /// Returns whether a read became owed.
    pub fn retry_task_detail(&mut self, task: &TaskStamp) -> bool {
        let details = &mut self.task_details;
        if !details.shown.contains(task)
            || details
                .states
                .get(task)
                .and_then(TaskDetailState::answer)
                .is_some()
        {
            return false;
        }
        details
            .states
            .insert(task.clone(), TaskDetailState::Loading);
        details.owed.insert(task.clone())
    }

    /// Read a task again after an accepted command changed it, keeping any
    /// admitted answer on screen meanwhile. A read already in flight may
    /// predate the change, so this one is owed after it settles.
    pub fn reread_task_detail(&mut self, task: &TaskStamp) {
        let details = &mut self.task_details;
        details
            .states
            .entry(task.clone())
            .and_modify(TaskDetailState::refresh)
            .or_insert(TaskDetailState::Loading);
        details.owed.insert(task.clone());
    }

    /// Mark one task a fact changed. A shown task keeps its admitted answer
    /// while it owes a replacement read; a task no surface shows is forgotten,
    /// so showing it again reads it fresh. Returns whether a read became owed.
    pub fn invalidate_task_detail(&mut self, task: &TaskStamp) -> bool {
        let details = &mut self.task_details;
        if !details.shown.contains(task) {
            details.states.remove(task);
            return false;
        }
        let Some(state) = details.states.get_mut(task) else {
            return false;
        };
        state.refresh();
        details.owed.insert(task.clone());
        true
    }

    /// Take up to `limit` owed reads in identity order, skipping any task
    /// whose earlier read is still in flight. A taken task owes nothing more
    /// until it is restored or invalidated again.
    pub fn take_task_detail_reads(
        &mut self,
        limit: usize,
        in_flight: impl Fn(&TaskStamp) -> bool,
    ) -> Vec<TaskStamp> {
        let owed = &mut self.task_details.owed;
        let taken = owed
            .iter()
            .filter(|task| !in_flight(task))
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        for task in &taken {
            owed.remove(task);
        }
        taken
    }

    /// Owe a taken read again because the connection could not send it.
    pub fn restore_task_detail_read(&mut self, task: TaskStamp) {
        self.task_details.owed.insert(task);
    }

    /// Apply one correlated detail response without crossing capability scope.
    pub fn apply_task_detail(
        &mut self,
        task: &TaskStamp,
        admitted: &Admitted<Response<TaskDetailAnswer>>,
    ) {
        if admitted.scope() != self.accepted.scope() {
            return;
        }
        let state = match &admitted.get().outcome {
            Outcome::Reply(reply) if reply.body.task.statement.body.born == *task => {
                TaskDetailState::Available(Rc::new(reply.body.clone()))
            }
            Outcome::Reply(_) => TaskDetailState::Unavailable {
                reason: "task::detail-identity-mismatch".to_owned(),
                detail: Some("the detail answer named a different durable task".to_owned()),
            },
            Outcome::Refusal(refusal) => {
                let diagnostic = refusal.diagnostic();
                TaskDetailState::Unavailable {
                    reason: diagnostic
                        .map_or("task::detail-refused", wire::RefusalDiagnostic::reason)
                        .to_owned(),
                    detail: diagnostic
                        .and_then(wire::RefusalDiagnostic::detail)
                        .map(str::to_owned),
                }
            }
            Outcome::Fault(fault) => TaskDetailState::Unavailable {
                reason: fault.reason.clone(),
                detail: Some(fault.detail.clone()),
            },
        };
        self.task_details.states.insert(task.clone(), state);
    }

    /// A read in flight at a dropped connection never answers, and facts
    /// published while it was down were never streamed: every shown task owes
    /// a fresh read, and details no surface shows are forgotten.
    pub(super) fn reconnect_task_details(&mut self) {
        let TaskDetails {
            states,
            shown,
            owed,
        } = &mut self.task_details;
        states.retain(|task, _| shown.contains(task));
        states.values_mut().for_each(TaskDetailState::refresh);
        *owed = states.keys().cloned().collect();
    }
}

#[cfg(test)]
mod tests {
    use super::TaskDetailState;

    #[test]
    fn only_the_resident_unknown_refusal_settles_a_task_as_missing() {
        let unavailable = |reason: &str| TaskDetailState::Unavailable {
            reason: reason.to_owned(),
            detail: None,
        };
        assert_eq!(unavailable("task::detail-unknown").known(), Some(false));
        for transient in [
            "task::detail-refused",
            "task::detail-identity-mismatch",
            "connection::closed",
        ] {
            assert_eq!(unavailable(transient).known(), None, "{transient}");
        }
        assert_eq!(TaskDetailState::Loading.known(), None);
    }
}
