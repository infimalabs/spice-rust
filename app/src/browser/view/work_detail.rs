//! Graph-first contextual overview for one complete selected task set.

use super::super::clock::{TaskTimePresentation, task_time, task_time_millis};
use super::super::watchband::lane_composer_href;
use super::*;
use crate::composer_markdown::inline::LineControls;
use crate::runtime::LaneView;
use crate::work::{
    TaskOverviewBase, TaskOverviewSelection, WorkDependency, WorkProjection, WorkTask,
};
use wire::TaskStamp;

pub(super) struct WorkDetailRender<'a, 'work> {
    pub work: &'a WorkProjection<'work>,
    pub lanes: Option<&'a LaneView>,
    pub jobs: Html,
    pub navigate: &'a Callback<navigation::Intent>,
    pub creator_messages: &'a [Rc<wire::AgentMessage>],
    pub selection: &'a TaskOverviewSelection,
    pub map: &'a UseStateHandle<work_map::WorkMapState>,
    pub runtime: Option<&'a AppRuntime>,
    pub room_controls: &'a super::super::rooms::Controls,
    pub sender: &'a SenderHandle,
    pub draft_controls: &'a super::super::drafts::Controls,
    pub connected: bool,
    pub can_note: bool,
    pub can_close: bool,
    pub can_conclude: bool,
    pub blocker_review: &'a UseStateHandle<bool>,
    pub blocker_review_button_ref: &'a NodeRef,
    pub blocker_review_close_ref: &'a NodeRef,
    pub notes: &'a NoteComposers,
    pub submit_note: Callback<(TaskStamp, SubmitEvent)>,
    pub grant_trust: Callback<task_detail::TrustGrantRequest>,
    pub request_task: Callback<TaskStamp>,
    pub open_overview: Callback<TaskOverviewSelection>,
    /// Opens a task's sheet from a card, returning focus to the activating element.
    pub open_message_task: Callback<(TaskStamp, web_sys::Element)>,
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
                                {render_header(&render, task, detail)}
                                {render_detail_body(&render, task, detail, &focus_task)}
                            </article>
                        }
                    })}
                }
            </section>
            {render.jobs.clone()}
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
        heading: sidebar::Heading::Title(overview_title(render.selection, render.runtime).into()),
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

/// The task's summary strip, read before any section beneath it: what the task
/// is, where it stands, who holds and reviews it, and how old it is. Before
/// detail arrives it reads the board row the overview already projects.
fn render_header(
    render: &WorkDetailRender<'_, '_>,
    stamp: &TaskStamp,
    detail: Option<&TaskDetailState>,
) -> Html {
    let title_id = format!("work-overview-task-title-{stamp}");
    let row = detail
        .and_then(TaskDetailState::answer)
        .map(|answer| &answer.task)
        .or_else(|| {
            render
                .work
                .tasks()
                .iter()
                .map(WorkTask::row)
                .find(|row| row.statement.body.born == *stamp)
        });
    let Some(row) = row else {
        return html! {
            <header class="work-task-header">
                <h3 id={title_id}>{format!("Task {stamp}")}</h3>
            </header>
        };
    };
    let statement = &row.statement.body;
    let workflow = &row.workflow.body;
    let messages = render.creator_messages.iter().map(AsRef::as_ref);
    let creator_context = crate::view::creator_context_href(&statement.creation, messages);
    let resolves = |task: &wire::TaskHandle| {
        task.born()
            .is_ok_and(|stamp| selection_has(render.selection, &stamp))
    };
    let phase = &workflow.phase;
    let flow = workflow
        .flow
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" → ");
    // The flow reads once, its current phase marked; a phase outside the flow
    // leads it so neither is lost.
    let steps = workflow.flow.iter().enumerate().map(|(index, step)| {
        let arrow = if index == 0 { "" } else { " → " };
        if step == phase {
            html! { <>{arrow}<strong>{step.to_string()}</strong></> }
        } else {
            html! { <>{arrow}{step.to_string()}</> }
        }
    });
    html! {
        <header class="work-task-header">
            <div class="work-task-header-title">
                <code class="work-task-handle">{statement.handle.to_string()}</code>
                <h3 id={title_id}>
                    {task_detail::render_task_title(
                        &row.presentation_title,
                        &LineControls::Live { resolves: &resolves },
                    )}
                </h3>
                {standing_pill(row)}
            </div>
            <p class="work-task-pitch" data-task-header="pitch">{statement.pitch.clone()}</p>
            <p class="work-task-header-facts">
                <span data-task-header="phase" title={format!("Phase {phase} of flow {flow}")}>
                    if !workflow.flow.contains(phase) {
                        <strong>{phase.to_string()}</strong>{" · "}
                    }
                    {for steps}
                </span>
                <span data-task-header="claim">{claimant(row, render)}</span>
                <span data-task-header="review">{review_assignment(workflow, render)}</span>
                {task_time_millis(statement.born.micros().div_euclid(1_000), TaskTimePresentation::Elapsed)}
                if let Some(href) = creator_context {
                    {task_detail::creator_context_link("work-context-link", href, statement, None)}
                }
            </p>
        </header>
    }
}

/// The task's kind and where it stands: a Design's lifecycle standing, else its
/// workflow status, after any declared intent.
fn standing_pill(row: &wire::intent::BoardTask) -> Html {
    let status = row.workflow.body.status.as_str();
    let (standing, title) = row.design_standing().map_or_else(
        || (status, status.to_owned()),
        |standing| (standing.key(), standing.to_string()),
    );
    let text = row.statement.body.intent.map_or_else(
        || standing.to_owned(),
        |intent| format!("{intent} · {standing}"),
    );
    html! {
        <span class="work-task-pill" data-task-standing={standing} {title}>{text}</span>
    }
}

/// Who holds the task, linked to its lane, or how the last holder let it go.
fn claimant(row: &wire::intent::BoardTask, render: &WorkDetailRender<'_, '_>) -> Html {
    let Some(claim) = &row.claim else {
        return html! { "Unclaimed" };
    };
    let verb = match claim.body.disposition {
        wire::ClaimDisposition::Active => "Claimed by ",
        wire::ClaimDisposition::Freed => "Freed by ",
        wire::ClaimDisposition::Lapsed => "Lapsed from ",
    };
    html! { <>{verb}{actor_link(&claim.body.actor, render)}</> }
}

/// The task's reviewer when one is assigned, else where review stands.
fn review_assignment(workflow: &wire::WorkflowHead, render: &WorkDetailRender<'_, '_>) -> Html {
    workflow.status.reviewer().map_or_else(
        || html! { {format!("Review {}", workflow.review_str())} },
        |actor| html! { <>{"Review by "}{actor_link(actor, render)}</> },
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
    let projected = render
        .work
        .tasks()
        .iter()
        .find(|task| task.row().statement.body.born == statement.born);
    // A Design's history closes the room's strip of Lanes and Maturity;
    // any other task closes its sheet with it.
    let in_room = render.runtime.is_some() && row.design_standing().is_some();
    let facts = facts(render, answer);
    let [description, acceptance, history] =
        crate::task_sheet::sheet_blocks(statement, facts.len());
    let facts = html! { <dl class="app-fact-grid">{for facts}</dl> };
    let (room_history, closing) = if in_room {
        let body = if history.count == 0 {
            html! { <p class="muted">{history.empty}</p> }
        } else {
            facts
        };
        let block = super::super::rooms::RoomBlock {
            key: "history",
            label: history.label,
            summary: html! {
                <>
                    {history.label}
                    <span class="app-count-badge">{history.count}</span>
                </>
            },
            body,
        };
        (Some(block), Html::default())
    } else {
        (None, sheet_block(&history, "app-sheet-section", facts))
    };
    let resolves = |task: &TaskStamp| selection_has(render.selection, task);
    html! {
        <div class="app-sheet-body">
            {sheet_block(
                &description,
                "app-sheet-section",
                task_detail::render_description(statement, resolves),
            )}
            {sheet_block(
                &acceptance,
                "app-sheet-section",
                task_detail::render_acceptance(
                    statement,
                    resolves,
                    &item_standings(render.runtime, statement),
                ),
            )}
            {super::super::drafts::render_editor(
                render.runtime,
                render.draft_controls,
                render.sender,
                projected.map_or(row, |task| task.row()),
            )}
            {task_detail::render_media(&answer.media)}
            {render_room(render, answer, room_history)}
            {render_relationships(render.work, projected, statement, &RelationTargets {
                selection: render.selection,
                focus: focus_task,
                navigate: render.navigate,
            })}
            {closing}
        </div>
    }
}

/// One native disclosure, closed on every render so no open state persists:
/// its summary carries the block's label and count, and an empty block
/// reads its absence instead of `body`.
fn sheet_block(block: &crate::task_sheet::SheetBlock, class: &'static str, body: Html) -> Html {
    let key = block.label.to_ascii_lowercase();
    html! {
        <details class={classes!(class, "app-sheet-block")} data-sheet-block={key}>
            <summary>
                {block.label}
                <span class="app-count-badge">{block.count}</span>
            </summary>
            if block.count == 0 {
                <p class="muted">{block.empty}</p>
            } else {
                {body}
            }
        </details>
    }
}

/// Every remaining scalar fact about the task, one `<div>` each: state,
/// lease, origin, claim context, and repository boundary.
fn facts(render: &WorkDetailRender<'_, '_>, answer: &wire::intent::TaskDetailAnswer) -> Vec<Html> {
    let row = &answer.task;
    let statement = &row.statement.body;
    let workflow = &row.workflow.body;
    let completion = workflow.status.completion();
    let lease = row.lease.as_ref();
    let mut facts = vec![
        project_fact(render, &statement.project),
        fact("Status", workflow.status.as_str().to_owned()),
    ];
    facts.extend(
        statement
            .priority
            .map(|priority| fact("Priority", priority.to_string())),
    );
    facts.extend(
        statement
            .taste
            .as_ref()
            .map(|hold| fact("Taste review", taste_review_text(hold))),
    );
    facts.extend([
        super::super::deferral::fact(
            render.runtime,
            render.sender,
            row,
            deadline_value(workflow.deferral.as_ref()),
        ),
        instant_fact("Completion", completion.as_ref()),
        fact(
            "Claim fact",
            row.claim
                .as_ref()
                .map_or_else(|| ABSENT.to_owned(), |claim| sequence(claim.position)),
        ),
        instant_fact("Lease start", lease.map(|lease| &lease.body.start)),
        html_fact(
            "Lease expiry",
            deadline_value(lease.map(|lease| &lease.body.expiry)),
        ),
        origin_fact(
            render,
            &statement.provenance,
            answer.origin_request.as_ref(),
        ),
        fact("Origin thread", row.origin_thread.clone()),
        creator_team_fact(row.creator_team, render),
        fact("Queue fact", sequence(statement.queue_origin)),
        fact("Statement revision", sequence(statement.revision)),
        fact("Workflow fact", sequence(row.workflow.position)),
    ]);
    claim_context_facts(&mut facts, row, render);
    git_context_facts(&mut facts, row);
    facts
}

/// A Design's current question items with their standings, once the room
/// projection is current; nothing for another task or before the snapshot.
fn item_standings(
    runtime: Option<&AppRuntime>,
    statement: &wire::intent::BoardStatement,
) -> Vec<wire::rooms::DesignItemStanding> {
    let Some(projection) = runtime.map(AppRuntime::room_projection) else {
        return Vec::new();
    };
    if statement.intent != Some(wire::TaskIntent::Design)
        || projection.status() != crate::runtime::RoomProjectionStatus::Available
    {
        return Vec::new();
    }
    projection.maturity(&statement.born).map_or_else(
        || wire::rooms::DesignItems::default().standings(&statement.acceptance),
        |maturity| maturity.items.standings(&statement.acceptance),
    )
}

/// The task's room beneath the composer that publishes into it, so a
/// submitted observation lands directly above the conversation it joins.
fn render_room(
    render: &WorkDetailRender<'_, '_>,
    answer: &wire::intent::TaskDetailAnswer,
    history: Option<super::super::rooms::RoomBlock>,
) -> Html {
    let statement = &answer.task.statement.body;
    let note_id = format!("work-detail-note-{}", statement.born);
    let composer = html! {
        <>
            {task_detail::render_settlement_record(
                crate::work::SettlementRecord::of(answer),
                |actor| actor_name(actor, render),
                |task| selection_has(render.selection, task),
            )}
            {task_detail::render_disposition_record(
                answer.disposition.as_ref(),
                |actor| actor_name(actor, render),
                |task| {
                    render
                        .work
                        .tasks()
                        .iter()
                        .find(|row| row.row().statement.body.born == *task)
                        .map_or_else(
                            || task.to_string(),
                            |row| row.row().statement.body.handle.to_string(),
                        )
                },
                |task| selection_has(render.selection, task),
            )}
            {task_detail::render_note_composer(
                note_id.clone(),
                statement.handle.as_str(),
                &statement.born,
                task_detail::NoteAccess {
                    connected: render.connected,
                    can_note: render.can_note,
                    closure: task_detail::closure(
                        render.can_close,
                        render.can_conclude,
                        &answer.task,
                    )
                    .as_ref(),
                },
                &render.notes.shown(&statement.born),
                task_detail::NoteCallbacks {
                    input: render.notes.draft_input(&statement.born),
                    submit: &render.submit_note,
                    open: render.notes.opener(&statement.born, note_id),
                },
            )}
        </>
    };
    match render.runtime {
        Some(runtime) => super::super::rooms::render_task_room(
            runtime,
            render.room_controls,
            render.sender,
            &render.open_message_task,
            &answer.task,
            composer,
            history,
        ),
        None => html! { <section class="app-sheet-section room-reading">{composer}</section> },
    }
}

/// What a fact reads when its value is absent.
const ABSENT: &str = "—";

fn fact(label: &'static str, value: String) -> Html {
    html! { <div><dt>{label}</dt><dd>{value}</dd></div> }
}

fn html_fact(label: &'static str, value: Html) -> Html {
    html! { <div><dt>{label}</dt><dd>{value}</dd></div> }
}

/// Where each taste held the statement, with the matched text the operator
/// reads beside it.
fn taste_review_text(hold: &wire::TasteHold) -> String {
    hold.matches
        .iter()
        .map(|found| {
            let place = format!(
                "{} line {}: {}",
                found.field.as_str(),
                found.line,
                found.taste
            );
            match &found.finding {
                Some(finding) => format!("{place} ({:?})", finding.trigger),
                None => place,
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// The task's origin key, followed by the request a steering origin names. A
/// parent task's origin opens that task's sheet; no other origin has one.
fn origin_fact(
    render: &WorkDetailRender<'_, '_>,
    origin: &wire::TaskProvenance,
    request: Option<&wire::intent::OriginRequest>,
) -> Html {
    let key = match origin {
        wire::TaskProvenance::Task(parent) => {
            let navigate = render.navigate.clone();
            let parent = parent.clone();
            html! {
                <button
                    type="button"
                    class="app-fact-link"
                    aria-label={format!("Open origin task {parent}")}
                    onclick={Callback::from(move |_| {
                        navigate.emit(navigation::Intent::Task(parent.clone()));
                    })}
                >{origin.to_string()}</button>
            }
        }
        _ if request.is_some() => html! { <code>{origin.to_string()}</code> },
        _ => html! { {origin.to_string()} },
    };
    html_fact(
        "Origin",
        html! {
            <>
                {key}
                if let Some(request) = request {
                    <p class="app-origin-request">
                        {request.text.clone()}
                        if request.truncated { {"…"} }
                    </p>
                }
            </>
        },
    )
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
        return fact("Creator team", ABSENT.to_owned());
    };
    let lanes = render.lanes.map_or(&[][..], |view| &view.lanes);
    let href = render
        .lanes
        .and_then(|view| view.chat_target_for_team(team))
        .and_then(|(_team, member)| lane_composer_href(render.lanes, member));
    html_fact(
        "Creator team",
        identity_link(
            crate::watchband::team_label_in(lanes, team),
            team.to_string(),
            href,
        ),
    )
}

/// A name the reader knows, its durable spelling on hover, linked to the Chat
/// composer that reaches it when one exists.
fn identity_link(display: String, identity: String, href: Option<String>) -> Html {
    let Some(href) = href else {
        return html! { <span title={identity}>{display}</span> };
    };
    html! {
        <a
            class="app-fact-link"
            title={identity}
            {href}
            aria-label={format!("Open {display} in Chat")}
        >{display}</a>
    }
}

fn instant_fact(label: &'static str, instant: Option<&wire::Instant>) -> Html {
    html_fact(
        label,
        instant.map_or_else(
            || html! { ABSENT },
            |instant| task_time(instant, TaskTimePresentation::Elapsed),
        ),
    )
}

fn deadline_value(instant: Option<&wire::Instant>) -> Html {
    instant.map_or_else(
        || html! { ABSENT },
        |instant| task_time(instant, TaskTimePresentation::Remaining),
    )
}

/// One actor by the name the reader knows, linked to its lane's Chat composer.
fn actor_link(actor: &wire::Actor, render: &WorkDetailRender<'_, '_>) -> Html {
    let href = match actor {
        wire::Actor::Lane(lane) => lane_composer_href(render.lanes, *lane),
        wire::Actor::Operator | wire::Actor::Spice | wire::Actor::Human(_) => None,
    };
    identity_link(actor_name(actor, render), actor.to_string(), href)
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

/// What activating a relation does: focus a task already in the selection, or
/// replace the selection with one outside it.
struct RelationTargets<'a> {
    selection: &'a TaskOverviewSelection,
    focus: &'a Callback<TaskStamp>,
    navigate: &'a Callback<navigation::Intent>,
}

fn render_relationships(
    work: &WorkProjection<'_>,
    task: Option<&WorkTask<'_>>,
    statement: &wire::intent::BoardStatement,
    targets: &RelationTargets<'_>,
) -> Html {
    // The projected edges add the construction roots a Design released,
    // which it waits on; a row outside the projection keeps its statement.
    let dependencies = task.map_or_else(
        || statement.dependencies.iter().collect::<Vec<_>>(),
        |task| {
            task.dependencies()
                .iter()
                .map(WorkDependency::born)
                .collect()
        },
    );
    let dependents = task
        .into_iter()
        .flat_map(|task| task.dependents().iter())
        .map(|dependent| {
            let row = work.tasks()[dependent.task()].row();
            render_task_link(work, &row.statement.body.born, "Dependent", targets)
        });
    if dependencies.is_empty() && task.is_none_or(|task| task.dependents().is_empty()) {
        return empty_section("No dependencies or dependents.");
    }
    let dependencies = dependencies
        .into_iter()
        .map(|born| render_task_link(work, born, "Dependency", targets));
    html! {
        <section class="app-sheet-section">
            <h3>{"Relationships"}</h3>
            <div class="work-relationship-links">{for dependencies.chain(dependents)}</div>
        </section>
    }
}

/// A section with nothing to show, collapsed to one muted line without the
/// heading and spacing of a populated section.
fn empty_section(text: &'static str) -> Html {
    html! { <section class="app-sheet-section app-sheet-empty"><p>{text}</p></section> }
}

fn selection_has(selection: &TaskOverviewSelection, born: &TaskStamp) -> bool {
    selection.tasks().contains(born)
}

/// One related task as a button: a selected relation focuses its sheet, and
/// one outside the selection replaces the selection with it.
fn render_task_link(
    work: &WorkProjection<'_>,
    born: &TaskStamp,
    relationship: &'static str,
    targets: &RelationTargets<'_>,
) -> Html {
    let row = work
        .tasks()
        .iter()
        .find(|task| task.row().statement.body.born == *born)
        .map(WorkTask::row);
    let selected = selection_has(targets.selection, born);
    let target = born.clone();
    let activate = if selected {
        let focus = targets.focus.clone();
        Callback::from(move |_| focus.emit(target.clone()))
    } else {
        let navigate = targets.navigate.clone();
        Callback::from(move |_| navigate.emit(navigation::Intent::Task(target.clone())))
    };
    html! {
        <button type="button" class="app-fact-link work-relationship-link" onclick={activate}>
            <span>
                {relationship}
                if !selected {
                    {" · outside selection"}
                }
            </span>
            <strong>{row.map_or_else(|| born.to_string(), |row| row.statement.body.handle.to_string())}</strong>
            // The relation is itself a button, so its title holds no control.
            if let Some(row) = row {
                <small>{task_detail::render_task_title(&row.presentation_title, &LineControls::Nested)}</small>
            }
        </button>
    }
}

/// Where the claim was won: its fact, its lane by name, that lane's occupant
/// and resident generation relative to the current ones, and its transcript.
fn claim_context_facts(
    facts: &mut Vec<Html>,
    row: &wire::intent::BoardTask,
    render: &WorkDetailRender<'_, '_>,
) {
    let Some(context) = &row.claim_context else {
        return;
    };
    let recovery = match &context.recovery {
        wire::intent::TaskTranscriptRecovery::Transcript { session } => {
            format!("Transcript session {session}")
        }
        wire::intent::TaskTranscriptRecovery::Unavailable { reason } => {
            format!("Transcript unavailable: {reason}")
        }
    };
    let lane = wire::Actor::Lane(context.lane);
    let seated = render
        .lanes
        .and_then(|lanes| lanes.member(context.lane))
        .map(|member| member.occupant == Some(context.occupant));
    let occupant = match seated {
        Some(true) => format!("{}, seated now", actor_name(&lane, render)),
        Some(false) => format!("{}, an earlier occupant", actor_name(&lane, render)),
        None => format!("{} occupant", actor_name(&lane, render)),
    };
    let epoch = match render.lanes.map(|lanes| lanes.epoch == context.epoch) {
        Some(true) => "Current resident".to_owned(),
        Some(false) => "An earlier resident".to_owned(),
        None => format!("Resident {}", crate::watchband::short(context.epoch)),
    };
    facts.extend([
        fact("Claimed fact", sequence(context.claimed)),
        html_fact("Claim lane", actor_link(&lane, render)),
        html_fact(
            "Occupant",
            html! { <span title={context.occupant.to_string()}>{occupant}</span> },
        ),
        html_fact(
            "Resident epoch",
            html! { <span title={context.epoch.to_string()}>{epoch}</span> },
        ),
        fact("Recovery", recovery),
    ]);
}

fn git_context_facts(facts: &mut Vec<Html>, row: &wire::intent::BoardTask) {
    let Some(context) = &row.git_context else {
        return;
    };
    facts.extend([
        fact("Repository lifecycle", context.kind.as_str().to_owned()),
        fact("Repository fact", sequence(context.fact)),
        fact("Head", context.boundary.head.clone()),
        fact("Integrated", context.boundary.integrated.clone()),
        fact(
            "Upstream",
            context
                .boundary
                .upstream
                .clone()
                .unwrap_or_else(|| ABSENT.to_owned()),
        ),
        fact("Local commits", context.boundary.local_commits.to_string()),
    ]);
}

/// A journal position in the condensed base52 spelling every surface reads.
fn sequence(position: wire::Seq) -> String {
    wire::SequenceCode::new(position.get()).to_string()
}
