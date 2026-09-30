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

/// Every message card and each team's mosaic consume this context, so it
/// compares only what they render. The commands are rebuilt each render
/// around fresh state; routing them through one shared cell keeps a
/// navigation or unrelated app render from re-rendering every retained card.
#[derive(Clone)]
pub(super) struct Context {
    pub inspection: UseStateHandle<Option<Inspection>>,
    commands: Rc<RefCell<Commands>>,
    quote_tasks: Rc<crate::runtime::QuoteTasks>,
}

impl PartialEq for Context {
    fn eq(&self, other: &Self) -> bool {
        self.inspection == other.inspection
            && Rc::ptr_eq(&self.commands, &other.commands)
            && self.quote_tasks == other.quote_tasks
    }
}

/// The latest render's commands; callers clone one out before emitting so a
/// render the command triggers can refresh the cell.
#[derive(Clone)]
struct Commands {
    request_detail: Callback<TaskStamp>,
    return_focus_to: Callback<web_sys::Element>,
    navigate: Callback<navigation::Intent>,
}

#[derive(Clone, PartialEq)]
struct DetailContext {
    detail: Option<TaskDetailState>,
    creator_context: Option<String>,
    /// Tasks the inspected description and observations name that the
    /// admitted board holds.
    board_tasks: std::collections::BTreeSet<TaskStamp>,
    can_note: bool,
    can_close: bool,
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
    let fresh = Commands {
        request_detail: task.request.clone(),
        return_focus_to: task.return_focus_to.clone(),
        navigate: navigate.clone(),
    };
    let commands = use_mut_ref(|| fresh.clone());
    *commands.borrow_mut() = fresh;
    let runtime = surface.read();
    let detail = (*inspection)
        .as_ref()
        .and_then(|inspection| {
            runtime
                .as_ref()
                .and_then(|runtime| runtime.task_detail(&inspection.task))
        })
        .cloned();
    let creator_context = detail.as_ref().and_then(|detail| {
        let answer = match detail {
            TaskDetailState::Available(answer) | TaskDetailState::Refreshing(answer) => answer,
            TaskDetailState::Loading | TaskDetailState::Unavailable { .. } => return None,
        };
        let runtime = runtime.as_ref()?;
        crate::view::creator_context_href(
            &answer.task.statement.body.creation,
            runtime.messages().iter().map(AsRef::as_ref),
        )
    });
    let board_tasks = detail
        .as_ref()
        .and_then(TaskDetailState::answer)
        .zip(runtime.as_ref().and_then(|runtime| runtime.board()))
        .map(|(answer, board)| {
            let description = answer.task.statement.body.description.as_deref();
            description
                .into_iter()
                .chain(answer.notes.iter().map(|note| note.text.as_str()))
                .flat_map(crate::watchband::task_mentions)
                .filter_map(|(_, task)| task.born().ok())
                .filter(|task| {
                    board
                        .tasks
                        .iter()
                        .any(|row| row.statement.body.born == *task)
                })
                .collect()
        })
        .unwrap_or_default();
    let note = (*inspection)
        .as_ref()
        .map(|inspection| task.notes.shown(&inspection.task))
        .unwrap_or_default();
    let quote_tasks = runtime
        .as_ref()
        .and_then(|runtime| runtime.board())
        .map(|board| Rc::clone(&board.quote_tasks))
        .unwrap_or_default();
    Controls {
        context: Context {
            inspection,
            commands,
            quote_tasks,
        },
        detail: DetailContext {
            detail,
            creator_context,
            board_tasks,
            can_note: runtime
                .as_ref()
                .is_some_and(|runtime| runtime.offers(CommandKind::TaskNote)),
            can_close: runtime
                .as_ref()
                .is_some_and(|runtime| runtime.offers(CommandKind::TaskClose)),
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
        self.request_detail(task);
    }

    fn request_detail(&self, task: TaskStamp) {
        let request = self.commands.borrow().request_detail.clone();
        request.emit(task);
    }

    /// Every task caused by the directions `message` quotes, and nothing else.
    pub fn message_quote_tasks(&self, message: &wire::AgentMessage) -> crate::runtime::QuoteTasks {
        crate::composer_markdown::referenced_quote_tasks(
            &message.quotes,
            &message.directive_contexts,
            &self.quote_tasks,
        )
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

    /// Replace the card-local quick view, when present, with the shared task
    /// detail layer. Both the inspector arrow and direct chip navigation cross
    /// this one state transition.
    pub fn open_detail(&self, task: TaskStamp, return_target: Option<web_sys::Element>) {
        self.inspection.set(None);
        let (navigate, return_focus_to) = {
            let commands = self.commands.borrow();
            (commands.navigate.clone(), commands.return_focus_to.clone())
        };
        if let Some(return_target) = return_target {
            return_focus_to.emit(return_target);
        }
        navigate.emit(navigation::Intent::Task(task));
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
        let context = properties.context.clone();
        let task = properties.inspection.task.clone();
        Callback::from(move |_| context.request_detail(task.clone()))
    };
    let open_detail = {
        let context = properties.context.clone();
        let task = properties.inspection.task.clone();
        Callback::from(move |_| {
            let focus_target = (*context.inspection).as_ref().and_then(return_target);
            context.open_detail(task.clone(), focus_target);
        })
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
                {render_detail(&detail, &properties.context, &properties.inspection, retry)}
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
    detail: &DetailContext,
    context: &Context,
    inspection: &Inspection,
    retry: Callback<MouseEvent>,
) -> Html {
    match detail.detail.as_ref() {
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
            render_available(detail, context, inspection, answer)
        }
    }
}

fn render_available(
    detail: &DetailContext,
    context: &Context,
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
    let creator_context = detail.creator_context.clone();
    let follow_creator = {
        let context = context.clone();
        Callback::from(move |event: MouseEvent| {
            if super::card_target::remember_creator_return(&event) {
                context.inspection.set(None);
            }
        })
    };
    html! {
        <div class="work-detail-content message-task-inspector-content">
            <div class="work-detail-identity">
                <code class="message-task-inspector-token" title={handle.clone()}>{handle.clone()}</code>
                if let Some(href) = creator_context {
                    {task_detail::creator_context_link(
                        "work-detail-context-link",
                        href,
                        statement,
                        Some(follow_creator),
                    )}
                }
            </div>
            <section class="work-detail-section">
                <h3>{"Contract"}</h3>
                {task_detail::render_contract(statement, |task| detail.board_tasks.contains(task), false)}
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
                {task_detail::render_closure_record(
                    answer.closure.as_ref(),
                    ToString::to_string,
                    |task| detail.board_tasks.contains(task),
                )}
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
                                {task_detail::render_task_text(&note.text, "work-note-body", |task| detail.board_tasks.contains(task))}
                            </li>
                        })}
                    </ol>
                }
                {task_detail::render_note_composer(
                    "message-task-inspector-note",
                    inspection.handle.as_str(),
                    &inspection.task,
                    detail.can_note,
                    task_detail::closure(detail.can_close, &answer.task).as_ref(),
                    &detail.note,
                    &detail.submit_note,
                )}
            </section>
        </div>
    }
}
