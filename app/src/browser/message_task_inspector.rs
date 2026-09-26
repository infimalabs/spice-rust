use super::clock::{TaskTimePresentation, task_time};
use super::*;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Inspection {
    pub card: String,
    pub handle: TaskHandle,
    pub task: TaskStamp,
    pub open: bool,
    pub return_focus: bool,
}

#[derive(Clone, PartialEq)]
pub(super) struct Context {
    pub inspection: UseStateHandle<Option<Inspection>>,
    request_detail: Callback<TaskStamp>,
    return_focus_to: Callback<web_sys::Element>,
    navigate: Callback<navigation::Intent>,
    quote_tasks: Rc<BTreeMap<String, TaskHandle>>,
}

#[derive(Clone, PartialEq)]
struct DetailContext {
    detail: Option<TaskDetailState>,
    can_note: bool,
    note: NoteComposer,
    submit_note: Callback<(TaskStamp, SubmitEvent)>,
}

pub(super) struct Controls {
    context: Context,
    detail: DetailContext,
}

#[hook]
pub(super) fn use_controls(
    surface: &UseStateHandle<SurfaceData>,
    task: &task_detail::Controls,
    navigate: &Callback<navigation::Intent>,
) -> Controls {
    let inspection = use_state(|| None::<Inspection>);
    let runtime = surface.read();
    let detail = (*inspection)
        .as_ref()
        .and_then(|inspection| {
            runtime
                .as_ref()
                .and_then(|runtime| runtime.task_detail(&inspection.task))
        })
        .cloned();
    let note = (*inspection)
        .as_ref()
        .map(|inspection| task.notes.shown(&inspection.task))
        .unwrap_or_default();
    let quote_tasks = runtime
        .as_ref()
        .and_then(|runtime| runtime.board())
        .map(|board| board.quote_tasks())
        .unwrap_or_default();
    Controls {
        context: Context {
            inspection,
            request_detail: task.request.clone(),
            return_focus_to: task.return_focus_to.clone(),
            navigate: navigate.clone(),
            quote_tasks: Rc::new(quote_tasks),
        },
        detail: DetailContext {
            detail,
            can_note: runtime
                .as_ref()
                .is_some_and(|runtime| runtime.offers(CommandKind::TaskNote)),
            note,
            submit_note: task.submit_note.clone(),
        },
    }
}

pub(super) fn provide(controls: Controls, canvas: Html) -> Html {
    html! {
        <ContextProvider<Context> context={controls.context}>
            <ContextProvider<DetailContext> context={controls.detail}>
                <image_viewer::Provider><mermaid::Provider>{canvas}</mermaid::Provider></image_viewer::Provider>
            </ContextProvider<DetailContext>>
        </ContextProvider<Context>>
    }
}

/// The quick view belongs to Chat and reads its task without selecting it in
/// Work. It owns Escape only while Chat is visible, and whichever way Chat
/// leaves view, including a handoff to Work, the view ends with it.
#[hook]
pub(super) fn use_surface_lifetime(controls: &Controls, visible: bool) {
    let context = controls.context.clone();
    let open = context
        .inspection
        .as_ref()
        .is_some_and(|inspection| inspection.open);
    let dismiss = {
        let context = context.clone();
        Callback::from(move |()| context.close(true))
    };
    overlay::use_escape_owner(visible && open, dismiss);
    let present = context.inspection.is_some();
    use_effect_with(visible, move |visible| {
        if !*visible && present {
            context.inspection.set(None);
        }
    });
}

impl Context {
    pub fn inspect(&self, card: String, handle: TaskHandle) {
        let Ok(task) = handle.born() else {
            return;
        };
        self.inspection.set(Some(Inspection {
            card,
            handle,
            task: task.clone(),
            open: true,
            return_focus: false,
        }));
        self.request_detail.emit(task);
    }

    pub fn quote_tasks(&self) -> &BTreeMap<String, TaskHandle> {
        &self.quote_tasks
    }

    pub fn open_quote_task(&self, handle: TaskHandle) {
        let Ok(task) = handle.born() else {
            return;
        };
        if self.quote_tasks.contains_key(&task.to_string()) {
            self.navigate.emit(navigation::Intent::Task(task));
        }
    }

    /// Begin closing the open view once; the card ends it when its layer's
    /// slide completes, or at once when nothing animates.
    pub fn close(&self, return_focus: bool) {
        let Some(mut inspection) = (*self.inspection)
            .clone()
            .filter(|inspection| inspection.open)
        else {
            return;
        };
        inspection.open = false;
        inspection.return_focus = return_focus;
        self.inspection.set(Some(inspection));
    }

    pub fn complete_close(&self, card: &str) {
        let closing = (*self.inspection)
            .as_ref()
            .is_some_and(|inspection| inspection.card == card && !inspection.open);
        if closing {
            self.inspection.set(None);
        }
    }

    /// Replace the card-local quick view with the shared task detail layer.
    fn open_detail(&self, task: TaskStamp) {
        let return_target = (*self.inspection).as_ref().and_then(return_target);
        self.inspection.set(None);
        self.navigate.emit(navigation::Intent::Task(task));
        if let Some(return_target) = return_target {
            self.return_focus_to.emit(return_target);
        }
    }
}

pub(super) fn return_target(inspection: &Inspection) -> Option<web_sys::Element> {
    let selector = format!(
        ".message-task-chip[data-task-handle=\"{}\"]",
        inspection.handle
    );
    web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id(&inspection.card))
        .and_then(|card| card.query_selector(&selector).ok().flatten())
}

#[derive(Properties, PartialEq)]
struct InspectorProperties {
    context: Context,
    inspection: Inspection,
    root: NodeRef,
    close: Callback<MouseEvent>,
}

#[function_component(MessageTaskInspector)]
fn message_task_inspector(properties: &InspectorProperties) -> Html {
    let detail =
        use_context::<DetailContext>().expect("task inspector detail context must be provided");
    let title = detail
        .detail
        .as_ref()
        .and_then(TaskDetailState::answer)
        .map_or_else(
            || format!("Task {}", properties.inspection.handle),
            |answer| answer.task.presentation_title.clone(),
        );
    let retry = {
        let request = properties.context.request_detail.clone();
        let task = properties.inspection.task.clone();
        Callback::from(move |_| request.emit(task.clone()))
    };
    let open_detail = {
        let context = properties.context.clone();
        let task = properties.inspection.task.clone();
        Callback::from(move |_| context.open_detail(task.clone()))
    };
    html! {
        <section
            class="message-task-inspector"
            aria-label={format!("Task {} quick view", properties.inspection.handle)}
            data-task-stamp={properties.inspection.task.to_string()}
        >
            <header class="message-task-inspector-header work-detail-header">
                <div>
                    <p class="work-detail-kicker">{"Task detail"}</p>
                    <h2 title={title.clone()}>{title}</h2>
                </div>
                <span class="message-actions">
                    <button
                        type="button"
                        class="card-action"
                        aria-label="Open task detail"
                        title="Open task detail"
                        onclick={open_detail}
                    >
                        <svg class="card-action-icon message-task-inspector-icon" width="15" height="15" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
                            <path d="M8 16 16 8M9 8h7v7"></path>
                        </svg>
                    </button>
                    <button
                        type="button"
                        class="card-action"
                        aria-label="Close task quick view"
                        title="Close task quick view"
                        onclick={properties.close.clone()}
                    >
                        <svg class="card-action-icon message-task-inspector-icon" width="15" height="15" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
                            <path d="M7 7 17 17M17 7 7 17"></path>
                        </svg>
                    </button>
                </span>
            </header>
            <div
                ref={properties.root.clone()}
                class="message-task-inspector-body"
                role="region"
                aria-label="Task detail contents"
                tabindex="-1"
            >
                {render_detail(&detail, &properties.inspection, retry)}
            </div>
        </section>
    }
}

pub(super) fn render(
    context: &Context,
    inspection: &Inspection,
    root: NodeRef,
    close: Callback<MouseEvent>,
) -> Html {
    html! {
        <MessageTaskInspector
            context={context.clone()}
            inspection={inspection.clone()}
            {root}
            {close}
        />
    }
}

fn render_detail(
    context: &DetailContext,
    inspection: &Inspection,
    retry: Callback<MouseEvent>,
) -> Html {
    match context.detail.as_ref() {
        None | Some(TaskDetailState::Loading) => html! {
            <div class="work-detail-loading message-task-inspector-state" role="status">
                <span class="work-detail-spinner" aria-hidden="true"></span>
                <p>{"Loading task evidence and observations…"}</p>
            </div>
        },
        Some(TaskDetailState::Unavailable { reason, detail }) => html! {
            <div class="work-detail-problem message-task-inspector-state" role="alert">
                <h3>{"Task detail unavailable"}</h3>
                <p><code class="message-task-inspector-token" title={reason.clone()}>{reason}</code></p>
                if let Some(detail) = detail {
                    <p>{detail}</p>
                }
                <button type="button" class="canvas-compact-control" onclick={retry}>{"Retry detail"}</button>
            </div>
        },
        Some(TaskDetailState::Available(answer) | TaskDetailState::Refreshing(answer)) => {
            render_available(context, inspection, answer)
        }
    }
}

fn render_available(
    context: &DetailContext,
    inspection: &Inspection,
    answer: &wire::intent::TaskDetailAnswer,
) -> Html {
    let statement = &answer.task.statement.body;
    let workflow = &answer.task.workflow.body;
    let note_start = answer.notes.len().saturating_sub(3);
    let handle = inspection.handle.to_string();
    let project = statement.project.clone();
    let status = workflow.status.clone();
    let phase = workflow.phase.to_string();
    let review = workflow.review.clone();
    html! {
        <div class="work-detail-content message-task-inspector-content">
            <div class="work-detail-identity">
                <code class="message-task-inspector-token" title={handle.clone()}>{handle}</code>
            </div>
            <section class="work-detail-section">
                <h3>{"Contract"}</h3>
                if let Some(description) = statement.description.as_ref() {
                    <p class="work-detail-description">{description}</p>
                } else {
                    <p class="muted">{"No description was recorded."}</p>
                }
            </section>
            {task_detail::render_media(&answer.media)}
            <section class="work-detail-section">
                <h3>{"State"}</h3>
                <dl class="work-detail-facts">
                    <div><dt>{"Project"}</dt><dd class="message-task-inspector-token" title={project.clone()}>{project}</dd></div>
                    <div><dt>{"Status"}</dt><dd class="message-task-inspector-token" title={status.clone()}>{status}</dd></div>
                    <div><dt>{"Current phase"}</dt><dd class="message-task-inspector-token" title={phase.clone()}>{phase}</dd></div>
                    <div><dt>{"Review"}</dt><dd class="message-task-inspector-token" title={review.clone()}>{review}</dd></div>
                </dl>
            </section>
            <section class="work-detail-section work-detail-activity">
                <div class="work-detail-section-heading">
                    <h3>{"Activity"}</h3>
                    <span>{format!("{} observations", answer.notes.len())}</span>
                </div>
                if answer.notes.is_empty() {
                    <p class="muted">{"No observations have been recorded."}</p>
                } else {
                    <ol class="work-note-list">
                        {for answer.notes[note_start..].iter().rev().map(|note| html! {
                            <li class="work-note-card" key={note.position.to_string()}>
                                <header>
                                    <span>{format!("Observation {}", note.position)}</span>
                                    {task_time(&note.at, TaskTimePresentation::DetailedElapsed)}
                                </header>
                                <p>{note.text.clone()}</p>
                            </li>
                        })}
                    </ol>
                }
                {task_detail::render_note_composer(
                    "message-task-inspector-note",
                    inspection.handle.as_str(),
                    &inspection.task,
                    context.can_note,
                    &context.note,
                    &context.submit_note,
                )}
            </section>
        </div>
    }
}
