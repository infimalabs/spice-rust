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
    pub select_task: Callback<TaskStamp>,
    pub close_task: Callback<()>,
    pub navigate: Callback<navigation::Intent>,
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
    Controls {
        context: Context {
            inspection,
            select_task: task.select.clone(),
            close_task: task.dismiss.clone(),
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
            open: true,
            return_focus: false,
        }));
        self.select_task.emit(task);
    }

    pub fn close(&self, return_focus: bool) {
        let Some(mut inspection) = (*self.inspection).clone() else {
            return;
        };
        inspection.return_focus = return_focus;
        if task_detail::reduced_motion() {
            self.inspection.set(None);
            self.close_task.emit(());
            restore_task_chip_focus(&inspection);
        } else {
            inspection.open = false;
            self.inspection.set(Some(inspection));
        }
    }

    pub fn complete_close(&self, card: &str) {
        let closing = (*self.inspection)
            .as_ref()
            .filter(|inspection| inspection.card == card && !inspection.open)
            .cloned();
        if let Some(inspection) = closing {
            self.inspection.set(None);
            self.close_task.emit(());
            restore_task_chip_focus(&inspection);
        }
    }

    fn open_work(&self, task: TaskStamp) {
        self.select_task.emit(task);
        self.inspection.set(None);
        self.navigate.emit(navigation::Intent::SelectedWork);
    }
}

fn restore_task_chip_focus(inspection: &Inspection) {
    if !inspection.return_focus {
        return;
    }
    let card = inspection.card.clone();
    let handle = inspection.handle.to_string();
    gloo_timers::callback::Timeout::new(0, move || {
        let Some(card) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id(&card))
        else {
            return;
        };
        let selector = format!(".message-task-chip[data-task-handle=\"{handle}\"]");
        if let Ok(Some(chip)) = card.query_selector(&selector)
            && let Ok(chip) = chip.dyn_into::<web_sys::HtmlElement>()
        {
            let options = web_sys::FocusOptions::new();
            options.set_prevent_scroll(true);
            let _ = chip.focus_with_options(&options);
        }
    })
    .forget();
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
            <header class="message-task-inspector-header work-detail-header">
                <div>
                    <p class="work-detail-kicker">{"Task detail"}</p>
                    <h2>{title}</h2>
                </div>
                <button
                    type="button"
                    class="work-detail-close"
                    aria-label="Close task quick view"
                    onclick={properties.close.clone()}
                >{"Close"}</button>
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
            <div class="work-detail-loading message-task-inspector-state" role="status">
                <span class="work-detail-spinner" aria-hidden="true"></span>
                <p>{"Loading task evidence and observations…"}</p>
            </div>
        },
        Some(TaskDetailState::Unavailable { reason, detail }) => html! {
            <div class="work-detail-problem message-task-inspector-state" role="alert">
                <h3>{"Task detail unavailable"}</h3>
                <p><code>{reason}</code></p>
                if let Some(detail) = detail {
                    <p>{detail}</p>
                }
                <button type="button" onclick={retry}>{"Retry detail"}</button>
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
        <div class="work-detail-content message-task-inspector-content">
            <div class="work-detail-identity">
                <code>{inspection.handle.to_string()}</code>
            </div>
            <section class="work-detail-section">
                <h3>{"Contract"}</h3>
                if let Some(description) = statement.description.as_ref() {
                    <p class="work-detail-description">{description}</p>
                } else {
                    <p class="muted">{"No description was recorded."}</p>
                }
            </section>
            <section class="work-detail-section">
                <h3>{"State"}</h3>
                <dl class="work-detail-facts">
                    <div><dt>{"Project"}</dt><dd>{statement.project.clone()}</dd></div>
                    <div><dt>{"Status"}</dt><dd>{workflow.status.clone()}</dd></div>
                    <div><dt>{"Current phase"}</dt><dd>{workflow.phase.to_string()}</dd></div>
                    <div><dt>{"Review"}</dt><dd>{workflow.review.clone()}</dd></div>
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
                        {for answer.notes[note_start..].iter().map(|note| html! {
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
                {render_note_composer(context, inspection)}
            </section>
        </div>
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
        <form
            class="form work-note-composer message-task-inspector-note"
            onsubmit={context.submit_note.clone()}
        >
            <label for="message-task-inspector-note">
                {format!("Observation for {}", inspection.handle)}
            </label>
            <textarea
                id="message-task-inspector-note"
                key={(*context.note_text).clone()}
                readonly={busy}
            >{(*context.note_text).clone()}</textarea>
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
