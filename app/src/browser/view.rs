//! Canvas surface rendering for the operator browser app.

use super::*;
use crate::work::{WorkArrangement, WorkProjection, WorkScope};

pub(super) mod chrome;
mod quota;
mod team_menu;
mod work_board;
mod work_detail;
mod work_map;

pub(super) use work_map::WorkMapState;

#[derive(Clone)]
pub(super) struct AgentControls {
    pub(super) seat: SeatDraft,
    pub(super) submit_seat: Callback<SubmitEvent>,
    /// Admit manual speech to the one browser-global playback queue.
    pub(super) speak: Callback<SpeechRequest>,
    /// Advance durable narration cursors and admit their live prefix.
    pub(super) narrate: Callback<NarrationBatch>,
    /// Stop and discard automatic speech while preserving explicit playback.
    pub(super) suppress_automatic_speech: Callback<()>,
    /// Exact rendered message currently owned by the speech queue.
    pub(super) speech_message: UseStateHandle<Option<String>>,
}

/// Render the reason the last submission was refused, or nothing.
///
/// The node is absent rather than empty while a submission stands. `role`
/// `alert` is an assertive live region, so an element that were always present
/// would be re-announced empty on every keystroke; leaving it out until there
/// is a sentence makes the announcement coincide with the refusal.
pub(super) fn render_notice(notice: &Notice) -> Html {
    if !notice.is_some() {
        return Html::default();
    }
    html! {
        <p class="form-refusal" role="alert">{notice.to_string()}</p>
    }
}

/// Pending direction count shared by compact and expanded presence indicators.
pub(super) fn member_pending(
    waiting: &[wire::intent::LaneInbox],
    submissions: &SubmissionStatus,
    lane: CanonicalUuid,
) -> usize {
    let backend = waiting
        .iter()
        .find(|row| row.lane == lane)
        .map(|row| row.pending.as_slice())
        .unwrap_or(&[]);
    submissions.pending(lane, backend)
}

/// One shared live-state marker for global and team action menus.
pub(super) fn render_member_presence(status: crate::runtime::LaneStatus) -> Html {
    let label = status.label();
    html! {
        <span
            class={classes!("agent-presence-indicator", "canvas-member-presence", status.key())}
            role="img"
            data-lane-status={status.key()}
            aria-label={label}
            title={label}
        ></span>
    }
}

pub(super) struct CanvasRender<'a> {
    pub connection: &'a UseStateHandle<Connection>,
    pub surface: &'a UseStateHandle<SurfaceData>,
    pub destination: CanvasDestination,
    pub card_target_notice: Option<String>,
    pub navigate: &'a Callback<navigation::Intent>,
    pub canvas_menu_open: &'a UseStateHandle<bool>,
    pub work_scope: &'a UseStateHandle<WorkScope>,
    pub work_scope_notice: &'a UseStateHandle<Option<String>>,
    pub work_arrangement: &'a UseStateHandle<WorkArrangement>,
    pub identity: &'a UseStateHandle<Option<CanvasIdentity>>,
    pub canvas_update: &'a UseStateHandle<Option<CanvasUpdate>>,
    pub team_controls: &'a UseReducerHandle<TeamControls>,
    pub composer_drafts: &'a UseReducerHandle<ComposerDrafts>,
    pub submissions: &'a SubmissionHandle,
    pub composer_drag: &'a super::watchband::PrimaryComposerDragState,
    pub team_fleet: &'a NodeRef,
    pub unsafe_intent: bool,
    pub sender: &'a SenderHandle,
    pub agents: &'a AgentControls,
    pub selected_task: &'a UseStateHandle<Option<wire::TaskStamp>>,
    pub detail_presented: bool,
    pub detail_closing: bool,
    pub work_map: &'a UseStateHandle<WorkMapState>,
    pub work_history: &'a work_history::Controls,
    pub team_settled: &'a super::team_settled::Controls,
    pub detail_close: &'a NodeRef,
    pub detail_sheet: &'a NodeRef,
    pub notes: &'a NoteComposers,
    pub submit_note: Callback<(wire::TaskStamp, SubmitEvent)>,
    pub select_task: Callback<wire::TaskStamp>,
    pub select_dash_task: Callback<wire::TaskStamp>,
    pub close_task: Callback<()>,
    pub leave_task: Callback<()>,
    pub leave_task_for_return: Callback<()>,
    pub retry_relations: Callback<MouseEvent>,
}

pub(super) fn render_canvas(render: CanvasRender<'_>, work: Option<&WorkProjection<'_>>) -> Html {
    let surface = render.surface.read();
    let cursor = surface
        .as_ref()
        .and_then(CanvasRuntime::resume)
        .map(|cursor| cursor.seq.to_string())
        .unwrap_or_else(|| "-".to_owned());
    let topology_revision = surface
        .as_ref()
        .and_then(CanvasRuntime::lanes)
        .map(|lanes| lanes.topology_revision.to_string());
    html! {
        <div
            class={classes!(
                "shell",
                (render.destination == CanvasDestination::Chat).then_some("shell--mosaic"),
            )}
            data-topology-revision={topology_revision}
            data-unsafe-intent={render.unsafe_intent.to_string()}
            data-submission-count={render.submissions.status.len().to_string()}
        >
            {chrome::render_command_bar(chrome::CommandBarRender {
                connection: render.connection,
                destination: render.destination,
                menu_open: render.canvas_menu_open,
                navigate: render.navigate,
                identity: render.identity,
                canvas_update: render.canvas_update,
                work_scope: render.work_scope,
                work,
                surface: render.surface,
                cursor: &cursor,
                sender: render.sender,
                submissions: &render.submissions.status,
            })}
            {watchband::render_feedback(surface.as_ref(), render.team_controls)}
            if let Some(notice) = &**render.work_scope_notice {
                <p class="visually-hidden" role="status" aria-live="polite">{notice}</p>
            }
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

pub(super) fn render_workspace(
    render: &CanvasRender<'_>,
    work: Option<&WorkProjection<'_>>,
) -> Html {
    let mosaic_active = render.destination == CanvasDestination::Chat;
    let active_axis = render.destination.axis();
    html! {
        <>
            <div
                key="agents"
                class="workspace mosaic-workspace"
                hidden={!mosaic_active}
                aria-hidden={(!mosaic_active).to_string()}
                inert={(!mosaic_active).then_some("true")}
                data-active={mosaic_active.to_string()}
            >
                <main class={classes!("mosaic-content", mosaic_active.then_some("content"))}>
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
    render: &CanvasRender<'_>,
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
    html! {
        <>
            <div class="page-heading">
                <div>
                    <h1>{analytics_heading(active_axis, work, render.work_scope)}</h1>
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
    CanvasDestination::for_axis(axis).label()
}
fn analytics_heading(axis: Axis, work: Option<&WorkProjection<'_>>, scope: &WorkScope) -> String {
    match axis {
        Axis::Topology => "Resident dashboard".to_owned(),
        Axis::Flow => scope
            .stem()
            .and_then(|identity| work.and_then(|projection| projection.stem(identity)))
            .map_or_else(
                || "All open work".to_owned(),
                |stem| format!("{} work", stem.identity()),
            ),
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

/// The seat form's fields as the operator has written them so far.
#[derive(Clone)]
pub(super) struct SeatDraft {
    pub(super) life: UseStateHandle<String>,
    pub(super) effort: UseStateHandle<String>,
    pub(super) notice: UseStateHandle<Notice>,
}

pub(super) fn render_seat_panel(runtime: Option<&CanvasRuntime>, controls: &AgentControls) -> Html {
    let can_seat = runtime.is_some_and(|runtime| runtime.offers(CommandKind::LaneSeat));
    let draft = &controls.seat;
    let catalog = runtime.and_then(CanvasRuntime::vendor_catalog);
    let selected = catalog.and_then(crate::compose::seat_vendor);
    let ready = can_seat && selected.is_some() && !draft.life.trim().is_empty();
    let default_model = selected.map_or("", |vendor| vendor.default_model.as_str());
    let default_effort = selected.map_or("", |vendor| vendor.default_effort.as_str());
    html! {
        <aside class="seat-panel">
            <form class="seat-form" onsubmit={controls.submit_seat.clone()}>
                <button type="submit" class="canvas-compact-control" disabled={!ready}>{"Seat agent"}</button>
                <label>
                    <span title="Name">{"Name"}</span>
                    <input id="seat-name" type="text" placeholder="Ava" />
                </label>
                <label>
                    <span title="Worktree">{"Worktree"}</span>
                    <input id="seat-worktree" type="text" placeholder="/absolute/path/to/worktree" />
                </label>
                <label>
                    <span title="Life">{"Life"}</span>
                    <select value={(*draft.life).clone()}
                        onchange={state_from_select(draft.life.clone())}>
                        {for wire::intent::TeamLife::ALL.iter().map(|life| html! {
                            <option
                                value={life.as_str()}
                                selected={life.as_str() == *draft.life}
                            >{life.as_str()}</option>
                        })}
                    </select>
                </label>
                <label>
                    <span title="Model">{"Model"}</span>
                    <input id="seat-model" type="text" list="lane-models"
                        placeholder={format!("default ({default_model})")} />
                </label>
                <datalist id="lane-models">
                    <option value={default_model.to_owned()} />
                </datalist>
                <label>
                    <span title="Effort">{"Effort"}</span>
                    <select value={(*draft.effort).clone()}
                        onchange={state_from_select(draft.effort.clone())}>
                        <option value="" selected={draft.effort.is_empty()}>
                            {format!("default ({default_effort})")}
                        </option>
                        {for catalog.into_iter().flat_map(|catalog| catalog.efforts.iter()).map(|effort| html! {
                            <option
                                value={effort.clone()}
                                selected={*effort == **draft.effort}
                            >{effort.clone()}</option>
                        })}
                    </select>
                </label>
                {render_notice(&draft.notice)}
            </form>
        </aside>
    }
}

pub(super) fn render_flow(render: &CanvasRender<'_>, work: Option<&WorkProjection<'_>>) -> Html {
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
    let scoped_stem = render
        .work_scope
        .stem()
        .filter(|identity| work.stem(identity).is_some());
    let scope_key = scoped_stem.map_or_else(|| "all".to_owned(), ToString::to_string);
    let stale = matches!(&**render.connection, Connection::Offline(_));
    let selected = (**render.selected_task).as_ref();
    let toggle_map_root = work_map::toggle_root_callback(render.work_map.clone());
    let creator_context = Callback::from(|event: MouseEvent| {
        super::card_target::remember_global_work_return(&event);
    });
    html! {
        <div class="work-layout" data-work-scope={scope_key}>
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
                        creator_messages: runtime.map(CanvasRuntime::messages).unwrap_or_default(),
                        toggle_map_root,
                        arrangement: render.work_arrangement,
                        lanes: runtime.and_then(CanvasRuntime::lanes),
                        history_state: runtime.map(CanvasRuntime::work_history),
                        history: render.work_history,
                    },
                )}
            </section>
        </div>
    }
}

fn render_task_detail(render: &CanvasRender<'_>, work: Option<&WorkProjection<'_>>) -> Html {
    let Some(work) = work else {
        return Html::default();
    };
    let surface = render.surface.read();
    let runtime = surface.as_ref();
    let selected = (**render.selected_task).as_ref();
    let detail = selected.and_then(|task| runtime.and_then(|runtime| runtime.task_detail(task)));
    let relation_key = selected.map(|selected| render.work_map.relations_key(selected));
    let relations = relation_key
        .as_ref()
        .and_then(|key| runtime.and_then(|runtime| runtime.task_relations(key)));
    work_detail::render(work_detail::WorkDetailRender {
        work,
        scope: render.work_scope,
        lanes: runtime.and_then(CanvasRuntime::lanes),
        navigate: render.navigate,
        team_controls: render.team_controls,
        creator_messages: runtime.map(CanvasRuntime::messages).unwrap_or_default(),
        selected,
        presented: render.detail_presented,
        closing: render.detail_closing,
        map: render.work_map,
        detail,
        relations,
        can_read_relations: runtime
            .is_some_and(|runtime| runtime.offers(CommandKind::TaskRelations)),
        can_note: runtime.is_some_and(|runtime| runtime.offers(CommandKind::TaskNote)),
        can_close: runtime.is_some_and(|runtime| runtime.offers(CommandKind::TaskClose)),
        close_ref: render.detail_close,
        sheet_ref: render.detail_sheet,
        notes: render.notes,
        submit_note: render.submit_note.clone(),
        select_task: render.select_task.clone(),
        close: render.close_task.clone(),
        leave: render.leave_task.clone(),
        leave_for_return: render.leave_task_for_return.clone(),
        retry_relations: render.retry_relations.clone(),
    })
}

/// The resident dashboard body and, once agent standings are held, the page
/// summary over the same standings the topology section renders.
fn render_topology(render: &CanvasRender<'_>) -> (Option<String>, Html) {
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
                            let host = team.host().map_or("Empty", |member| {
                                crate::watchband::agent_message_label(&member.worktree, member.name.as_deref())
                            });
                            render_topology_group(
                                format!("{host} team"),
                                Some((team.lane, render.team_controls)),
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

fn render_operator_and_resident(
    settings: Option<&wire::intent::GlobalSettingsAnswer>,
    identity: Option<&CanvasIdentity>,
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
    team: Option<(CanonicalUuid, &UseReducerHandle<TeamControls>)>,
    agents: Vec<(&crate::runtime::LaneMember, &crate::watchband::Standing)>,
) -> Html {
    // The composer lives in the team's Chat pane, which a team showing Dash or
    // Work leaves inert, so following the link first shows that pane.
    let show_chat = team.map(|(team, controls)| {
        let controls = controls.clone();
        Callback::from(move |_: MouseEvent| {
            controls.dispatch(TeamControlAction::ShowChat { team });
        })
    });
    let team = team.map(|(team, _)| team);
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
                    render_topology_agent(member, standing, show_chat.clone())
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
    show_chat: Option<Callback<MouseEvent>>,
) -> Html {
    debug_assert_eq!(member.lane, standing.lane, "standings keep member order");
    let status = member.status(standing.pending);
    let branch = crate::watchband::agent_message_label(&standing.worktree, None);
    let lane = standing.lane.to_string();
    let (tag, href, label) = if show_chat.is_some() {
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
            onclick={show_chat}
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
            {super::watchband::render_composer_vendor_icon(standing, "dash")}
        </@>
    }
}
