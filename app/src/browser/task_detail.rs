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
    pub request: Callback<TaskStamp>,
    pub close: Callback<()>,
    pub submit_note: Callback<(TaskStamp, SubmitEvent)>,
}

/// `mounted` is whether the Work surface currently renders the detail layer;
/// only a mounted layer owns Escape, animates its close, or returns focus.
#[hook]
pub(super) fn use_controls(
    sender: SenderHandle,
    surface: UseStateHandle<SurfaceData>,
    mounted: bool,
) -> Controls {
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
    let request = request_callback(sender.clone(), surface.clone());
    let select = select_callback(
        request.clone(),
        selected.clone(),
        presented.clone(),
        closing.clone(),
        notice.clone(),
    );
    let complete_close =
        complete_close_callback(selected.clone(), presented.clone(), closing.clone());
    let close = close_callback(
        selected.clone(),
        (*presented && mounted).then_some(closing.clone()),
        complete_close.clone(),
    );
    use_present(
        selected.is_some() && mounted && !*presented,
        presented.clone(),
    );
    use_close_transition(*closing, mounted, sheet_ref.clone(), complete_close.clone());
    use_focus(&selected, mounted, close_ref.clone());
    overlay::use_escape_owner(selected.is_some() && !*closing && mounted, close.clone());
    let submit_note = note_submit_callback(
        sender,
        surface,
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
        request,
        close,
        submit_note,
    }
}

/// Select one task for the Work sheet and make sure its detail is read.
fn select_callback(
    request: Callback<TaskStamp>,
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
        request.emit(task);
    })
}

/// Read one task's detail unless an admitted answer is already held; every
/// surface showing a task reads it here without selecting it anywhere.
fn request_callback(
    sender: SenderHandle,
    surface: UseStateHandle<SurfaceData>,
) -> Callback<TaskStamp> {
    Callback::from(move |task: TaskStamp| {
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

/// Close through the sheet transition only when a presented sheet is on
/// screen to animate; otherwise nothing would ever finish the transition.
fn close_callback(
    selected: UseStateHandle<Option<TaskStamp>>,
    animated: Option<UseStateHandle<bool>>,
    complete: Callback<()>,
) -> Callback<()> {
    Callback::from(move |()| {
        if selected.is_none() {
            return;
        }
        match animated.as_ref().filter(|_| !reduced_motion()) {
            Some(closing) => closing.set(true),
            None => complete.emit(()),
        }
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

pub(super) fn reduced_motion() -> bool {
    web_sys::window()
        .and_then(|window| {
            window
                .match_media("(prefers-reduced-motion: reduce)")
                .ok()
                .flatten()
        })
        .is_none_or(|query| query.matches())
}

/// Present the sheet one frame after a mounted layer renders it unpresented,
/// so reselecting the shown task, or a selection made before its layer
/// mounted, still slides in from the opening state instead of sticking there.
#[hook]
fn use_present(pending: bool, presented: UseStateHandle<bool>) {
    use_effect_with(pending, move |pending| {
        let frame = pending.then(|| {
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

/// Finish a close when the sheet's slide ends or is canceled, or at once when
/// the layer is no longer mounted to run it.
#[hook]
fn use_close_transition(closing: bool, mounted: bool, sheet: NodeRef, complete: Callback<()>) {
    use_effect_with(
        (closing, mounted, sheet.clone(), complete),
        move |(closing, mounted, sheet, complete)| {
            if *closing && !*mounted {
                complete.emit(());
            }
            let registration = (*closing && *mounted)
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
                    for kind in CLOSE_TRANSITION_EVENTS {
                        element
                            .add_event_listener_with_callback(
                                kind,
                                callback.as_ref().unchecked_ref(),
                            )
                            .ok()?;
                    }
                    Some((element, callback))
                })
                .flatten();
            move || {
                if let Some((element, callback)) = registration {
                    for kind in CLOSE_TRANSITION_EVENTS {
                        let _ = element.remove_event_listener_with_callback(
                            kind,
                            callback.as_ref().unchecked_ref(),
                        );
                    }
                }
            }
        },
    );
}

const CLOSE_TRANSITION_EVENTS: [&str; 2] = ["transitionend", "transitioncancel"];

/// Focus enters the sheet once each time a selected sheet mounts, and a close
/// returns it to the card that opened a sheet the operator saw.
#[hook]
pub(super) fn use_focus(
    selected: &UseStateHandle<Option<TaskStamp>>,
    mounted: bool,
    close: NodeRef,
) {
    let last_seen = use_mut_ref(|| None::<TaskStamp>);
    {
        let last_seen = last_seen.clone();
        use_effect_with(
            ((**selected).clone(), mounted),
            move |(selected, mounted)| {
                let Some(task) = selected else {
                    return;
                };
                *last_seen.borrow_mut() = mounted.then(|| task.clone());
                if *mounted && let Some(button) = close.cast::<web_sys::HtmlElement>() {
                    let options = web_sys::FocusOptions::new();
                    options.set_prevent_scroll(true);
                    let _ = button.focus_with_options(&options);
                }
            },
        );
    }
    use_effect_with((**selected).clone(), move |selected| {
        if selected.is_some() {
            return;
        }
        let target = last_seen.borrow_mut().take();
        let focused = target
            .as_ref()
            .and_then(|task| {
                let document = web_sys::window()?.document()?;
                let selector = format!(".work-card-open[data-task-stamp=\"{task}\"]");
                document.query_selector(&selector).ok().flatten()
            })
            .filter(overlay::returnable)
            .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
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

/// Append one observation to the task named by the submitting surface.
pub(super) fn note_submit_callback(
    sender: SenderHandle,
    surface: UseStateHandle<SurfaceData>,
    note_text: UseStateHandle<String>,
    note_notice: UseStateHandle<Notice>,
    note_submission: NoteSubmissionHandle,
) -> Callback<(TaskStamp, SubmitEvent)> {
    Callback::from(move |(task, event): (TaskStamp, SubmitEvent)| {
        event.prevent_default();
        let text = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
            .and_then(|form| form.query_selector("textarea").ok().flatten())
            .and_then(|element| element.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
            .map(|textarea| textarea.value())
            .unwrap_or_default();
        note_text.set(text.clone());
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
        match compose::note(handle.as_str(), &text) {
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
