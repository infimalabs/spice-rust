//! Deterministic card workspace for the global Work surface.

use std::cmp::Ordering;

use super::*;
use crate::work::{WorkAvailability, WorkProject, WorkProjection, WorkScope, WorkTask};
use wire::ClaimDisposition;

use super::super::clock::Countdown;

const OPERATIONAL_BUCKETS: [WorkAvailability; 4] = [
    WorkAvailability::InFlight,
    WorkAvailability::Ready,
    WorkAvailability::Blocked,
    WorkAvailability::Deferred,
];

pub(super) fn render(
    work: &WorkProjection<'_>,
    scope: &WorkScope,
    stale: bool,
    selected: Option<&wire::TaskStamp>,
    select: Callback<wire::TaskStamp>,
) -> Html {
    let projects = work
        .projects()
        .iter()
        .enumerate()
        .filter(|(_, project)| project.counts().has_open())
        .filter(|(_, project)| project_in_scope(work, project, scope))
        .collect::<Vec<_>>();
    let history = history_tasks(work, scope);
    html! {
        <div class="work-board" aria-label="Work board">
            if stale {
                <p class="work-board-state" role="status">
                    {"Showing the last admitted board snapshot while reconnecting."}
                </p>
            }
            if projects.is_empty() {
                <div class="empty">{"No open tasks in this scope."}</div>
            } else {
                <div class="work-swimlanes">
                    {for projects.into_iter().map(|(index, project)| {
                        render_project(work, project, index, selected, &select)
                    })}
                </div>
            }
            <details class="work-history">
                <summary>
                    <span>{"History"}</span>
                    <span class="work-history-count">{history.len()}</span>
                </summary>
                if history.is_empty() {
                    <div class="empty">{"No completed or deleted tasks in this scope."}</div>
                } else {
                    <div class="work-history-list">
                        {for history.into_iter().map(|task| render_card(task, selected, &select))}
                    </div>
                }
            </details>
        </div>
    }
}

fn render_project(
    work: &WorkProjection<'_>,
    project: &WorkProject<'_>,
    index: usize,
    selected: Option<&wire::TaskStamp>,
    select: &Callback<wire::TaskStamp>,
) -> Html {
    let heading = format!("work-project-{index}");
    html! {
        <section
            class="work-swimlane"
            data-work-project={project.project().to_owned()}
            aria-labelledby={heading.clone()}
        >
            <header class="work-swimlane-heading">
                <div>
                    <p class="work-project-suffix">{project_suffix(project.project())}</p>
                    <h2 id={heading}>{project.project()}</h2>
                </div>
                <span class="section-count">{format!("{} open", project.counts().open())}</span>
            </header>
            <div class="work-buckets">
                {for OPERATIONAL_BUCKETS.into_iter().map(|availability| {
                    render_bucket(work, project, availability, selected, select)
                })}
            </div>
        </section>
    }
}

fn render_bucket(
    work: &WorkProjection<'_>,
    project: &WorkProject<'_>,
    availability: WorkAvailability,
    selected: Option<&wire::TaskStamp>,
    select: &Callback<wire::TaskStamp>,
) -> Html {
    let tasks = project
        .tasks()
        .iter()
        .map(|&index| &work.tasks()[index])
        .filter(|task| task.availability() == availability)
        .collect::<Vec<_>>();
    let label = availability_label(availability);
    html! {
        <section class="work-bucket" data-work-bucket={availability_key(availability)}>
            <header class="work-bucket-heading">
                <h3>{label}</h3>
                <span aria-label={format!("{} tasks", tasks.len())}>{tasks.len()}</span>
            </header>
            if tasks.is_empty() {
                <p class="work-bucket-empty">{format!("No {} tasks.", label.to_lowercase())}</p>
            } else {
                <div class="work-card-list">
                    {for tasks.into_iter().map(|task| render_card(task, selected, select))}
                </div>
            }
        </section>
    }
}

fn render_card(
    task: &WorkTask<'_>,
    selected: Option<&wire::TaskStamp>,
    select: &Callback<wire::TaskStamp>,
) -> Html {
    let row = task.row();
    let statement = &row.statement.body;
    let workflow = &row.workflow.body;
    let availability = task.availability();
    let label = availability_label(availability);
    let owner = active_owner(task);
    let priority = priority_label(&statement.tags);
    let progress = flow_progress(workflow);
    let blocker = blocker_summary(task);
    let wake = wake_summary(task);
    let dependencies = task.dependencies().len();
    let dependents = task.dependents().len();
    let completion = workflow.completion.as_ref().map(ToString::to_string);
    let born = statement.born.clone();
    let is_selected = selected == Some(&born);
    let open = {
        let select = select.clone();
        let born = born.clone();
        Callback::from(move |_| select.emit(born.clone()))
    };
    html! {
        <article
            key={born.to_string()}
            class={classes!("work-card", is_selected.then_some("work-card--selected"))}
            data-work-availability={availability_key(availability)}
            data-selected={is_selected.to_string()}
            aria-label={format!("{}: {}, {label}", statement.handle, row.presentation_title())}
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
                <span class="work-card-priority" title="Declared priority">{format!("P:{priority}")}</span>
            </header>
            <h4>{row.presentation_title()}</h4>
            <div class="work-card-badges">
                <span class={phase_class(&workflow.status)}>{workflow.phase.to_string()}</span>
                <span class="tag">{progress}</span>
                <span class="tag">{project_suffix(&statement.project)}</span>
            </div>
            <dl class="work-card-facts">
                <div><dt>{"Owner"}</dt><dd>{owner}</dd></div>
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

fn active_owner(task: &WorkTask<'_>) -> String {
    task.row()
        .claim
        .as_ref()
        .filter(|claim| claim.body.disposition == ClaimDisposition::Active)
        .map(|claim| claim.body.actor.to_string())
        .or_else(|| {
            task.row()
                .workflow
                .body
                .assignment
                .as_ref()
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| "Unassigned".to_owned())
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

fn priority_label(tags: &[String]) -> String {
    tags.iter()
        .find_map(|tag| tag.strip_prefix("priority:"))
        .map(|priority| match priority {
            "critical" => "C".to_owned(),
            "high" => "H".to_owned(),
            "medium" => "M".to_owned(),
            "low" => "L".to_owned(),
            "none" => "N".to_owned(),
            other => other.to_owned(),
        })
        .unwrap_or_else(|| "—".to_owned())
}

fn project_suffix(project: &str) -> &str {
    project
        .trim_start_matches('.')
        .split_once('.')
        .map_or("root", |(_, suffix)| suffix)
}

fn terminal_summary(status: &str, completion: Option<&str>) -> String {
    completion.map_or_else(
        || status.to_owned(),
        |instant| format!("{status} · {instant}"),
    )
}

fn project_in_scope(
    work: &WorkProjection<'_>,
    project: &WorkProject<'_>,
    scope: &WorkScope,
) -> bool {
    scope
        .stem()
        .is_none_or(|identity| work.stems()[project.stem()].identity() == identity)
}

fn history_tasks<'a>(work: &'a WorkProjection<'a>, scope: &WorkScope) -> Vec<&'a WorkTask<'a>> {
    let mut tasks = work
        .tasks()
        .iter()
        .filter(|task| task.availability() == WorkAvailability::Terminal)
        .filter(|task| project_in_scope(work, &work.projects()[task.project()], scope))
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
