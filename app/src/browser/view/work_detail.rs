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
    pub navigate: &'a Callback<navigation::Intent>,
    pub team_controls: &'a UseReducerHandle<TeamControls>,
    pub creator_messages: &'a [Rc<wire::AgentMessage>],
    pub selected: Option<&'a TaskStamp>,
    pub presented: bool,
    pub closing: bool,
    pub map: &'a UseStateHandle<work_map::WorkMapState>,
    pub detail: Option<&'a TaskDetailState>,
    pub relations: Option<&'a TaskRelationsState>,
    pub can_read_relations: bool,
    pub can_note: bool,
    pub can_close: bool,
    pub close_ref: &'a NodeRef,
    pub sheet_ref: &'a NodeRef,
    pub notes: &'a NoteComposers,
    pub submit_note: Callback<(TaskStamp, SubmitEvent)>,
    pub select_task: Callback<TaskStamp>,
    pub request_task: Callback<TaskStamp>,
    pub close: Callback<()>,
    pub retry_relations: Callback<MouseEvent>,
}

pub(super) fn render(render: WorkDetailRender<'_, '_>) -> Html {
    let Some(selected) = render.selected else {
        return Html::default();
    };
    let map_open = render.map.mode() == work_map::WorkDetailMode::Map;
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
                        class="work-detail-close app-compact-control"
                        aria-label="Close task detail"
                        onclick={close_click}
                    >{"Close"}</button>
                </header>
                {work_map::render_tabs(render.map, selected)}
                <div
                    id="work-detail-panel"
                    class="work-detail-panel"
                    role="tabpanel"
                    aria-labelledby="work-detail-tab"
                    aria-hidden={map_open.to_string()}
                    hidden={map_open}
                >
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
                let request = render.request_task.clone();
                let task = selected.clone();
                Callback::from(move |_| request.emit(task.clone()))
            };
            html! {
                <div class="work-detail-problem" role="alert">
                    <h3>{"Task detail unavailable"}</h3>
                    <p><code>{reason}</code></p>
                    if let Some(detail) = detail {
                        <p>{detail}</p>
                    }
                    <button type="button" class="app-compact-control" onclick={retry}>{"Retry detail"}</button>
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
    let messages = render.creator_messages.iter().map(AsRef::as_ref);
    let creator_context = crate::view::creator_context_href(&statement.creation, messages);
    html! {
        <div
            class="work-detail-content"
            onclick={task_detail::task_text_chip_click(&render.select_task)}
            onkeydown={task_detail::task_text_chip_keydown(&render.select_task)}
        >
            <div class="work-detail-identity">
                <code>{statement.handle.to_string()}</code>
                <chrome::CopyButton
                    class="app-compact-control"
                    text={AttrValue::from(statement.handle.to_string())}
                    subject="task handle"
                >{"Copy identity"}</chrome::CopyButton>
                if let Some(href) = creator_context {
                    {task_detail::creator_context_link(
                        "work-detail-context-link",
                        href,
                        statement,
                        None,
                    )}
                }
            </div>
            <crate::browser::toast::Notice message={scope_notice.map(AttrValue::from)} />
            <section class="work-detail-section">
                <h3>{"Contract"}</h3>
                {task_detail::render_contract(statement, |task| board_has(render.work, task), true)}
            </section>
            {task_detail::render_media(&answer.media)}
            <section class="work-detail-section">
                <h3>{"State"}</h3>
                <dl class="work-detail-facts">
                    {project_fact(render, &statement.project)}
                    {fact("Status", workflow.status.clone())}
                    {fact("Current phase", workflow.phase.to_string())}
                    {fact("Flow", workflow.flow.iter().map(ToString::to_string).collect::<Vec<_>>().join(" → "))}
                    {fact("Review", workflow.review.clone())}
                    {assignment_fact(workflow.assignment.as_ref(), render)}
                    {priority_fact(&statement.tags)}
                    {fact("Tags", if statement.tags.is_empty() { "None".to_owned() } else { statement.tags.join(", ") })}
                    {deadline_fact("Deferral", workflow.deferral.as_ref(), "None")}
                    {instant_fact("Completion", workflow.completion.as_ref(), "Open")}
                </dl>
                {render_possession(row, render)}
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
                    {creator_team_fact(row.creator_team, render)}
                    {fact("Queue fact", statement.queue_origin.to_string())}
                    {fact("Statement revision", statement.revision.to_string())}
                    {fact("Workflow fact", row.workflow.position.to_string())}
                </dl>
                {render_claim_context(row)}
                {render_git_context(row)}
            </section>
            {render_activity(render, answer)}
            <button type="button" class="work-detail-back app-compact-control" onclick={render.close.reform(|_: MouseEvent| ())}>
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
            {task_detail::render_closure_record(
                answer.closure.as_ref(),
                |actor| work_card::actor_name(actor, render.lanes),
                |task| board_has(render.work, task),
            )}
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
                            {task_detail::render_task_text(&note.text, "work-note-body", |task| board_has(render.work, task))}
                        </li>
                    })}
                </ol>
            }
            {task_detail::render_note_composer(
                "work-detail-note",
                statement.handle.as_str(),
                &statement.born,
                render.can_note,
                task_detail::closure(render.can_close, &answer.task).as_ref(),
                &render.notes.shown(&statement.born),
                &render.submit_note,
            )}
        </section>
    }
}

fn fact(label: &'static str, value: String) -> Html {
    html! { <div><dt>{label}</dt><dd>{value}</dd></div> }
}

fn project_fact(render: &WorkDetailRender<'_, '_>, project: &str) -> Html {
    let Some(scope) = render.work.scope_for_project(project) else {
        return fact("Project", project.to_owned());
    };
    let navigate = render.navigate.clone();
    let destination = scope.clone();
    let project = project.to_owned();
    html! {
        <div>
            <dt>{"Project"}</dt>
            <dd title={project.clone()}>
                <button
                    type="button"
                    class="work-detail-fact-link"
                    data-task-project-link={project.clone()}
                    aria-label={format!("Show {project} in Work")}
                    onclick={Callback::from(move |_| {
                        navigate.emit(navigation::Intent::Work(destination.clone()));
                    })}
                >{project}</button>
            </dd>
        </div>
    }
}

fn creator_team_fact(team: Option<wire::CanonicalUuid>, render: &WorkDetailRender<'_, '_>) -> Html {
    let Some(team) = team else {
        return fact("Creator team", "None".to_owned());
    };
    let destination = render
        .lanes
        .and_then(|lanes| lanes.chat_target_for_team(team));
    linked_identity_fact(
        "Creator team",
        team.to_string(),
        team.to_string(),
        String::new(),
        destination,
        render,
    )
}

fn linked_identity_fact(
    label: &'static str,
    display: String,
    identity: String,
    suffix: String,
    destination: Option<(wire::CanonicalUuid, wire::CanonicalUuid)>,
    render: &WorkDetailRender<'_, '_>,
) -> Html {
    let Some((team, member)) = destination else {
        return html! {
            <div><dt>{label}</dt><dd title={identity}>{display}{suffix}</dd></div>
        };
    };
    let href = crate::view::card_href(&super::super::watchband::primary_textarea_id(
        &member.to_string(),
    ));
    // Following the link changes the route, which closes the detail.
    let controls = render.team_controls.clone();
    let follow = Callback::from(move |event: MouseEvent| {
        if super::super::card_target::follows_in_tab(&event) {
            controls.dispatch(TeamControlAction::ShowChat { team });
        }
    });
    html! {
        <div>
            <dt>{label}</dt>
            <dd title={identity}>
                <a
                    class="work-detail-fact-link"
                    data-task-chat-link={member.to_string()}
                    {href}
                    aria-label={format!("Open {display} in Chat")}
                    onclick={follow}
                >{display}</a>
                {suffix}
            </dd>
        </div>
    }
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

fn assignment_fact(assignment: Option<&wire::Actor>, render: &WorkDetailRender<'_, '_>) -> Html {
    assignment.map_or_else(
        || fact("Assignment", "Unassigned".to_owned()),
        |actor| actor_fact("Assignment", actor, String::new(), render),
    )
}

/// A fact naming one actor for the reader, its durable spelling on hover.
fn actor_fact(
    label: &'static str,
    actor: &wire::Actor,
    suffix: String,
    render: &WorkDetailRender<'_, '_>,
) -> Html {
    let destination = match actor {
        wire::Actor::Lane(lane) => render
            .lanes
            .and_then(|lanes| lanes.chat_target_for_lane(*lane)),
        wire::Actor::Operator | wire::Actor::Spice => None,
    };
    linked_identity_fact(
        label,
        work_card::actor_name(actor, render.lanes),
        actor.to_string(),
        suffix,
        destination,
        render,
    )
}

fn priority_fact(tags: &[String]) -> Html {
    tags.iter()
        .find_map(|tag| tag.strip_prefix("priority:"))
        .map_or_else(Html::default, |priority| {
            fact("Priority", priority.to_owned())
        })
}

fn render_possession(row: &wire::intent::BoardTask, render: &WorkDetailRender<'_, '_>) -> Html {
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
                        render,
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

/// Whether the admitted board holds `born`, so a contract chip can open it.
fn board_has(work: &WorkProjection<'_>, born: &TaskStamp) -> bool {
    work.tasks()
        .iter()
        .any(|task| task.row().statement.body.born == *born)
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
