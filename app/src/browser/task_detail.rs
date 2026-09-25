//! Selection, focus, and observation lifecycle for contextual Work detail.

use super::*;
pub(super) struct Controls {
    pub selected: UseStateHandle<Option<TaskStamp>>,
    pub presented: UseStateHandle<bool>,
    pub closing: UseStateHandle<bool>,
    pub close_ref: NodeRef,
    pub sheet_ref: NodeRef,
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
    let presented = use_state_eq(|| false);
    let closing = use_state_eq(|| false);
    let close_ref = use_node_ref();
    let sheet_ref = use_node_ref();
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
        presented.clone(),
        closing.clone(),
        notice.clone(),
    );
    let complete_close =
        complete_close_callback(selected.clone(), presented.clone(), closing.clone());
    let close = close_callback(
        selected.clone(),
        presented.clone(),
        closing.clone(),
        complete_close.clone(),
    );
    use_present((*selected).clone(), presented.clone());
    use_close_transition(*closing, sheet_ref.clone(), complete_close);
    use_focus(&selected, close_ref.clone());
    use_escape_close(selected.is_some(), close.clone());
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
        presented,
        closing,
        close_ref,
        sheet_ref,
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
    presented: UseStateHandle<bool>,
    closing: UseStateHandle<bool>,
    note_notice: UseStateHandle<Notice>,
) -> Callback<TaskStamp> {
    Callback::from(move |task: TaskStamp| {
        presented.set(false);
        closing.set(false);
        selected.set(Some(task.clone()));
        note_notice.set(Notice::none());
        let request = {
            let mut data = surface.write();
            let Some(runtime) = data.as_mut() else {
                return;
            };
            if runtime
                .task_detail(&task)
                .and_then(TaskDetailState::answer)
                .is_some()
            {
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

fn close_callback(
    selected: UseStateHandle<Option<TaskStamp>>,
    presented: UseStateHandle<bool>,
    closing: UseStateHandle<bool>,
    complete: Callback<()>,
) -> Callback<()> {
    Callback::from(move |()| {
        if selected.is_none() {
            return;
        }
        if reduced_motion() {
            complete.emit(());
            return;
        }
        presented.set(false);
        closing.set(true);
    })
}

fn complete_close_callback(
    selected: UseStateHandle<Option<TaskStamp>>,
    presented: UseStateHandle<bool>,
    closing: UseStateHandle<bool>,
) -> Callback<()> {
    Callback::from(move |()| {
        presented.set(false);
        closing.set(false);
        selected.set(None);
    })
}

fn reduced_motion() -> bool {
    web_sys::window()
        .and_then(|window| {
            window
                .match_media("(prefers-reduced-motion: reduce)")
                .ok()
                .flatten()
        })
        .is_none_or(|query| query.matches())
}

#[hook]
fn use_present(selected: Option<TaskStamp>, presented: UseStateHandle<bool>) {
    use_effect_with(selected, move |selected| {
        let frame = selected.as_ref().map(|_| {
            let window = web_sys::window().expect("Canvas presentation has a browser window");
            let show = presented.clone();
            let callback = Closure::once(move |_: f64| show.set(true));
            let frame = window
                .request_animation_frame(callback.as_ref().unchecked_ref())
                .expect("Canvas window accepts a presentation frame");
            (window, frame, callback)
        });
        move || {
            if let Some((window, frame, callback)) = frame {
                let _ = window.cancel_animation_frame(frame);
                drop(callback);
            }
        }
    });
}

#[hook]
fn use_close_transition(closing: bool, sheet: NodeRef, complete: Callback<()>) {
    use_effect_with(
        (closing, sheet.clone(), complete),
        move |(closing, sheet, complete)| {
            let registration = closing
                .then(|| {
                    let element = sheet.cast::<web_sys::Element>()?;
                    let target = element.clone();
                    let complete = complete.clone();
                    let callback =
                        Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
                            let property = js_sys::Reflect::get(
                                event.as_ref(),
                                &JsValue::from_str("propertyName"),
                            )
                            .ok()
                            .and_then(|value| value.as_string());
                            let from_sheet = event
                                .target()
                                .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                                .is_some_and(|element| element == target);
                            if from_sheet && property.as_deref() == Some("transform") {
                                complete.emit(());
                            }
                        });
                    element
                        .add_event_listener_with_callback(
                            "transitionend",
                            callback.as_ref().unchecked_ref(),
                        )
                        .ok()?;
                    Some((element, callback))
                })
                .flatten();
            move || {
                if let Some((element, callback)) = registration {
                    let _ = element.remove_event_listener_with_callback(
                        "transitionend",
                        callback.as_ref().unchecked_ref(),
                    );
                }
            }
        },
    );
}

#[hook]
fn use_escape_close(open: bool, close: Callback<()>) {
    use_effect_with(open, move |open| {
        let registration = (*open)
            .then(|| {
                let document = web_sys::window()?.document()?;
                let callback = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
                    move |event: web_sys::KeyboardEvent| {
                        if event.key() == "Escape" {
                            event.prevent_default();
                            close.emit(());
                        }
                    },
                );
                document
                    .add_event_listener_with_callback("keydown", callback.as_ref().unchecked_ref())
                    .ok()?;
                Some((document, callback))
            })
            .flatten();
        move || {
            if let Some((document, callback)) = registration {
                let _ = document.remove_event_listener_with_callback(
                    "keydown",
                    callback.as_ref().unchecked_ref(),
                );
            }
        }
    });
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
        if selected.is_some() {
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
            let Some(detail) = data
                .as_ref()
                .and_then(|runtime| runtime.task_detail(&task))
                .and_then(TaskDetailState::answer)
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
