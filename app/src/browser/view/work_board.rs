//! One deterministic card board for the global Work surface.
//!
//! Every in-scope project shares the same four lifecycle lanes. Cards default
//! to the resident allocator's offer order, so the task offered next leads;
//! newest-first creation order and attribute clusters behind thin labelled
//! separators are secondary arrangements of the same cards. The board header
//! stays pinned under the command bar while the page scrolls.

use std::cmp::Ordering;

use super::*;
use crate::runtime::{LaneView, WorkHistoryState};
use crate::watchband;
use crate::work::{
    self as model, WorkArrangement, WorkAvailability, WorkCluster, WorkClusterKey, WorkClustering,
    WorkOrder, WorkProjection, WorkScope, WorkTask,
};
use wire::ClaimDisposition;

use super::super::clock::Countdown;
use super::super::work_history::Controls as WorkHistoryControls;

const OPEN_LANES: [WorkAvailability; 4] = [
    WorkAvailability::InFlight,
    WorkAvailability::Ready,
    WorkAvailability::Blocked,
    WorkAvailability::Deferred,
];

const ORDERS: [(WorkOrder, &str, &str); 2] = [
    (
        WorkOrder::Offer,
        "Next up",
        "The order the allocator offers work: the next task leads",
    ),
    (WorkOrder::Newest, "Newest", "Newest created first"),
];

const CLUSTERINGS: [(WorkClustering, &str); 3] = [
    (WorkClustering::None, "None"),
    (WorkClustering::Project, "Project"),
    (WorkClustering::Phase, "Phase"),
];

pub(super) struct WorkBoardControls<'a> {
    pub map: &'a UseStateHandle<WorkMapState>,
    pub select: Callback<wire::TaskStamp>,
    pub toggle_map_root: Callback<wire::TaskStamp>,
    pub arrangement: &'a UseStateHandle<WorkArrangement>,
    pub lanes: Option<&'a LaneView>,
    pub history_state: Option<&'a WorkHistoryState>,
    pub history: &'a WorkHistoryControls,
}

/// What every card on the board reads besides its own task.
struct Cards<'a> {
    selected: Option<&'a wire::TaskStamp>,
    map: &'a UseStateHandle<WorkMapState>,
    select: &'a Callback<wire::TaskStamp>,
    toggle_map_root: &'a Callback<wire::TaskStamp>,
    lanes: Option<&'a LaneView>,
}

pub(super) fn render(
    work: &WorkProjection<'_>,
    scope: &WorkScope,
    stale: bool,
    selected: Option<&wire::TaskStamp>,
    controls: WorkBoardControls<'_>,
) -> Html {
    let WorkBoardControls {
        map,
        select,
        toggle_map_root,
        arrangement: arrangement_state,
        lanes,
        history_state,
        history: history_controls,
    } = controls;
    let cards = Cards {
        selected,
        map,
        select: &select,
        toggle_map_root: &toggle_map_root,
        lanes,
    };
    let arrangement = **arrangement_state;
    let board_lanes =
        OPEN_LANES.map(|availability| (availability, work.lane(availability, scope, arrangement)));
    let open = board_lanes
        .iter()
        .flat_map(|(_, clusters)| clusters)
        .map(|cluster| cluster.tasks.len())
        .sum::<usize>();
    let history = history_tasks(work);
    html! {
        <div
            class="work-board"
            aria-label="Work board"
            data-work-order={order_key(arrangement.order)}
            data-work-clustering={clustering_key(arrangement.clustering)}
        >
            <header class="work-board-header">
                <h2>{"Open tasks"}</h2>
                <span class="section-count">{format!("{open} tasks")}</span>
                {render_arrangement(arrangement_state)}
            </header>
            if stale {
                <p class="work-board-state" role="status">
                    {"Showing the last admitted board snapshot while reconnecting."}
                </p>
            }
            if open == 0 {
                <div class="empty">{"No open tasks in this scope."}</div>
            } else {
                <div class="work-lanes">
                    {for board_lanes.into_iter().map(|(availability, clusters)| {
                        render_lane(availability, &clusters, &cards)
                    })}
                </div>
            }
            {render_history_archive(history, history_state, history_controls, &cards)}
        </div>
    }
}

fn render_history_archive(
    history: Vec<&WorkTask<'_>>,
    state: Option<&WorkHistoryState>,
    controls: &WorkHistoryControls,
    cards: &Cards<'_>,
) -> Html {
    html! {
        <details
            class="work-history"
            open={*controls.open}
            ontoggle={controls.toggle.clone()}
        >
            <summary>
                <span>{"History"}</span>
                <span class="work-history-count">{history_count(state)}</span>
            </summary>
            if *controls.open {
                <div class="work-history-finder" role="search" aria-label="Find terminal tasks">
                    <label>
                        <span>{"Search"}</span>
                        <input
                            type="search"
                            maxlength={wire::intent::TASK_HISTORY_QUERY_BYTES_MAX.to_string()}
                            placeholder="Handle, title, project, or description"
                            value={(*controls.query).clone()}
                            oninput={state_from_input(controls.query.clone())}
                        />
                    </label>
                    <label>
                        <span>{"Completed by"}</span>
                        <input
                            type="date"
                            value={(*controls.cutoff).clone()}
                            oninput={state_from_input(controls.cutoff.clone())}
                        />
                    </label>
                    <div class="work-history-filter-actions">
                        <button type="button" onclick={controls.apply.clone()}>{"Apply"}</button>
                        <button type="button" onclick={controls.clear.clone()}>{"Clear"}</button>
                    </div>
                </div>
                if let Some(issue) = controls.validation.as_ref() {
                    <p class="form-refusal work-history-filter-issue" role="alert">{issue}</p>
                }
                if state.is_some_and(WorkHistoryState::loading) {
                    <p class="work-board-state" role="status">{"Loading history page…"}</p>
                }
                if let Some(issue) = state.and_then(WorkHistoryState::issue) {
                    <div class="work-history-actions">
                        <p class="form-refusal" role="alert">{issue}</p>
                        <button type="button" onclick={controls.retry.clone()}>{"Retry"}</button>
                    </div>
                }
                if history.is_empty() && state.and_then(WorkHistoryState::total) == Some(0) {
                    <div class="empty">
                        {if state.is_some_and(WorkHistoryState::filtered) {
                            "No history matches these filters."
                        } else {
                            "No completed or deleted tasks."
                        }}
                    </div>
                } else if !history.is_empty() {
                    {render_history(history, cards)}
                }
                {render_history_pagination(state, controls)}
            }
        </details>
    }
}

fn render_history_pagination(
    state: Option<&WorkHistoryState>,
    controls: &WorkHistoryControls,
) -> Html {
    let Some(state) = state.filter(|state| state.total().is_some()) else {
        return Html::default();
    };
    html! {
        <div class="work-history-pagination">
            <div class="work-history-range" role="status">
                <strong>{history_count(Some(state))}</strong>
                <span>{history_page_position(state)}</span>
            </div>
            <div class="work-history-page-actions" aria-label="History pages">
                <button
                    type="button"
                    disabled={state.loading() || !state.has_newer()}
                    onclick={controls.newer.clone()}
                >
                    {"Newer"}
                </button>
                <button
                    type="button"
                    disabled={state.loading() || !state.has_older()}
                    onclick={controls.older.clone()}
                >
                    {"Older"}
                </button>
            </div>
        </div>
    }
}

fn history_count(state: Option<&WorkHistoryState>) -> String {
    state.and_then(WorkHistoryState::total).map_or_else(
        || "Not loaded".to_owned(),
        |total| {
            format!(
                "{}–{} of {total}",
                state.map_or(0, WorkHistoryState::first_result),
                state.map_or(0, WorkHistoryState::last_result),
            )
        },
    )
}

fn history_page_position(state: &WorkHistoryState) -> &'static str {
    match (state.has_newer(), state.has_older()) {
        (false, false) => "Newest and oldest page",
        (false, true) => "Newest page",
        (true, false) => "Oldest page",
        (true, true) => "Between newest and oldest",
    }
}
fn render_history(history: Vec<&WorkTask<'_>>, cards: &Cards<'_>) -> Html {
    let mut groups: Vec<(String, Vec<&WorkTask<'_>>)> = Vec::new();
    for task in history {
        let date = history_date(task);
        if let Some((current, tasks)) = groups.last_mut()
            && *current == date
        {
            tasks.push(task);
        } else {
            groups.push((date, vec![task]));
        }
    }
    html! {
        <div class="work-history-groups">
            {for groups.into_iter().map(|(date, tasks)| html! {
                <section key={date.clone()} class="work-history-date">
                    <h3>{date}</h3>
                    <div class="work-history-list">
                        {for tasks.into_iter().map(|task| render_card(task, cards))}
                    </div>
                </section>
            })}
        </div>
    }
}

fn history_date(task: &WorkTask<'_>) -> String {
    task.row()
        .workflow
        .body
        .completion
        .map(|instant| {
            let mut instant = instant.to_string();
            if let Some(separator) = instant.find('T') {
                instant.truncate(separator);
            }
            instant
        })
        .unwrap_or_else(|| "Completion date unavailable".to_owned())
}

fn render_arrangement(state: &UseStateHandle<WorkArrangement>) -> Html {
    let current = **state;
    let choice = |label: &'static str, title: Option<&'static str>, pressed, next| {
        let state = state.clone();
        html! {
            <button
                type="button"
                title={title}
                aria-pressed={bool::to_string(&pressed)}
                onclick={Callback::from(move |_| state.set(next))}
            >{label}</button>
        }
    };
    html! {
        <div class="work-arrangement">
            <div class="work-arrangement-group" role="group" aria-label="Order cards">
                <span aria-hidden="true">{"Order"}</span>
                {for ORDERS.into_iter().map(|(order, label, title)| choice(
                    label,
                    Some(title),
                    current.order == order,
                    WorkArrangement { order, ..current },
                ))}
            </div>
            <div class="work-arrangement-group" role="group" aria-label="Cluster cards">
                <span aria-hidden="true">{"Cluster"}</span>
                {for CLUSTERINGS.into_iter().map(|(clustering, label)| choice(
                    label,
                    None,
                    current.clustering == clustering,
                    WorkArrangement { clustering, ..current },
                ))}
            </div>
        </div>
    }
}

fn render_lane(
    availability: WorkAvailability,
    clusters: &[WorkCluster<'_, '_>],
    cards: &Cards<'_>,
) -> Html {
    let key = availability_key(availability);
    let label = availability_label(availability);
    let heading = format!("work-lane-{key}");
    let count = clusters
        .iter()
        .map(|cluster| cluster.tasks.len())
        .sum::<usize>();
    html! {
        <section
            key={key}
            class="work-lane"
            data-work-lane={key}
            aria-labelledby={heading.clone()}
        >
            <header class="work-lane-heading">
                <h3 id={heading}>{label}</h3>
                <span aria-label={format!("{count} tasks")}>{count}</span>
            </header>
            if count == 0 {
                <p class="work-lane-empty">{format!("No {} tasks.", label.to_lowercase())}</p>
            } else {
                <div class="work-card-list">
                    {for clusters.iter().flat_map(|cluster| {
                        let separator = cluster_label(cluster.key).map(|name| html! {
                            <p key={format!("cluster:{name}")} class="work-lane-cluster">{name}</p>
                        });
                        separator
                            .into_iter()
                            .chain(cluster.tasks.iter().map(|task| render_card(task, cards)))
                    })}
                </div>
            }
        </section>
    }
}

/// The separator text for one cluster; the unclustered lane has none.
fn cluster_label(key: WorkClusterKey<'_>) -> Option<String> {
    match key {
        WorkClusterKey::Lane => None,
        WorkClusterKey::Project(project) => Some(project.to_owned()),
        WorkClusterKey::Phase(phase) => Some(phase.to_owned()),
    }
}

fn claimant_accent(task: &WorkTask<'_>, lanes: Option<&LaneView>) -> Option<usize> {
    let wire::Actor::Lane(lane) = task.working_claimant()? else {
        return None;
    };
    watchband::lane_accent(&lanes?.lanes, *lane)
}

fn render_card(task: &WorkTask<'_>, cards: &Cards<'_>) -> Html {
    let row = task.row();
    let statement = &row.statement.body;
    let workflow = &row.workflow.body;
    let availability = task.availability();
    let label = availability_label(availability);
    let owner = task.owner();
    let claim_accent = claimant_accent(task, cards.lanes);
    let owner_name = owner.map_or_else(
        || "Unassigned".to_owned(),
        |actor| actor_name(actor, cards.lanes),
    );
    let priority = priority_label(&statement.tags);
    let progress = flow_progress(workflow);
    let blocker = blocker_summary(task);
    let wake = wake_summary(task);
    let dependencies = task.dependencies().len();
    let dependents = task.dependents().len();
    let completion = workflow.completion.as_ref().map(ToString::to_string);
    let born = statement.born.clone();
    let is_selected = cards.selected == Some(&born);
    let is_pinned = cards.map.pinned(&born);
    let pin_disabled = !is_pinned && cards.map.pin_limit_reached();
    let toggle_pin = {
        let toggle_map_root = cards.toggle_map_root.clone();
        let born = born.clone();
        Callback::from(move |_| toggle_map_root.emit(born.clone()))
    };
    let open = {
        let select = cards.select.clone();
        let born = born.clone();
        Callback::from(move |_| select.emit(born.clone()))
    };
    html! {
        <article
            key={born.to_string()}
            class={classes!(
                "work-card",
                is_selected.then_some("work-card--selected"),
                is_pinned.then_some("work-card--mapped"),
            )}
            data-work-availability={availability_key(availability)}
            data-work-project={statement.project.clone()}
            data-selected={is_selected.to_string()}
            data-work-claimed={task.working_claimant().is_some().to_string()}
            data-work-claim-accent={claim_accent.map(|accent| accent.to_string())}
            data-accent={claim_accent.map(|accent| accent.to_string())}
            aria-label={format!(
                "{}: {}, {}, {label}",
                statement.handle,
                row.presentation_title(),
                statement.project,
            )}
        >
            <button
                type="button"
                class="work-card-open"
                data-task-stamp={born.to_string()}
                aria-label={format!("Open {} details", statement.handle)}
                aria-current={is_selected.then_some("true")}
                onclick={open}
            ></button>
            <header class="work-card-heading">
                <span class="work-card-handle">{statement.handle.to_string()}</span>
                if let Some(priority) = priority {
                    <span class="work-card-priority" title="Declared priority">{format!("P:{priority}")}</span>
                }
            </header>
            <h4>{row.presentation_title()}</h4>
            <div class="work-card-badges">
                <span class={phase_class(&workflow.status)}>{workflow.phase.to_string()}</span>
                <span class="tag" title="Phase in flow">{progress}</span>
                <span class="tag work-card-project" title="Project">{statement.project.clone()}</span>
            </div>
            <dl class="work-card-facts">
                <div>
                    <dt>{"Owner"}</dt>
                    <dd title={owner.map(ToString::to_string)}>{owner_name}</dd>
                </div>
                {render_lease(task)}
                if let Some(blocker) = blocker {
                    <div class="work-card-alert"><dt>{"Blocked"}</dt><dd>{blocker}</dd></div>
                }
                if let Some(wake) = wake {
                    <div class="work-card-wake"><dt>{"Wake"}</dt><dd>{wake}</dd></div>
                }
                if dependencies > 0 || dependents > 0 {
                    <div>
                        <dt>{"Links"}</dt>
                        <dd>{format!("{dependencies} dependencies · {dependents} dependents")}</dd>
                    </div>
                }
                if availability == WorkAvailability::Terminal {
                    <div>
                        <dt>{"Result"}</dt>
                        <dd>{terminal_summary(&workflow.status, completion.as_deref())}</dd>
                    </div>
                }
            </dl>
            <button
                type="button"
                class="work-card-map-toggle"
                aria-label={format!(
                    "{} {} in relationship map",
                    if is_pinned { "Remove" } else { "Pin" },
                    statement.handle,
                )}
                aria-pressed={is_pinned.to_string()}
                disabled={pin_disabled}
                onclick={toggle_pin}
            >{if is_pinned { "Mapped" } else { "+ Map" }}</button>
        </article>
    }
}

fn render_lease(task: &WorkTask<'_>) -> Html {
    let active = task
        .row()
        .claim
        .as_ref()
        .is_some_and(|claim| claim.body.disposition == ClaimDisposition::Active);
    let Some(lease) = active.then_some(task.row().lease.as_ref()).flatten() else {
        return Html::default();
    };
    let expiry = lease.body.expiry.get().as_millisecond();
    html! {
        <div>
            <dt>{"Lease"}</dt>
            <dd><Countdown {expiry} class="work-card-lease-time" /></dd>
        </div>
    }
}

/// One actor named for Work readers, resolved against the current topology.
pub(super) fn actor_name(actor: &wire::Actor, lanes: Option<&LaneView>) -> String {
    let member = match actor {
        wire::Actor::Lane(lane) => lanes.and_then(|lanes| lanes.member(*lane)),
        wire::Actor::Operator | wire::Actor::Spice => None,
    };
    model::owner_label(actor, member)
}

fn blocker_summary(task: &WorkTask<'_>) -> Option<String> {
    if task.availability() != WorkAvailability::Blocked {
        return None;
    }
    let dependencies = task.blocked_dependencies().count();
    match (dependencies, task.environment_blocked()) {
        (0, true) => Some("environment".to_owned()),
        (count, true) => Some(format!("{count} dependencies + environment")),
        (count, false) => Some(format!("{count} dependencies")),
    }
}

fn wake_summary(task: &WorkTask<'_>) -> Option<String> {
    (task.availability() == WorkAvailability::Deferred)
        .then(|| {
            task.row()
                .workflow
                .body
                .deferral
                .as_ref()
                .map(ToString::to_string)
        })
        .flatten()
}

fn flow_progress(workflow: &wire::WorkflowHead) -> String {
    let current = workflow
        .flow
        .iter()
        .position(|phase| phase == &workflow.phase)
        .map_or_else(|| "?".to_owned(), |position| (position + 1).to_string());
    format!("{current}/{}", workflow.flow.len())
}

fn priority_label(tags: &[String]) -> Option<&str> {
    tags.iter()
        .find_map(|tag| tag.strip_prefix("priority:"))
        .map(|priority| match priority {
            "critical" => "C",
            "high" => "H",
            "medium" => "M",
            "low" => "L",
            "none" => "N",
            other => other,
        })
}

fn terminal_summary(status: &str, completion: Option<&str>) -> String {
    completion.map_or_else(
        || status.to_owned(),
        |instant| format!("{status} · {instant}"),
    )
}

fn history_tasks<'a>(work: &'a WorkProjection<'a>) -> Vec<&'a WorkTask<'a>> {
    let mut tasks = work
        .tasks()
        .iter()
        .filter(|task| task.availability() == WorkAvailability::Terminal)
        .collect::<Vec<_>>();
    tasks.sort_by(history_order);
    tasks
}

fn history_order(left: &&WorkTask<'_>, right: &&WorkTask<'_>) -> Ordering {
    right
        .row()
        .workflow
        .body
        .completion
        .cmp(&left.row().workflow.body.completion)
        .then_with(|| {
            left.row()
                .statement
                .body
                .handle
                .cmp(&right.row().statement.body.handle)
        })
}

const fn order_key(order: WorkOrder) -> &'static str {
    match order {
        WorkOrder::Offer => "offer",
        WorkOrder::Newest => "newest",
    }
}

const fn clustering_key(clustering: WorkClustering) -> &'static str {
    match clustering {
        WorkClustering::None => "none",
        WorkClustering::Project => "project",
        WorkClustering::Phase => "phase",
    }
}

const fn availability_key(availability: WorkAvailability) -> &'static str {
    match availability {
        WorkAvailability::InFlight => "in-flight",
        WorkAvailability::Ready => "ready",
        WorkAvailability::Blocked => "blocked",
        WorkAvailability::Deferred => "deferred",
        WorkAvailability::Terminal => "terminal",
    }
}

const fn availability_label(availability: WorkAvailability) -> &'static str {
    match availability {
        WorkAvailability::InFlight => "In flight",
        WorkAvailability::Ready => "Ready",
        WorkAvailability::Blocked => "Blocked",
        WorkAvailability::Deferred => "Deferred",
        WorkAvailability::Terminal => "History",
    }
}
