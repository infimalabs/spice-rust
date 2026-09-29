//! Contextual detail drawer for one durable Work task identity over any page.

use super::super::clock::{TaskTimePresentation, task_time};
use super::super::work_card;
use super::*;
use crate::TaskRelationsState;
use crate::runtime::LaneView;
use crate::work::{WorkProjection, WorkScope, WorkTask};
use wire::TaskStamp;

pub(super) struct WorkDetailRender<'a, 'work> {
    pub work: &'a WorkProjection<'work>,
    pub scope: &'a WorkScope,
    pub lanes: Option<&'a LaneView>,
    pub selected: Option<&'a TaskStamp>,
    pub presented: bool,
    pub closing: bool,
    pub map: &'a UseStateHandle<work_map::WorkMapState>,
    pub detail: Option<&'a TaskDetailState>,
    pub relations: Option<&'a TaskRelationsState>,
    pub can_read_relations: bool,
    pub stale: bool,
    pub can_note: bool,
    pub close_ref: &'a NodeRef,
    pub sheet_ref: &'a NodeRef,
    pub notes: &'a NoteComposers,
    pub submit_note: Callback<(TaskStamp, SubmitEvent)>,
    pub select_task: Callback<TaskStamp>,
    pub close: Callback<()>,
    pub retry_relations: Callback<MouseEvent>,
}

pub(super) fn render(render: WorkDetailRender<'_, '_>) -> Html {
    let Some(selected) = render.selected else {
        return Html::default();
    };
    let map_open = render.map.mode() == work_map::WorkDetailMode::Map;
    let refreshing = render.detail.is_some_and(TaskDetailState::refreshing);
    let close_click = render.close.reform(|_: MouseEvent| ());
    let state = if render.closing {
        "closing"
    } else if render.presented {
        "open"
    } else {
        "opening"
    };
    html! {
        <div
            class="work-detail-layer"
            data-work-detail-state={state}
            inert={render.closing.then_some("true")}
        >
            <button
                type="button"
                class="work-detail-backdrop"
                aria-label="Close task detail"
                onclick={close_click.clone()}
            ></button>
            <aside
                ref={render.sheet_ref.clone()}
                class={classes!("work-detail", map_open.then_some("work-detail--map"))}
                role="dialog"
                aria-modal="true"
                aria-labelledby="work-detail-title"
                data-task-stamp={selected.to_string()}
            >
                <header class="work-detail-header">
                    <div>
                        <p class="work-detail-kicker">{if map_open { "Task map" } else { "Task detail" }}</p>
                        <h2 id="work-detail-title">{detail_title(render.detail, selected)}</h2>
                    </div>
                    <button
                        ref={render.close_ref.clone()}
                        type="button"
                        class="work-detail-close canvas-compact-control"
                        aria-label="Close task detail"
                        onclick={close_click}
                    >{"Close"}</button>
                </header>
                if render.stale {
                    <p class="work-detail-state" role="status">
                        {"Showing admitted detail while reconnecting. Updates and observations will resume when Canvas reconnects."}
                    </p>
                }
                {work_map::render_tabs(render.map, selected)}
                <div
                    id="work-detail-panel"
                    class="work-detail-panel"
                    role="tabpanel"
                    aria-labelledby="work-detail-tab"
                    aria-hidden={map_open.to_string()}
                    hidden={map_open}
                >
                    <p class="work-detail-state" role="status" hidden={!refreshing}>
                        {"Refreshing task evidence without closing this detail."}
                    </p>
                    {render_detail_body(&render, selected)}
                </div>
                <div
                    id="work-map-panel"
                    class="work-detail-panel"
                    role="tabpanel"
                    aria-labelledby="work-map-tab"
                    aria-hidden={(!map_open).to_string()}
                    hidden={!map_open}
                >
                    {work_map::render(work_map::WorkMapRender {
                        selected,
                        state: render.map,
                        select_task: render.select_task.clone(),
                        relations: render.relations,
                        can_read_relations: render.can_read_relations,
                        retry: render.retry_relations.clone(),
                        work: render.work,
                    })}
                </div>
            </aside>
        </div>
    }
}

fn detail_title(detail: Option<&TaskDetailState>, selected: &TaskStamp) -> String {
    detail.and_then(TaskDetailState::answer).map_or_else(
        || format!("Task {selected}"),
        |answer| answer.task.presentation_title.clone(),
    )
}

fn render_detail_body(render: &WorkDetailRender<'_, '_>, selected: &TaskStamp) -> Html {
    match render.detail {
        None | Some(TaskDetailState::Loading) => html! {
            <div class="work-detail-loading" role="status">
                <span class="work-detail-spinner" aria-hidden="true"></span>
                <p>{"Loading task evidence and observations…"}</p>
            </div>
        },
        Some(TaskDetailState::Unavailable { reason, detail }) => {
            let retry = {
                let select = render.select_task.clone();
                let task = selected.clone();
                Callback::from(move |_| select.emit(task.clone()))
            };
            html! {
                <div class="work-detail-problem" role="alert">
                    <h3>{"Task detail unavailable"}</h3>
                    <p><code>{reason}</code></p>
                    if let Some(detail) = detail {
                        <p>{detail}</p>
                    }
                    <button type="button" class="canvas-compact-control" onclick={retry}>{"Retry detail"}</button>
                </div>
            }
        }
        Some(TaskDetailState::Available(answer) | TaskDetailState::Refreshing(answer)) => {
            render_available(render, answer)
        }
    }
}

fn render_available(
    render: &WorkDetailRender<'_, '_>,
    answer: &wire::intent::TaskDetailAnswer,
) -> Html {
    let row = &answer.task;
    let statement = &row.statement.body;
    let workflow = &row.workflow.body;
    let projected = render
        .work
        .tasks()
        .iter()
        .find(|task| task.row().statement.body.born == statement.born);
    let copy_identity = {
        let identity = statement.handle.to_string();
        Callback::from(move |_| chrome::copy_text(&identity))
    };
    let scope_notice = projected.and_then(|task| {
        let project = &render.work.projects()[task.project()];
        let stem = render.work.stems()[project.stem()].identity();
        render
            .scope
            .stem()
            .filter(|active| *active != stem)
            .map(|active| {
                format!(
                    "This task is now in {} under {}; your active {} scope remains unchanged.",
                    statement.project, stem, active
                )
            })
    });
    html! {
        <div class="work-detail-content">
            <div class="work-detail-identity">
                <code>{statement.handle.to_string()}</code>
                <button type="button" class="canvas-compact-control" onclick={copy_identity}>{"Copy identity"}</button>
            </div>
            if let Some(notice) = scope_notice {
                <p class="work-detail-scope" role="status">{notice}</p>
            }
            <section class="work-detail-section">
                <h3>{"Contract"}</h3>
                if let Some(description) = &statement.description {
                    <p class="work-detail-description">{description}</p>
                } else {
                    <p class="muted">{"No description was recorded."}</p>
                }
                if statement.acceptance.is_empty() {
                    <p class="muted">{"No acceptance criteria were recorded."}</p>
                } else {
                    <h4>{"Acceptance"}</h4>
                    <ul>{for statement.acceptance.iter().map(|criterion| html! {
                        <li>{criterion}</li>
                    })}</ul>
                }
            </section>
            {task_detail::render_media(&answer.media)}
            <section class="work-detail-section">
                <h3>{"State"}</h3>
                <dl class="work-detail-facts">
                    {fact("Project", statement.project.clone())}
                    {fact("Status", workflow.status.clone())}
                    {fact("Current phase", workflow.phase.to_string())}
                    {fact("Flow", workflow.flow.iter().map(ToString::to_string).collect::<Vec<_>>().join(" → "))}
                    {fact("Review", workflow.review.clone())}
                    {assignment_fact(workflow.assignment.as_ref(), render.lanes)}
                    {priority_fact(&statement.tags)}
                    {fact("Tags", if statement.tags.is_empty() { "None".to_owned() } else { statement.tags.join(", ") })}
                    {deadline_fact("Deferral", workflow.deferral.as_ref(), "None")}
                    {instant_fact("Completion", workflow.completion.as_ref(), "Open")}
                </dl>
                {render_possession(row, render.lanes)}
                {render_blockers(projected)}
            </section>
            <section class="work-detail-section">
                <h3>{"Relationships"}</h3>
                {render_relationships(render.work, projected, statement, &render.select_task)}
            </section>
            <section class="work-detail-section">
                <h3>{"Provenance"}</h3>
                <dl class="work-detail-facts">
                    {fact("Origin", statement.provenance.to_string())}
                    {fact("Origin thread", row.origin_thread.clone())}
                    {fact("Creator team", row.creator_team.map_or_else(|| "None".to_owned(), |team| team.to_string()))}
                    {fact("Queue fact", statement.queue_origin.to_string())}
                    {fact("Statement revision", statement.revision.to_string())}
                    {fact("Workflow fact", row.workflow.position.to_string())}
                </dl>
                {render_claim_context(row)}
                {render_git_context(row)}
            </section>
            {render_activity(render, answer)}
            <button type="button" class="work-detail-back canvas-compact-control" onclick={render.close.reform(|_: MouseEvent| ())}>
                {"Back to Work"}
            </button>
        </div>
    }
}

/// The task's observations and the composer that appends to them.
fn render_activity(
    render: &WorkDetailRender<'_, '_>,
    answer: &wire::intent::TaskDetailAnswer,
) -> Html {
    let statement = &answer.task.statement.body;
    html! {
        <section class="work-detail-section work-detail-activity">
            <div class="work-detail-section-heading">
                <h3>{"Activity"}</h3>
                <span>{format!("{} observations", answer.notes.len())}</span>
            </div>
            if answer.notes.is_empty() {
                <p class="muted">{"No observations have been recorded."}</p>
            } else {
                <ol class="work-note-list">
                    {for answer.notes.iter().rev().map(|note| html! {
                        <li class="work-note-card" key={note.position.to_string()} data-note-position={note.position.to_string()}>
                            <header>
                                <strong title={note.author.to_string()}>
                                    {work_card::actor_name(&note.author, render.lanes)}
                                </strong>
                                {task_time(&note.at, TaskTimePresentation::DetailedElapsed)}
                            </header>
                            <p>{note.text.clone()}</p>
                        </li>
                    })}
                </ol>
            }
            {task_detail::render_note_composer(
                "work-detail-note",
                statement.handle.as_str(),
                &statement.born,
                render.can_note,
                &render.notes.shown(&statement.born),
                &render.submit_note,
            )}
        </section>
    }
}

fn fact(label: &'static str, value: String) -> Html {
    html! { <div><dt>{label}</dt><dd>{value}</dd></div> }
}

fn instant_fact(label: &'static str, instant: Option<&wire::Instant>, empty: &'static str) -> Html {
    let value = instant.map_or_else(
        || html! { empty },
        |instant| task_time(instant, TaskTimePresentation::DetailedElapsed),
    );
    html! { <div><dt>{label}</dt><dd>{value}</dd></div> }
}

fn deadline_fact(
    label: &'static str,
    instant: Option<&wire::Instant>,
    empty: &'static str,
) -> Html {
    let value = instant.map_or_else(
        || html! { empty },
        |instant| task_time(instant, TaskTimePresentation::DetailedRemaining),
    );
    html! { <div><dt>{label}</dt><dd>{value}</dd></div> }
}

fn assignment_fact(assignment: Option<&wire::Actor>, lanes: Option<&LaneView>) -> Html {
    assignment.map_or_else(
        || fact("Assignment", "Unassigned".to_owned()),
        |actor| actor_fact("Assignment", actor, String::new(), lanes),
    )
}

/// A fact naming one actor for the reader, its durable spelling on hover.
fn actor_fact(
    label: &'static str,
    actor: &wire::Actor,
    suffix: String,
    lanes: Option<&LaneView>,
) -> Html {
    html! {
        <div>
            <dt>{label}</dt>
            <dd title={actor.to_string()}>{work_card::actor_name(actor, lanes)}{suffix}</dd>
        </div>
    }
}

fn priority_fact(tags: &[String]) -> Html {
    tags.iter()
        .find_map(|tag| tag.strip_prefix("priority:"))
        .map_or_else(Html::default, |priority| {
            fact("Priority", priority.to_owned())
        })
}

fn render_possession(row: &wire::intent::BoardTask, lanes: Option<&LaneView>) -> Html {
    let claim = row.claim.as_ref();
    let lease = row.lease.as_ref();
    html! {
        <div class="work-detail-subsection">
            <h4>{"Claim and lease"}</h4>
            <dl class="work-detail-facts">
                {claim.map_or_else(|| fact("Claim", "Unclaimed".to_owned()), |claim| {
                    actor_fact(
                        "Claim",
                        &claim.body.actor,
                        format!(" · {:?}", claim.body.disposition),
                        lanes,
                    )
                })}
                {fact("Claim fact", claim.map_or_else(|| "None".to_owned(), |claim| claim.position.to_string()))}
                {instant_fact("Lease start", lease.map(|lease| &lease.body.start), "None")}
                {deadline_fact("Lease expiry", lease.map(|lease| &lease.body.expiry), "None")}
            </dl>
        </div>
    }
}

fn render_blockers(task: Option<&WorkTask<'_>>) -> Html {
    let Some(task) = task else {
        return html! { <p class="muted">{"Blocker projection is unavailable in the current board snapshot."}</p> };
    };
    let dependencies = task.blocked_dependencies().count();
    let environment = task.environment_blocked();
    let summary = match (dependencies, environment) {
        (0, false) => "No active blockers.".to_owned(),
        (0, true) => "Environmental requirements are not satisfied.".to_owned(),
        (count, false) => format!("{count} incomplete dependencies."),
        (count, true) => {
            format!("{count} incomplete dependencies and unsatisfied environmental requirements.")
        }
    };
    html! { <p class={classes!("work-detail-blockers", (dependencies > 0 || environment).then_some("active"))}>{summary}</p> }
}

fn render_relationships(
    work: &WorkProjection<'_>,
    task: Option<&WorkTask<'_>>,
    statement: &wire::intent::BoardStatement,
    select: &Callback<TaskStamp>,
) -> Html {
    let dependencies = statement
        .dependencies
        .iter()
        .map(|born| render_task_link(work, born, "Dependency", select));
    let dependents = task
        .into_iter()
        .flat_map(|task| task.dependents().iter())
        .map(|dependent| {
            let row = work.tasks()[dependent.task()].row();
            render_task_link(work, &row.statement.body.born, "Dependent", select)
        });
    if statement.dependencies.is_empty() && task.is_none_or(|task| task.dependents().is_empty()) {
        return html! { <p class="muted">{"No dependencies or dependents."}</p> };
    }
    html! { <div class="work-detail-links">{for dependencies.chain(dependents)}</div> }
}

fn render_task_link(
    work: &WorkProjection<'_>,
    born: &TaskStamp,
    relationship: &'static str,
    select: &Callback<TaskStamp>,
) -> Html {
    let row = work
        .tasks()
        .iter()
        .find(|task| task.row().statement.body.born == *born)
        .map(WorkTask::row);
    let target = born.clone();
    let activate = {
        let select = select.clone();
        Callback::from(move |_| select.emit(target.clone()))
    };
    html! {
        <button type="button" class="work-detail-link" onclick={activate}>
            <span>{relationship}</span>
            <strong>{row.map_or_else(|| born.to_string(), |row| row.statement.body.handle.to_string())}</strong>
            if let Some(row) = row {
                <small>{row.presentation_title.clone()}</small>
            }
        </button>
    }
}

fn render_claim_context(row: &wire::intent::BoardTask) -> Html {
    let Some(context) = &row.claim_context else {
        return Html::default();
    };
    let recovery = match &context.recovery {
        wire::intent::TaskTranscriptRecovery::Transcript { session } => {
            format!("Transcript session {session}")
        }
        wire::intent::TaskTranscriptRecovery::Unavailable { reason } => {
            format!("Transcript unavailable: {reason}")
        }
    };
    html! {
        <div class="work-detail-subsection">
            <h4>{"Claim provenance"}</h4>
            <dl class="work-detail-facts">
                {fact("Claimed fact", context.claimed.to_string())}
                {fact("Lane", context.lane.to_string())}
                {fact("Occupant", context.occupant.to_string())}
                {fact("Resident epoch", context.epoch.to_string())}
                {fact("Recovery", recovery)}
            </dl>
        </div>
    }
}

fn render_git_context(row: &wire::intent::BoardTask) -> Html {
    let Some(context) = &row.git_context else {
        return Html::default();
    };
    html! {
        <div class="work-detail-subsection">
            <h4>{"Repository boundary"}</h4>
            <dl class="work-detail-facts">
                {fact("Lifecycle", context.kind.as_str().to_owned())}
                {fact("Fact", context.fact.to_string())}
                {fact("Head", context.boundary.head.clone())}
                {fact("Integrated", context.boundary.integrated.clone())}
                {fact("Upstream", context.boundary.upstream.clone().unwrap_or_else(|| "None".to_owned()))}
                {fact("Local commits", context.boundary.local_commits.to_string())}
            </dl>
        </div>
    }
}
