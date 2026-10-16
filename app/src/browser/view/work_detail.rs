//! Graph-first contextual overview for one complete selected task set.

use super::super::clock::{TaskTimePresentation, task_time};
use super::*;
use crate::composer_markdown::inline::LineControls;
use crate::runtime::LaneView;
use crate::work::{TaskOverviewBase, TaskOverviewSelection, WorkProjection, WorkTask};
use wire::TaskStamp;

pub(super) struct WorkDetailRender<'a, 'work> {
    pub work: &'a WorkProjection<'work>,
    pub lanes: Option<&'a LaneView>,
    pub navigate: &'a Callback<navigation::Intent>,
    pub creator_messages: &'a [Rc<wire::AgentMessage>],
    pub selection: &'a TaskOverviewSelection,
    pub map: &'a UseStateHandle<work_map::WorkMapState>,
    pub runtime: Option<&'a AppRuntime>,
    pub room_controls: &'a super::super::rooms::Controls,
    pub sender: &'a SenderHandle,
    pub draft_controls: &'a super::super::drafts::Controls,
    pub can_note: bool,
    pub can_close: bool,
    pub blocker_review: &'a UseStateHandle<bool>,
    pub blocker_review_button_ref: &'a NodeRef,
    pub blocker_review_close_ref: &'a NodeRef,
    pub notes: &'a NoteComposers,
    pub submit_note: Callback<(TaskStamp, SubmitEvent)>,
    pub grant_trust: Callback<task_detail::TrustGrantRequest>,
    pub request_task: Callback<TaskStamp>,
    pub open_overview: Callback<TaskOverviewSelection>,
    pub retry_relations: Callback<MouseEvent>,
}

pub(super) fn render(render: WorkDetailRender<'_, '_>) -> sidebar::Content {
    let focus_task = focus_task_callback();
    let count = render.selection.tasks().len();
    let blockers = super::work_blockers::BlockerPresentation::new(render.work, render.selection);
    let body = html! {
        <div
            class="work-overview"
            data-task-count={count.to_string()}
            data-task-stamps={render.selection.tasks().iter().map(ToString::to_string).collect::<Vec<_>>().join(" ")}
        >
            {blockers.render_rail(render.blocker_review, render.blocker_review_button_ref, render.navigate)}
            {work_map::render(work_map::WorkMapRender {
                selection: render.selection,
                state: render.map,
                focus_task: focus_task.clone(),
                open_overview: render.open_overview.clone(),
                work: render.work,
                runtime: render.runtime,
                retry: render.retry_relations.clone(),
            })}
            <section class="work-overview-details" aria-labelledby="work-overview-details-title">
                <div class="work-map-heading">
                    <h3 id="work-overview-details-title">{"Task details"}</h3>
                    <span>{format!("{count} selected")}</span>
                </div>
                if render.selection.is_empty() {
                    <p class="work-map-empty">{"No task details exist for this selection."}</p>
                } else {
                    {for render.selection.tasks().iter().map(|task| {
                        let detail = render.runtime.and_then(|runtime| runtime.task_detail(task));
                        html! {
                            <article
                                key={task.to_string()}
                                class="work-overview-task"
                                data-task-stamp={task.to_string()}
                                tabindex="-1"
                                aria-labelledby={format!("work-overview-task-title-{task}")}
                                onclick={task_detail::task_text_chip_click(&focus_task)}
                                onkeydown={task_detail::task_text_chip_keydown(&focus_task)}
                            >
                                <div class="work-overview-task-heading">
                                    <p class="app-sheet-kicker">{"Selected task"}</p>
                                    <h3 id={format!("work-overview-task-title-{task}")}>
                                        {detail_title(detail, task, render.selection)}
                                    </h3>
                                </div>
                                {render_detail_body(&render, task, detail, &focus_task)}
                            </article>
                        }
                    })}
                }
            </section>
            if let Some(lanes) = render.lanes {
                {super::work_jobs::render(&lanes.lanes, render.selection.tasks())}
            }
        </div>
    };
    let overlay = blockers.render_review(
        render.runtime,
        render.blocker_review,
        render.blocker_review_close_ref,
        render.navigate,
        render
            .runtime
            .is_some_and(|runtime| runtime.offers(wire::CommandKind::TrustGrant)),
        &render.grant_trust,
    );
    let dismiss_overlay = (**render.blocker_review).then(|| {
        let blocker_review = render.blocker_review.clone();
        Callback::from(move |()| blocker_review.set(false))
    });
    sidebar::Content {
        label: "Task overview".into(),
        title: overview_title(render.selection, render.runtime).into(),
        width: sidebar::Width::Large,
        base_accent: base_accent(&render),
        body,
        overlay,
        overlay_open: **render.blocker_review,
        dismiss_overlay,
        anchor: None,
    }
}

/// The palette index of the base the overview shows, as the team console,
/// working-group console, or Design global rail wears it.
fn base_accent(render: &WorkDetailRender<'_, '_>) -> Option<usize> {
    match render.work.overview_base(render.selection) {
        TaskOverviewBase::Page => None,
        TaskOverviewBase::Design => Some(crate::watchband::DESIGN_BASE),
        TaskOverviewBase::Team(team) => render
            .lanes?
            .lanes
            .iter()
            .find(|row| row.lane == team)
            .map(|row| row.accent(0)),
        TaskOverviewBase::Group(group) => super::super::rooms::group_base(
            &render.runtime?.room_projection().partition().groups,
            group,
        ),
    }
}

fn overview_title(selection: &TaskOverviewSelection, runtime: Option<&AppRuntime>) -> String {
    let count = selection.tasks().len();
    format!(
        "{} · {count} {}",
        context_label(selection, runtime),
        if count == 1 { "task" } else { "tasks" },
    )
}

/// The selection's heading, naming a team from its lanes and a working group
/// from its lead Design.
pub(super) fn context_label(
    selection: &TaskOverviewSelection,
    runtime: Option<&AppRuntime>,
) -> String {
    let lanes = runtime
        .and_then(AppRuntime::lanes)
        .map_or(&[][..], |view| &view.lanes);
    selection.context().label(
        |team| crate::watchband::team_label_in(lanes, team),
        |group, designs| {
            runtime.map_or_else(
                || group.to_string(),
                |runtime| super::super::rooms::lead_label(runtime, group, designs.first()),
            )
        },
    )
}

pub(super) fn focus_task_callback() -> Callback<TaskStamp> {
    Callback::from(move |task: TaskStamp| {
        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            return;
        };
        let selector = format!(".work-overview-task[data-task-stamp=\"{task}\"]");
        let Some(element) = document.query_selector(&selector).ok().flatten() else {
            return;
        };
        element.scroll_into_view();
        if let Ok(element) = element.dyn_into::<web_sys::HtmlElement>() {
            let _ = element.focus();
        }
    })
}

fn detail_title(
    detail: Option<&TaskDetailState>,
    selected: &TaskStamp,
    selection: &TaskOverviewSelection,
) -> Html {
    let Some(answer) = detail.and_then(TaskDetailState::answer) else {
        return html! { {format!("Task {selected}")} };
    };
    let resolves = |task: &wire::TaskHandle| {
        task.born()
            .is_ok_and(|stamp| selection_has(selection, &stamp))
    };
    task_detail::render_task_title(
        &answer.task.presentation_title,
        &LineControls::Live {
            resolves: &resolves,
        },
    )
}

fn render_detail_body(
    render: &WorkDetailRender<'_, '_>,
    selected: &TaskStamp,
    detail: Option<&TaskDetailState>,
    focus_task: &Callback<TaskStamp>,
) -> Html {
    match detail {
        None | Some(TaskDetailState::Loading) => html! {
            <div class="work-detail-placeholder" aria-busy="true"></div>
        },
        Some(TaskDetailState::Unavailable { reason, detail }) => {
            let retry = {
                let request = render.request_task.clone();
                let task = selected.clone();
                Callback::from(move |_| request.emit(task.clone()))
            };
            let problem = detail.as_ref().map_or_else(
                || format!("Task detail unavailable: {reason}"),
                |detail| format!("Task detail unavailable: {reason}. {detail}"),
            );
            html! {
                <div class="work-detail-placeholder">
                    <crate::browser::toast::Notice text={Some(AttrValue::from(problem))} />
                    <button type="button" class="app-compact-control" onclick={retry}>{"Retry detail"}</button>
                </div>
            }
        }
        Some(TaskDetailState::Available(answer) | TaskDetailState::Refreshing(answer)) => {
            render_available(render, answer, focus_task)
        }
    }
}

fn render_available(
    render: &WorkDetailRender<'_, '_>,
    answer: &wire::intent::TaskDetailAnswer,
    focus_task: &Callback<TaskStamp>,
) -> Html {
    let row = &answer.task;
    let statement = &row.statement.body;
    let workflow = &row.workflow.body;
    let projected = render
        .work
        .tasks()
        .iter()
        .find(|task| task.row().statement.body.born == statement.born);
    let messages = render.creator_messages.iter().map(AsRef::as_ref);
    let creator_context = crate::view::creator_context_href(&statement.creation, messages);
    let design_room = render
        .runtime
        .filter(|_| row.design_standing().is_some())
        .map_or_else(Html::default, |runtime| {
            super::super::rooms::render_design_room(
                runtime,
                render.room_controls,
                render.sender,
                row,
            )
        });
    let completion = workflow.status.completion();
    html! {
        <div class="app-sheet-body">
            <div class="app-sheet-identity">
                <code>{statement.handle.to_string()}</code>
                <chrome::CopyButton
                    class="app-compact-control"
                    text={AttrValue::from(statement.handle.to_string())}
                    subject="task handle"
                >{"Copy identity"}</chrome::CopyButton>
                if let Some(href) = creator_context {
                    {task_detail::creator_context_link(
                        "work-context-link",
                        href,
                        statement,
                        None,
                    )}
                }
            </div>
            <section class="app-sheet-section">
                <h3>{"Contract"}</h3>
                {task_detail::render_contract(statement, |task| selection_has(render.selection, task), true)}
            </section>
            {super::super::drafts::render_editor(
                render.runtime,
                render.draft_controls,
                render.sender,
                projected.map_or(row, |task| task.row()),
            )}
            {task_detail::render_media(&answer.media)}
            <section class="app-sheet-section">
                <h3>{"State"}</h3>
                <dl class="app-fact-grid">
                    {project_fact(render, &statement.project)}
                    {fact("Status", workflow.status.as_str().to_owned())}
                    if let Some(stranded) = &row.stranded {
                        {fact("Stranded by", stranded.controller.to_string())}
                        {fact("Settle", stranded.settle.clone())}
                    }
                    {fact("Current phase", workflow.phase.to_string())}
                    {fact("Flow", workflow.flow.iter().map(ToString::to_string).collect::<Vec<_>>().join(" → "))}
                    {fact("Review", workflow.review_str().to_owned())}
                    {assignment_fact(workflow.status.reviewer(), render)}
                    {statement.priority.map_or_else(Html::default, |priority| fact("Priority", priority.to_string()))}
                    {statement.taste.map_or_else(Html::default, |_| fact("Taste review", "Required".to_owned()))}
                    {deadline_fact("Deferral", workflow.deferral.as_ref(), "None")}
                    {instant_fact("Completion", completion.as_ref(), "Open")}
                </dl>
                {render_possession(row, render)}
            </section>
            {design_room}
            <section class="app-sheet-section">
                <h3>{"Relationships"}</h3>
                {render_relationships(render.work, projected, statement, render.selection, focus_task)}
            </section>
            <section class="app-sheet-section">
                <h3>{"Provenance"}</h3>
                <dl class="app-fact-grid">
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
            {render_activity(render, answer, projected.map_or(row, |task| task.row()))}
        </div>
    }
}

/// The task's observations, newest first, beneath the composer that appends
/// to them, so a submitted observation lands directly under its draft. An
/// open OOPS report's File sits in that composer's action row, so reading,
/// observing, and filing it happen in one place.
fn render_activity(
    render: &WorkDetailRender<'_, '_>,
    answer: &wire::intent::TaskDetailAnswer,
    row: &wire::intent::BoardTask,
) -> Html {
    let statement = &answer.task.statement.body;
    html! {
        <section class="app-sheet-section work-activity">
            <div class="app-section-heading">
                <h3>{"Activity"}</h3>
                <span>{format!("{} observations", answer.notes.len())}</span>
            </div>
            {task_detail::render_closure_record(
                answer.closure.as_ref(),
                |actor| actor_name(actor, render),
                |task| selection_has(render.selection, task),
            )}
            {task_detail::render_note_composer(
                format!("work-detail-note-{}", statement.born),
                statement.handle.as_str(),
                &statement.born,
                render.can_note,
                task_detail::closure(render.can_close, &answer.task).as_ref(),
                &render.notes.shown(&statement.born),
                &render.submit_note,
                super::super::drafts::render_oops_file_action(
                    render.runtime,
                    render.draft_controls,
                    render.sender,
                    row,
                ),
            )}
            {super::super::drafts::render_oops_file_notice(render.draft_controls, row)}
            if answer.notes.is_empty() {
                <p class="muted">{"No observations have been recorded."}</p>
            } else {
                <ol class="work-note-list">
                    {for answer.notes.iter().rev().map(|note| html! {
                        <li class="work-note-card" key={note.position.to_string()} data-note-position={note.position.to_string()}>
                            <header>
                                <strong title={note.author.to_string()}>
                                    {actor_name(&note.author, render)}
                                </strong>
                                {task_time(&note.at, TaskTimePresentation::DetailedElapsed)}
                            </header>
                            {task_detail::render_task_text(&note.text, "work-note-body", |task| selection_has(render.selection, task))}
                        </li>
                    })}
                </ol>
            }
        </section>
    }
}

fn fact(label: &'static str, value: String) -> Html {
    html! { <div><dt>{label}</dt><dd>{value}</dd></div> }
}

fn project_fact(render: &WorkDetailRender<'_, '_>, project: &str) -> Html {
    let navigate = render.navigate.clone();
    let target = project.to_owned();
    let project = project.to_owned();
    html! {
        <div>
            <dt>{"Project"}</dt>
            <dd title={project.clone()}>
                <button
                    type="button"
                    class="app-fact-link"
                    data-task-project-link={project.clone()}
                    aria-label={format!("Show {project} in task overview")}
                    onclick={Callback::from(move |_| {
                        navigate.emit(navigation::Intent::Project(target.clone()));
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
    )
}

fn linked_identity_fact(
    label: &'static str,
    display: String,
    identity: String,
    suffix: String,
    destination: Option<(wire::CanonicalUuid, wire::CanonicalUuid)>,
) -> Html {
    let Some((_team, member)) = destination else {
        return html! {
            <div><dt>{label}</dt><dd title={identity}>{display}{suffix}</dd></div>
        };
    };
    let href = crate::view::card_href(&super::super::watchband::primary_textarea_id(
        &member.to_string(),
    ));
    html! {
        <div>
            <dt>{label}</dt>
            <dd title={identity}>
                <a
                    class="app-fact-link"
                    data-task-chat-link={member.to_string()}
                    {href}
                    aria-label={format!("Open {display} in Chat")}
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
        wire::Actor::Operator | wire::Actor::Spice | wire::Actor::Human(_) => None,
    };
    linked_identity_fact(
        label,
        actor_name(actor, render),
        actor.to_string(),
        suffix,
        destination,
    )
}

fn actor_name(actor: &wire::Actor, render: &WorkDetailRender<'_, '_>) -> String {
    let member = match actor {
        wire::Actor::Lane(lane) => render.lanes.and_then(|lanes| lanes.member(*lane)),
        wire::Actor::Operator | wire::Actor::Spice | wire::Actor::Human(_) => None,
    };
    crate::work::owner_label(
        actor,
        member,
        render.runtime.and_then(AppRuntime::principals),
    )
}

fn render_possession(row: &wire::intent::BoardTask, render: &WorkDetailRender<'_, '_>) -> Html {
    let claim = row.claim.as_ref();
    let lease = row.lease.as_ref();
    html! {
        <div class="app-sheet-subsection">
            <h4>{"Claim and lease"}</h4>
            <dl class="app-fact-grid">
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

fn render_relationships(
    work: &WorkProjection<'_>,
    task: Option<&WorkTask<'_>>,
    statement: &wire::intent::BoardStatement,
    selection: &TaskOverviewSelection,
    focus: &Callback<TaskStamp>,
) -> Html {
    let dependencies = statement
        .dependencies
        .iter()
        .map(|born| render_task_link(work, born, "Dependency", selection, focus));
    let dependents = task
        .into_iter()
        .flat_map(|task| task.dependents().iter())
        .map(|dependent| {
            let row = work.tasks()[dependent.task()].row();
            render_task_link(
                work,
                &row.statement.body.born,
                "Dependent",
                selection,
                focus,
            )
        });
    if statement.dependencies.is_empty() && task.is_none_or(|task| task.dependents().is_empty()) {
        return html! { <p class="muted">{"No dependencies or dependents."}</p> };
    }
    html! { <div class="work-relationship-links">{for dependencies.chain(dependents)}</div> }
}

fn selection_has(selection: &TaskOverviewSelection, born: &TaskStamp) -> bool {
    selection.tasks().contains(born)
}

fn render_task_link(
    work: &WorkProjection<'_>,
    born: &TaskStamp,
    relationship: &'static str,
    selection: &TaskOverviewSelection,
    focus: &Callback<TaskStamp>,
) -> Html {
    let row = work
        .tasks()
        .iter()
        .find(|task| task.row().statement.body.born == *born)
        .map(WorkTask::row);
    let selected = selection_has(selection, born);
    // A selected relation is itself a button, so its title holds no control.
    let resolves = |task: &wire::TaskHandle| {
        task.born()
            .is_ok_and(|stamp| selection_has(selection, &stamp))
    };
    let controls = if selected {
        LineControls::Nested
    } else {
        LineControls::Live {
            resolves: &resolves,
        }
    };
    let contents = html! {
        <>
            <span>{if selected { relationship } else { "Outside selection" }}</span>
            <strong>{row.map_or_else(|| born.to_string(), |row| row.statement.body.handle.to_string())}</strong>
            if let Some(row) = row {
                <small>{task_detail::render_task_title(&row.presentation_title, &controls)}</small>
            }
        </>
    };
    if !selected {
        return html! {
            <article class="work-relationship-link work-relationship-link--external">{contents}</article>
        };
    }
    let target = born.clone();
    let activate = {
        let focus = focus.clone();
        Callback::from(move |_| focus.emit(target.clone()))
    };
    html! {
        <button type="button" class="work-relationship-link" onclick={activate}>{contents}</button>
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
        <div class="app-sheet-subsection">
            <h4>{"Claim provenance"}</h4>
            <dl class="app-fact-grid">
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
        <div class="app-sheet-subsection">
            <h4>{"Repository boundary"}</h4>
            <dl class="app-fact-grid">
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
