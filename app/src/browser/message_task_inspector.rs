use super::clock::{TaskTimePresentation, task_time};
use super::*;

const CARD_PROPERTIES: [&str; 5] = [
    "--mosaic-base",
    "--mosaic-rows",
    "--mosaic-track",
    "--mosaic-span",
    "--mosaic-measure-span",
];
const PLANE_PROPERTIES: [&str; 2] = ["--mosaic-max-row", "--mosaic-plane-rows"];

#[derive(Clone, PartialEq, Eq)]
pub(super) struct LayoutSnapshot {
    pub card: [i32; 5],
    pub plane: [i32; 2],
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Inspection {
    pub card: String,
    pub handle: TaskHandle,
    pub task: TaskStamp,
    pub scroll_top: i32,
    pub layout: LayoutSnapshot,
}

#[derive(Clone, PartialEq)]
pub(super) struct Context {
    pub inspection: UseStateHandle<Option<Inspection>>,
    pub detail: Option<TaskDetailState>,
    pub can_note: bool,
    pub note_text: UseStateHandle<String>,
    pub note_notice: UseStateHandle<Notice>,
    pub note_submission: UseStateHandle<NoteSubmission>,
    pub submit_note: Callback<SubmitEvent>,
    pub select_task: Callback<TaskStamp>,
    pub close_task: Callback<()>,
    pub navigate: Callback<CanvasDestination>,
}

#[hook]
pub(super) fn use_controls(
    surface: &UseStateHandle<SurfaceData>,
    task: &task_detail::Controls,
    navigate: &Callback<CanvasDestination>,
) -> Context {
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
    Context {
        inspection,
        detail,
        can_note: runtime
            .as_ref()
            .is_some_and(|runtime| runtime.offers(CommandKind::TaskNote)),
        note_text: task.note_text.clone(),
        note_notice: task.notice.clone(),
        note_submission: task.submission.status.clone(),
        submit_note: task.submit_note.clone(),
        select_task: task.select.clone(),
        close_task: task.close.clone(),
        navigate: navigate.clone(),
    }
}

pub(super) fn provide(context: Context, canvas: Html) -> Html {
    html! {
        <ContextProvider<Context> {context}>
            <image_viewer::Provider><mermaid::Provider>{canvas}</mermaid::Provider></image_viewer::Provider>
        </ContextProvider<Context>>
    }
}

impl Context {
    pub fn inspect(
        &self,
        card: String,
        handle: TaskHandle,
        scroll_top: i32,
        layout: LayoutSnapshot,
    ) {
        let Ok(task) = handle.born() else {
            return;
        };
        self.inspection.set(Some(Inspection {
            card,
            handle,
            task: task.clone(),
            scroll_top,
            layout,
        }));
        self.select_task.emit(task);
    }

    pub fn close(&self) {
        let inspection = (*self.inspection).clone();
        if let Some(inspection) = inspection.as_ref() {
            restore_frame(inspection);
        }
        self.inspection.set(None);
        self.close_task.emit(());
        if let Some(inspection) = inspection {
            gloo_timers::callback::Timeout::new(0, move || restore_frame(&inspection)).forget();
        }
    }

    fn open_work(&self, task: TaskStamp) {
        self.select_task.emit(task);
        self.inspection.set(None);
        self.navigate.emit(CanvasDestination::Work);
    }
}

pub(super) fn restore_frame(inspection: &Inspection) {
    let Some(card) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id(&inspection.card))
        .and_then(|card| card.dyn_into::<web_sys::HtmlElement>().ok())
    else {
        return;
    };
    for (property, value) in CARD_PROPERTIES.iter().zip(inspection.layout.card) {
        let value = value.to_string();
        if card.style().get_property_value(property).ok().as_deref() != Some(&value) {
            let _ = card.style().set_property(property, &value);
        }
    }
    if let Ok(Some(plane)) = card.closest(".lane-mosaic-plane")
        && let Ok(plane) = plane.dyn_into::<web_sys::HtmlElement>()
    {
        for (property, value) in PLANE_PROPERTIES.iter().zip(inspection.layout.plane) {
            let value = value.to_string();
            if plane.style().get_property_value(property).ok().as_deref() != Some(&value) {
                let _ = plane.style().set_property(property, &value);
            }
        }
        if plane.scroll_top() != inspection.scroll_top {
            plane.set_scroll_top(inspection.scroll_top);
        }
    }
}

pub(super) fn render(
    context: &Context,
    inspection: &Inspection,
    root: NodeRef,
    close: Callback<MouseEvent>,
) -> Html {
    let retry = {
        let select = context.select_task.clone();
        let task = inspection.task.clone();
        Callback::from(move |_| select.emit(task.clone()))
    };
    let open_work = {
        let context = context.clone();
        let task = inspection.task.clone();
        Callback::from(move |_| context.open_work(task.clone()))
    };
    html! {
        <section
            ref={root}
            class="message-task-inspector"
            tabindex="-1"
            aria-label={format!("Task {} quick view", inspection.handle)}
            data-task-stamp={inspection.task.to_string()}
        >
            <header class="message-task-inspector-header">
                <div>
                    <span>{"Task quick view"}</span>
                    <strong>{inspection.handle.to_string()}</strong>
                </div>
                <button type="button" aria-label="Close task quick view" onclick={close}>{"Close"}</button>
            </header>
            <div class="message-task-inspector-body">
                {render_detail(context, inspection, retry)}
            </div>
            <footer class="message-task-inspector-footer">
                <button type="button" onclick={open_work}>{"Open in Work"}</button>
            </footer>
        </section>
    }
}

fn render_detail(context: &Context, inspection: &Inspection, retry: Callback<MouseEvent>) -> Html {
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
    context: &Context,
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

fn render_note_composer(context: &Context, inspection: &Inspection) -> Html {
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
                    value={(*context.note_text).clone()}
                    readonly={busy}
                    oninput={state_from_textarea(context.note_text.clone())}
                ></textarea>
            </label>
            <button type="submit" disabled={busy || context.note_text.trim().is_empty()}>
                {if busy { "Saving…" } else { "Add observation" }}
            </button>
            if context.note_notice.is_some() {
                <p class="form-refusal" role="alert">{context.note_notice.to_string()}</p>
            }
            {status}
        </form>
    }
}
