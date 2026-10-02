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
use crate::work::{
    WorkArrangement, WorkAvailability, WorkCluster, WorkClusterKey, WorkClustering, WorkOrder,
    WorkProjection, WorkScope, WorkTask,
};

use super::super::clock::local_date;
use super::super::work_card::{self, Density, MapControls};
use super::super::work_history::Controls as WorkHistoryControls;

/// Board lanes in reading order: workable tasks lead, then work already held.
const OPEN_LANES: [WorkAvailability; 4] = [
    WorkAvailability::Ready,
    WorkAvailability::InFlight,
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
    pub creator_context: Callback<MouseEvent>,
    pub creator_messages: &'a [Rc<wire::AgentMessage>],
    pub toggle_map_root: Callback<wire::TaskStamp>,
    pub arrangement: &'a UseStateHandle<WorkArrangement>,
    pub lanes: Option<&'a LaneView>,
    pub history_state: Option<&'a WorkHistoryState>,
    pub history: &'a WorkHistoryControls,
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
        creator_context,
        creator_messages,
        toggle_map_root,
        arrangement: arrangement_state,
        lanes,
        history_state,
        history: history_controls,
    } = controls;
    let cards = work_card::Controls {
        selected,
        select: &select,
        lanes,
        density: Density::Board,
        map: Some(MapControls {
            state: map,
            toggle_root: &toggle_map_root,
        }),
        messages: None,
        creator_context: Some(&creator_context),
        creator_messages,
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
            <crate::browser::toast::Notice
                message={stale.then_some(AttrValue::from(
                    "Showing the last admitted board snapshot while reconnecting.",
                ))}
            />
            <div class="work-lanes">
                {for board_lanes.into_iter().map(|(availability, clusters)| {
                    render_lane(availability, &clusters, &cards)
                })}
            </div>
            {render_history_archive(history, history_state, history_controls, &cards)}
        </div>
    }
}

fn render_history_archive(
    history: Vec<&WorkTask<'_>>,
    state: Option<&WorkHistoryState>,
    controls: &WorkHistoryControls,
    cards: &work_card::Controls<'_>,
) -> Html {
    html! {
        <details
            class="work-history"
            open={*controls.open}
            ontoggle={controls.toggle.clone()}
        >
            <summary>
                <span>{"History"}</span>
                <span class="work-history-count">{crate::browser::archive_status::count(history_range(state))}</span>
            </summary>
            // Once a page has loaded the body stays mounted, bounded to that
            // one page, so closing can transition out instead of cutting.
            if *controls.open || state.and_then(WorkHistoryState::total).is_some() {
                <div class="work-history-finder" role="search" aria-label="Find terminal tasks">
                    <label>
                        <span>{"Search"}</span>
                        <input
                            id="work-history-query"
                            type="search"
                            maxlength={wire::intent::TASK_HISTORY_QUERY_BYTES_MAX.to_string()}
                            placeholder="Handle, title, project, or description"
                        />
                    </label>
                    <label>
                        <span>{"Completed by"}</span>
                        <input
                            id="work-history-cutoff"
                            type="date"
                        />
                    </label>
                    <div class="work-history-filter-actions app-control-rail team-mode-rail">
                        <button type="button" class="team-mode" onclick={controls.apply.clone()}>{"Apply"}</button>
                        <button type="button" class="team-mode" onclick={controls.clear.clone()}>{"Clear"}</button>
                    </div>
                </div>
                if let Some(issue) = controls.validation.as_ref() {
                    <p class="form-refusal work-history-filter-issue" role="alert">{issue}</p>
                }
                {crate::browser::archive_status::render(&crate::browser::archive_status::ArchiveStatus {
                    range: history_range(state),
                    has_newer: state.is_some_and(WorkHistoryState::has_newer),
                    has_older: state.is_some_and(WorkHistoryState::has_older),
                    loading: state.is_none_or(WorkHistoryState::loading),
                    issue: state.and_then(WorkHistoryState::issue),
                    pages_label: "History pages",
                    newer: controls.newer.clone(),
                    older: controls.older.clone(),
                    retry: controls.retry.clone(),
                })}
                <div class="work-history-entries">
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
                </div>
            }
        </details>
    }
}

fn history_range(state: Option<&WorkHistoryState>) -> Option<(u64, u64, u64)> {
    let state = state?;
    Some((state.first_result(), state.last_result(), state.total()?))
}

fn render_history(history: Vec<&WorkTask<'_>>, cards: &work_card::Controls<'_>) -> Html {
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
                        {for tasks.into_iter().map(|task| work_card::render(task, cards))}
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
        .as_ref()
        .map_or_else(|| "Completion date unavailable".to_owned(), local_date)
}

fn render_arrangement(state: &UseStateHandle<WorkArrangement>) -> Html {
    let current = **state;
    let choice = |label: &'static str, title: Option<&'static str>, pressed, next| {
        let state = state.clone();
        html! {
            <button
                type="button"
                class={classes!("team-mode", pressed.then_some("selected"))}
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
                <span class="app-control-rail team-mode-rail">
                {for ORDERS.into_iter().map(|(order, label, title)| choice(
                    label,
                    Some(title),
                    current.order == order,
                    WorkArrangement { order, ..current },
                ))}
                </span>
            </div>
            <div class="work-arrangement-group" role="group" aria-label="Cluster cards">
                <span aria-hidden="true">{"Cluster"}</span>
                <span class="app-control-rail team-mode-rail">
                {for CLUSTERINGS.into_iter().map(|(clustering, label)| choice(
                    label,
                    None,
                    current.clustering == clustering,
                    WorkArrangement { clustering, ..current },
                ))}
                </span>
            </div>
        </div>
    }
}

fn render_lane(
    availability: WorkAvailability,
    clusters: &[WorkCluster<'_, '_>],
    cards: &work_card::Controls<'_>,
) -> Html {
    let key = availability.key();
    let label = work_card::availability_label(availability);
    let heading_id = format!("work-lane-{key}");
    let count = clusters
        .iter()
        .map(|cluster| cluster.tasks.len())
        .sum::<usize>();
    // The lane heading rides in the reserved gutter of the lane's first card,
    // or of its empty message, beside the first cluster's label.
    let heading = html! {
        <header class="work-lane-heading">
            <h3 id={heading_id.clone()}>{label}</h3>
            <span aria-label={format!("{count} tasks")}>{count}</span>
        </header>
    };
    html! {
        <section
            key={key}
            class="work-lane"
            data-work-lane={key}
            aria-labelledby={heading_id}
        >
            <div
                class="work-lane-track"
                tabindex="0"
                role="region"
                aria-label={format!("{label} task row")}
            >
                if count == 0 {
                    <div class="work-lane-vacancy">
                        <div class="work-card-gutter">{heading}</div>
                        <p class="work-lane-empty">{format!("No {} tasks.", label.to_lowercase())}</p>
                    </div>
                } else {
                    <div class="work-card-list">
                        {for clusters.iter().enumerate().flat_map(|(cluster_index, cluster)| {
                            let label = cluster_label(cluster.key);
                            let heading = heading.clone();
                            cluster.tasks.iter().enumerate().map(move |(index, task)| {
                                work_card::render_grouped(
                                    task,
                                    cards,
                                    (cluster_index == 0 && index == 0).then(|| heading.clone()),
                                    if index == 0 { label } else { None },
                                )
                            })
                        })}
                    </div>
                }
            </div>
        </section>
    }
}

/// The gutter text for a cluster; the unclustered lane leaves it empty.
const fn cluster_label(key: WorkClusterKey<'_>) -> Option<&str> {
    match key {
        WorkClusterKey::Lane => None,
        WorkClusterKey::Project(project) => Some(project),
        WorkClusterKey::Phase(phase) => Some(phase),
    }
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
