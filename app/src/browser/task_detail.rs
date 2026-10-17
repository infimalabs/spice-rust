//! Selection, focus, and observation lifecycle for contextual task detail.

use super::*;
use crate::browser::clock::{TaskTimePresentation, task_time};
use crate::work::TaskOverviewSelection;

#[derive(Clone)]
pub(super) struct TrustGrantRequest {
    pub task: TaskStamp,
    pub revision: wire::Seq,
    pub digest: wire::Digest,
}
pub(super) struct Controls {
    pub blocker_review: UseStateHandle<bool>,
    pub blocker_review_button_ref: NodeRef,
    pub blocker_review_close_ref: NodeRef,
    pub notes: NoteComposers,
    /// Ask again for one shown task whose read produced no answer.
    pub request: Callback<TaskStamp>,
    pub submit_note: Callback<(TaskStamp, SubmitEvent)>,
    pub grant_trust: Callback<TrustGrantRequest>,
}

/// Own task-specific state for the task content mounted in the app sidebar.
#[hook]
pub(super) fn use_controls(
    sender: SenderHandle,
    surface: UseStateHandle<SurfaceData>,
    selection: Option<TaskOverviewSelection>,
    handed: &[crate::composer_drafts::HandedNote],
) -> Controls {
    let blocker_review = use_state_eq(|| false);
    let blocker_review_button_ref = use_node_ref();
    let blocker_review_close_ref = use_node_ref();
    let handed = || {
        handed
            .iter()
            .map(|note| {
                let composer = NoteComposer {
                    draft: note.text.clone(),
                    ..NoteComposer::default()
                };
                (note.task.clone(), composer)
            })
            .collect::<BTreeMap<_, _>>()
    };
    let notes = NoteComposers {
        state: use_mut_ref(handed),
        published: use_state(handed),
    };
    let request = request_callback(sender.clone(), surface.clone());
    {
        let sender = sender.clone();
        let surface = surface.clone();
        // A selection can precede the runtime (a cold permalink), and a
        // changed scope replaces it, so demand follows the runtime as well.
        let scope = surface
            .read()
            .as_ref()
            .map(|runtime| runtime.principal().scope());
        use_effect_with((selection.clone(), scope), move |(selection, _)| {
            let tasks = selection
                .iter()
                .flat_map(|selection| selection.tasks().iter().cloned());
            let owed = surface
                .write()
                .as_mut()
                .is_some_and(|runtime| runtime.show_task_details(tasks));
            surface.set(surface.publication());
            if owed {
                let _admitted = admit_action(&sender, ClientAction::TaskDetails);
            }
        });
    }
    {
        let blocker_review = blocker_review.clone();
        use_effect_with(selection, move |_| blocker_review.set(false));
    }
    use_blocker_review_focus(
        *blocker_review,
        blocker_review_button_ref.clone(),
        blocker_review_close_ref.clone(),
    );
    let close_blocker_review = {
        let blocker_review = blocker_review.clone();
        Callback::from(move |()| blocker_review.set(false))
    };
    overlay::use_escape_owner(*blocker_review, close_blocker_review);
    let submit_note = note_submit_callback(sender.clone(), surface.clone(), notes.clone());
    let grant_trust = trust_grant_callback(sender, surface);
    Controls {
        blocker_review,
        blocker_review_button_ref,
        blocker_review_close_ref,
        notes,
        request,
        submit_note,
        grant_trust,
    }
}

fn trust_grant_callback(
    sender: SenderHandle,
    surface: UseStateHandle<SurfaceData>,
) -> Callback<TrustGrantRequest> {
    Callback::from(move |request: TrustGrantRequest| {
        let reserved = {
            let mut data = surface.write();
            data.as_mut().is_some_and(|runtime| {
                runtime.begin_trust_grant(&request.task, request.revision, &request.digest)
            })
        };
        if !reserved {
            return;
        }
        surface.set(surface.publication());
        if !admit_action(
            &sender,
            ClientAction::TrustGrant {
                task: request.task,
                revision: request.revision,
                digest: request.digest.clone(),
            },
        ) {
            if let Some(runtime) = surface.write().as_mut() {
                runtime.settle_trust_grant(
                    request.digest,
                    Some(
                        "The browser could not queue this approval. Retry when connected."
                            .to_owned(),
                    ),
                );
            }
            surface.set(surface.publication());
        }
    })
}

/// Ask again for one shown task whose read produced no answer.
fn request_callback(
    sender: SenderHandle,
    surface: UseStateHandle<SurfaceData>,
) -> Callback<TaskStamp> {
    Callback::from(move |task: TaskStamp| {
        let owed = surface
            .write()
            .as_mut()
            .is_some_and(|runtime| runtime.retry_task_detail(&task));
        if owed {
            surface.set(surface.publication());
            let _admitted = admit_action(&sender, ClientAction::TaskDetails);
        }
    })
}

/// Move focus into the sheet-local blocker review and back to its launcher.
#[hook]
fn use_blocker_review_focus(open: bool, launcher: NodeRef, close: NodeRef) {
    let prior = use_mut_ref(|| false);
    use_effect_with(open, move |open| {
        let target = if *open {
            close.cast::<web_sys::HtmlElement>()
        } else if *prior.borrow() {
            launcher.cast::<web_sys::HtmlElement>()
        } else {
            None
        };
        *prior.borrow_mut() = *open;
        if let Some(target) = target {
            let options = web_sys::FocusOptions::new();
            options.set_prevent_scroll(true);
            let _ = target.focus_with_options(&options);
        }
    });
}

/// One authored task text field through the shared Markdown dialect and the
/// message body surface, so math, diagrams, and images render as in cards.
///
/// Descriptions, acceptance criteria, observations, and closure validation all
/// render here. Chips name only tasks `resolves` admits, and the surrounding
/// surface decides what activating one opens.
pub(super) fn render_task_text(
    markdown: &str,
    class: &'static str,
    resolves: impl Fn(&TaskStamp) -> bool,
) -> Html {
    let rendered = crate::composer_markdown::task_text::render_task_text(markdown, resolves);
    let sources = rendered
        .diagrams
        .into_iter()
        .map(AttrValue::from)
        .collect::<Vec<_>>();
    html! {
        <div class={classes!("task-text", class)}>
            <super::mermaid::MarkdownBody
                html={AttrValue::from(rendered.html)}
                sources={std::rc::Rc::new(sources)}
                final_answer=true
                quote_tasks={std::rc::Rc::default()}
            />
        </div>
    }
}

/// One authored task title as an inline Markdown line in the message dialect.
///
/// A live line chips the tasks its controls admit and leaves activating them
/// to the surrounding surface; a line inside another control renders handles
/// and links as text.
pub(super) fn render_task_title(
    title: &str,
    controls: &crate::composer_markdown::inline::LineControls<'_>,
) -> Html {
    let html = crate::composer_markdown::inline::render_line(title, controls);
    html! {
        <super::mermaid::MarkdownBody
            html={AttrValue::from(html)}
            sources={std::rc::Rc::default()}
            final_answer=true
            quote_tasks={std::rc::Rc::default()}
            inline=true
        />
    }
}

/// A task's authored contract: quick detail shows the description alone, and
/// deep detail adds every acceptance criterion or open Design question.
pub(super) fn render_contract(
    statement: &wire::intent::BoardStatement,
    resolves: impl Fn(&TaskStamp) -> bool + Copy,
    acceptance: bool,
) -> Html {
    let design = statement.intent == Some(wire::TaskIntent::Design);
    html! {
        <>
            if let Some(description) = statement.description.as_deref() {
                {render_task_text(description, "work-detail-description", resolves)}
            } else {
                <p class="muted">{"No description was recorded."}</p>
            }
            if acceptance {
                if statement.acceptance.is_empty() {
                    <p class="muted">{
                        if design {
                            "No open questions were recorded."
                        } else {
                            "No acceptance criteria were recorded."
                        }
                    }</p>
                } else {
                    <h4>{if design { "Questions" } else { "Acceptance" }}</h4>
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
        .closest(":is(.task-text, .markdown-line) .message-task-chip")
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
        <section class="app-sheet-section app-sheet-media">
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

/// Command submits an observation; Shift+Command uses the offered Close action.
fn note_keyboard_submit(enabled: bool) -> Callback<KeyboardEvent> {
    Callback::from(move |event: KeyboardEvent| {
        if !enabled || event.key() != "Enter" || !(event.meta_key() || event.ctrl_key()) {
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
        if event.shift_key() {
            if event.repeat() {
                return;
            }
            let Some(button) = form
                .query_selector("button[value=\"close\"]:enabled")
                .ok()
                .flatten()
                .and_then(|button| button.dyn_into::<web_sys::HtmlElement>().ok())
            else {
                return;
            };
            let _ = form.request_submit_with_submitter(Some(&button));
        } else {
            let _ = form.request_submit();
        }
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

/// Whether the operator may settle a task from its composer, whose live claim
/// that settlement takes, and which controlled tasks it closes with it.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct TaskClosure {
    holder: Option<wire::Actor>,
    controlled: Vec<wire::TaskHandle>,
    closable: bool,
}

/// The settlement a composer offers for `heads`: none without the capability
/// or once the task is settled. Designs conclude rather than close, but remain
/// directly withdrawable.
pub(super) fn closure(can_close: bool, heads: &wire::intent::BoardTask) -> Option<TaskClosure> {
    let settled = !heads.workflow.body.status.is_pending();
    (can_close && !settled).then(|| TaskClosure {
        holder: live_holder(heads),
        controlled: heads.controlled.clone(),
        closable: heads.statement.body.intent != Some(wire::TaskIntent::Design),
    })
}

fn live_holder(heads: &wire::intent::BoardTask) -> Option<wire::Actor> {
    heads
        .claim
        .as_ref()
        .filter(|claim| claim.body.disposition == wire::ClaimDisposition::Active)
        .map(|claim| claim.body.actor.clone())
}

/// The operator's closure of a task. Closing records no note, so this is the
/// one place its validation reads back, beside the observations it replaced.
pub(super) fn render_closure_record(
    closure: Option<&wire::intent::TaskDetailClosure>,
    name: impl Fn(&wire::Actor) -> String,
    resolves: impl Fn(&TaskStamp) -> bool,
) -> Html {
    let Some(closure) = closure else {
        return Html::default();
    };
    let displaced = closure
        .claimed
        .as_ref()
        .map(|actor| format!("Took the claim from {}.", name(actor)));
    html! {
        <article class="work-note-card work-closure-card" data-closure-position={closure.position.to_string()}>
            <header>
                <strong title={closure.author.to_string()}>
                    {format!("Closed by {} at {}", name(&closure.author), closure.phase)}
                </strong>
                {task_time(&closure.at, TaskTimePresentation::DetailedElapsed)}
            </header>
            {render_task_text(&closure.validation, "work-note-body", resolves)}
            if let Some(displaced) = displaced {
                <p class="work-closure-claim">{displaced}</p>
            }
        </article>
    }
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

/// Submitter values that settle the task with the composed text.
const CLOSE_SUBMITTER: &str = "close";
const DELETE_SUBMITTER: &str = "delete";

#[derive(Clone, Copy, PartialEq, Eq)]
enum ComposerSettlement {
    Close,
    Delete,
}

fn submitted_settlement(event: &SubmitEvent) -> Option<ComposerSettlement> {
    match event
        .submitter()
        .and_then(|button| button.get_attribute("value"))
        .as_deref()
    {
        Some(CLOSE_SUBMITTER) => Some(ComposerSettlement::Close),
        Some(DELETE_SUBMITTER) => Some(ComposerSettlement::Delete),
        _ => None,
    }
}

/// The one room composer every surface showing `task` renders, bound to that
/// task and preserving only that task's draft while feedback uses its
/// task-owned toast. Typed input enters that retained draft immediately, so a
/// connection loss can leave the same composer visible and read-only until it
/// returns. Its draft publishes an observation in the task's room; with a
/// `closure`, the same draft can instead close the task as its validation.
/// `actions` are the task's own further controls, kept in the composer's action
/// row so they follow an observation.
#[expect(
    clippy::too_many_arguments,
    reason = "one composer binds its task, capabilities, draft, and action row"
)]
pub(super) fn render_note_composer(
    id: String,
    handle: &str,
    task: &TaskStamp,
    connected: bool,
    can_note: bool,
    closure: Option<&TaskClosure>,
    composer: &NoteComposer,
    input: Callback<InputEvent>,
    submit: &Callback<(TaskStamp, SubmitEvent)>,
    actions: Html,
) -> Html {
    if connected && !can_note {
        return html! {
            <>
                <p class="work-note-capability">{"This capability can read this room but cannot publish observations."}</p>
                <div class="team-composer-controls work-note-actions">{actions}</div>
            </>
        };
    }
    let busy = composer.submission.busy();
    let locked = busy || !connected;
    let posting = composer.submission == NoteSubmission::Posting;
    let closing = composer.submission == NoteSubmission::Closing;
    let deleting = composer.submission == NoteSubmission::Deleting;
    let form_key = task.to_string();
    let task = task.clone();
    let label_id = id.clone();
    html! {
        <form
            key={form_key.clone()}
            class="form work-note-composer"
            onsubmit={submit.reform(move |event| (task.clone(), event))}
        >
            <label for={label_id}>{format!("Observation for {handle}")}</label>
            <textarea
                id={id}
                data-note-task={form_key}
                value={composer.draft.clone()}
                readonly={locked}
                oninput={input}
                onkeydown={note_keyboard_submit(!locked)}
            />
            <div class="team-composer-controls work-note-actions">
                <button type="submit" class="team-send" disabled={locked}>
                    {if posting { "Posting…" } else { "Add" }}
                </button>
                if let Some(closure) = closure {
                    if closure.closable {
                        <button
                            type="submit"
                            class="team-send"
                            value={CLOSE_SUBMITTER}
                            disabled={locked}
                            title={closure.holder.as_ref().map(|holder| format!("Closing takes this task from {holder}"))}
                        >
                            {if closing { "Closing…" } else { "Close" }}
                        </button>
                    }
                    <button
                        type="submit"
                        class="team-send"
                        value={DELETE_SUBMITTER}
                        disabled={locked}
                        title={closure.holder.as_ref().map(|holder| format!("Deleting takes this task from {holder}"))}
                    >
                        {if deleting { "Deleting…" } else { "Delete" }}
                    </button>
                }
                {actions}
            </div>
        </form>
    }
}

/// Publish one observation in the room of the task named by the submitting
/// surface, or settle that task with the draft as validation/reason when a
/// destructive submitter sent the form.
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
        let requested_settlement = submitted_settlement(&event);
        let (handle, head, settlement) = {
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
                detail.task.statement.position,
                requested_settlement.map(|settlement| {
                    (
                        settlement,
                        close_witnesses(&detail.task),
                        live_holder(&detail.task).is_some(),
                        detail.task.controlled.clone(),
                    )
                }),
            )
        };
        if let Some((settlement, expected, force_claimed, controlled)) = settlement {
            queue_settlement(
                &sender,
                &notes,
                &task,
                handle,
                &text,
                expected,
                force_claimed,
                &controlled,
                settlement,
            );
            return;
        }
        let text = match compose::observation(&text) {
            Ok(text) => text,
            Err(refusal) => {
                notes.update(&task, |composer| composer.notice = refusal.into());
                return;
            }
        };
        let respond = {
            let notes = notes.clone();
            let task = task.clone();
            Rc::new(
                move |result: Result<wire::rooms::RoomPostAnswer, String>| match result {
                    Ok(_) => notes.update(&task, |composer| *composer = NoteComposer::default()),
                    Err(issue) => notes.update(&task, |composer| {
                        composer.submission = NoteSubmission::Failed(issue);
                    }),
                },
            )
        };
        if admit_action(
            &sender,
            ClientAction::RoomPost(crate::client_action::RoomPostAction {
                room: handle,
                head,
                text,
                respond,
            }),
        ) {
            notes.update(&task, |composer| {
                composer.notice = Notice::none();
                composer.submission = NoteSubmission::Posting;
            });
        } else {
            retain_note_failure(&notes, &task);
        }
    })
}

/// Validate, confirm, and queue one destructive task settlement against the
/// exact heads and controlled closure set the operator saw.
#[expect(
    clippy::too_many_arguments,
    reason = "the composer supplies one fully witnessed settlement"
)]
fn queue_settlement(
    sender: &SenderHandle,
    notes: &NoteComposers,
    task: &TaskStamp,
    handle: wire::TaskHandle,
    text: &str,
    expected: Vec<wire::Witness>,
    force_claimed: bool,
    controlled: &[wire::TaskHandle],
    settlement: ComposerSettlement,
) {
    let text = match compose::close(text) {
        Ok(text) => text,
        Err(refusal) => {
            notes.update(task, |composer| composer.notice = refusal.into());
            return;
        }
    };
    if !confirm_settlement(settlement, &handle, &text, controlled) {
        return;
    }
    let queued = action_identity(sender).is_some_and(|id| {
        let action = match settlement {
            ComposerSettlement::Close => ClientAction::Close {
                id,
                task: handle,
                validation: text,
                expected,
                force_claimed,
            },
            ComposerSettlement::Delete => ClientAction::Delete {
                id,
                task: handle,
                reason: text,
                expected,
                force_claimed,
            },
        };
        admit_action(sender, action)
    });
    notes.update(task, |composer| {
        composer.notice = Notice::none();
        composer.submission = if queued {
            match settlement {
                ComposerSettlement::Close => NoteSubmission::Closing,
                ComposerSettlement::Delete => NoteSubmission::Deleting,
            }
        } else {
            NoteSubmission::Failed(
                "Settlement could not be queued; the draft was retained.".to_owned(),
            )
        };
    });
}

fn confirm_settlement(
    settlement: ComposerSettlement,
    handle: &wire::TaskHandle,
    text: &str,
    controlled: &[wire::TaskHandle],
) -> bool {
    let verb = match settlement {
        ComposerSettlement::Close => "Close",
        ComposerSettlement::Delete => "Delete",
    };
    let mut message = format!("{verb} {handle}?\n\nReason or validation:\n{text}");
    if !controlled.is_empty() {
        message.push_str("\n\nThis also closes these controlled tasks:");
        for task in controlled {
            message.push_str("\n- ");
            message.push_str(task.as_str());
        }
    }
    web_sys::window()
        .and_then(|window| window.confirm_with_message(&message).ok())
        .unwrap_or(false)
}

fn retain_note_failure(notes: &NoteComposers, task: &TaskStamp) {
    notes.update(task, |composer| {
        composer.notice = Notice::none();
        composer.submission = NoteSubmission::Failed(
            "Observation could not be queued; the draft was retained.".to_owned(),
        );
    });
}
