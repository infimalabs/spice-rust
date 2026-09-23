//! Selection, focus, and observation lifecycle for contextual Work detail.

use super::*;
pub(super) struct Controls {
    pub selected: UseStateHandle<Option<TaskStamp>>,
    pub close_ref: NodeRef,
    pub note_text: UseStateHandle<String>,
    pub notice: UseStateHandle<Notice>,
    pub submission: NoteSubmissionHandle,
    pub select: Callback<TaskStamp>,
    pub close: Callback<()>,
    pub submit_note: Callback<SubmitEvent>,
}

#[hook]
pub(super) fn use_controls(sender: SenderHandle, surface: UseStateHandle<SurfaceData>) -> Controls {
    let selected = use_state(|| None::<TaskStamp>);
    let close_ref = use_node_ref();
    let note_text = use_state(String::new);
    let notice = use_state(Notice::none);
    let submission_status = use_state(NoteSubmission::default);
    let submission = NoteSubmissionHandle {
        state: use_mut_ref(NoteSubmission::default),
        status: submission_status,
    };
    let select = select_callback(
        sender.clone(),
        surface.clone(),
        selected.clone(),
        notice.clone(),
    );
    let close = close_callback(selected.clone());
    use_focus(&selected, close_ref.clone());
    let submit_note = note_submit_callback(
        sender,
        surface,
        selected.clone(),
        note_text.clone(),
        notice.clone(),
        submission.clone(),
    );
    Controls {
        selected,
        close_ref,
        note_text,
        notice,
        submission,
        select,
        close,
        submit_note,
    }
}

pub(super) fn select_callback(
    sender: SenderHandle,
    surface: UseStateHandle<SurfaceData>,
    selected: UseStateHandle<Option<TaskStamp>>,
    note_notice: UseStateHandle<Notice>,
) -> Callback<TaskStamp> {
    Callback::from(move |task: TaskStamp| {
        selected.set(Some(task.clone()));
        note_notice.set(Notice::none());
        let request = {
            let mut data = surface.write();
            let Some(runtime) = data.as_mut() else {
                return;
            };
            if matches!(
                runtime.task_detail(&task),
                Some(TaskDetailState::Available(_))
            ) {
                false
            } else {
                runtime.begin_task_detail(task.clone());
                true
            }
        };
        surface.set(surface.publication());
        if request {
            let _admitted = admit_action(&sender, ClientAction::TaskDetail { task });
        }
    })
}

pub(super) fn close_callback(selected: UseStateHandle<Option<TaskStamp>>) -> Callback<()> {
    Callback::from(move |()| selected.set(None))
}

#[hook]
pub(super) fn use_focus(selected: &UseStateHandle<Option<TaskStamp>>, close: NodeRef) {
    let last_selected = use_mut_ref(|| None::<TaskStamp>);
    use_effect_with((**selected).clone(), move |selected| {
        let target = match selected {
            Some(task) => {
                *last_selected.borrow_mut() = Some(task.clone());
                None
            }
            None => last_selected.borrow_mut().take(),
        };
        let opening = selected.is_some();
        let delay = if opening { 0 } else { 50 };
        gloo_timers::callback::Timeout::new(delay, move || {
            if opening {
                if let Some(button) = close.cast::<web_sys::HtmlElement>() {
                    let _ = button.focus();
                }
                return;
            }
            let focused = target
                .as_ref()
                .and_then(|task| {
                    let document = web_sys::window()?.document()?;
                    let selector = format!(".work-card-open[data-task-stamp=\"{task}\"]");
                    document.query_selector(&selector).ok().flatten()
                })
                .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
                .filter(|element| element.offset_width() > 0 || element.offset_height() > 0)
                .is_some_and(|element| element.focus().is_ok());
            if !focused
                && target.is_some()
                && let Some(document) = web_sys::window().and_then(|window| window.document())
                && let Ok(Some(board)) = document.query_selector(".work-board-section")
                && let Ok(board) = board.dyn_into::<web_sys::HtmlElement>()
            {
                let _ = board.focus();
            }
        })
        .forget();
    });
}

pub(super) fn note_submit_callback(
    sender: SenderHandle,
    surface: UseStateHandle<SurfaceData>,
    selected: UseStateHandle<Option<TaskStamp>>,
    note_text: UseStateHandle<String>,
    note_notice: UseStateHandle<Notice>,
    note_submission: NoteSubmissionHandle,
) -> Callback<SubmitEvent> {
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        let Some(task) = (*selected).clone() else {
            return;
        };
        let (handle, after) = {
            let data = surface.read();
            let Some(TaskDetailState::Available(detail)) =
                data.as_ref().and_then(|runtime| runtime.task_detail(&task))
            else {
                return;
            };
            (
                detail.task.statement.body.handle.clone(),
                detail.notes.last().map(|note| note.position),
            )
        };
        match compose::note(handle.as_str(), &note_text) {
            Ok(note) => {
                let Some(id) = action_identity(&sender) else {
                    return;
                };
                let text = note.text.clone();
                if admit_action(
                    &sender,
                    ClientAction::Note {
                        id,
                        task: note.task,
                        note: note.text,
                        after,
                    },
                ) {
                    note_notice.set(Notice::none());
                    note_submission.replace(NoteSubmission::Pending { task, text, after });
                }
            }
            Err(refusal) => note_notice.set(refusal.into()),
        }
    })
}
