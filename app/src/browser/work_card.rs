//! Shared task-card rendering for the global board and team Work panels.

use super::clock::{TaskTimePresentation, task_time};
use super::view::WorkMapState;
use super::*;
use crate::runtime::LaneView;
use crate::watchband;
use crate::work::{self as model, WorkAvailability, WorkTask};
use wire::ClaimDisposition;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Density {
    Board,
    Compact,
}

pub(super) struct MapControls<'a> {
    pub state: &'a UseStateHandle<WorkMapState>,
    pub toggle_root: &'a Callback<wire::TaskStamp>,
}

pub(super) struct MessageControls<'a> {
    pub links: &'a watchband::WorkMessageLinks<'a>,
    pub team: wire::CanonicalUuid,
    pub follow: &'a Callback<MouseEvent>,
}

/// What one card reads besides its task.
pub(super) struct Controls<'a> {
    pub selected: Option<&'a wire::TaskStamp>,
    pub select: &'a Callback<wire::TaskStamp>,
    pub lanes: Option<&'a LaneView>,
    pub density: Density,
    pub map: Option<MapControls<'a>>,
    pub messages: Option<MessageControls<'a>>,
}

pub(super) fn render(task: &WorkTask<'_>, controls: &Controls<'_>) -> Html {
    let row = task.row();
    let statement = &row.statement.body;
    let workflow = &row.workflow.body;
    let availability = task.availability();
    let label = availability_label(availability);
    let claim_accent = claimant_accent(task, controls.lanes);
    let priority = priority_label(&statement.tags);
    let progress = flow_progress(workflow);
    let born = statement.born.clone();
    let message_target = controls
        .messages
        .as_ref()
        .and_then(|messages| messages.links.task(&statement.handle))
        .map(super::watchband::agent_message_source_id);
    let return_id = message_target.as_ref().and_then(|_| {
        controls
            .messages
            .as_ref()
            .map(|messages| format!("team-work-task-{}-{born}", messages.team))
    });
    let is_selected = controls.selected == Some(&born);
    let is_pinned = controls
        .map
        .as_ref()
        .is_some_and(|map| map.state.pinned(&born));
    let open = {
        let select = controls.select.clone();
        let born = born.clone();
        Callback::from(move |_| select.emit(born.clone()))
    };
    html! {
        <article
            key={born.to_string()}
            id={return_id}
            tabindex={message_target.as_ref().map(|_| "-1")}
            data-card-return-origin={message_target.as_ref().map(|_| "work")}
            class={classes!(
                "work-card",
                (controls.density == Density::Compact).then_some("work-card--compact"),
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
                    if let (Some(target), Some(messages)) = (&message_target, &controls.messages) {
                        <a
                            class="work-message-link"
                            href={crate::view::card_href(target)}
                            aria-label={format!(
                                "Open newest message mentioning {} in Chat",
                                statement.handle,
                            )}
                            onclick={messages.follow.clone()}
                        >{"Chat"}</a>
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

fn claimant_accent(task: &WorkTask<'_>, lanes: Option<&LaneView>) -> Option<usize> {
    let wire::Actor::Lane(lane) = task.working_claimant()? else {
        return None;
    };
    watchband::lane_accent(&lanes?.lanes, *lane)
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
