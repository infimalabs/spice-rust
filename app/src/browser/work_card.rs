//! Shared task-card rendering for the global Work board.

use super::clock::{TaskTimePresentation, task_time};
use super::view::WorkMapState;
use super::*;
use crate::runtime::LaneView;
use crate::work::{self as model, WorkAvailability, WorkTask};
use wire::ClaimDisposition;

pub(super) struct MapControls<'a> {
    pub state: &'a UseStateHandle<WorkMapState>,
    pub toggle_root: &'a Callback<wire::TaskStamp>,
}

/// What one card reads besides its task.
pub(super) struct Controls<'a> {
    pub selected: Option<&'a wire::TaskStamp>,
    pub select: &'a Callback<wire::TaskStamp>,
    pub lanes: Option<&'a LaneView>,
    pub map: Option<MapControls<'a>>,
    pub creator_context: Option<&'a Callback<MouseEvent>>,
    pub creator_messages: &'a [Rc<wire::AgentMessage>],
}

struct CellGutter<'a> {
    grouped: bool,
    lead: Option<Html>,
    label: Option<&'a str>,
}

/// The reserved gutter above a grouped card: the lane heading it may host
/// leads, and its cluster label aligns to the card's far edge.
fn render_gutter(gutter: CellGutter<'_>) -> Html {
    if gutter.lead.is_none() && gutter.label.is_none() {
        return Html::default();
    }
    html! {
        <div class="work-card-gutter">
            {gutter.lead.unwrap_or_default()}
            if let Some(label) = gutter.label {
                <p class="work-card-group-label">{label}</p>
            }
        </div>
    }
}

fn open_task(select: &Callback<wire::TaskStamp>, born: &wire::TaskStamp) -> Callback<MouseEvent> {
    let select = select.clone();
    let born = born.clone();
    Callback::from(move |_| select.emit(born.clone()))
}
pub(super) fn render(task: &WorkTask<'_>, controls: &Controls<'_>) -> Html {
    render_cell(
        task,
        controls,
        CellGutter {
            grouped: false,
            lead: None,
            label: None,
        },
    )
}

/// A global-board card in the lane's reserved gutter, which may host the
/// lane heading and the label of the cluster the card opens. Every global card
/// uses this path so changing the arrangement only changes gutter content and
/// keyed card order.
pub(super) fn render_grouped(
    task: &WorkTask<'_>,
    controls: &Controls<'_>,
    lead: Option<Html>,
    group_label: Option<&str>,
) -> Html {
    render_cell(
        task,
        controls,
        CellGutter {
            grouped: true,
            lead,
            label: group_label,
        },
    )
}

fn render_cell(task: &WorkTask<'_>, controls: &Controls<'_>, gutter: CellGutter<'_>) -> Html {
    let row = task.row();
    let statement = &row.statement.body;
    let workflow = &row.workflow.body;
    let availability = task.availability();
    let label = availability_label(availability);
    let claim_accent = task.working_claimant_accent(controls.lanes);
    let priority = priority_label(&statement.tags);
    let progress = flow_progress(workflow);
    let born = statement.born.clone();
    let creator_context = crate::view::creator_context_href(
        &statement.creation,
        controls.creator_messages.iter().map(AsRef::as_ref),
    );
    let has_chat_target = creator_context.is_some();
    let return_id = has_chat_target.then(|| format!("work-task-{born}"));
    let is_selected = controls.selected == Some(&born);
    let is_pinned = controls
        .map
        .as_ref()
        .is_some_and(|map| map.state.pinned(&born));
    let cell_class = classes!(
        "work-card-cell",
        gutter.grouped.then_some("work-card-cell--grouped"),
    );
    let gutter = render_gutter(gutter);
    let open = open_task(controls.select, &born);
    // The cell is the list item and the size container, so a card's layout
    // follows its own width rather than the section that holds the list.
    html! {
        <div key={born.to_string()} class={cell_class}>
            {gutter}
            <article
                id={return_id}
                tabindex={has_chat_target.then_some("-1")}
                data-card-return-origin={has_chat_target.then_some("work")}
                class={classes!(
                    "work-card",
                    is_selected.then_some("work-card--selected"),
                    is_pinned.then_some("work-card--mapped"),
                )}
                data-work-availability={availability.key()}
                data-work-disposition={
                    (availability == WorkAvailability::Terminal).then(|| workflow.status.clone())
                }
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
                    <span class="work-card-heading-actions">
                        if let Some(priority) = priority {
                            <span class="work-card-priority" title="Declared priority">{format!("P:{priority}")}</span>
                        }
                        if let Some(href) = creator_context {
                            {super::task_detail::creator_context_link(
                                "work-message-link",
                                href,
                                statement,
                                controls.creator_context.cloned(),
                            )}
                        }
                    </span>
                </header>
                <h4>{row.presentation_title()}</h4>
                <div class="work-card-badges">
                    <span class={phase_class(&workflow.status)}>{workflow.phase.to_string()}</span>
                    <span class="tag" title="Phase in flow">{progress}</span>
                    <span class="tag work-card-project" title="Project">{statement.project.clone()}</span>
                </div>
                {render_facts(task, controls)}
                {render_map_toggle(&statement.handle, &born, controls)}
            </article>
        </div>
    }
}

fn render_facts(task: &WorkTask<'_>, controls: &Controls<'_>) -> Html {
    let owner = task.owner();
    let owner_name = owner.map_or_else(
        || "Unassigned".to_owned(),
        |actor| actor_name(actor, controls.lanes),
    );
    let dependencies = task.dependencies().len();
    let dependents = task.dependents().len();
    html! {
        <dl class="work-card-facts">
            <div>
                <dt>{"Owner"}</dt>
                <dd title={owner.map(ToString::to_string)}>{owner_name}</dd>
            </div>
            {render_lease(task)}
            if let Some(blocker) = blocker_summary(task) {
                <div class="work-card-alert"><dt>{"Blocked"}</dt><dd>{blocker}</dd></div>
            }
            if let Some(wake) = wake_instant(task) {
                <div class="work-card-wake"><dt>{"Wake"}</dt><dd>{task_time(wake, TaskTimePresentation::CompactRemaining)}</dd></div>
            }
            if dependencies > 0 || dependents > 0 {
                <div>
                    <dt>{"Links"}</dt>
                    <dd>{format!("{dependencies} dependencies · {dependents} dependents")}</dd>
                </div>
            }
            if task.availability() == WorkAvailability::Terminal {
                {render_result(&task.row().workflow.body)}
            }
        </dl>
    }
}

fn render_map_toggle(
    handle: &impl std::fmt::Display,
    born: &wire::TaskStamp,
    controls: &Controls<'_>,
) -> Html {
    let Some(map) = controls.map.as_ref() else {
        return Html::default();
    };
    let is_pinned = map.state.pinned(born);
    let toggle_pin = {
        let toggle_map_root = map.toggle_root.clone();
        let born = born.clone();
        Callback::from(move |_| toggle_map_root.emit(born.clone()))
    };
    html! {
        <button
            type="button"
            class="work-card-map-toggle"
            aria-label={format!(
                "{} {handle} in relationship map",
                if is_pinned { "Remove" } else { "Pin" },
            )}
            aria-pressed={is_pinned.to_string()}
            disabled={!is_pinned && map.state.pin_limit_reached()}
            onclick={toggle_pin}
        >{if is_pinned { "Mapped" } else { "+ Map" }}</button>
    }
}

fn render_result(workflow: &wire::WorkflowHead) -> Html {
    html! {
        <div>
            <dt>{"Result"}</dt>
            <dd class="work-card-result">
                {workflow.status.clone()}
                if let Some(completion) = workflow.completion.as_ref() {
                    <span aria-hidden="true">{"·"}</span>
                    {task_time(completion, TaskTimePresentation::CompactElapsed)}
                }
            </dd>
        </div>
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
    html! {
        <div>
            <dt>{"Lease"}</dt>
            <dd>{task_time(&lease.body.expiry, TaskTimePresentation::CompactRemaining)}</dd>
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

fn wake_instant<'a>(task: &'a WorkTask<'_>) -> Option<&'a wire::Instant> {
    if task.availability() == WorkAvailability::Deferred {
        task.row().workflow.body.deferral.as_ref()
    } else {
        None
    }
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

pub(super) const fn availability_label(availability: WorkAvailability) -> &'static str {
    match availability {
        WorkAvailability::InFlight => "In flight",
        WorkAvailability::Ready => "Ready",
        WorkAvailability::Blocked => "Blocked",
        WorkAvailability::Deferred => "Deferred",
        WorkAvailability::Terminal => "History",
    }
}
