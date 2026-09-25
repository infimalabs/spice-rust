use super::clock::{TaskTimePresentation, task_time};
use super::*;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Inspection {
    pub card: String,
    pub handle: TaskHandle,
    pub task: TaskStamp,
}

#[derive(Clone, PartialEq)]
pub(super) struct Context {
    pub inspection: UseStateHandle<Option<Inspection>>,
    pub select_task: Callback<TaskStamp>,
    pub close_task: Callback<()>,
    pub navigate: Callback<CanvasDestination>,
}

#[derive(Clone, PartialEq)]
struct DetailContext {
    detail: Option<TaskDetailState>,
    can_note: bool,
    note_text: UseStateHandle<String>,
    note_notice: UseStateHandle<Notice>,
    note_submission: UseStateHandle<NoteSubmission>,
    submit_note: Callback<SubmitEvent>,
}

pub(super) struct Controls {
    context: Context,
    detail: DetailContext,
}

#[hook]
pub(super) fn use_controls(
    surface: &UseStateHandle<SurfaceData>,
    task: &task_detail::Controls,
    navigate: &Callback<CanvasDestination>,
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
    Controls {
        context: Context {
            inspection,
            select_task: task.select.clone(),
            close_task: task.close.clone(),
            navigate: navigate.clone(),
        },
        detail: DetailContext {
            detail,
            can_note: runtime
                .as_ref()
                .is_some_and(|runtime| runtime.offers(CommandKind::TaskNote)),
            note_text: task.note_text.clone(),
            note_notice: task.notice.clone(),
            note_submission: task.submission.status.clone(),
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

impl Context {
    pub fn inspect(&self, card: String, handle: TaskHandle) {
        let Ok(task) = handle.born() else {
            return;
        };
        self.inspection.set(Some(Inspection {
            card,
            handle,
            task: task.clone(),
        }));
        self.select_task.emit(task);
    }

    pub fn close(&self) {
        self.inspection.set(None);
        self.close_task.emit(());
    }

    fn open_work(&self, task: TaskStamp) {
        self.select_task.emit(task);
        self.inspection.set(None);
        self.navigate.emit(CanvasDestination::Work);
    }
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
    let retry = {
        let select = properties.context.select_task.clone();
        let task = properties.inspection.task.clone();
        Callback::from(move |_| select.emit(task.clone()))
    };
    let open_work = {
        let context = properties.context.clone();
        let task = properties.inspection.task.clone();
        Callback::from(move |_| context.open_work(task.clone()))
    };
    html! {
        <section
            ref={properties.root.clone()}
            class="message-task-inspector"
            tabindex="-1"
            aria-label={format!("Task {} quick view", properties.inspection.handle)}
            data-task-stamp={properties.inspection.task.to_string()}
        >
            <header class="message-task-inspector-header">
                <div>
                    <span>{"Task quick view"}</span>
                    <strong>{properties.inspection.handle.to_string()}</strong>
                </div>
                <button type="button" aria-label="Close task quick view" onclick={properties.close.clone()}>{"Close"}</button>
            </header>
            <div class="message-task-inspector-body">
                {render_detail(&detail, &properties.inspection, retry)}
            </div>
            <footer class="message-task-inspector-footer">
                <button
                    type="button"
                    class="message-task-inspector-open-work"
                    aria-label="Open task in Work"
                    title="Open task in Work"
                    onclick={open_work}
                >
                    {"↗"}
                </button>
            </footer>
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
            <div class="message-task-inspector-state" role="status">
                <span class="work-detail-spinner" aria-hidden="true"></span>
                <p>{"Loading task details…"}</p>
            </div>
        },
        Some(TaskDetailState::Unavailable { reason, detail }) => html! {
            <div class="message-task-inspector-state" role="alert">
                <strong>{reason}</strong>
                if let Some(detail) = detail {
                    <p>{detail}</p>
                }
                <button type="button" onclick={retry}>{"Retry"}</button>
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
    html! {
        <>
            <section class="message-task-inspector-summary">
                <h3>{answer.task.presentation_title.clone()}</h3>
                if let Some(description) = statement.description.as_ref() {
                    <p>{description}</p>
                }
                <dl>
                    <div><dt>{"Project"}</dt><dd>{statement.project.clone()}</dd></div>
                    <div><dt>{"Status"}</dt><dd>{workflow.status.clone()}</dd></div>
                    <div><dt>{"Phase"}</dt><dd>{workflow.phase.to_string()}</dd></div>
                    <div><dt>{"Review"}</dt><dd>{workflow.review.clone()}</dd></div>
                </dl>
            </section>
            <section class="message-task-inspector-notes">
                <h3>{format!("Recent observations ({})", answer.notes.len())}</h3>
                if answer.notes.is_empty() {
                    <p class="muted">{"No observations have been recorded."}</p>
                } else {
                    <ol>
                        {for answer.notes[note_start..].iter().map(|note| html! {
                            <li key={note.position.to_string()}>
                                {task_time(&note.at, TaskTimePresentation::DetailedElapsed)}
                                <p>{note.text.clone()}</p>
                            </li>
                        })}
                    </ol>
                }
            </section>
            {render_note_composer(context, inspection)}
        </>
    }
}

fn render_note_composer(context: &DetailContext, inspection: &Inspection) -> Html {
    if !context.can_note {
        return html! {
            <p class="work-note-capability">{"This capability can read observations but cannot append them."}</p>
        };
    }
    let busy = context.note_submission.busy();
    let status = match &*context.note_submission {
        NoteSubmission::Idle => Html::default(),
        NoteSubmission::Pending { task, .. } => html! {
            <p class="work-note-pending" role="status">{format!("Saving observation for {task}…")}</p>
        },
        NoteSubmission::Reconciling { task, .. } => html! {
            <p class="work-note-pending" role="status">{format!("Saved for {task}; waiting for durable activity…")}</p>
        },
        NoteSubmission::Failed(issue) => html! {
            <p class="form-refusal" role="alert">{issue}</p>
        },
    };
    html! {
        <form class="form message-task-inspector-note" onsubmit={context.submit_note.clone()}>
            <label>
                <span>{format!("Observation for {}", inspection.handle)}</span>
                <textarea
                    key={(*context.note_text).clone()}
                    readonly={busy}
                >{(*context.note_text).clone()}</textarea>
            </label>
            <button type="submit" disabled={busy}>
                {if busy { "Saving…" } else { "Add observation" }}
            </button>
            if context.note_notice.is_some() {
                <p class="form-refusal" role="alert">{context.note_notice.to_string()}</p>
            }
            {status}
        </form>
    }
}
