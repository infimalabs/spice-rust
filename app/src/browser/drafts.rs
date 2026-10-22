//! The operator's unfiled Drafts: prospective work with an admitted public
//! project and editable content. No allocator, dependency, claim, or strip sees
//! a Draft until the explicit file transition turns it into ordinary filed
//! work.

use std::cell::Cell;
use std::rc::Rc;

use wasm_bindgen::{JsCast, JsValue};
use wire::intent::{BoardTask, TaskAdd, TaskAnswer, TaskFile};
use wire::{CommandKind, TaskIntent, TaskStamp, Witness};
use yew::prelude::*;

use super::sender::{SenderHandle, admit_action};
use crate::AppRuntime;
use crate::client_action::{ClientAction, DraftRequest};
use crate::draft_form::{DraftField, DraftFields, DraftForm, admitted_pitch, criteria};

thread_local! {
    static LAST_DRAFT_MICROS: Cell<i64> = const { Cell::new(0) };
}

/// The submit button value that files the Draft instead of saving it.
const FILE_SUBMITTER: &str = "file";

#[derive(Clone)]
pub(super) struct Controls {
    /// The outcome of the last Draft control, shown beside the Drafts.
    notice: UseStateHandle<Option<String>>,
}

#[hook]
pub(super) fn use_controls() -> Controls {
    Controls {
        notice: use_state(|| None),
    }
}

impl Controls {
    fn send(&self, sender: &SenderHandle, request: DraftRequest, landed: Callback<TaskAnswer>) {
        let notice = self.notice.clone();
        let respond = std::rc::Rc::new(move |result: Result<TaskAnswer, String>| match result {
            Ok(answer) => landed.emit(answer),
            Err(error) => notice.set(Some(error)),
        });
        if !admit_action(sender, ClientAction::Draft { request, respond }) {
            self.notice
                .set(Some("The task change could not be queued.".to_owned()));
        }
    }

    fn notice(&self) -> Html {
        (*self.notice).clone().map_or_else(Html::default, |notice| {
            html! { <p class="room-notice" role="status" data-draft-notice="">{notice}</p> }
        })
    }
}

/// The operator's open Drafts, each opening its task detail, above a compact
/// disclosure that reveals the full creator only on demand.
pub(super) fn render_section(
    runtime: Option<&AppRuntime>,
    controls: &Controls,
    sender: &SenderHandle,
    open: &Callback<TaskStamp>,
) -> Html {
    let board = runtime.and_then(AppRuntime::board);
    let drafts = board.map_or_else(Vec::new, |board| {
        board
            .tasks
            .iter()
            .filter(|task| open_draft(task))
            .collect::<Vec<_>>()
    });
    let creates = runtime.is_some_and(|runtime| runtime.offers(CommandKind::TaskAdd));
    let submit = {
        let controls = controls.clone();
        let sender = sender.clone();
        let open = open.clone();
        let taken = board.map_or_else(Vec::new, |board| {
            board
                .tasks
                .iter()
                .map(|task| task.statement.body.born.clone())
                .collect()
        });
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            let Some(form) = submitted_form(&event) else {
                return;
            };
            match creation(&form, &taken) {
                Ok(request) => {
                    let notice = controls.notice.clone();
                    let open = open.clone();
                    let landed = Callback::from(move |answer: TaskAnswer| {
                        form.reset();
                        notice.set(Some(format!("Recorded Draft {}.", answer.task)));
                        if let Ok(born) = answer.task.born() {
                            open.emit(born);
                        }
                    });
                    controls.send(&sender, request, landed);
                }
                Err(problem) => controls.notice.set(Some(problem)),
            }
        })
    };
    html! {
        <section class="app-sheet-section draft-section" aria-labelledby="spice-drafts-heading" data-drafts="">
            <div class="section-heading">
                <h2 id="spice-drafts-heading">{"Drafts"}</h2>
                <span class="section-count">{format!("{} unfiled", drafts.len())}</span>
            </div>
            {controls.notice()}
            if drafts.is_empty() {
                <p class="empty">{"No unfiled Drafts."}</p>
            } else {
                {unfiled_list(drafts, open)}
            }
            if creates {
                <details class="draft-create">
                    <summary class="draft-create-summary">
                        <span>{"New Draft"}</span>
                        <small>{"Task · Plan · Design"}</small>
                    </summary>
                    <form class="draft-form draft-form-create" data-draft-create="" onsubmit={submit}>
                        {content_fields(None)}
                        <div class="draft-actions">
                            <button type="submit">{"Create Draft"}</button>
                        </div>
                    </form>
                </details>
            }
        </section>
    }
}

/// Unfiled rows, each a button that opens its task detail.
fn unfiled_list(tasks: Vec<&BoardTask>, open: &Callback<TaskStamp>) -> Html {
    html! {
        <ul class="draft-list">
            {for tasks.into_iter().map(|task| {
                let born = task.statement.body.born.clone();
                let onclick = open.reform(move |_: MouseEvent| born.clone());
                html! {
                    <li key={task.statement.body.born.to_string()}>
                        <button type="button" class="app-compact-control" data-unfiled-task={task.statement.body.handle.to_string()} {onclick}>
                            <code>{task.statement.body.handle.to_string()}</code>
                            <span>{task.presentation_title.clone()}</span>
                            <small class="work-task-pitch">{task.statement.body.pitch.clone()}</small>
                        </button>
                    </li>
                }
            })}
        </ul>
    }
}

/// The editor an open Draft's task detail carries: save revises its content,
/// and file validates it as complete ordinary work and files it atomically.
pub(super) fn render_editor(
    runtime: Option<&AppRuntime>,
    controls: &Controls,
    sender: &SenderHandle,
    row: &BoardTask,
) -> Html {
    if !open_draft(row) {
        return Html::default();
    }
    html! {
        <DraftEditor
            key={row.statement.body.born.to_string()}
            row={row.clone()}
            edits={runtime.is_some_and(|runtime| runtime.offers(CommandKind::TaskEdit))}
            files={runtime.is_some_and(|runtime| runtime.offers(CommandKind::TaskFile))}
            controls={controls.clone()}
            sender={sender.clone()}
        />
    }
}

#[derive(Properties)]
struct EditorProps {
    row: BoardTask,
    edits: bool,
    files: bool,
    controls: Controls,
    sender: SenderHandle,
}

impl PartialEq for EditorProps {
    fn eq(&self, other: &Self) -> bool {
        self.row == other.row
            && self.edits == other.edits
            && self.files == other.files
            && self.controls.notice == other.controls.notice
            && Rc::ptr_eq(&self.sender.queue, &other.sender.queue)
    }
}

/// One open Draft's editor. Its fields are its own: each render only rebases
/// them onto a newer admitted revision, so live refreshes never take what the
/// operator typed, and its key reopens it for another Draft.
#[function_component]
fn DraftEditor(props: &EditorProps) -> Html {
    let form = use_mut_ref(|| DraftForm::open(&props.row));
    let redraw = use_force_update();
    form.borrow_mut().refresh(&props.row);
    let edit = {
        let form = form.clone();
        Callback::from(move |(field, value): (DraftField, String)| {
            form.borrow_mut().edit(field, value);
            redraw.force_update();
        })
    };
    let submit = {
        let form = form.clone();
        let controls = props.controls.clone();
        let sender = props.sender.clone();
        let row = props.row.clone();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            let filing = event.submitter().is_some_and(|button| {
                button.get_attribute("value").as_deref() == Some(FILE_SUBMITTER)
            });
            let notice = controls.notice.clone();
            let request = if filing {
                Ok(filing_request(&row))
            } else {
                form.borrow().revision()
            };
            match request {
                Ok(request) => {
                    let sent = form.borrow().fields().clone();
                    let form = form.clone();
                    let landed = Callback::from(move |answer: TaskAnswer| {
                        if filing {
                            notice.set(Some(format!("Filed {}.", answer.task)));
                        } else {
                            form.borrow_mut().saved(sent.clone());
                            notice.set(Some(format!("Saved Draft {}.", answer.task)));
                        }
                    });
                    controls.send(&sender, request, landed);
                }
                Err(problem) => notice.set(Some(problem)),
            }
        })
    };
    let fields = form.borrow();
    html! {
        <section class="app-sheet-section" data-draft-editor={props.row.statement.body.handle.to_string()}>
            <h3>{"Draft"}</h3>
            <p class="muted">{"Unfiled and private to the operator and its origin thread. Save revises it; File admits it as ordinary work under current policy, or refuses and keeps the Draft unchanged."}</p>
            {props.controls.notice()}
            <form class="draft-form" onsubmit={submit}>
                {content_fields(Some((fields.fields(), &edit)))}
                <div class="draft-actions">
                    <button type="submit" class="app-compact-control" disabled={!props.edits}>{"Save Draft"}</button>
                    <button type="submit" class="app-compact-control" value={FILE_SUBMITTER} disabled={!props.files}>{"File"}</button>
                </div>
            </form>
        </section>
    }
}

fn open_draft(task: &BoardTask) -> bool {
    task.statement.body.filing.is_draft() && task.workflow.body.status.is_pending()
}

/// The editable content of a new Draft, or of an open Draft's editor, whose
/// controls show its fields and report each edit.
fn content_fields(editing: Option<(&DraftFields, &Callback<(DraftField, String)>)>) -> Html {
    let value = |field: DraftField| editing.map(|(fields, _)| fields.get(field).to_owned());
    let typed = |field: DraftField| {
        editing.map(|(_, edit)| edit.reform(move |event: InputEvent| (field, event_value(&event))))
    };
    let chosen = editing
        .map(|(_, edit)| edit.reform(|event: Event| (DraftField::Intent, event_value(&event))));
    let intent = editing.and_then(|(fields, _)| fields.intent().ok().flatten());
    let questions = intent == Some(TaskIntent::Design);
    let textarea_rows = if editing.is_some() { "3" } else { "2" };
    html! {
        <>
            <label class="draft-field-title"><span>{"Title"}</span><input name="title" type="text" required=true value={value(DraftField::Title)} oninput={typed(DraftField::Title)} /></label>
            <label class="draft-field-pitch"><span>{"Pitch"}</span><input name="pitch" type="text" required=true maxlength="160" placeholder="One plain sentence saying why this work matters" value={value(DraftField::Pitch)} oninput={typed(DraftField::Pitch)} /></label>
            <label class="draft-field-project"><span>{"Project"}</span><input name="project" type="text" required=true placeholder="spice.task" value={value(DraftField::Project)} oninput={typed(DraftField::Project)} /></label>
            <label class="draft-field-kind">
                <span>{"Kind"}</span>
                <select name="intent" onchange={chosen}>
                    <option value="" selected={intent.is_none()}>{"Task"}</option>
                    {for TaskIntent::ALL.into_iter().map(|choice| html! {
                        <option value={choice.as_str()} selected={intent == Some(choice)}>
                            {if choice == TaskIntent::Plan { "Plan" } else { "Design" }}
                        </option>
                    })}
                </select>
            </label>
            if editing.is_some() {
                <label class="draft-field-flow"><span>{"Flow"}</span><input name="flow" type="text" value={value(DraftField::Flow)} oninput={typed(DraftField::Flow)} /></label>
            }
            <label class="draft-field-description"><span>{"Description"}</span><textarea name="description" rows={textarea_rows} value={value(DraftField::Description)} oninput={typed(DraftField::Description)} /></label>
            <label class="draft-field-acceptance">
                <span>{if questions { "Questions" } else { "Acceptance" }}</span>
                <textarea
                    name="acceptance"
                    rows={textarea_rows}
                    placeholder={if questions { "One open question per line" } else { "One criterion per line" }}
                    value={value(DraftField::Acceptance)}
                    oninput={typed(DraftField::Acceptance)}
                />
            </label>
        </>
    }
}

/// The value of the control an event fired on.
fn event_value(event: &Event) -> String {
    event
        .target()
        .and_then(|target| js_sys::Reflect::get(&target, &JsValue::from_str("value")).ok())
        .and_then(|value| value.as_string())
        .unwrap_or_default()
}

fn submitted_form(event: &SubmitEvent) -> Option<web_sys::HtmlFormElement> {
    event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::HtmlFormElement>().ok())
}

/// One named control's current value.
fn field(form: &web_sys::HtmlFormElement, name: &str) -> String {
    form.query_selector(&format!("[name=\"{name}\"]"))
        .ok()
        .flatten()
        .and_then(|element| js_sys::Reflect::get(&element, &JsValue::from_str("value")).ok())
        .and_then(|value| value.as_string())
        .unwrap_or_default()
}

fn chosen_intent(form: &web_sys::HtmlFormElement) -> Result<Option<TaskIntent>, String> {
    let intent = field(form, "intent");
    if intent.is_empty() {
        return Ok(None);
    }
    intent.parse().map(Some)
}

fn creation(form: &web_sys::HtmlFormElement, taken: &[TaskStamp]) -> Result<DraftRequest, String> {
    let title = field(form, "title").trim().to_owned();
    let project = field(form, "project").trim().to_owned();
    if title.is_empty() || project.is_empty() {
        return Err("A Draft needs a title and an admitted public project.".to_owned());
    }
    let pitch = admitted_pitch(&field(form, "pitch"))?;
    let description = field(form, "description");
    let born = mint_stamp(taken)?;
    Ok(DraftRequest::Create {
        request: TaskAdd {
            title,
            pitch,
            project: Some(project),
            description: (!description.trim().is_empty()).then_some(description),
            acceptance: criteria(&field(form, "acceptance")),
            requirements: wire::EnvironmentRequirements::default(),
            origin: None,
            flow: None,
            depends: Vec::new(),
            intent: chosen_intent(form)?,
            preparation: None,
            claim: false,
            draft: true,
        },
        expected: vec![Witness::Absence { task: born }],
    })
}

/// The guarded file transition for `row`, under its current project.
fn filing_request(row: &BoardTask) -> DraftRequest {
    let task = row.statement.body.born.clone();
    DraftRequest::File {
        request: TaskFile {
            task: row.statement.body.handle.clone(),
            into: None,
            preparation: None,
            claim: false,
        },
        expected: vec![
            Witness::Task {
                task: task.clone(),
                head: row.statement.position,
            },
            Witness::Workflow {
                task: task.clone(),
                head: row.workflow.position,
                phase: row.workflow.body.phase.clone(),
            },
            Witness::Deferral {
                task,
                head: row.workflow.position,
                deferred: row.workflow.body.deferral.is_some(),
            },
        ],
    }
}

/// A birth stamp no current task holds, strictly after any this page minted.
fn mint_stamp(taken: &[TaskStamp]) -> Result<TaskStamp, String> {
    let now = (js_sys::Date::now() * 1_000.0) as i64;
    LAST_DRAFT_MICROS.with(|last| {
        let mut next = now.max(last.get().saturating_add(1));
        loop {
            let stamp = TaskStamp::for_micros(next).map_err(|error| error.to_string())?;
            if !taken.contains(&stamp) {
                last.set(next);
                return Ok(stamp);
            }
            next = next.saturating_add(1);
        }
    })
}
