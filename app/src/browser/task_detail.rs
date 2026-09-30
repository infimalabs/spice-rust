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
    pub request: Callback<TaskStamp>,
    pub close: Callback<()>,
    pub submit_note: Callback<(TaskStamp, SubmitEvent)>,
}

/// The task detail the current view asks for: the route's Work task, or a
/// browser-local task opened over Dash or Chat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Request {
    pub task: Option<TaskStamp>,
    /// The view is a Chat card route, which owns focus: ending the request
    /// then drops the sheet without its close slide or focus return.
    pub leaving: bool,
}

/// `mounted` is whether the Canvas root can render the detail layer; only a
/// mounted layer owns Escape, animates its close, or returns focus. `close`
/// ends the request; the sheet follows the request that results.
#[hook]
pub(super) fn use_controls(
    sender: SenderHandle,
    surface: UseStateHandle<SurfaceData>,
    mounted: bool,
    requested: Request,
    close: Callback<()>,
) -> Controls {
    let selected = use_state(|| None::<TaskStamp>);
    let return_focus = use_mut_ref(|| None::<web_sys::Element>);
    let presented = use_state_eq(|| false);
    let closing = use_state_eq(|| false);
    let close_ref = use_node_ref();
    let sheet_ref = use_node_ref();
    let notes = NoteComposers {
        state: use_mut_ref(BTreeMap::new),
        published: use_state(BTreeMap::new),
    };
    let ready = surface.read().is_some();
    let request = request_callback(sender.clone(), surface.clone());
    let select = select_callback(
        request.clone(),
        selected.clone(),
        presented.clone(),
        closing.clone(),
        return_focus.clone(),
    );
    let complete_close =
        complete_close_callback(selected.clone(), presented.clone(), closing.clone());
    let leave = leave_callback(
        selected.clone(),
        presented.clone(),
        closing.clone(),
        return_focus.clone(),
    );
    use_request(requested, ready, select, leave, &selected, closing.clone());
    use_present(
        selected.is_some() && mounted && !*presented,
        presented.clone(),
    );
    overlay::use_close_transition(
        *closing,
        *presented && mounted && !overlay::reduced_motion(),
        sheet_ref.clone(),
        complete_close,
    );
    use_focus(
        &selected,
        mounted,
        close_ref.clone(),
        sheet_ref.clone(),
        return_focus.clone(),
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
        request,
        close,
        submit_note,
    }
}

/// Follow the requested task: show it, read it once a runtime exists, and
/// close the sheet when the request ends.
#[hook]
fn use_request(
    requested: Request,
    ready: bool,
    select: Callback<TaskStamp>,
    leave: Callback<()>,
    selected: &UseStateHandle<Option<TaskStamp>>,
    closing: UseStateHandle<bool>,
) {
    let shown = selected.is_some();
    use_effect_with((requested.task, ready), move |(task, _)| match task {
        Some(task) => select.emit(task.clone()),
        None if shown && requested.leaving => leave.emit(()),
        None if shown => closing.set(true),
        None => {}
    });
}

/// Show one task in the shared detail sheet and make sure its detail is read.
fn select_callback(
    request: Callback<TaskStamp>,
    selected: UseStateHandle<Option<TaskStamp>>,
    presented: UseStateHandle<bool>,
    closing: UseStateHandle<bool>,
    return_focus: Rc<RefCell<Option<web_sys::Element>>>,
) -> Callback<TaskStamp> {
    Callback::from(move |task: TaskStamp| {
        request.emit(task.clone());
        if selected.as_ref() == Some(&task) && !*closing {
            return;
        }
        if selected.is_none() {
            *return_focus.borrow_mut() = active_focus_target();
        }
        if selected.is_none() || *closing {
            presented.set(false);
            closing.set(false);
        }
        selected.set(Some(task));
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

/// Drop the sheet for a view that owns focus, without its close slide.
fn leave_callback(
    selected: UseStateHandle<Option<TaskStamp>>,
    presented: UseStateHandle<bool>,
    closing: UseStateHandle<bool>,
    return_focus: Rc<RefCell<Option<web_sys::Element>>>,
) -> Callback<()> {
    Callback::from(move |()| {
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

/// One authored task text field through the sanitized, task-aware renderer.
///
/// Descriptions, acceptance criteria, and observations all render here, so
/// every detail surface shows the same Markdown and the same chips. Chips name
/// only tasks `resolves` admits, and the surrounding surface decides what
/// activating one opens.
pub(super) fn render_task_text(
    markdown: &str,
    class: &'static str,
    resolves: impl Fn(&TaskStamp) -> bool,
) -> Html {
    let html = crate::composer_markdown::task_text::render_task_text(markdown, resolves);
    html! {
        <div class={classes!("task-text", class)}>
            {Html::from_html_unchecked(html.into())}
        </div>
    }
}

/// A task's authored contract: quick detail shows the description alone, and
/// deep detail adds every acceptance criterion.
pub(super) fn render_contract(
    statement: &wire::intent::BoardStatement,
    resolves: impl Fn(&TaskStamp) -> bool + Copy,
    acceptance: bool,
) -> Html {
    html! {
        <>
            if let Some(description) = statement.description.as_deref() {
                {render_task_text(description, "work-detail-description", resolves)}
            } else {
                <p class="muted">{"No description was recorded."}</p>
            }
            if acceptance {
                if statement.acceptance.is_empty() {
                    <p class="muted">{"No acceptance criteria were recorded."}</p>
                } else {
                    <h4>{"Acceptance"}</h4>
                    <ul>{for statement.acceptance.iter().map(|criterion| html! {
                        <li>{render_task_text(criterion, "work-detail-criterion", resolves)}</li>
                    })}</ul>
                }
            }
        </>
    }
}

/// Open the task a task-text chip names in this detail surface.
pub(super) fn task_text_chip_click(select: &Callback<TaskStamp>) -> Callback<MouseEvent> {
    let select = select.clone();
    Callback::from(move |event: MouseEvent| {
        if let Some(task) = task_text_chip_task(event.target()) {
            event.prevent_default();
            select.emit(task);
        }
    })
}

/// The keyboard twin of [`task_text_chip_click`] for a chip's button role.
pub(super) fn task_text_chip_keydown(select: &Callback<TaskStamp>) -> Callback<KeyboardEvent> {
    let select = select.clone();
    Callback::from(move |event: KeyboardEvent| {
        if event.key() != "Enter" && event.key() != " " {
            return;
        }
        if let Some(task) = task_text_chip_task(event.target()) {
            event.prevent_default();
            select.emit(task);
        }
    })
}

fn task_text_chip_task(target: Option<web_sys::EventTarget>) -> Option<TaskStamp> {
    target?
        .dyn_into::<web_sys::Element>()
        .ok()?
        .closest(".task-text .message-task-chip")
        .ok()??
        .get_attribute("data-task-handle")?
        .parse::<wire::TaskHandle>()
        .ok()?
        .born()
        .ok()
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

/// The link to the transcript offset recorded when this task was admitted,
/// labelled with that moment's live relative age.
///
/// The boundary is captured in the same admission that mints the task stamp,
/// so the stamp's moment is the linked offset's time whether or not that
/// transcript page is loaded.
pub(super) fn creator_context_link(
    class: &'static str,
    href: String,
    statement: &wire::intent::BoardStatement,
    onclick: Option<Callback<MouseEvent>>,
) -> Html {
    let created = statement.born.micros().div_euclid(1_000);
    html! {
        <a
            class={classes!(class, "creator-context-link")}
            data-creator-context="true"
            {href}
            title="Creator context at task creation"
            aria-label={format!(
                "Open creator context for {} in Chat, from when it was created",
                statement.handle,
            )}
            {onclick}
        >
            <super::clock::Age authored={created} class={classes!("creator-context-age")} />
        </a>
    }
}

/// Whether the operator may close a task from its composer, and whose live
/// claim that closure takes.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct TaskClosure {
    holder: Option<wire::Actor>,
}

/// The closure a composer offers for `heads`: none without the capability or
/// once the task is settled.
pub(super) fn closure(can_close: bool, heads: &wire::intent::BoardTask) -> Option<TaskClosure> {
    let settled = matches!(heads.workflow.body.status.as_str(), "completed" | "deleted");
    (can_close && !settled).then(|| TaskClosure {
        holder: live_holder(heads),
    })
}

fn live_holder(heads: &wire::intent::BoardTask) -> Option<wire::Actor> {
    heads
        .claim
        .as_ref()
        .filter(|claim| claim.body.disposition == wire::ClaimDisposition::Active)
        .map(|claim| claim.body.actor.clone())
}

/// The heads a closure decides against, spelled as the resident's witnesses.
fn close_witnesses(heads: &wire::intent::BoardTask) -> Vec<wire::Witness> {
    let task = heads.statement.body.born.clone();
    vec![
        wire::Witness::Task {
            task: task.clone(),
            head: heads.statement.position,
        },
        wire::Witness::Workflow {
            task: task.clone(),
            head: heads.workflow.position,
            phase: heads.workflow.body.phase.clone(),
        },
        wire::Witness::Claim {
            task,
            head: heads.claim.as_ref().map(|claim| claim.position),
        },
    ]
}

/// The submit button value that sends the draft as a closing validation.
const CLOSE_SUBMITTER: &str = "close";

fn submitted_close(event: &SubmitEvent) -> bool {
    event
        .submitter()
        .is_some_and(|button| button.get_attribute("value").as_deref() == Some(CLOSE_SUBMITTER))
}

/// The one observation composer every surface showing `task` renders, bound
/// to that task and preserving only that task's draft while feedback uses its task-owned toast.
/// With a `closure`, the same draft can instead close the task as its validation.
pub(super) fn render_note_composer(
    id: &'static str,
    handle: &str,
    task: &TaskStamp,
    can_note: bool,
    closure: Option<&TaskClosure>,
    composer: &NoteComposer,
    submit: &Callback<(TaskStamp, SubmitEvent)>,
) -> Html {
    if !can_note {
        return html! {
            <p class="work-note-capability">{"This capability can read observations but cannot append them."}</p>
        };
    }
    let busy = composer.submission.busy();
    let closing = composer.submission == NoteSubmission::Closing;
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
                {if busy && !closing { "Saving…" } else { "Add observation" }}
            </button>
            if let Some(closure) = closure {
                <button
                    type="submit"
                    class="canvas-compact-control"
                    value={CLOSE_SUBMITTER}
                    disabled={busy}
                    title={closure.holder.as_ref().map(|holder| format!("Closing takes this task from {holder}"))}
                >
                    {if closing {
                        "Closing…"
                    } else if closure.holder.is_some() {
                        "Close task from its holder"
                    } else {
                        "Close task"
                    }}
                </button>
            }
        </form>
    }
}

/// Append one observation to the task named by the submitting surface, or
/// close that task with the draft as its validation when the close button
/// submitted the form.
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
        let closing = submitted_close(&event);
        let (handle, after, close) = {
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
                closing.then(|| {
                    (
                        close_witnesses(&detail.task),
                        live_holder(&detail.task).is_some(),
                    )
                }),
            )
        };
        if let Some((expected, force_claimed)) = close {
            queue_close(
                &sender,
                &notes,
                &task,
                handle,
                &text,
                expected,
                force_claimed,
            );
            return;
        }
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

/// Queue one closure whose witnesses are the heads the operator closed from.
fn queue_close(
    sender: &SenderHandle,
    notes: &NoteComposers,
    task: &TaskStamp,
    handle: wire::TaskHandle,
    text: &str,
    expected: Vec<wire::Witness>,
    force_claimed: bool,
) {
    let validation = match compose::close(text) {
        Ok(validation) => validation,
        Err(refusal) => {
            notes.update(task, |composer| composer.notice = refusal.into());
            return;
        }
    };
    let queued = action_identity(sender).is_some_and(|id| {
        admit_action(
            sender,
            ClientAction::Close {
                id,
                task: handle,
                validation,
                expected,
                force_claimed,
            },
        )
    });
    notes.update(task, |composer| {
        composer.notice = Notice::none();
        composer.submission = if queued {
            NoteSubmission::Closing
        } else {
            NoteSubmission::Failed(
                "Closure could not be queued; the draft was retained.".to_owned(),
            )
        };
    });
}

fn retain_note_failure(notes: &NoteComposers, task: &TaskStamp) {
    notes.update(task, |composer| {
        composer.notice = Notice::none();
        composer.submission = NoteSubmission::Failed(
            "Observation could not be queued; the draft was retained.".to_owned(),
        );
    });
}
