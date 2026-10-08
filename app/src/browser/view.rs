//! App surface rendering for the operator browser.

use super::*;
use crate::work::{TaskOverviewSelection, WorkProjection};

pub(super) mod chrome;
mod quota;
mod sidebar;
mod spice_sidebar;
mod work_blockers;
mod work_detail;
mod work_jobs;
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
            class={classes!("app-presence-pip", "app-presence-pip--member", status.key())}
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
    pub card_target_notice: Option<String>,
    pub navigate: &'a Callback<navigation::Intent>,
    pub spice_sidebar_open: bool,
    pub open_spice_sidebar: Callback<web_sys::Element>,
    pub identity: &'a UseStateHandle<Option<AppIdentity>>,
    pub app_update: &'a UseStateHandle<Option<AppUpdate>>,
    pub team_controls: &'a UseReducerHandle<TeamControls>,
    pub composer_drafts: &'a UseReducerHandle<ComposerDrafts>,
    pub submissions: &'a SubmissionHandle,
    pub composer_drag: &'a super::watchband::PrimaryComposerDragState,
    pub composer_slots: &'a super::watchband::ComposerSlots,
    pub team_drag: &'a super::watchband::TeamDragState,
    pub team_fleet: &'a NodeRef,
    pub unsafe_intent: bool,
    pub sender: &'a SenderHandle,
    pub agents: &'a AgentControls,
    pub sidebar_content: &'a UseStateHandle<Option<super::sidebar::Content>>,
    pub sidebar_presented: bool,
    pub sidebar_closing: bool,
    pub work_map: &'a UseStateHandle<WorkMapState>,
    pub sidebar_close: &'a NodeRef,
    pub sidebar_sheet: &'a NodeRef,
    pub blocker_review: &'a UseStateHandle<bool>,
    pub blocker_review_button: &'a NodeRef,
    pub blocker_review_close: &'a NodeRef,
    pub notes: &'a NoteComposers,
    pub submit_note: Callback<(wire::TaskStamp, SubmitEvent)>,
    pub grant_trust: Callback<task_detail::TrustGrantRequest>,
    pub select_task: Callback<wire::TaskStamp>,
    pub open_overview: Callback<TaskOverviewSelection>,
    pub open_message_task: Callback<(wire::TaskStamp, web_sys::Element)>,
    pub request_task: Callback<wire::TaskStamp>,
    pub retry_relations: Callback<MouseEvent>,
    pub close_sidebar: Callback<()>,
}

pub(super) fn render_app(render: AppRender<'_>, work: Option<&WorkProjection<'_>>) -> Html {
    let surface = render.surface.read();
    let topology_revision = surface
        .as_ref()
        .and_then(AppRuntime::lanes)
        .map(|lanes| lanes.topology_revision.to_string());
    html! {
        <div
            class="shell"
            data-topology-revision={topology_revision}
            data-unsafe-intent={render.unsafe_intent.to_string()}
            data-submission-count={render.submissions.status.len().to_string()}
        >
            {chrome::render_command_bar(chrome::CommandBarRender {
                connection: render.connection,
                navigate: render.navigate,
                spice_sidebar_open: render.spice_sidebar_open,
                open_spice_sidebar: &render.open_spice_sidebar,
                app_update: render.app_update,
                work,
                surface: render.surface,
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
            {render_sidebar(&render, work)}
        </div>
    }
}

fn render_workspace(render: &AppRender<'_>, work: Option<&WorkProjection<'_>>) -> Html {
    html! {
        <div class="workspace mosaic-workspace">
            <main class="mosaic-content content">
                {watchband::render_team(render, work)}
            </main>
        </div>
    }
}

fn render_sidebar(render: &AppRender<'_>, work: Option<&WorkProjection<'_>>) -> Html {
    let sidebar_content = render.sidebar_content.as_ref();
    let Some(content) = sidebar_content.as_ref() else {
        return Html::default();
    };
    let surface = render.surface.read();
    let runtime = surface.as_ref();
    let cursor = runtime
        .and_then(AppRuntime::resume)
        .map(|cursor| cursor.seq.to_string())
        .unwrap_or_else(|| "-".to_owned());
    let content = match content {
        super::sidebar::Content::Spice => render_spice_sidebar(render, runtime, &cursor),
        super::sidebar::Content::Tasks(selection) => {
            let Some(work) = work else {
                return Html::default();
            };
            work_detail::render(work_detail::WorkDetailRender {
                work,
                lanes: runtime.and_then(AppRuntime::lanes),
                navigate: render.navigate,
                creator_messages: runtime.map(AppRuntime::messages).unwrap_or_default(),
                selection,
                map: render.work_map,
                runtime,
                can_note: runtime.is_some_and(|runtime| runtime.offers(CommandKind::TaskNote)),
                can_close: runtime.is_some_and(|runtime| runtime.offers(CommandKind::TaskClose)),
                blocker_review: render.blocker_review,
                blocker_review_button_ref: render.blocker_review_button,
                blocker_review_close_ref: render.blocker_review_close,
                notes: render.notes,
                submit_note: render.submit_note.clone(),
                grant_trust: render.grant_trust.clone(),
                request_task: render.request_task.clone(),
                open_overview: render.open_overview.clone(),
                retry_relations: render.retry_relations.clone(),
            })
        }
    };
    sidebar::render(sidebar::SidebarRender {
        content,
        presented: render.sidebar_presented,
        closing: render.sidebar_closing,
        close_ref: render.sidebar_close,
        sheet_ref: render.sidebar_sheet,
        close: render.close_sidebar.clone(),
    })
}
fn render_spice_sidebar(
    render: &AppRender<'_>,
    runtime: Option<&AppRuntime>,
    cursor: &str,
) -> sidebar::Content {
    let context = spice_sidebar::Context {
        sender: render.sender,
        submissions: &render.submissions.status,
    };
    let body = html! {
        <div class="app-spice-sidebar-content">
            {spice_sidebar::render_operator_settings(runtime, render.sender)}
            {spice_sidebar::render_resident(
                render.connection,
                runtime,
                cursor,
                (**render.identity).as_ref(),
                (**render.app_update).as_ref(),
            )}
            {spice_sidebar::render_global_settings(runtime, render.sender)}
            {quota::render_sidebar(runtime)}
            {render_spice_topology(render, runtime)}
            {spice_sidebar::render_arrangement(runtime, &context)}
        </div>
    };
    let anchor = chrome::render_spice_button(chrome::SpiceButton {
        connection: render.connection,
        app_update: (**render.app_update).as_ref(),
        runtime,
        place: chrome::SpicePlace::Anchor,
        node_ref: render.sidebar_close.clone(),
        onclick: render.close_sidebar.reform(|_: MouseEvent| ()),
    });
    sidebar::Content {
        label: "Spice".into(),
        title: "Operator, resident & teams".into(),
        body,
        overlay: Html::default(),
        overlay_open: false,
        dismiss_overlay: None,
        anchor: Some(anchor),
    }
}

fn render_spice_topology(render: &AppRender<'_>, runtime: Option<&AppRuntime>) -> Html {
    let Some(lanes) = runtime.and_then(AppRuntime::lanes) else {
        return html! {
            <section
                class="app-sheet-section app-spice-topology"
                aria-labelledby="spice-resident-topology-heading"
                data-state="loading"
            >
                <div class="section-heading">
                    <h2 id="spice-resident-topology-heading">{"Resident topology"}</h2>
                    <span class="section-count">{"Waiting for agent state…"}</span>
                </div>
                <div class="app-spice-topology-placeholder" aria-hidden="true"></div>
            </section>
        };
    };
    let waiting = runtime
        .and_then(AppRuntime::inbox)
        .map(|inbox| inbox.lanes.as_slice())
        .unwrap_or(&[]);
    let tasks = runtime
        .and_then(AppRuntime::board)
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
    let totals = [
        topology_count(standings.len(), "agent", "agents"),
        topology_count(teams.len(), "team", "teams"),
        format!("{live} live"),
        topology_count(holding, "holding a task", "holding tasks"),
        format!("{pending} pending"),
    ]
    .join(" · ");
    html! {
        <section
            class="app-sheet-section app-spice-topology"
            aria-labelledby="spice-resident-topology-heading"
            data-state="ready"
        >
            <div class="section-heading">
                <h2 id="spice-resident-topology-heading">{"Resident topology"}</h2>
                <span class="section-count">{totals}</span>
            </div>
            if lanes.lanes.is_empty() {
                {empty("No agent lanes are connected.")}
            } else {
                <div class="app-spice-topology-groups">
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
    }
}

/// A team's display name, shared by the topology summary and team-addressed
/// pages.
fn team_name(team: &crate::runtime::LaneRow) -> String {
    let host = team.host().map_or("Empty", |member| {
        crate::watchband::agent_message_label(&member.worktree, member.name.as_deref())
    });
    format!("{host} team")
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
            class="app-spice-topology-group"
            data-topology-group={team.map(|team| team.to_string())}
            data-group-kind={if team.is_some() { "team" } else { "independent" }}
            data-span={agents.len().clamp(1, TOPOLOGY_COLUMNS).to_string()}
        >
            <h3 title={team.map(|team| format!("Group {team}"))}>
                {heading}
                <span>{topology_count(agents.len(), "agent", "agents")}</span>
            </h3>
            <div class="app-spice-topology-agent-list">
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
            class="app-spice-topology-agent"
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
            <p class="app-spice-topology-agent-task" title={standing.task_title.clone()}>{
                match (&standing.handle, &standing.phase) {
                    (Some(handle), Some(phase)) => format!("{handle}, {phase}"),
                    (Some(handle), None) => handle.clone(),
                    (None, _) => "\u{a0}".to_owned(),
                }
            }</p>
            <p>{crate::watchband::jobs_kept(&standing.jobs).unwrap_or_else(|| "\u{a0}".to_owned())}</p>
            <p class="app-spice-topology-agent-model">{crate::watchband::model_display(standing.model.as_deref())}</p>
            {super::watchband::render_provider_emblem(standing, "sidebar", None)}
        </@>
    }
}
