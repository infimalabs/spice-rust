//! Out-of-flow feedback for retained task detail and observation state.

use super::*;

#[derive(Clone, PartialEq, Eq)]
enum PendingFeedback {
    NoteSaving { task: TaskStamp, subject: String },
    NoteReconciling { task: TaskStamp, subject: String },
    Closing { task: TaskStamp, subject: String },
    Reconnecting { task: TaskStamp },
    DetailRefresh { task: TaskStamp },
    RelationsRefresh { task: TaskStamp },
}

impl PendingFeedback {
    fn task(&self) -> &TaskStamp {
        match self {
            Self::NoteSaving { task, .. }
            | Self::NoteReconciling { task, .. }
            | Self::Closing { task, .. }
            | Self::Reconnecting { task }
            | Self::DetailRefresh { task }
            | Self::RelationsRefresh { task } => task,
        }
    }

    fn message(&self) -> AttrValue {
        match self {
            Self::NoteSaving { subject, .. } => {
                format!("Saving observation for {subject}…").into()
            }
            Self::NoteReconciling { subject, .. } => {
                format!("Saved for {subject}; waiting for durable activity…").into()
            }
            Self::Closing { subject, .. } => format!("Closing {subject}…").into(),
            Self::Reconnecting { .. } => {
                "Showing admitted detail while reconnecting. Updates and observations will resume when spice reconnects.".into()
            }
            Self::DetailRefresh { .. } => {
                "Refreshing task evidence without closing this detail.".into()
            }
            Self::RelationsRefresh { .. } => {
                "Refreshing relationships without replacing this map.".into()
            }
        }
    }

    fn completion(&self) -> Option<AttrValue> {
        match self {
            Self::NoteSaving { subject, .. } | Self::NoteReconciling { subject, .. } => {
                Some(format!("Observation saved for {subject}.").into())
            }
            Self::Closing { subject, .. } => Some(format!("Closed {subject} as completed.").into()),
            Self::Reconnecting { .. }
            | Self::DetailRefresh { .. }
            | Self::RelationsRefresh { .. } => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct SettledFeedback {
    task: TaskStamp,
    message: String,
}

#[derive(Clone, Default, PartialEq, Eq)]
struct Feedback {
    pending: Option<PendingFeedback>,
    settled: Option<SettledFeedback>,
}

fn task_subject(runtime: Option<&AppRuntime>, task: &TaskStamp) -> String {
    runtime
        .and_then(|runtime| runtime.task_detail(task))
        .and_then(TaskDetailState::answer)
        .map_or_else(
            || task.to_string(),
            |detail| detail.task.statement.body.handle.to_string(),
        )
}

fn feedback(state: &AppState) -> Feedback {
    let surface = state.surface.read();
    let runtime = surface.as_ref();
    let mut note_pending = None;
    let mut note_settled = None;
    for (task, composer) in state.work_detail.notes.published.iter() {
        if note_pending.is_none() {
            note_pending = match &composer.submission {
                NoteSubmission::Pending { .. } => Some(PendingFeedback::NoteSaving {
                    task: task.clone(),
                    subject: task_subject(runtime, task),
                }),
                NoteSubmission::Reconciling { .. } => Some(PendingFeedback::NoteReconciling {
                    task: task.clone(),
                    subject: task_subject(runtime, task),
                }),
                NoteSubmission::Closing => Some(PendingFeedback::Closing {
                    task: task.clone(),
                    subject: task_subject(runtime, task),
                }),
                NoteSubmission::Idle | NoteSubmission::Failed(_) => None,
            };
        }
        if note_settled.is_none() {
            note_settled = composer
                .notice
                .is_some()
                .then(|| SettledFeedback {
                    task: task.clone(),
                    message: composer.notice.to_string(),
                })
                .or_else(|| match &composer.submission {
                    NoteSubmission::Failed(issue) => Some(SettledFeedback {
                        task: task.clone(),
                        message: issue.clone(),
                    }),
                    NoteSubmission::Idle
                    | NoteSubmission::Pending { .. }
                    | NoteSubmission::Reconciling { .. }
                    | NoteSubmission::Closing => None,
                });
        }
        if note_pending.is_some() && note_settled.is_some() {
            break;
        }
    }
    let surface_pending = state.work_detail.selected.as_ref().and_then(|task| {
        let detail = runtime.and_then(|runtime| runtime.task_detail(task));
        if matches!(&*state.connection, Connection::Offline(_))
            && detail.and_then(TaskDetailState::answer).is_some()
        {
            return Some(PendingFeedback::Reconnecting { task: task.clone() });
        }
        if state.work_map.map_open() {
            let key = state.work_map.relations_key(task);
            if matches!(
                runtime.and_then(|runtime| runtime.task_relations(&key)),
                Some(crate::TaskRelationsState::Refreshing(_))
            ) {
                return Some(PendingFeedback::RelationsRefresh { task: task.clone() });
            }
        } else if matches!(detail, Some(TaskDetailState::Refreshing(_))) {
            return Some(PendingFeedback::DetailRefresh { task: task.clone() });
        }
        None
    });
    Feedback {
        pending: note_pending.or(surface_pending),
        settled: note_settled,
    }
}

/// Keep pending task feedback visible until the state that caused it settles.
#[hook]
pub(super) fn use_feedback(state: &AppState) {
    let feedback = feedback(state);
    let previous = use_mut_ref(Feedback::default);
    let toaster = toast::use_task_toaster();
    use_effect_with(feedback, move |current| {
        let prior = previous.borrow().clone();
        if let Some(pending) = &current.pending {
            if prior.pending.as_ref() != Some(pending) {
                toaster.pending(pending.message());
            }
        } else if let Some(settled) = &current.settled
            && (prior.settled.as_ref() != Some(settled)
                || prior
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.task() == &settled.task))
        {
            toaster.settled(settled.message.clone());
        } else if let Some(pending) = &prior.pending {
            if let Some(message) = pending.completion() {
                toaster.settled(message);
            } else {
                toaster.clear();
            }
        }
        *previous.borrow_mut() = current.clone();
    });
}
