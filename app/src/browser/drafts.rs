//! The operator's Drafts: prospective work with an admitted public project and
//! editable content that no allocator, dependency, claim, or strip sees until
//! the explicit file transition turns it into ordinary filed work.

use std::cell::Cell;

use wasm_bindgen::{JsCast, JsValue};
use wire::intent::{BoardTask, TaskAdd, TaskAnswer, TaskEdit, TaskFile};
use wire::{CommandKind, TaskIntent, TaskIntentEdit, TaskStamp, Witness};
use yew::prelude::*;

use super::sender::{SenderHandle, admit_action};
use crate::AppRuntime;
use crate::client_action::{ClientAction, DraftRequest};

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
                .set(Some("The Draft change could not be queued.".to_owned()));
        }
    }

    fn notice(&self) -> Html {
        (*self.notice).clone().map_or_else(Html::default, |notice| {
            html! { <p class="room-notice" role="status" data-draft-notice="">{notice}</p> }
        })
    }
}

/// The operator's open Drafts, each opening its task detail, above the form
/// that records a new one.
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
                <p class="empty">{"No Drafts. A Draft is private, editable, and never offered to an agent until it is filed."}</p>
            } else {
                <ul class="draft-list">
                    {for drafts.into_iter().map(|task| {
                        let born = task.statement.body.born.clone();
                        let onclick = open.reform(move |_: MouseEvent| born.clone());
                        html! {
                            <li key={task.statement.body.born.to_string()}>
                                <button type="button" class="app-compact-control" data-draft-task={task.statement.body.handle.to_string()} {onclick}>
                                    <code>{task.statement.body.handle.to_string()}</code>
                                    <span>{task.presentation_title.clone()}</span>
                                </button>
                            </li>
                        }
                    })}
                </ul>
            }
            if creates {
                <form class="draft-form" data-draft-create="" onsubmit={submit}>
                    <strong>{"New Draft"}</strong>
                    {content_fields(None)}
                    <div class="draft-actions">
                        <button type="submit">{"Create Draft"}</button>
                    </div>
                </form>
            }
        </section>
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
    let edits = runtime.is_some_and(|runtime| runtime.offers(CommandKind::TaskEdit));
    let files = runtime.is_some_and(|runtime| runtime.offers(CommandKind::TaskFile));
    let statement = &row.statement.body;
    let submit = {
        let controls = controls.clone();
        let sender = sender.clone();
        let row = row.clone();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            let filing = event.submitter().is_some_and(|button| {
                button.get_attribute("value").as_deref() == Some(FILE_SUBMITTER)
            });
            let Some(form) = submitted_form(&event) else {
                return;
            };
            let notice = controls.notice.clone();
            let request = if filing {
                Ok(filing_request(&row))
            } else {
                revision(&form, &row)
            };
            match request {
                Ok(request) => {
                    let landed = Callback::from(move |answer: TaskAnswer| {
                        notice.set(Some(if filing {
                            format!("Filed {}.", answer.task)
                        } else {
                            format!("Saved Draft {}.", answer.task)
                        }));
                    });
                    controls.send(&sender, request, landed);
                }
                Err(problem) => notice.set(Some(problem)),
            }
        })
    };
    html! {
        <section class="app-sheet-section" data-draft-editor={statement.handle.to_string()}>
            <h3>{"Draft"}</h3>
            <p class="muted">{"Unfiled and private to the operator and its origin thread. Save revises it; File admits it as ordinary work under current policy, or refuses and keeps the Draft unchanged."}</p>
            {controls.notice()}
            <form
                key={format!("{}-{}", statement.born, row.statement.position)}
                class="draft-form"
                onsubmit={submit}
            >
                {content_fields(Some(row))}
                <div class="draft-actions">
                    <button type="submit" disabled={!edits}>{"Save Draft"}</button>
                    <button type="submit" value={FILE_SUBMITTER} disabled={!files}>{"File"}</button>
                </div>
            </form>
        </section>
    }
}

fn open_draft(task: &BoardTask) -> bool {
    task.statement.body.filing.is_draft()
        && task.workflow.body.status == "pending"
        && task.workflow.body.completion.is_none()
}

/// The editable content of a new Draft, or of `row` when it exists.
fn content_fields(row: Option<&BoardTask>) -> Html {
    let statement = row.map(|row| &row.statement.body);
    let title = row.map(|row| row.presentation_title.clone());
    let project = statement.map(|statement| statement.project.clone());
    let description = statement.and_then(|statement| statement.description.clone());
    let acceptance = statement.map(|statement| statement.acceptance.join("\n"));
    let intent = statement.and_then(|statement| statement.intent);
    let flow = row.map(|row| {
        row.workflow
            .body
            .flow
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",")
    });
    html! {
        <>
            <label><span>{"Title"}</span><input name="title" type="text" required=true value={title} /></label>
            <label><span>{"Project"}</span><input name="project" type="text" required=true placeholder="spice.task" value={project} /></label>
            <label>
                <span>{"Kind"}</span>
                <select name="intent">
                    <option value="" selected={intent.is_none()}>{"Task"}</option>
                    {for TaskIntent::ALL.into_iter().map(|choice| html! {
                        <option value={choice.as_str()} selected={intent == Some(choice)}>
                            {if choice == TaskIntent::Plan { "Plan" } else { "Design" }}
                        </option>
                    })}
                </select>
            </label>
            if row.is_some() {
                <label><span>{"Flow"}</span><input name="flow" type="text" value={flow} /></label>
            }
            <label><span>{"Description"}</span><textarea name="description" rows="3" value={description} /></label>
            <label><span>{"Acceptance"}</span><textarea name="acceptance" rows="3" placeholder="One criterion per line" value={acceptance} /></label>
        </>
    }
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

fn criteria(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
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
    let description = field(form, "description");
    let born = mint_stamp(taken)?;
    Ok(DraftRequest::Create {
        request: TaskAdd {
            title,
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

/// The fields of `form` that differ from `row`'s current content.
fn revision(form: &web_sys::HtmlFormElement, row: &BoardTask) -> Result<DraftRequest, String> {
    let statement = &row.statement.body;
    let moved = |name: &str, current: &str| {
        let value = field(form, name);
        (value.trim() != current.trim()).then(|| value.trim().to_owned())
    };
    let acceptance = criteria(&field(form, "acceptance"));
    let intent = chosen_intent(form)?;
    let flow = row
        .workflow
        .body
        .flow
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let request = TaskEdit {
        task: statement.handle.clone(),
        title: moved("title", &row.presentation_title),
        description: moved(
            "description",
            statement.description.as_deref().unwrap_or_default(),
        ),
        acceptance: if acceptance == statement.acceptance {
            Vec::new()
        } else {
            acceptance
        },
        requirements: None,
        intent: (intent != statement.intent).then_some(match intent {
            Some(intent) => TaskIntentEdit::Set { intent },
            None => TaskIntentEdit::Clear {},
        }),
        evidence: None,
        priority: None,
        project: moved("project", &statement.project),
        flow: moved("flow", &flow),
    };
    if request.title.is_none()
        && request.description.is_none()
        && request.acceptance.is_empty()
        && request.intent.is_none()
        && request.project.is_none()
        && request.flow.is_none()
    {
        return Err("Nothing changed in this Draft.".to_owned());
    }
    Ok(DraftRequest::Edit {
        request,
        expected: vec![Witness::Task {
            task: statement.born.clone(),
            head: row.statement.position,
        }],
    })
}

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
