//! Predecessor-style lane composer rendered inside the Lanes surface.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{DragEvent, HtmlInputElement, HtmlSelectElement, PointerEvent};

use super::clock::{Age, Countdown};
use super::*;
use crate::composer_drafts::ComposerDraft;
use crate::watchband::{self, Standing, SubmissionAuthority, SubmissionTarget};

mod agent_menu;
mod attention;
mod command_preview;
mod composer;
mod controls;
mod feedback;
mod fleet;
mod global_jobs;
mod member_controls;
mod mosaic_view;
mod state;
mod team_drag;
mod team_menu;
mod vendor_icon;

pub(super) use command_preview::use_command_modifier_preview;
use composer::{
    AgentMessageCard, ComposerTextarea, drop_on_lane, keyboard_submit, paste_images,
    primary_composer_dragging, render_image_indicators, render_quote_shards, team_quote_sources,
};
pub(super) use composer::{
    PRIMARY_TEXTAREA_PREFIX, PrimaryComposerDragState, agent_message_source_id,
    primary_textarea_id, use_primary_composer_drag_state,
};
pub(super) use controls::{TeamControlAction, TeamControls, TeamSpeech};
use controls::{TeamLife, render_team_controls};
pub(in crate::browser) use fleet::use_team_menu_escape;
use fleet::{FleetRender, render_empty_fleet, render_populated_fleet};
pub(super) use global_jobs::render_global_jobs;
pub(in crate::browser) use mosaic_view::LIFECYCLE_CARD_PREFIX;
pub(crate) use mosaic_view::use_markdown_image_resolution_events;
use mosaic_view::{ActivityCard, CardLayout, HistorySender, MosaicPlane};
pub(in crate::browser) use state::{ComposerAction, ComposerDrafts, use_composer_issue_toast};
use team_drag::team_dragging;
pub(super) use team_drag::{TeamDragState, use_team_drag_state};
pub(super) use vendor_icon::render_composer_vendor_icon;

const MAX_ATTACHMENTS: usize = wire::media::MAX_MEDIA_ITEMS;
const MAX_ATTACHMENT_BYTES: f64 = wire::media::MAX_MEDIA_BYTES as f64;

#[derive(Clone, Copy)]
pub(super) struct BroadcastTarget {
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    epoch: CanonicalUuid,
}

/// The observer that toasts lane and team request progress and outcomes. It mounts
/// once at the app root so a request settles in the toast whichever
/// surface is showing.
pub(super) fn render_feedback(runtime: Option<&AppRuntime>, controls: &TeamControls) -> Html {
    html! { <feedback::RequestFeedback notices={feedback::Notices::of(runtime, controls)} /> }
}

/// Render every live team as one packed surface.
///
/// This is the sole Lanes conversation path: each team owns exactly one
/// composer and activity plane, however many independently steerable members
/// the team contains. Only a team earns a surface: an agent seated alone under
/// its own identity is not a formation, so it draws no surface here and stays
/// reachable through the global menu and each team's import list instead.
pub(super) fn render_team(
    render: &view::AppRender<'_>,
    work: Option<&crate::work::WorkProjection<'_>>,
) -> Html {
    let view::AppRender {
        connection,
        surface,
        sender,
        composer_drafts: drafts,
        submissions,
        team_fleet,
        team_controls,
        composer_drag,
        team_drag,
        select_task,
        open_message_task,
        navigate,
        agents: controls,
        ..
    } = render;
    let surface_state = (*surface).clone();
    let surface = surface.read();
    let Some(runtime) = surface.as_ref() else {
        return Html::default();
    };
    let Some(lanes) = runtime.lanes() else {
        return Html::default();
    };
    let waiting = runtime
        .inbox()
        .map(|inbox| inbox.lanes.as_slice())
        .unwrap_or(&[]);
    let tasks = runtime
        .board()
        .map(|board| board.tasks.as_slice())
        .unwrap_or(&[]);
    let mut standings = watchband::standings(&lanes.lanes, waiting, tasks);
    for standing in &mut standings {
        standing.pending = view::member_pending(waiting, &submissions.status, standing.lane);
        standing.waking = view::member_waking(waiting, &submissions.status, standing.lane);
    }
    let authority =
        SubmissionAuthority::derive(connection.is_online(), |kind| runtime.offers(kind));
    let can_configure = runtime.offers(CommandKind::TeamConfigure);
    let can_arrange_teams = runtime.offers(CommandKind::TeamArrange);
    let topology = Rc::new(watchband::topology(&lanes.lanes));
    let quote_agents = Rc::new(
        lanes
            .lanes
            .iter()
            .flat_map(|team| {
                let members = team.members.len();
                team.members
                    .iter()
                    .enumerate()
                    .map(move |(position, member)| {
                        (
                            (member.lane, member.occupant),
                            crate::composer_markdown::QuoteAgent {
                                accent: watchband::accent(members, position),
                                label: watchband::agent_message_label(
                                    &member.worktree,
                                    member.name.as_deref(),
                                )
                                .to_owned(),
                            },
                        )
                    })
            })
            .collect::<crate::composer_markdown::QuoteAgents>(),
    );
    let teams = lanes
        .lanes
        .iter()
        .filter(|team| team.teamed())
        .collect::<Vec<_>>();
    let render = FleetRender {
        work,
        navigate,
        select_task,
        open_message_task,
        runtime,
        epoch: lanes.epoch,
        topology_revision: lanes.topology_revision,
        authority,
        can_arrange_teams,
        can_configure,
        topology,
        quote_agents,
        surface: surface_state,
        sender,
        drafts,
        submissions,
        team_controls,
        controls,
        composer_drag,
        team_drag,
    };
    if teams.is_empty() {
        return render_empty_fleet(team_fleet, &render);
    }
    render_populated_fleet(&teams, &standings, team_fleet, &render)
}

#[hook]
pub(super) fn use_team_fleet_right_edge(host: NodeRef, team_count: usize) {
    use_effect_with((host, team_count), move |(host, _)| {
        let registration = host.cast::<web_sys::HtmlElement>().and_then(|element| {
            element.set_scroll_left(element.scroll_width());
            let observed = element.clone();
            let callback =
                Closure::<dyn FnMut(js_sys::Array, web_sys::ResizeObserver)>::new(move |_, _| {
                    observed.set_scroll_left(observed.scroll_width())
                });
            let observer = web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref()).ok()?;
            observer.observe(&element);
            Some((observer, callback))
        });
        move || {
            if let Some((observer, _callback)) = registration {
                observer.disconnect();
            }
        }
    });
}

#[allow(
    clippy::too_many_arguments,
    reason = "one team render owns its controls"
)]
fn render_compose(
    runtime: &AppRuntime,
    team: &crate::runtime::LaneRow,
    quote_agents: &Rc<crate::composer_markdown::QuoteAgents>,
    standings: &[Standing],
    epoch: CanonicalUuid,
    topology_revision: u64,
    topology: &[(CanonicalUuid, Vec<CanonicalUuid>)],
    authority: SubmissionAuthority,
    can_arrange: bool,
    sender: &SenderHandle,
    composer_drag: &PrimaryComposerDragState,
    drafts: &UseReducerHandle<ComposerDrafts>,
    submissions: &SubmissionHandle,
    team_controls: &UseReducerHandle<TeamControls>,
    can_configure: bool,
    controls: &view::AgentControls,
    surface: &UseStateHandle<SurfaceData>,
    select: &Callback<wire::TaskStamp>,
    open_message_task: &Callback<(wire::TaskStamp, web_sys::Element)>,
    attention: &attention::TeamAttention,
) -> Html {
    if team.members.is_empty() {
        return empty("No agents are assigned. Open Import agent to add one.");
    }
    let control = team_controls.current(team.lane, team.subscription);
    let submit = submit_team_callback(
        &team.members,
        epoch,
        authority,
        sender.clone(),
        drafts.clone(),
        submissions.clone(),
    );
    let broadcast_targets = broadcast_targets(&team.members, epoch, authority.send);
    let broadcast_help = format!("composer-broadcast-help-{}", team.lane);
    let ready = control.pending_life.is_none()
        && team.members.iter().any(|member| {
            authority
                .target(member.occupant, member.wakeable, epoch)
                .is_some()
        });
    let cards = activity_cards(runtime, team, quote_agents);
    let member_command = member_controls::command_callback(sender, surface, team.lane);
    let history_targets = team_history_targets(runtime, team);
    html! {
        <div class="team-compose" onclick={Callback::from(agent_menu::dismiss_others)}>
            <form class="team-composer" onsubmit={submit.clone()}>
                <div class="team-agents">
                    {for team.members.iter().zip(standings.iter()).map(|(member, standing)| {
                        let header = primary_composer_header(runtime, team, member.lane, standing);
                        render_agent_composer(
                            member,
                            standing,
                            epoch,
                            topology_revision,
                            topology,
                            standing.pending,
                            authority,
                            can_arrange,
                            agent_menu::render(agent_menu::AgentMenu {
                                runtime,
                                member,
                                standing,
                                select,
                                command: &member_command,
                                can_arrange,
                                can_configure,
                                topology_revision,
                                topology,
                                sender,
                                attention: attention.lane(member.lane),
                            }),
                            sender,
                            composer_drag,
                            drafts,
                            submissions,
                            &broadcast_targets,
                            &broadcast_help,
                            header.stream,
                            header.prose,
                        )
                    })}
                </div>
                {render_team_controls(
                    team.lane,
                    team.subscription,
                    topology_revision,
                    &control,
                    team_controls,
                    &controls.set_automatic_speech,
                    sender,
                    can_configure,
                    ready,
                )}
                <span id={broadcast_help} class="visually-hidden">
                    {"Control or Command plus Shift and Enter broadcasts this primary message and its direct images to every occupied agent in this team."}
                </span>
            </form>
            {render_activity(
                team,
                &cards,
                drafts,
                controls,
                open_message_task,
                &control,
                HistoryPaging {
                    sender,
                    targets: history_targets,
                    revision: runtime.message_history_revision(),
                },
            )}
        </div>
    }
}

fn history_target(
    runtime: &AppRuntime,
    lane: CanonicalUuid,
) -> Option<(CanonicalUuid, (wire::MessageHistoryCursor, u64))> {
    runtime
        .message_history_target(lane)
        .map(|cursor| (lane, (cursor, runtime.message_history_completion(lane))))
}

fn team_history_targets(
    runtime: &AppRuntime,
    team: &crate::runtime::LaneRow,
) -> BTreeMap<CanonicalUuid, (wire::MessageHistoryCursor, u64)> {
    team.members
        .iter()
        .filter_map(|member| history_target(runtime, member.lane))
        .collect()
}

fn broadcast_targets(
    members: &[crate::runtime::LaneMember],
    epoch: CanonicalUuid,
    can_send: bool,
) -> Vec<BroadcastTarget> {
    if !can_send {
        return Vec::new();
    }
    members
        .iter()
        .filter_map(|member| {
            member.occupant.map(|occupant| BroadcastTarget {
                lane: member.lane,
                occupant,
                epoch,
            })
        })
        .collect()
}

/// Every stable assistant-authored record the team can render, newest first.
/// Observation time is the primary display order. The admitted lane and source
/// locus break equal-time ties without parsing a rendered DOM identity. The
/// persistent plane uses this complete order to distinguish edge additions
/// from structural middle insertions during staged hydration.
fn activity_cards(
    runtime: &AppRuntime,
    team: &crate::runtime::LaneRow,
    quote_agents: &Rc<crate::composer_markdown::QuoteAgents>,
) -> Vec<ActivityCard> {
    // A card carries only the agents, sources, and tasks its quotes name:
    // unrelated roster or board changes cannot re-render and remeasure it.
    let unquoted = Rc::new(crate::composer_markdown::QuoteAgents::new());
    let quote_sources = team_quote_sources(runtime, team);
    let unquoted_sources = Rc::new(crate::composer_markdown::QuoteSources::new());
    let quote_tasks = runtime
        .board()
        .map(|board| Rc::clone(&board.quote_tasks))
        .unwrap_or_default();
    let unquoted_tasks = Rc::new(crate::runtime::QuoteTasks::new());
    let mut cards = runtime
        .messages()
        .iter()
        .filter_map(|message| {
            let (position, member) = team
                .members
                .iter()
                .enumerate()
                .find(|(_, member)| member.lane == message.lane)?;
            Some(ActivityCard::Agent {
                accent: watchband::accent(team.members.len(), position),
                member: member.clone(),
                quote_agents: if message.quotes.is_empty() && message.directive_contexts.is_empty()
                {
                    Rc::clone(&unquoted)
                } else {
                    Rc::new(crate::composer_markdown::referenced_quote_agents(
                        &message.quotes,
                        &message.directive_contexts,
                        quote_agents,
                    ))
                },
                quote_sources: if message.quotes.is_empty() && message.directive_contexts.is_empty()
                {
                    Rc::clone(&unquoted_sources)
                } else {
                    Rc::new(crate::composer_markdown::referenced_quote_sources(
                        &message.quotes,
                        &message.directive_contexts,
                        &quote_sources,
                    ))
                },
                quote_tasks: if message.quotes.is_empty() && message.directive_contexts.is_empty() {
                    Rc::clone(&unquoted_tasks)
                } else {
                    Rc::new(crate::composer_markdown::referenced_quote_tasks(
                        &message.quotes,
                        &message.directive_contexts,
                        &quote_tasks,
                    ))
                },
                quote_lane: crate::composer_drafts::quote_lane(&team.members, message),
                message: Rc::clone(message),
            })
        })
        .collect::<Vec<_>>();
    cards.extend(
        runtime
            .lifecycle_boundaries()
            .iter()
            .filter_map(|boundary| {
                let (position, member) = team
                    .members
                    .iter()
                    .enumerate()
                    .find(|(_, member)| member.lane == boundary.lane())?;
                Some(ActivityCard::Lifecycle {
                    accent: watchband::accent(team.members.len(), position),
                    member: member.clone(),
                    boundary: boundary.clone(),
                })
            }),
    );
    cards.sort_by_key(|card| Reverse(card.chronology_key()));
    cards
}

fn render_activity(
    team: &crate::runtime::LaneRow,
    cards: &[ActivityCard],
    drafts: &UseReducerHandle<ComposerDrafts>,
    controls: &view::AgentControls,
    open_message_task: &Callback<(wire::TaskStamp, web_sys::Element)>,
    team_control: &controls::TeamControl,
    history: HistoryPaging<'_>,
) -> Html {
    let empty = cards.is_empty();
    let narrate = controls.narrate.clone();
    let history_sender = HistorySender::new(history.sender.clone());
    html! {
        <div
            id={crate::view::team_stream_card_target(team.lane)}
            tabindex="-1"
            class="team-activity-grid"
            aria-live="polite"
            aria-label="Live agent activity"
            data-empty={empty.to_string()}
        >
            if empty {
                <div class="mosaic-message-empty">{"No pending direction."}</div>
            }
            <MosaicPlane
                key={team.lane.to_string()}
                lane={team.lane}
                life={team_control.life}
                members={team.members.len()}
                cards={cards.to_vec()}
                quote_dispatcher={drafts.dispatcher()}
                open_message_task={open_message_task.clone()}
                speak={controls.speak.clone()}
                {narrate}
                speech={team_control.speech}
                speech_message={(*controls.speech_message).clone()}
                history_targets={history.targets}
                history_revision={history.revision}
                history={history_sender}
            />
        </div>
    }
}

struct HistoryPaging<'a> {
    sender: &'a SenderHandle,
    targets: BTreeMap<CanonicalUuid, (wire::MessageHistoryCursor, u64)>,
    revision: u64,
}

/// One lease instant as a ticking age, or `vacant` for a lane holding no claim.
fn lease_age(instant: Option<i64>, class: &'static str, vacant: &'static str) -> Html {
    match instant {
        Some(authored) => html! { <Age {authored} class={class} /> },
        None => html! {
            <span
                class={class}
                data-age-slot={watchband::AGE_SLOT_WIDTH.to_string()}
            >{vacant}</span>
        },
    }
}

struct ComposerStreamLink {
    target: String,
    label: String,
    display_millis: Option<i64>,
}

struct ComposerHeader {
    stream: ComposerStreamLink,
    prose: String,
}

/// Resolve one human-facing message once, then derive every header field from it.
///
/// Tool previews, turn starts, and compaction boundaries remain useful activity
/// cards, but none is the assistant prose printed beside this age. Selecting
/// them here would make the clock describe a different event from the text it
/// labels. Only this lane's own prose is eligible: a lane without any, such as
/// a vacant member that has never run, shows the empty header rather than the
/// latest message another member of the team happened to produce.
fn primary_composer_header(
    runtime: &AppRuntime,
    team: &crate::runtime::LaneRow,
    lane: CanonicalUuid,
    standing: &Standing,
) -> ComposerHeader {
    runtime.latest_lane_prose(lane).map_or_else(
        || ComposerHeader {
            stream: ComposerStreamLink {
                target: crate::view::team_stream_card_target(team.lane),
                label: "Jump to team activity".to_owned(),
                display_millis: None,
            },
            prose: "No agent prose yet".to_owned(),
        },
        |(message, prose)| ComposerHeader {
            stream: ComposerStreamLink {
                target: agent_message_source_id(message),
                label: format!("Jump to latest {} message", standing.label),
                display_millis: Some(crate::runtime::agent_message_display_millis(message)),
            },
            prose,
        },
    )
}

fn copy_dragover() -> Callback<DragEvent> {
    Callback::from(move |event: DragEvent| {
        if let Some(transfer) = event.data_transfer() {
            transfer.set_drop_effect("copy");
        }
        event.prevent_default();
    })
}

struct PrimaryComposerDragging {
    enabled: bool,
    start: Callback<PointerEvent>,
    movement: Callback<PointerEvent>,
    finish: Callback<PointerEvent>,
    cancel: Callback<PointerEvent>,
    lost_capture: Callback<PointerEvent>,
}

struct AgentTeamControl {
    disabled: bool,
    activate: Callback<MouseEvent>,
}

fn agent_split_control(
    source_team: CanonicalUuid,
    lane: CanonicalUuid,
    can_arrange: bool,
    topology_revision: u64,
    topology: &[(CanonicalUuid, Vec<CanonicalUuid>)],
    sender: &SenderHandle,
) -> AgentTeamControl {
    let enabled = can_arrange
        && topology.iter().any(|(team, members)| {
            *team == source_team && members.len() > 1 && members.contains(&lane)
        });
    let topology = enabled.then(|| topology.to_vec());
    let sender = sender.clone();
    let activate = Callback::from(move |event: MouseEvent| {
        let Some(topology) = &topology else {
            return;
        };
        let Ok(new_team) = random_uuid() else {
            return;
        };
        let Some(teams) = watchband::split_member(topology, source_team, lane, new_team) else {
            return;
        };
        let teams = teams
            .into_iter()
            .map(|(team, lanes)| TeamArrangement { team, lanes })
            .collect();
        if admit_action(
            &sender,
            ClientAction::ArrangeTeams {
                topology_revision,
                teams,
            },
        ) {
            agent_menu::close(&event);
        }
    });
    AgentTeamControl {
        disabled: !enabled,
        activate,
    }
}

fn agent_leave_control(
    source_team: CanonicalUuid,
    lane: CanonicalUuid,
    can_configure: bool,
    topology_revision: u64,
    topology: &[(CanonicalUuid, Vec<CanonicalUuid>)],
    sender: &SenderHandle,
) -> AgentTeamControl {
    let enabled = can_configure && watchband::can_leave_all_teams(topology, source_team, lane);
    let sender = sender.clone();
    let activate = Callback::from(move |event: MouseEvent| {
        if enabled
            && admit_action(
                &sender,
                ClientAction::ConfigureTeam {
                    topology_revision,
                    team: lane,
                    member: Some(lane),
                    life: None,
                },
            )
        {
            agent_menu::close(&event);
        }
    });
    AgentTeamControl {
        disabled: !enabled,
        activate,
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "one member composer needs its standing and shared team interaction handles"
)]
fn render_agent_composer(
    member: &crate::runtime::LaneMember,
    standing: &Standing,
    epoch: CanonicalUuid,
    topology_revision: u64,
    topology: &[(CanonicalUuid, Vec<CanonicalUuid>)],
    pending: usize,
    authority: SubmissionAuthority,
    can_arrange: bool,
    menu: Html,
    sender: &SenderHandle,
    composer_drag: &PrimaryComposerDragState,
    drafts: &UseReducerHandle<ComposerDrafts>,
    submissions: &SubmissionHandle,
    broadcast_targets: &[BroadcastTarget],
    broadcast_help: &str,
    stream: ComposerStreamLink,
    prose: String,
) -> Html {
    let lane_id = member.lane;
    let lane_key = lane_id.to_string();
    let draft = drafts.get(&lane_key).cloned().unwrap_or_default();
    let target = authority.target(member.occupant, member.wakeable, epoch);
    let wake = target == Some(SubmissionTarget::Wake);
    let command_submit_eligible = target.is_some();
    let broadcast_target_eligible = broadcast_targets
        .iter()
        .any(|target| target.lane == lane_id);
    let broadcast_source_eligible = broadcast_target_eligible;
    let onkeydown = keyboard_submit(
        lane_id,
        target,
        lane_key.clone(),
        sender.clone(),
        drafts.clone(),
        submissions.clone(),
        broadcast_targets.to_vec(),
    );
    let ondragover = copy_dragover();
    let ondrop = drop_on_lane(
        lane_id,
        target,
        lane_key.clone(),
        sender.clone(),
        drafts.clone(),
    );
    let dragging = primary_composer_dragging(
        lane_id,
        can_arrange,
        topology_revision,
        topology,
        sender,
        composer_drag,
    );
    let status = member.status(standing.waking);
    html! {
        <section
            key={lane_key.clone()}
            class={classes!(
                "team-agent",
                member.occupant.is_none().then_some("team-agent--vacant"),
            )}
            data-agent-lane={lane_id.to_string()}
            data-wake-and-send={wake.to_string()}
            data-accent={standing.accent.to_string()}
            title={watchband::standing_title(standing)}
            {ondragover}
            {ondrop}
        >
            <div
                class="agent-composer-stack"
                data-agent-composer-stack={lane_id.to_string()}
                data-has-quote-shards={(!draft.quotes.is_empty()).to_string()}
            >
                {render_composer_body(
                    lane_id,
                    &lane_key,
                    &draft,
                    command_submit_eligible,
                    broadcast_target_eligible,
                    broadcast_source_eligible,
                    broadcast_help,
                    standing,
                    status,
                    pending,
                    drafts,
                    onkeydown.clone(),
                    menu,
                    stream,
                    prose,
                    dragging,
                )}
                {render_quote_shards(
                    &lane_key,
                    &draft,
                    status,
                    drafts,
                    onkeydown,
                    &composer_placeholder(standing, pending),
                )}
            </div>
        </section>
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the composer body renders one target's typed state and browser callbacks"
)]
fn render_composer_body(
    lane: CanonicalUuid,
    lane_key: &str,
    draft: &ComposerDraft,
    command_submit_eligible: bool,
    broadcast_target_eligible: bool,
    broadcast_source_eligible: bool,
    broadcast_help: &str,
    standing: &Standing,
    status: crate::runtime::LaneStatus,
    pending: usize,
    drafts: &UseReducerHandle<ComposerDrafts>,
    onkeydown: Callback<KeyboardEvent>,
    menu: Html,
    stream: ComposerStreamLink,
    prose: String,
    dragging: PrimaryComposerDragging,
) -> Html {
    let onpaste = paste_images(lane_key.to_owned(), None, drafts.clone());
    // Pointer capture stays on prose, leaving textarea editing and attachment
    // drag/drop untouched while the lifted composer can cross sibling slots.
    let PrimaryComposerDragging {
        enabled: drag_enabled,
        start: drag_start,
        movement: drag_movement,
        finish: drag_finish,
        cancel: drag_cancel,
        lost_capture: drag_lost_capture,
    } = dragging;
    html! {
        <div
            class="mosaic-composer composer-shard composer-shard--primary"
            data-shard-kind="primary"
            data-shard-target-id={lane.to_string()}
            data-primary-composer-draggable={drag_enabled.to_string()}
            data-command-submit-eligible={command_submit_eligible.to_string()}
            data-broadcast-target-eligible={broadcast_target_eligible.to_string()}
            data-broadcast-source-eligible={broadcast_source_eligible.to_string()}
            data-draft-unsafe={(
                !draft.body.is_empty()
                    || !draft.quotes.is_empty()
                    || !draft.attachments.is_empty()
            ).to_string()}
            onpointermove={drag_movement}
            onpointerup={drag_finish}
            onpointercancel={drag_cancel}
        >
            {render_composer_heading(
                status,
                menu,
                stream,
                prose,
                render_image_indicators(lane_key, &draft.attachments, None, drafts),
                drag_enabled,
                drag_start,
                drag_lost_capture,
            )}
            {render_composer_vendor_icon(standing, "composer")}
            <ComposerTextarea
                key={draft.body.clone()}
                id={primary_textarea_id(lane_key)}
                lane={lane_key.to_owned()}
                drafts={drafts.dispatcher()}
                label={format!("Message for agent {lane}")}
                body={draft.body.clone()}
                placeholder={composer_placeholder(standing, pending)}
                rows="4"
                primary_target={Some(lane.to_string())}
                quote_identity={None::<String>}
                broadcast_help={Some(broadcast_help.to_owned())}
                {onkeydown}
                {onpaste}
            />
        </div>
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the shard heading carries one agent's prose, drafts, menu, and drag callbacks"
)]
fn render_composer_heading(
    status: crate::runtime::LaneStatus,
    menu: Html,
    stream: ComposerStreamLink,
    prose: String,
    indicators: Html,
    drag_enabled: bool,
    drag_start: Callback<PointerEvent>,
    drag_lost_capture: Callback<PointerEvent>,
) -> Html {
    let ComposerStreamLink {
        target,
        label,
        display_millis,
    } = stream;
    html! {
        <header class="composer-shard-heading">
            <a
                class={classes!(
                    "composer-shard-link",
                    "composer-shard-age",
                    status.key(),
                )}
                href={crate::view::card_href(&target)}
                aria-label={label}
                data-lane-status={status.key()}
            >{lease_age(display_millis, "composer-shard-age-label", "—")}</a>
            <strong
                class="composer-shard-prose"
                title={prose.clone()}
                data-composer-drag-handle={drag_enabled.to_string()}
                onpointerdown={drag_start}
                onlostpointercapture={drag_lost_capture}
            >{prose}</strong>
            {indicators}
            {menu}
        </header>
    }
}

fn composer_placeholder(standing: &Standing, pending: usize) -> String {
    let branch = watchband::agent_message_label(&standing.worktree, None);
    let target = if standing.label == branch {
        standing.label.clone()
    } else {
        format!("{}, {branch}", standing.label)
    };
    let mut lines = vec![
        target,
        format!("{pending} pending, {}", standing.liveness.as_str()),
    ];
    if let Some(handle) = &standing.handle {
        lines.push(match &standing.phase {
            Some(phase) => format!("{handle}, {phase}"),
            None => handle.clone(),
        });
    }
    if let Some(kept) = watchband::jobs_kept(&standing.jobs) {
        lines.push(kept);
    }
    lines.join("\n")
}

pub(in crate::browser) fn current_composer_draft(
    lane_key: &str,
    drafts: &ComposerDrafts,
) -> ComposerDraft {
    let mut draft = drafts.get(lane_key).cloned().unwrap_or_default();
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return draft;
    };
    if let Some(textarea) = document
        .get_element_by_id(&primary_textarea_id(lane_key))
        .and_then(|element| element.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
    {
        draft.body = textarea.value();
    }
    for quote in &mut draft.quotes {
        if let Some(textarea) = document
            .get_element_by_id(&format!("composer-quote-{lane_key}-{}", quote.identity))
            .and_then(|element| element.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
        {
            quote.body = textarea.value();
        }
    }
    draft
}

fn clear_primary_composer(lane_key: &str) {
    if let Some(textarea) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id(&primary_textarea_id(lane_key)))
        .and_then(|element| element.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
    {
        textarea.set_value("");
    }
}

fn submit_team_callback(
    members: &[crate::runtime::LaneMember],
    epoch: CanonicalUuid,
    authority: SubmissionAuthority,
    sender: SenderHandle,
    drafts: UseReducerHandle<ComposerDrafts>,
    submissions: SubmissionHandle,
) -> Callback<SubmitEvent> {
    let targets = members
        .iter()
        .map(|member| {
            let target = authority.target(member.occupant, member.wakeable, epoch);
            (member.lane, target, member.lane.to_string())
        })
        .collect::<Vec<_>>();
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        let Ok(fanout) = random_uuid() else {
            return;
        };
        for (lane, target, lane_key) in &targets {
            submit_draft(
                fanout,
                *lane,
                *target,
                lane_key,
                &sender,
                &drafts,
                &submissions,
            );
        }
    })
}

fn submit_draft(
    fanout: CanonicalUuid,
    lane: CanonicalUuid,
    target: Option<SubmissionTarget>,
    lane_key: &str,
    sender: &SenderHandle,
    drafts: &UseReducerHandle<ComposerDrafts>,
    submissions: &SubmissionHandle,
) -> bool {
    let mut current = drafts.0.clone();
    *current.draft_mut(lane_key.to_owned()) = current_composer_draft(lane_key, drafts);
    let Some(prepared) = current.prepare(lane_key) else {
        return false;
    };
    let Ok(id) = random_uuid() else {
        return false;
    };
    let authored_millis = js_sys::Date::now() as i64;
    let submission = match target {
        Some(SubmissionTarget::Resident { occupant, epoch }) => Submission::new(
            id,
            fanout,
            lane,
            (occupant, epoch, authored_millis),
            prepared.message,
            prepared.attachments,
            prepared.snapshot,
        ),
        Some(SubmissionTarget::Wake) => Submission::wake(
            id,
            fanout,
            lane,
            authored_millis,
            prepared.message,
            prepared.attachments,
            prepared.snapshot,
        ),
        None => return false,
    };
    if !queue_submission(sender, submissions, submission) {
        return false;
    }
    clear_primary_composer(lane_key);
    drafts.dispatch(ComposerAction::Clear {
        lane: lane_key.to_owned(),
    });
    true
}

fn submit_broadcast(
    source_lane: CanonicalUuid,
    source_key: &str,
    targets: &[BroadcastTarget],
    sender: &SenderHandle,
    drafts: &UseReducerHandle<ComposerDrafts>,
    submissions: &SubmissionHandle,
) -> bool {
    if !targets.iter().any(|target| target.lane == source_lane) {
        return false;
    }
    let mut current = drafts.0.clone();
    *current.draft_mut(source_key.to_owned()) = current_composer_draft(source_key, drafts);
    let Some(prepared) = current.prepare_primary(source_key) else {
        return false;
    };
    let message = crate::composer_markdown::mark_broadcast(&prepared.message);
    let Ok(fanout) = random_uuid() else {
        return false;
    };
    let authored_millis = js_sys::Date::now() as i64;
    let payload = Submission::payload(fanout, authored_millis, message, prepared.attachments);
    let mut snapshot = Some(prepared.snapshot);
    let mut batch = Vec::with_capacity(targets.len());
    for target in targets {
        let Ok(id) = random_uuid() else {
            return false;
        };
        let residency = (target.occupant, target.epoch);
        let snapshot = if target.lane == source_lane {
            snapshot.take().unwrap_or_default()
        } else {
            ComposerDraft::default()
        };
        let submission = Submission::broadcast(
            id,
            fanout,
            target.lane,
            residency,
            Rc::clone(&payload),
            snapshot,
            source_lane,
        );
        batch.push(submission);
    }
    if !queue_submissions(sender, submissions, batch) {
        return false;
    }
    clear_primary_composer(source_key);
    drafts.dispatch(ComposerAction::ClearPrimary {
        lane: source_key.to_owned(),
    });
    true
}
