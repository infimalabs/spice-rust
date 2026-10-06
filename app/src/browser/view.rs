//! App surface rendering for the operator browser.

use super::*;
use crate::work::{TaskOverviewSelection, WorkArrangement, WorkProjection, WorkScope};

pub(super) mod chrome;
mod direction_history;
mod quota;
mod team_menu;
mod work_board;
mod work_detail;
mod work_map;

pub(super) use work_map::WorkMapState;

#[derive(Clone)]
pub(super) struct AgentControls {
    /// Admit manual speech to the one browser-global playback queue.
    pub(super) speak: Callback<SpeechRequest>,
    /// Advance durable narration cursors and admit their live prefix.
    pub(super) narrate: Callback<NarrationBatch>,
    /// Set one team's durable automatic-speech admission policy.
    pub(super) set_automatic_speech: Callback<(String, bool)>,
    /// Exact rendered message hunk currently owned by the speech queue.
    pub(super) speech_owner: UseStateHandle<Option<SpeechOwner>>,
}

/// Pending direction count shared by compact and expanded presence indicators.
pub(super) fn member_pending(
    waiting: &[wire::intent::LaneInbox],
    submissions: &SubmissionStatus,
    lane: CanonicalUuid,
) -> usize {
    submissions.pending(lane, member_backend(waiting, lane))
}

/// Pending direction that can wake the lane, the only kind that marks it queued.
pub(super) fn member_waking(
    waiting: &[wire::intent::LaneInbox],
    submissions: &SubmissionStatus,
    lane: CanonicalUuid,
) -> usize {
    submissions.waking(lane, member_backend(waiting, lane))
}

fn member_backend(
    waiting: &[wire::intent::LaneInbox],
    lane: CanonicalUuid,
) -> &[wire::intent::PendingDirective] {
    waiting
        .iter()
        .find(|row| row.lane == lane)
        .map_or(&[], |row| row.pending.as_slice())
}

/// One shared live-state marker for global and team action menus.
pub(super) fn render_member_presence(status: crate::runtime::LaneStatus) -> Html {
    let label = status.label();
    html! {
        <span
            class={classes!("agent-presence-indicator", "app-member-presence", status.key())}
            role="img"
            data-lane-status={status.key()}
            aria-label={label}
            title={label}
        ></span>
    }
}

pub(super) struct AppRender<'a> {
    pub connection: &'a UseStateHandle<Connection>,
    pub surface: &'a UseStateHandle<SurfaceData>,
    pub destination: AppDestination,
    pub card_target_notice: Option<String>,
    pub navigate: &'a Callback<navigation::Intent>,
    pub app_menu_open: &'a UseStateHandle<bool>,
    pub work_scope: &'a WorkScope,
    pub work_arrangement: &'a UseStateHandle<WorkArrangement>,
    pub identity: &'a UseStateHandle<Option<AppIdentity>>,
    pub app_update: &'a UseStateHandle<Option<AppUpdate>>,
    pub team_controls: &'a UseReducerHandle<TeamControls>,
    pub composer_drafts: &'a UseReducerHandle<ComposerDrafts>,
    pub submissions: &'a SubmissionHandle,
    pub composer_drag: &'a super::watchband::PrimaryComposerDragState,
    pub team_drag: &'a super::watchband::TeamDragState,
    pub team_fleet: &'a NodeRef,
    pub unsafe_intent: bool,
    pub sender: &'a SenderHandle,
    pub agents: &'a AgentControls,
    pub selected_task: &'a UseStateHandle<Option<TaskOverviewSelection>>,
    pub detail_presented: bool,
    pub detail_closing: bool,
    pub work_map: &'a UseStateHandle<WorkMapState>,
    pub direction_history: &'a super::direction_history::Controls,
    pub work_history: &'a work_history::Controls,
    pub detail_close: &'a NodeRef,
    pub detail_sheet: &'a NodeRef,
    pub notes: &'a NoteComposers,
    pub submit_note: Callback<(wire::TaskStamp, SubmitEvent)>,
    pub select_task: Callback<wire::TaskStamp>,
    pub open_message_task: Callback<(wire::TaskStamp, web_sys::Element)>,
    pub request_task: Callback<wire::TaskStamp>,
    pub retry_relations: Callback<MouseEvent>,
    pub close_task: Callback<()>,
}

pub(super) fn render_app(render: AppRender<'_>, work: Option<&WorkProjection<'_>>) -> Html {
    let surface = render.surface.read();
    let cursor = surface
        .as_ref()
        .and_then(AppRuntime::resume)
        .map(|cursor| cursor.seq.to_string())
        .unwrap_or_else(|| "-".to_owned());
    let topology_revision = surface
        .as_ref()
        .and_then(AppRuntime::lanes)
        .map(|lanes| lanes.topology_revision.to_string());
    html! {
        <div
            class={classes!(
                "shell",
                (render.destination == AppDestination::Chat).then_some("shell--mosaic"),
            )}
            data-topology-revision={topology_revision}
            data-unsafe-intent={render.unsafe_intent.to_string()}
            data-submission-count={render.submissions.status.len().to_string()}
        >
            {chrome::render_command_bar(chrome::CommandBarRender {
                connection: render.connection,
                destination: render.destination,
                menu_open: render.app_menu_open,
                navigate: render.navigate,
                identity: render.identity,
                app_update: render.app_update,
                work_scope: render.work_scope,
                work,
                surface: render.surface,
                cursor: &cursor,
                sender: render.sender,
                submissions: &render.submissions.status,
            })}
            {watchband::render_feedback(surface.as_ref(), render.team_controls)}
            if let Some(notice) = &render.card_target_notice {
                <p
                    class="visually-hidden"
                    role="status"
                    aria-live="polite"
                    data-card-target-status=""
                >{notice.clone()}</p>
            }
            {render_workspace(&render, work)}
            {render_task_detail(&render, work)}
        </div>
    }
}

pub(super) fn render_workspace(render: &AppRender<'_>, work: Option<&WorkProjection<'_>>) -> Html {
    let mosaic_active = render.destination == AppDestination::Chat;
    let active_axis = render.destination.axis();
    html! {
        <>
            <div
                key="agents"
                class="workspace mosaic-workspace"
                aria-hidden={(!mosaic_active).to_string()}
                inert={(!mosaic_active).then_some("true")}
                data-active={mosaic_active.to_string()}
            >
                <main class="mosaic-content content">
                    {watchband::render_team(render, work)}
                </main>
            </div>
            if let Some(active_axis) = active_axis {
                <div key="analytics" class="workspace">
                    <main class="content">
                        {render_analytics(render, active_axis, work)}
                    </main>
                </div>
            }
        </>
    }
}

fn render_analytics(
    render: &AppRender<'_>,
    active_axis: Axis,
    work: Option<&WorkProjection<'_>>,
) -> Html {
    let (summary, body) = match active_axis {
        Axis::Flow => (
            work.map(|work| flow_summary(work, render.work_scope)),
            render_flow(render, work),
        ),
        Axis::Topology => render_topology(render),
    };
    let surface = render.surface.read();
    let lanes = surface.as_ref().and_then(|runtime| runtime.lanes());
    html! {
        <>
            <div class="page-heading">
                <div>
                    <h1>{analytics_heading(active_axis, work, render.work_scope, lanes)}</h1>
                    if let Some(summary) = summary {
                        <p>{summary}</p>
                    }
                </div>
            </div>
            {body}
        </>
    }
}

pub(super) const fn axis_label(axis: Axis) -> &'static str {
    AppDestination::for_axis(axis).label()
}
fn analytics_heading(
    axis: Axis,
    work: Option<&WorkProjection<'_>>,
    scope: &WorkScope,
    lanes: Option<&crate::runtime::LaneView>,
) -> String {
    match axis {
        Axis::Topology => "Resident dashboard".to_owned(),
        Axis::Flow => match scope {
            WorkScope::AllOpen => "All open work".to_owned(),
            WorkScope::Team { team, stem } => {
                let name = lanes
                    .and_then(|lanes| lanes.lanes.iter().find(|row| row.lane == *team))
                    .map_or_else(|| format!("Team {team}"), team_name);
                stem.as_ref().map_or_else(
                    || format!("{name} work"),
                    |stem| format!("{name} {stem} work"),
                )
            }
            WorkScope::Stem(identity) => work
                .and_then(|projection| projection.stem(identity))
                .map_or_else(
                    || "All open work".to_owned(),
                    |stem| format!("{} work", stem.identity()),
                ),
        },
    }
}

fn flow_summary(work: &WorkProjection<'_>, scope: &WorkScope) -> String {
    let counts = work.scope_counts(scope);
    format!(
        "{} open: {} ready, {} active, {} blocked, {} deferred",
        counts.open(),
        counts.ready(),
        counts.in_flight(),
        counts.blocked(),
        counts.deferred(),
    )
}

pub(super) fn render_flow(render: &AppRender<'_>, work: Option<&WorkProjection<'_>>) -> Html {
    let Some(work) = work else {
        return match &**render.connection {
            Connection::Starting | Connection::Connecting => {
                empty("Loading the current board snapshot.")
            }
            Connection::Offline(_) => empty("Board snapshot unavailable while reconnecting."),
            Connection::Online { .. } => empty("No board snapshot is available."),
        };
    };
    let surface = render.surface.read();
    let runtime = surface.as_ref();
    let scope_key = match render.work_scope {
        WorkScope::AllOpen => "all".to_owned(),
        WorkScope::Stem(identity) if work.stem(identity).is_some() => identity.to_string(),
        WorkScope::Stem(_) => "all".to_owned(),
        WorkScope::Team { team, stem } if work.contains_scope(render.work_scope) != Some(false) => {
            stem.as_ref().map_or_else(
                || format!("team:{team}"),
                |stem| format!("team:{team}:{stem}"),
            )
        }
        WorkScope::Team { .. } => "all".to_owned(),
    };
    // Every reconnect attempt passes back through Connecting, so the snapshot
    // stays stale until the connection is live again; the notice then toasts
    // once per outage rather than once per attempt.
    let stale = !matches!(&**render.connection, Connection::Online { .. });
    let selected = (**render.selected_task)
        .as_ref()
        .and_then(TaskOverviewSelection::first);
    let toggle_map_root = work_map::toggle_root_callback(render.work_map.clone());
    let creator_context = Callback::from(|event: MouseEvent| {
        super::card_target::remember_global_work_return(&event);
    });
    let lanes = runtime
        .and_then(AppRuntime::lanes)
        .map_or(&[][..], |lanes| lanes.lanes.as_slice());
    html! {
        <div class="work-layout" data-work-scope={scope_key}>
            {super::watchband::render_global_jobs(lanes)}
            <section tabindex="-1" class="section work-board-section">
                {work_board::render(
                    work,
                    render.work_scope,
                    stale,
                    selected,
                    work_board::WorkBoardControls {
                        map: render.work_map,
                        select: render.select_task.clone(),
                        creator_context,
                        creator_messages: runtime.map(AppRuntime::messages).unwrap_or_default(),
                        toggle_map_root,
                        arrangement: render.work_arrangement,
                        lanes: runtime.and_then(AppRuntime::lanes),
                        history_state: runtime.map(AppRuntime::work_history),
                        history: render.work_history,
                    },
                )}
            </section>
            if let Some(runtime) = runtime {
                {direction_history::render(
                    runtime.direction_history(),
                    runtime.lanes(),
                    runtime.board(),
                    runtime.messages(),
                    render.direction_history,
                    &render.select_task,
                )}
            }
        </div>
    }
}

fn render_task_detail(render: &AppRender<'_>, work: Option<&WorkProjection<'_>>) -> Html {
    let Some(work) = work else {
        return Html::default();
    };
    let Some(selection) = (**render.selected_task).as_ref() else {
        return Html::default();
    };
    let surface = render.surface.read();
    let runtime = surface.as_ref();
    work_detail::render(work_detail::WorkDetailRender {
        work,
        scope: render.work_scope,
        lanes: runtime.and_then(AppRuntime::lanes),
        navigate: render.navigate,
        creator_messages: runtime.map(AppRuntime::messages).unwrap_or_default(),
        selection,
        presented: render.detail_presented,
        closing: render.detail_closing,
        map: render.work_map,
        runtime,
        can_note: runtime.is_some_and(|runtime| runtime.offers(CommandKind::TaskNote)),
        can_close: runtime.is_some_and(|runtime| runtime.offers(CommandKind::TaskClose)),
        close_ref: render.detail_close,
        sheet_ref: render.detail_sheet,
        notes: render.notes,
        submit_note: render.submit_note.clone(),
        request_task: render.request_task.clone(),
        open_task: render.select_task.clone(),
        retry_relations: render.retry_relations.clone(),
        close: render.close_task.clone(),
    })
}

/// The resident dashboard body and, once agent standings are held, the page
/// summary over the same standings the topology section renders.
fn render_topology(render: &AppRender<'_>) -> (Option<String>, Html) {
    let surface = render.surface.read();
    let Some(runtime) = surface.as_ref() else {
        return (None, empty("No admitted topology is available."));
    };
    let identity_section = render_operator_and_resident(
        runtime.settings(),
        (**render.identity).as_ref(),
        render.connection,
    );
    let Some(lanes) = runtime.lanes() else {
        let body = html! {
            <>
                {identity_section}
                {empty("No agent snapshot is available.")}
            </>
        };
        return (None, body);
    };
    // The same join the chat's lane tiles read, over answers already held.
    let waiting = runtime
        .inbox()
        .map(|inbox| inbox.lanes.as_slice())
        .unwrap_or(&[]);
    let tasks = runtime
        .board()
        .map(|board| board.tasks.as_slice())
        .unwrap_or(&[]);
    let mut standings = crate::watchband::standings(&lanes.lanes, waiting, tasks);
    for standing in &mut standings {
        standing.pending = member_pending(waiting, &render.submissions.status, standing.lane);
        standing.waking = member_waking(waiting, &render.submissions.status, standing.lane);
    }
    let live = standings
        .iter()
        .filter(|standing| standing.thread.is_some())
        .count();
    let holding = standings
        .iter()
        .filter(|standing| standing.handle.is_some())
        .count();
    let pending = standings
        .iter()
        .map(|standing| standing.pending)
        .sum::<usize>();
    let (teams, solos): (Vec<_>, Vec<_>) = lanes.lanes.iter().partition(|team| team.teamed());
    let summary = format!(
        "{}: {live} live, {}, {pending} pending",
        topology_count(standings.len(), "agent", "agents"),
        topology_count(holding, "holding a task", "holding tasks"),
    );
    let totals = [
        topology_count(standings.len(), "agent", "agents"),
        topology_count(teams.len(), "team", "teams"),
        format!("{live} live"),
        topology_count(holding, "holding a task", "holding tasks"),
        format!("{pending} pending"),
    ]
    .join(" · ");
    let quota = quota::render_dash(Some(runtime));
    let body = html! {
        <>
            {identity_section}
            <section class="section dash-overview-topology-section" aria-labelledby="resident-topology-heading">
                <div class="section-heading">
                    <h2 id="resident-topology-heading">{"Resident topology"}</h2>
                    <span class="section-count">{totals}</span>
                </div>
                if lanes.lanes.is_empty() {
                    {empty("No agent lanes are connected.")}
                } else {
                    <div class="dash-overview-topology-groups">
                        {for teams.iter().map(|team| {
                            render_topology_group(
                                team_name(team),
                                Some(team.lane),
                                team_agents(team, &standings).collect(),
                            )
                        })}
                        if !solos.is_empty() {
                            {render_topology_group(
                                "Independent".to_owned(),
                                None,
                                solos.iter().flat_map(|team| team_agents(team, &standings)).collect(),
                            )}
                        }
                    </div>
                }
            </section>
            {quota}
        </>
    };
    (Some(summary), body)
}

/// A team's display name: its host's label, as Dash titles the team group, so
/// pages addressed by the team's UUID name it the same way.
fn team_name(team: &crate::runtime::LaneRow) -> String {
    let host = team.host().map_or("Empty", |member| {
        crate::watchband::agent_message_label(&member.worktree, member.name.as_deref())
    });
    format!("{host} team")
}

fn render_operator_and_resident(
    settings: Option<&wire::intent::GlobalSettingsAnswer>,
    identity: Option<&AppIdentity>,
    connection: &Connection,
) -> Html {
    let appliance = identity.map_or_else(
        || "Loading".to_owned(),
        |identity| identity.version.product.to_string(),
    );
    let (resident_status, resident_epoch, resident_epoch_title) = match connection {
        Connection::Starting => ("Starting", "Pending".to_owned(), None),
        Connection::Connecting => ("Connecting", "Pending".to_owned(), None),
        Connection::Online { epoch } => (
            "Online",
            epoch.split('-').next().unwrap_or(epoch).to_owned(),
            Some(epoch.clone()),
        ),
        Connection::Offline(_) => ("Reconnecting", "Unavailable".to_owned(), None),
    };
    html! {
        <section class="section dash-overview-operator-section" data-operator-member="">
            <div class="section-heading">
                <h2>{"Operator & resident"}</h2>
                <span class="section-count">{"Resident · "}{resident_status}</span>
            </div>
            <div class="dash-overview-identity-cards">
                <article
                    class="dash-overview-operator"
                    data-operator-state={if settings.is_some() { "ready" } else { "loading" }}
                >
                    if let Some(settings) = settings {
                        {render_operator_avatar(&settings.operator)}
                        <div class="dash-overview-operator-identity">
                            <h3>{settings.operator.name.as_deref().unwrap_or("Operator")}</h3>
                            <p>{settings.operator.email.as_deref().unwrap_or("No email configured")}</p>
                        </div>
                    } else {
                        <span
                            class="operator-avatar"
                            data-avatar-source="loading"
                            aria-hidden="true"
                        >{"…"}</span>
                        <div class="dash-overview-operator-identity">
                            <h3>{"Operator"}</h3>
                            <p>{"Identity loading"}</p>
                        </div>
                    }
                </article>
                <article class="dash-overview-resident" aria-label="Resident details">
                    <h3>{"Resident"}</h3>
                    <dl class="dash-overview-agent-facts">
                        <div>
                            <dt>{"Status"}</dt>
                            <dd>{resident_status}</dd>
                        </div>
                        <div>
                            <dt>{"Appliance"}</dt>
                            <dd>{appliance}</dd>
                        </div>
                        <div>
                            <dt>{"Epoch"}</dt>
                            <dd title={resident_epoch_title}>{resident_epoch}</dd>
                        </div>
                    </dl>
                </article>
            </div>
        </section>
    }
}

pub(super) fn render_operator_avatar(operator: &wire::OperatorIdentity) -> Html {
    let label = operator.name.as_deref().unwrap_or("Operator");
    match operator.gravatar.as_ref() {
        Some(gravatar) => html! {
            <span class="operator-avatar" data-avatar-source="gravatar">
                <img
                    src={format!("https://gravatar.com/avatar/{}?s=160&d=retro", gravatar.as_str())}
                    alt={format!("{label} Gravatar")}
                />
                if let Some(initials) = operator.initials.as_deref() {
                    <span class="operator-avatar-badge" aria-hidden="true">{initials}</span>
                }
            </span>
        },
        None => html! {
            <span
                class="operator-avatar"
                data-avatar-source="initials"
                role="img"
                aria-label={format!("{label} badge")}
            >{operator.initials.as_deref().unwrap_or("?")}</span>
        },
    }
}

fn topology_count(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

/// A lane's members paired with their joined standings, in member order.
fn team_agents<'a>(
    team: &'a crate::runtime::LaneRow,
    standings: &'a [crate::watchband::Standing],
) -> impl Iterator<
    Item = (
        &'a crate::runtime::LaneMember,
        &'a crate::watchband::Standing,
    ),
> {
    team.members.iter().zip(
        standings
            .iter()
            .filter(move |standing| standing.team == team.lane),
    )
}

/// Widest topology grid, in tile columns; `app/style.css` steps the grid down
/// from here as the section narrows.
const TOPOLOGY_COLUMNS: usize = 4;

/// One team, or every independent lane together, as a titled run of tiles.
/// The group spans one grid column per tile up to the grid's width, so small
/// groups share a row instead of leaving it empty.
fn render_topology_group(
    heading: String,
    team: Option<CanonicalUuid>,
    agents: Vec<(&crate::runtime::LaneMember, &crate::watchband::Standing)>,
) -> Html {
    let linked = team.is_some();
    html! {
        <section
            class="dash-overview-topology-group"
            data-topology-group={team.map(|team| team.to_string())}
            data-group-kind={if team.is_some() { "team" } else { "independent" }}
            data-span={agents.len().clamp(1, TOPOLOGY_COLUMNS).to_string()}
        >
            <h3 title={team.map(|team| format!("Group {team}"))}>
                {heading}
                <span>{topology_count(agents.len(), "agent", "agents")}</span>
            </h3>
            <div class="dash-overview-agent-list">
                {for agents.into_iter().map(|(member, standing)| {
                    render_topology_agent(member, standing, linked)
                })}
            </div>
        </section>
    }
}

/// One lane in the chat's tile language: accent-dealt name, presence, the
/// "pending, liveness" line, the held task, kept jobs, and the provider emblem.
/// Every row is always drawn so a tile's geometry never follows its data.
///
/// A teamed lane's tile is a link to its primary composer in Chat. Independent
/// lanes have no Chat surface, so their tiles stay inert.
fn render_topology_agent(
    member: &crate::runtime::LaneMember,
    standing: &crate::watchband::Standing,
    linked: bool,
) -> Html {
    debug_assert_eq!(member.lane, standing.lane, "standings keep member order");
    let status = member.status(standing.waking);
    let branch = crate::watchband::agent_message_label(&standing.worktree, None);
    let lane = standing.lane.to_string();
    let (tag, href, label) = if linked {
        (
            "a",
            Some(crate::view::card_href(
                &super::watchband::primary_textarea_id(&lane),
            )),
            Some(format!("Open {}'s composer in Chat", standing.label)),
        )
    } else {
        ("article", None, None)
    };
    html! {
        <@{tag}
            class="dash-overview-agent"
            href={href}
            aria-label={label}
            data-accent={standing.accent.to_string()}
            data-agent-lane={lane.clone()}
            data-lane-status={status.key()}
            title={format!("Lane {lane}\nGroup {}", standing.team)}
        >
            <header>
                {render_member_presence(status)}
                <strong>{standing.label.clone()}</strong>
                if standing.label != branch {
                    <span title={standing.worktree.clone()}>{branch.to_owned()}</span>
                }
            </header>
            <p>{format!("{} pending, {}", standing.pending, standing.liveness.as_str())}</p>
            <p class="dash-overview-agent-task" title={standing.task_title.clone()}>{
                match (&standing.handle, &standing.phase) {
                    (Some(handle), Some(phase)) => format!("{handle}, {phase}"),
                    (Some(handle), None) => handle.clone(),
                    (None, _) => "\u{a0}".to_owned(),
                }
            }</p>
            <p>{crate::watchband::jobs_kept(&standing.jobs).unwrap_or_else(|| "\u{a0}".to_owned())}</p>
            <p class="dash-overview-agent-model">{crate::watchband::model_display(standing.model.as_deref())}</p>
            {super::watchband::render_composer_vendor_icon(standing, "dash", None)}
        </@>
    }
}
