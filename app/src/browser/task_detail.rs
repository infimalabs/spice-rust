//! Selection, focus, and observation lifecycle for contextual task detail.

use super::*;
pub(super) struct Controls {
    pub selected: UseStateHandle<Option<TaskStamp>>,
    pub presented: UseStateHandle<bool>,
    pub closing: UseStateHandle<bool>,
    pub close_ref: NodeRef,
    pub sheet_ref: NodeRef,
    /// Override the captured launch control with a stable surviving element.
    pub return_focus_to: Callback<web_sys::Element>,
    pub notes: NoteComposers,
    pub select: Callback<TaskStamp>,
    pub select_history: Callback<TaskStamp>,
    pub request: Callback<TaskStamp>,
    pub close: Callback<()>,
    pub leave: Callback<()>,
    pub submit_note: Callback<(TaskStamp, SubmitEvent)>,
}

/// `mounted` is whether the Canvas root can render the detail layer; only a
/// mounted layer owns Escape, animates its close, or returns focus.
#[hook]
pub(super) fn use_controls(
    sender: SenderHandle,
    surface: UseStateHandle<SurfaceData>,
    mounted: bool,
) -> Controls {
    let selected = use_state(|| None::<TaskStamp>);
    let history_owned = use_state_eq(|| false);
    let return_focus = use_mut_ref(|| None::<web_sys::Element>);
    let presented = use_state_eq(|| false);
    let closing = use_state_eq(|| false);
    let close_ref = use_node_ref();
    let sheet_ref = use_node_ref();
    let notes = NoteComposers {
        state: use_mut_ref(BTreeMap::new),
        published: use_state(BTreeMap::new),
    };
    let request = request_callback(sender.clone(), surface.clone());
    let select = select_callback(
        request.clone(),
        selected.clone(),
        presented.clone(),
        closing.clone(),
        return_focus.clone(),
    );
    let select_history = history_select_callback(select.clone(), history_owned.clone());
    let complete_close =
        complete_close_callback(selected.clone(), presented.clone(), closing.clone());
    let close = close_callback(selected.clone(), closing.clone(), history_owned.clone());
    let leave = leave_callback(
        selected.clone(),
        presented.clone(),
        closing.clone(),
        history_owned.clone(),
        return_focus.clone(),
    );
    use_present(
        selected.is_some() && mounted && !*presented,
        presented.clone(),
    );
    overlay::use_close_transition(
        *closing,
        *presented && mounted && !overlay::reduced_motion(),
        sheet_ref.clone(),
        complete_close.clone(),
    );
    use_focus(
        &selected,
        mounted,
        close_ref.clone(),
        sheet_ref.clone(),
        return_focus.clone(),
    );
    use_history(
        request.clone(),
        selected.clone(),
        presented.clone(),
        closing.clone(),
        history_owned,
    );
    overlay::use_escape_owner(selected.is_some() && !*closing && mounted, close.clone());
    let return_focus_to = {
        let return_focus = return_focus.clone();
        Callback::from(move |target| *return_focus.borrow_mut() = Some(target))
    };
    let submit_note = note_submit_callback(sender, surface, notes.clone());
    Controls {
        selected,
        presented,
        closing,
        close_ref,
        return_focus_to,
        sheet_ref,
        notes,
        select,
        select_history,
        request,
        close,
        leave,
        submit_note,
    }
}

const TASK_DETAIL_STATE: &str = "canvasTaskDetail";

fn history_task() -> Option<TaskStamp> {
    let history = web_sys::window()?.history().ok()?;
    js_sys::Reflect::get(&history.state().ok()?, &TASK_DETAIL_STATE.into())
        .ok()?
        .as_string()?
        .parse()
        .ok()
}

fn push_history_task(task: &TaskStamp) -> bool {
    let Some(history) = web_sys::window().and_then(|window| window.history().ok()) else {
        return false;
    };
    let state = js_sys::Object::new();
    if let Ok(current) = history.state()
        && let Some(current) = current.dyn_ref::<js_sys::Object>()
    {
        js_sys::Object::assign(&state, current);
    }
    if js_sys::Reflect::set(
        &state,
        &TASK_DETAIL_STATE.into(),
        &JsValue::from(task.to_string()),
    )
    .is_err()
    {
        return false;
    }
    history.push_state(&state, "").is_ok()
}

fn history_select_callback(
    select: Callback<TaskStamp>,
    history_owned: UseStateHandle<bool>,
) -> Callback<TaskStamp> {
    Callback::from(move |task: TaskStamp| {
        history_owned.set(push_history_task(&task));
        select.emit(task);
    })
}

/// Select one task for the shared detail sheet and make sure its detail is read.
fn select_callback(
    request: Callback<TaskStamp>,
    selected: UseStateHandle<Option<TaskStamp>>,
    presented: UseStateHandle<bool>,
    closing: UseStateHandle<bool>,
    return_focus: Rc<RefCell<Option<web_sys::Element>>>,
) -> Callback<TaskStamp> {
    Callback::from(move |task: TaskStamp| {
        if selected.as_ref() == Some(&task) {
            return;
        }
        if selected.is_none() {
            *return_focus.borrow_mut() = active_focus_target();
        }
        if selected.is_none() || *closing {
            presented.set(false);
            closing.set(false);
        }
        selected.set(Some(task.clone()));
        request.emit(task);
    })
}

fn active_focus_target() -> Option<web_sys::Element> {
    web_sys::window()?
        .document()?
        .active_element()
        .filter(|element| {
            element
                .dyn_ref::<web_sys::HtmlElement>()
                .is_some_and(|element| element.tab_index() >= 0)
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

/// Begin closing the selected sheet; its slide, or the absence of one to
/// run, completes the close.
fn close_callback(
    selected: UseStateHandle<Option<TaskStamp>>,
    closing: UseStateHandle<bool>,
    history_owned: UseStateHandle<bool>,
) -> Callback<()> {
    Callback::from(move |()| {
        if selected.is_none() {
            return;
        }
        if *history_owned
            && history_task().as_ref() == selected.as_ref()
            && web_sys::window()
                .and_then(|window| window.history().ok())
                .is_some_and(|history| history.back().is_ok())
        {
            return;
        }
        closing.set(true);
    })
}

/// Leave the detail layer for a native route navigation without consuming the
/// history entry that Back uses to restore this task.
fn leave_callback(
    selected: UseStateHandle<Option<TaskStamp>>,
    presented: UseStateHandle<bool>,
    closing: UseStateHandle<bool>,
    history_owned: UseStateHandle<bool>,
    return_focus: Rc<RefCell<Option<web_sys::Element>>>,
) -> Callback<()> {
    Callback::from(move |()| {
        history_owned.set(false);
        return_focus.borrow_mut().take();
        presented.set(false);
        closing.set(false);
        selected.set(None);
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
/// Reconcile the same-URL task-detail history step on Back and Forward.
#[hook]
fn use_history(
    request: Callback<TaskStamp>,
    selected: UseStateHandle<Option<TaskStamp>>,
    presented: UseStateHandle<bool>,
    closing: UseStateHandle<bool>,
    history_owned: UseStateHandle<bool>,
) {
    let known = use_mut_ref(|| None::<TaskStamp>);
    *known.borrow_mut() = (*selected).clone();
    let owned = use_mut_ref(|| false);
    *owned.borrow_mut() = *history_owned;
    use_effect_with((), move |()| {
        let callback = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
            let current = known.borrow().clone();
            let owns_entry = *owned.borrow();
            match (history_task(), current.as_ref(), owns_entry) {
                (Some(task), shown, false) => {
                    history_owned.set(true);
                    closing.set(false);
                    if shown != Some(&task) {
                        presented.set(false);
                        selected.set(Some(task.clone()));
                        request.emit(task);
                    }
                }
                (None, Some(_), true) => {
                    history_owned.set(false);
                    closing.set(true);
                }
                _ => {}
            }
        });
        let window = web_sys::window().and_then(|window| {
            window
                .add_event_listener_with_callback("popstate", callback.as_ref().unchecked_ref())
                .ok()
                .map(|()| window)
        });
        move || {
            if let Some(window) = window {
                let _ = window.remove_event_listener_with_callback(
                    "popstate",
                    callback.as_ref().unchecked_ref(),
                );
            }
        }
    });
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

/// Focus enters the sheet only when it opens. Replacing the selected task in
/// an already-open sheet focuses its active tab instead of replaying entry
/// focus, and a close returns to the card for the last selected task.
#[hook]
pub(super) fn use_focus(
    selected: &UseStateHandle<Option<TaskStamp>>,
    mounted: bool,
    close: NodeRef,
    sheet: NodeRef,
    return_focus: Rc<RefCell<Option<web_sys::Element>>>,
) {
    let last_seen = use_mut_ref(|| None::<TaskStamp>);
    let mounted_selection = use_mut_ref(|| None::<TaskStamp>);
    {
        let last_seen = last_seen.clone();
        let mounted_selection = mounted_selection.clone();
        use_effect_with(
            ((**selected).clone(), mounted),
            move |(selected, mounted)| {
                let previous = mounted_selection.borrow_mut().take();
                if *mounted {
                    *mounted_selection.borrow_mut() = selected.clone();
                    if let Some(task) = selected {
                        *last_seen.borrow_mut() = Some(task.clone());
                    }
                    if previous.is_some()
                        && selected.is_some()
                        && previous.as_ref() != selected.as_ref()
                        && let Some(tab) = sheet
                            .cast::<web_sys::Element>()
                            .and_then(|sheet| {
                                sheet
                                    .query_selector("[role=\"tab\"][aria-selected=\"true\"]")
                                    .ok()
                                    .flatten()
                            })
                            .and_then(|tab| tab.dyn_into::<web_sys::HtmlElement>().ok())
                    {
                        let options = web_sys::FocusOptions::new();
                        options.set_prevent_scroll(true);
                        let _ = tab.focus_with_options(&options);
                    }
                }
            },
        );
    }
    use_effect_with(selected.is_some() && mounted, move |active| {
        if *active && let Some(button) = close.cast::<web_sys::HtmlElement>() {
            let options = web_sys::FocusOptions::new();
            options.set_prevent_scroll(true);
            let _ = button.focus_with_options(&options);
        }
    });
    use_effect_with((**selected).clone(), move |selected| {
        if selected.is_some() {
            return;
        }
        let target = last_seen.borrow_mut().take();
        let focused = return_focus
            .borrow_mut()
            .take()
            .filter(overlay::returnable)
            .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
            .is_some_and(|element| element.focus().is_ok());
        let focused = focused
            || target
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

/// Render task-attached images with the same preview and lightbox contract as
/// message media. An empty task contributes no attachment section.
pub(super) fn render_media(media: &[wire::Media]) -> Html {
    if media.is_empty() {
        return Html::default();
    }
    html! {
        <section class="work-detail-section work-detail-media">
            <h3>{"Attachments"}</h3>
            <div class="message-attachments">
                {for media.iter().map(|media| {
                    let identity = media
                        .identity()
                        .expect("admitted task media has a content identity")
                        .to_string();
                    let action = format!("Open {} full size", media.name);
                    html! {
                        <button
                            key={identity.clone()}
                            type="button"
                            class="message-image message-image--openable"
                            title={action.clone()}
                            aria-label={action}
                            data-image-preview=""
                            data-image-name={media.name.clone()}
                        >
                            <img
                                src={media.data_url.clone()}
                                alt={media.name.clone()}
                                data-media-identity={identity}
                            />
                        </button>
                    }
                })}
            </div>
        </section>
    }
}

/// Submit the surrounding observation form on the primary command chord.
fn note_keyboard_submit(enabled: bool) -> Callback<KeyboardEvent> {
    Callback::from(move |event: KeyboardEvent| {
        if !enabled
            || event.key() != "Enter"
            || !(event.meta_key() || event.ctrl_key())
            || event.shift_key()
        {
            return;
        }
        let Some(form) = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
            .and_then(|target| target.closest("form").ok().flatten())
            .and_then(|form| form.dyn_into::<web_sys::HtmlFormElement>().ok())
        else {
            return;
        };
        event.prevent_default();
        let _ = form.request_submit();
    })
}

/// The one observation composer every surface showing `task` renders, bound
/// to that task and preserving only that task's draft while feedback uses the root toast.
pub(super) fn render_note_composer(
    id: &'static str,
    handle: &str,
    task: &TaskStamp,
    can_note: bool,
    composer: &NoteComposer,
    submit: &Callback<(TaskStamp, SubmitEvent)>,
) -> Html {
    if !can_note {
        return html! {
            <p class="work-note-capability">{"This capability can read observations but cannot append them."}</p>
        };
    }
    let busy = composer.submission.busy();
    let form_key = task.to_string();
    let task = task.clone();
    html! {
        <form
            key={form_key}
            class="form work-note-composer"
            onsubmit={submit.reform(move |event| (task.clone(), event))}
        >
            <label for={id}>{format!("Observation for {handle}")}</label>
            <textarea
                {id}
                key={composer.draft.clone()}
                readonly={busy}
                onkeydown={note_keyboard_submit(!busy)}
            >{composer.draft.clone()}</textarea>
            <button type="submit" class="canvas-compact-control" disabled={busy}>
                {if busy { "Saving…" } else { "Add observation" }}
            </button>
        </form>
    }
}

/// Append one observation to the task named by the submitting surface.
pub(super) fn note_submit_callback(
    sender: SenderHandle,
    surface: UseStateHandle<SurfaceData>,
    notes: NoteComposers,
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
        notes.update(&task, |composer| composer.draft.clone_from(&text));
        let (handle, after) = {
            let data = surface.read();
            let Some(detail) = data
                .as_ref()
                .and_then(|runtime| runtime.task_detail(&task))
                .and_then(TaskDetailState::answer)
            else {
                notes.update(&task, |composer| {
                    composer.notice = Notice::none();
                    composer.submission = NoteSubmission::Failed(
                        "Task detail changed before the observation could be queued; the draft was retained."
                            .to_owned(),
                    );
                });
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
                    retain_note_failure(&notes, &task);
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
                    notes.update(&task, |composer| {
                        composer.notice = Notice::none();
                        composer.submission = NoteSubmission::Pending { text, after };
                    });
                } else {
                    retain_note_failure(&notes, &task);
                }
            }
            Err(refusal) => notes.update(&task, |composer| composer.notice = refusal.into()),
        }
    })
}

fn retain_note_failure(notes: &NoteComposers, task: &TaskStamp) {
    notes.update(task, |composer| {
        composer.notice = Notice::none();
        composer.submission = NoteSubmission::Failed(
            "Observation could not be queued; the draft was retained.".to_owned(),
        );
    });
}
