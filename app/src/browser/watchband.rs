//! Predecessor-style lane composer rendered inside the Lanes surface.

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{DragEvent, HtmlInputElement};

use super::clock::{Age, Countdown};
use super::*;
use crate::composer_drafts::{ComposerDraft, compose};
use crate::watchband::{self, Standing, TeamTaskSectionState};

mod command_preview;
mod composer;
mod controls;
mod fleet;
mod mosaic_view;
mod state;
mod team_menu;
mod vendor_icon;

pub(super) use command_preview::use_command_modifier_preview;
use composer::{
    AgentMessageCard, agent_message_source_id, drop_on_lane, keyboard_submit, paste_images,
    primary_textarea_id, render_image_indicators, render_quote_shards, update_body,
};
pub(super) use controls::{TeamControlAction, TeamControls, TeamSpeech};
use controls::{TeamLife, render_team_controls};
use fleet::{FleetRender, TeamMode, carries_team, render_empty_fleet, render_populated_fleet};
use mosaic_view::{ActivityCard, CardLayout, HistorySender, MosaicPlane};
pub(in crate::browser) use state::{ComposerAction, ComposerDrafts};
use vendor_icon::render_composer_vendor_icon;

const MAX_ATTACHMENTS: usize = wire::media::MAX_MEDIA_ITEMS;
const MAX_ATTACHMENT_BYTES: f64 = wire::media::MAX_MEDIA_BYTES as f64;
const LANE_DRAG_MIME: &str = "application/x-spice-lane";

/// Authority a detached draft may use for its next transition.
#[derive(Clone, Copy)]
enum SubmissionTarget {
    Resident {
        occupant: CanonicalUuid,
        epoch: CanonicalUuid,
    },
    Wake,
}

#[derive(Clone, Copy)]
pub(super) struct BroadcastTarget {
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    epoch: CanonicalUuid,
}

/// Render every live team as one packed surface.
///
/// This is the sole Lanes conversation path: each team owns exactly one
/// composer and activity plane, however many independently steerable members
/// the team contains.
pub(super) fn render_team(
    surface: &SurfaceData,
    sender: &SenderHandle,
    drafts: &UseReducerHandle<ComposerDrafts>,
    submissions: &SubmissionHandle,
    team_fleet: &NodeRef,
    team_controls: &UseReducerHandle<TeamControls>,
    controls: &view::AgentControls,
) -> Html {
    let surface = surface.read();
    let Some(runtime) = surface.as_ref() else {
        return empty("No admitted live team is available.");
    };
    let Some(lanes) = runtime.lanes() else {
        return empty("No agent snapshot is available.");
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
        let backend = waiting
            .iter()
            .find(|row| row.lane == standing.lane)
            .map(|row| row.pending.as_slice())
            .unwrap_or(&[]);
        standing.pending = submissions.status.pending(standing.lane, backend);
    }
    let can_send = runtime.offers(CommandKind::SteerSend);
    let can_wake = runtime.offers(CommandKind::LaneState) && runtime.offers(CommandKind::LaneRead);
    let can_configure = runtime.offers(CommandKind::TeamConfigure);
    let can_arrange_lanes = runtime.offers(CommandKind::LaneArrange);
    let can_arrange_teams = runtime.offers(CommandKind::TeamArrange);
    let topology = watchband::topology(&lanes.lanes);
    if lanes.lanes.is_empty() {
        return render_empty_fleet(runtime, lanes.epoch, team_fleet, team_controls, controls);
    }
    let render = FleetRender {
        runtime,
        epoch: lanes.epoch,
        topology_revision: lanes.topology_revision,
        can_send,
        can_wake,
        can_arrange_lanes,
        can_arrange_teams,
        can_configure,
        topology,
        sender,
        drafts,
        submissions,
        team_controls,
        controls,
    };
    render_populated_fleet(&lanes.lanes, &standings, team_fleet, &render)
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
    runtime: &CanvasRuntime,
    team: &crate::runtime::LaneRow,
    standings: &[Standing],
    epoch: CanonicalUuid,
    topology_revision: u64,
    can_send: bool,
    can_wake: bool,
    can_arrange: bool,
    sender: &SenderHandle,
    drafts: &UseReducerHandle<ComposerDrafts>,
    submissions: &SubmissionHandle,
    team_controls: &UseReducerHandle<TeamControls>,
    can_configure: bool,
    controls: &view::AgentControls,
) -> Html {
    if team.members.is_empty() {
        return empty("No agents are assigned. Open Team to add one.");
    }
    let authoritative_life = team.subscription;
    let control = team_controls.current(team.lane, authoritative_life);
    let submit = submit_team_callback(
        &team.members,
        epoch,
        can_wake,
        sender.clone(),
        drafts.clone(),
        submissions.clone(),
    );
    let broadcast_targets = broadcast_targets(&team.members, epoch, can_send);
    let broadcast_help = format!("composer-broadcast-help-{}", team.lane);
    let ready = can_send
        && control.pending_life.is_none()
        && team.members.iter().any(|member| {
            (member.occupant.is_some() || (member.bound && can_wake))
                && drafts
                    .get(&member.lane.to_string())
                    .is_some_and(|draft| !compose(draft).trim().is_empty())
        });
    let cards = activity_cards(runtime, team);
    let dismiss_agent_menus = Callback::from(dismiss_other_agent_action_menus);
    let history_targets = team
        .members
        .iter()
        .filter_map(|member| history_target(runtime, member.lane))
        .collect();
    html! {
        <div class="team-compose" onclick={dismiss_agent_menus}>
            <form class="team-composer" onsubmit={submit.clone()}>
                <div class="team-agents">
                    {for team.members.iter().zip(standings.iter()).map(|(member, standing)| {
                        let header = primary_composer_header(
                            &cards,
                            team,
                            member.lane,
                            standing,
                        );
                        render_agent_composer(
                            team.lane,
                            member,
                            standing,
                            epoch,
                            topology_revision,
                            runtime
                                .lanes()
                                .and_then(|lanes| lanes.renewal(team.lane, member.lane)),
                            standing.pending,
                            can_send,
                            can_wake,
                            can_arrange,
                            sender,
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
                    authoritative_life,
                    topology_revision,
                    &control,
                    team_controls,
                    &controls.suppress_automatic_speech,
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
                control.speech,
                control.life,
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
    runtime: &CanvasRuntime,
    lane: CanonicalUuid,
) -> Option<(CanonicalUuid, (Option<wire::MessageLocus>, u64))> {
    runtime.message_history_target(lane).map(|cursor| {
        (
            lane,
            (cursor.before, runtime.message_history_completion(lane)),
        )
    })
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

/// Every stable assistant-authored record the team can render in total chronology.
/// Observation time is the primary display order. The admitted lane and source
/// locus break equal-time ties without parsing a rendered DOM identity. The
/// persistent plane uses this complete order to distinguish edge additions
/// from structural middle insertions during staged hydration.
fn activity_cards(runtime: &CanvasRuntime, team: &crate::runtime::LaneRow) -> Vec<ActivityCard> {
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
    cards.sort_by_key(ActivityCard::chronology_key);
    cards
}

fn render_activity(
    team: &crate::runtime::LaneRow,
    cards: &[ActivityCard],
    drafts: &UseReducerHandle<ComposerDrafts>,
    controls: &view::AgentControls,
    speech: TeamSpeech,
    life: TeamLife,
    history: HistoryPaging<'_>,
) -> Html {
    let empty = cards.is_empty();
    let narrate = controls.narrate.clone();
    let history_sender = HistorySender::new(history.sender.clone());
    html! {
        <div
            id={format!("team-stream-{}", team.lane)}
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
                {life}
                members={team.members.len()}
                cards={cards.to_vec()}
                quote_dispatcher={drafts.dispatcher()}
                speak={controls.speak.clone()}
                {narrate}
                {speech}
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
    targets: BTreeMap<CanonicalUuid, (Option<wire::MessageLocus>, u64)>,
    revision: u64,
}

fn render_details(
    runtime: &CanvasRuntime,
    team: &crate::runtime::LaneRow,
    standings: &[Standing],
    mode: TeamMode,
    controls: &view::AgentControls,
) -> Html {
    html! {
        <div class={classes!(
            "team-detail-pane",
            (mode == TeamMode::Team).then_some("with-seat")
        )}>
            <div class="team-detail-grid">
                {for team.members.iter().zip(standings.iter()).map(|(member, standing)| {
                    render_agent_details(member, standing, mode)
                })}
            </div>
            if mode == TeamMode::Team {
                {view::render_seat_panel(Some(runtime), controls)}
            }
        </div>
    }
}

fn render_tasks(
    runtime: &CanvasRuntime,
    team: &crate::runtime::LaneRow,
    standings: &[Standing],
) -> Html {
    let active = standings
        .iter()
        .filter(|standing| standing.handle.is_some())
        .collect::<Vec<_>>();
    let queued = standings
        .iter()
        .filter(|standing| standing.pending > 0)
        .collect::<Vec<_>>();
    let active_state = TeamTaskSectionState::active(runtime.board().is_some(), active.len());
    let direction_state = TeamTaskSectionState::direction(runtime.inbox().is_some(), queued.len());
    let policy = TeamLife::from(team.subscription).label();
    let active_summary = runtime.board().map_or_else(
        || "Task state unavailable".to_owned(),
        |_| format!("{} active", active.len()),
    );
    html! {
        <main class="team-task-workspace" aria-label="Team tasks">
            <header class="team-task-summary">
                <div>
                    <span class="team-task-eyebrow">{"Team work"}</span>
                    <strong>{active_summary}</strong>
                </div>
                <div
                    class="team-task-policy"
                    data-task-policy-extension=""
                    aria-label={format!("Task policy: {policy}")}
                >
                    <span>{"Task policy"}</span>
                    <strong>{policy}</strong>
                </div>
            </header>
            <section class="team-task-section" aria-labelledby={format!("active-tasks-{}", team.lane)}>
                <header>
                    <h3 id={format!("active-tasks-{}", team.lane)}>{"Active tasks"}</h3>
                    if !active.is_empty() {
                        <span>{active.len()}</span>
                    }
                </header>
                if let Some(message) = active_state.message() {
                    <p class="team-task-empty" role={active_state.unavailable().then_some("status")}>{message}</p>
                } else {
                    <ol class="team-task-list">
                        {for active.into_iter().map(render_task_row)}
                    </ol>
                }
            </section>
            <section class="team-task-section" aria-labelledby={format!("direction-queue-{}", team.lane)}>
                <header>
                    <h3 id={format!("direction-queue-{}", team.lane)}>{"Direction queue"}</h3>
                    if !queued.is_empty() {
                        <span>{queued.iter().map(|standing| standing.pending).sum::<usize>()}</span>
                    }
                </header>
                if let Some(message) = direction_state.message() {
                    <p class="team-task-empty" role={direction_state.unavailable().then_some("status")}>{message}</p>
                } else {
                    <ol class="team-direction-list">
                        {for queued.into_iter().map(render_direction_row)}
                    </ol>
                }
            </section>
        </main>
    }
}

fn render_task_row(standing: &Standing) -> Html {
    let handle = standing.handle.clone().unwrap_or_default();
    let title = standing
        .task_title
        .clone()
        .unwrap_or_else(|| "Task title unavailable".to_owned());
    let phase = standing
        .phase
        .clone()
        .unwrap_or_else(|| "unknown".to_owned());
    let expiry = lease_countdown(standing.expires_at, "team-task-age", "unavailable");
    html! {
        <li
            class="team-task-row"
            data-task-handle={handle.clone()}
            data-agent-lane={standing.lane.to_string()}
            data-accent={standing.accent.to_string()}
        >
            <div class="team-task-row-main">
                <code>{handle}</code>
                <strong class="team-task-title">{title}</strong>
            </div>
            <div class="team-task-row-meta">
                <span class="team-task-member">
                    <span class="agent-presence" aria-hidden="true"></span>
                    <span class="team-task-member-name">{standing.label.clone()}</span>
                </span>
                <span class="team-task-phase">{phase}</span>
                <span class="team-task-renewed">{"expires "}{expiry}</span>
            </div>
        </li>
    }
}

fn render_direction_row(standing: &Standing) -> Html {
    let preview = standing
        .headline
        .clone()
        .unwrap_or_else(|| "Preview unavailable".to_owned());
    html! {
        <li
            class="team-direction-row"
            data-agent-lane={standing.lane.to_string()}
            data-accent={standing.accent.to_string()}
        >
            <span class="team-task-member">
                <span class="agent-presence" aria-hidden="true"></span>
                <strong class="team-task-member-name">{standing.label.clone()}</strong>
            </span>
            <span class="team-direction-preview">{preview}</span>
            <span class="team-direction-count">{format!("{} pending", standing.pending)}</span>
        </li>
    }
}

/// One lease instant as a ticking age, or `vacant` for a lane holding no claim.
fn lease_age(instant: Option<i64>, class: &'static str, vacant: &'static str) -> Html {
    match instant {
        Some(authored) => html! { <Age {authored} class={class} /> },
        None => html! {
            <span
                class={class}
                data-age-slot={watchband::AGE_LABEL_WIDTH.to_string()}
            >{vacant}</span>
        },
    }
}

/// One authoritative lease expiry as a ticking countdown.
fn lease_countdown(expiry: Option<i64>, class: &'static str, vacant: &'static str) -> Html {
    match expiry {
        Some(expiry) => html! { <Countdown {expiry} class={class} /> },
        None => html! {
            <span
                class={class}
                data-age-slot={watchband::AGE_LABEL_WIDTH.to_string()}
                data-age-clock="countdown"
            >{vacant}</span>
        },
    }
}

fn render_agent_details(
    member: &crate::runtime::LaneMember,
    standing: &Standing,
    mode: TeamMode,
) -> Html {
    let occupant = member
        .occupant
        .map(|value| value.to_string())
        .unwrap_or_else(|| "vacant".to_owned());
    html! {
        <article
            class="team-detail-card"
            data-accent={standing.accent.to_string()}
            title={watchband::standing_title(standing)}
        >
            <header>
                <span class="agent-presence"></span>
                <strong>{standing.label.clone()}</strong>
                <span>{standing.vendor.clone()}</span>
            </header>
            {match mode {
                TeamMode::Team => html! {
                    <dl>
                        <div><dt>{"Agent channel"}</dt><dd>{standing.lane.to_string()}</dd></div>
                        <div><dt>{"Occupant"}</dt><dd>{occupant}</dd></div>
                        <div><dt>{"Worktree"}</dt><dd>{standing.worktree.clone()}</dd></div>
                        <div>
                            <dt>{"Pending"}</dt>
                            <dd>{format!("{} pending", standing.pending)}</dd>
                        </div>
                        <div>
                            <dt>{"Liveness"}</dt>
                            <dd>{standing.liveness.as_str()}</dd>
                        </div>
                        {for standing.jobs.iter().map(|job| html! {
                            <div>
                                <dt>{format!("Job {}", job.id)}</dt>
                                <dd>{format!("{} · {}", job.standing.as_str(), job.command)}</dd>
                            </div>
                        })}
                        <div>
                            <dt>{"Vendor / residency policy"}</dt>
                            <dd>{format!("{} / {}", standing.vendor, standing.life)}</dd>
                        </div>
                    </dl>
                },
                TeamMode::Lanes | TeamMode::Tasks => Html::default(),
            }}
        </article>
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
    cards: &[ActivityCard],
    team: &crate::runtime::LaneRow,
    lane: CanonicalUuid,
    standing: &Standing,
) -> ComposerHeader {
    let messages = cards.iter().filter_map(|card| match card {
        ActivityCard::Agent { message, .. } => Some(message.as_ref()),
        ActivityCard::Lifecycle { .. } => None,
    });
    watchband::latest_lane_prose(lane, messages).map_or_else(
        || ComposerHeader {
            stream: ComposerStreamLink {
                target: format!("team-stream-{}", team.lane),
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

fn renewal_callback(
    topology_revision: u64,
    team: CanonicalUuid,
    lane: CanonicalUuid,
    requested: bool,
    sender: &SenderHandle,
) -> Callback<MouseEvent> {
    let sender = sender.clone();
    Callback::from(move |event: MouseEvent| {
        if admit_action(
            &sender,
            ClientAction::RenewLane {
                topology_revision,
                team,
                lane,
                requested,
            },
        ) {
            event.stop_propagation();
        }
    })
}

fn copy_dragover() -> Callback<DragEvent> {
    Callback::from(move |event: DragEvent| {
        if let Some(transfer) = event.data_transfer() {
            // A team in flight is passing over on its way to a team surface;
            // claiming it here would drop a whole team onto one composer.
            if carries_team(&transfer) {
                return;
            }
            transfer.set_drop_effect("copy");
        }
        event.prevent_default();
    })
}

struct PrimaryComposerDragging {
    enabled: bool,
    start: Callback<DragEvent>,
    over: Callback<DragEvent>,
    drop: Callback<DragEvent>,
}

fn lane_drag_transfer(event: &DragEvent) -> Option<web_sys::DataTransfer> {
    let transfer = event.data_transfer()?;
    transfer
        .types()
        .includes(&JsValue::from_str(LANE_DRAG_MIME), 0)
        .then_some(transfer)
}

fn composer_drag_origin_is_interactive(event: &DragEvent) -> bool {
    event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        .and_then(|target| target.closest("textarea, input, button, summary, a").ok())
        .flatten()
        .is_some()
}

fn primary_composer_dragging(
    lane: CanonicalUuid,
    enabled: bool,
    topology_revision: u64,
    sender: &SenderHandle,
) -> PrimaryComposerDragging {
    let start = Callback::from(move |event: DragEvent| {
        if composer_drag_origin_is_interactive(&event) {
            event.prevent_default();
            event.stop_propagation();
            return;
        }
        event.stop_propagation();
        if let Some(transfer) = event.data_transfer() {
            let _ = transfer.set_data(LANE_DRAG_MIME, &lane.to_string());
            transfer.set_effect_allowed("move");
        }
    });
    let over = Callback::from(move |event: DragEvent| {
        let Some(transfer) = lane_drag_transfer(&event) else {
            return;
        };
        event.prevent_default();
        event.stop_propagation();
        transfer.set_drop_effect("move");
    });
    let sender = sender.clone();
    let drop = Callback::from(move |event: DragEvent| {
        let Some(transfer) = lane_drag_transfer(&event) else {
            return;
        };
        event.prevent_default();
        event.stop_propagation();
        let source = transfer
            .get_data(LANE_DRAG_MIME)
            .ok()
            .and_then(|source| source.parse::<CanonicalUuid>().ok());
        let Some(source) = source else {
            return;
        };
        if enabled
            && source != lane
            && !admit_action(
                &sender,
                ClientAction::ArrangeLane {
                    topology_revision,
                    lane: source,
                    before: lane,
                },
            )
        {
            transfer.set_drop_effect("none");
        }
    });
    PrimaryComposerDragging {
        enabled,
        start,
        over,
        drop,
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "one member composer needs its standing and shared team interaction handles"
)]
fn render_agent_composer(
    team: CanonicalUuid,
    member: &crate::runtime::LaneMember,
    standing: &Standing,
    epoch: CanonicalUuid,
    topology_revision: u64,
    renewal: Option<&wire::LaneRenewalRecord>,
    pending: usize,
    can_send: bool,
    can_wake: bool,
    can_arrange: bool,
    sender: &SenderHandle,
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
    let occupant = member.occupant;
    let wake = can_send && can_wake && member.bound && occupant.is_none();
    let target = occupant
        .filter(|_| can_send)
        .map(|occupant| SubmissionTarget::Resident { occupant, epoch })
        .or_else(|| wake.then_some(SubmissionTarget::Wake));
    let command_submit_eligible = target.is_some() && !draft.body.trim().is_empty();
    let broadcast_target_eligible = broadcast_targets
        .iter()
        .any(|target| target.lane == lane_id);
    let broadcast_source_eligible = broadcast_target_eligible && !draft.body.trim().is_empty();
    let renewal =
        watchband::agent_renewal_control(&standing.label, occupant.is_some(), can_send, renewal);
    let renew = renewal_callback(
        topology_revision,
        team,
        lane_id,
        renewal.next_requested,
        sender,
    );
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
        occupant,
        epoch,
        lane_key.clone(),
        sender.clone(),
        drafts.clone(),
    );
    let dragging = primary_composer_dragging(lane_id, can_arrange, topology_revision, sender);
    let status = member.status(standing.pending);
    html! {
        <section
            key={lane_key.clone()}
            class={classes!(
                "team-agent",
                occupant.is_none().then_some("team-agent--vacant"),
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
                    renewal,
                    renew,
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
    renewal: watchband::AgentRenewalControl,
    renew: Callback<MouseEvent>,
    stream: ComposerStreamLink,
    prose: String,
    dragging: PrimaryComposerDragging,
) -> Html {
    let oninput = update_body(lane_key.to_owned(), drafts.clone());
    let onpaste = paste_images(lane_key.to_owned(), None, drafts.clone());
    // Firefox gives draggable ancestors ownership of textarea pointer gestures.
    // Keep only prose draggable; the composer remains the lane and draft target.
    let PrimaryComposerDragging {
        enabled: drag_enabled,
        start: drag_start,
        over: drag_over,
        drop: drag_drop,
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
            ondragover={drag_over}
            ondrop={drag_drop}
        >
            {render_composer_heading(
                lane,
                standing,
                status,
                renewal,
                renew,
                stream,
                prose,
                render_image_indicators(lane_key, &draft.attachments, None, drafts),
                drag_enabled,
                drag_start,
            )}
            <textarea
                id={primary_textarea_id(lane_key)}
                aria-label={format!("Message for agent {lane}")}
                data-primary-composer-target-id={lane.to_string()}
                aria-keyshortcuts="Control+Shift+Enter Meta+Shift+Enter"
                aria-describedby={broadcast_help.to_owned()}
                rows="4"
                value={draft.body.clone()}
                placeholder={composer_placeholder(standing, pending)}
                {oninput}
                {onkeydown}
                {onpaste}
            ></textarea>
            {render_composer_vendor_icon(standing)}
            if let Some(issue) = &draft.issue {
                <p class="composer-issue" role="status">{issue.clone()}</p>
            }
        </div>
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the shard heading carries one agent's identity, prose, drafts, and drawer callbacks"
)]
fn render_composer_heading(
    lane: CanonicalUuid,
    standing: &Standing,
    status: crate::runtime::LaneStatus,
    renewal: watchband::AgentRenewalControl,
    renew: Callback<MouseEvent>,
    stream: ComposerStreamLink,
    prose: String,
    indicators: Html,
    drag_enabled: bool,
    drag_start: Callback<DragEvent>,
) -> Html {
    let renew = Callback::from(move |event: MouseEvent| {
        close_agent_action_menu(&event);
        renew.emit(event);
    });
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
                href={format!("#{target}")}
                aria-label={label}
                data-lane-status={status.key()}
            >{lease_age(display_millis, "composer-shard-age-label", "—")}</a>
            <strong
                class="composer-shard-prose"
                title={prose.clone()}
                data-composer-drag-handle={drag_enabled.to_string()}
                draggable={drag_enabled.to_string()}
                ondragstart={drag_start}
            >{prose}</strong>
            {indicators}
            <details class="agent-action-menu">
                <summary aria-label={format!("Actions for {}", standing.label)}>
                    <span class="composer-menu-icon" aria-hidden="true">
                        <span></span><span></span><span></span>
                    </span>
                </summary>
                <div
                    class="agent-action-drawer"
                    data-agent-drawer-target-id={lane.to_string()}
                >
                    <button
                        class="agent-renew-request"
                        type="button"
                        disabled={renewal.disabled}
                        aria-label={renewal.label.clone()}
                        aria-pressed={renewal.pressed.to_string()}
                        title={renewal.title.clone()}
                        data-renewal-state={renewal.state}
                        data-renewal-team={standing.team.to_string()}
                        data-renewal-lane={lane.to_string()}
                        data-renewal-requested={renewal.pressed.to_string()}
                        data-renewal-next-requested={renewal.next_requested.to_string()}
                        data-renewal-refusal={renewal.refusal.clone()}
                        data-renewal-blocked={renewal.blocked}
                        onclick={renew}
                    >
                        <strong>{renewal.heading}</strong>
                        <span>{renewal.detail}</span>
                        if let Some(blocked) = renewal.blocked {
                            <span class="agent-renew-blocked">{blocked}</span>
                        }
                    </button>
                </div>
            </details>
        </header>
    }
}

fn close_agent_action_menu(event: &MouseEvent) {
    // Yew delegates listeners from the app root, so `current_target` cannot
    // locate the details element. The actual click target remains inside it.
    if let Some(menu) = event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        .and_then(|target| target.closest(".agent-action-menu").ok().flatten())
    {
        let _removed = menu.remove_attribute("open");
    }
}

fn dismiss_other_agent_action_menus(event: MouseEvent) {
    let keep = event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        .and_then(|target| target.closest(".agent-action-menu").ok().flatten());
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    let Ok(menus) = document.query_selector_all(".agent-action-menu[open]") else {
        return;
    };
    for index in 0..menus.length() {
        let Some(menu) = menus
            .get(index)
            .and_then(|node| node.dyn_into::<web_sys::Element>().ok())
        else {
            continue;
        };
        if keep.as_ref() == Some(&menu) {
            continue;
        }
        let _removed = menu.remove_attribute("open");
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
    if let Some(kept) = watchband::jobs_kept(&standing.jobs) {
        lines.push(kept);
    }
    if let Some(handle) = &standing.handle {
        lines.push(match &standing.phase {
            Some(phase) => format!("{handle}, {phase}"),
            None => handle.clone(),
        });
    }
    lines.join("\n")
}

fn submit_team_callback(
    members: &[crate::runtime::LaneMember],
    epoch: CanonicalUuid,
    can_wake: bool,
    sender: SenderHandle,
    drafts: UseReducerHandle<ComposerDrafts>,
    submissions: SubmissionHandle,
) -> Callback<SubmitEvent> {
    let targets = members
        .iter()
        .map(|member| {
            let target = member
                .occupant
                .map(|occupant| SubmissionTarget::Resident { occupant, epoch })
                .or_else(|| (can_wake && member.bound).then_some(SubmissionTarget::Wake));
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
    let Some(prepared) = drafts.0.prepare(lane_key) else {
        return false;
    };
    let Ok(id) = random_uuid() else {
        return false;
    };
    let submission = match target {
        Some(SubmissionTarget::Resident { occupant, epoch }) => Submission::new(
            id,
            fanout,
            lane,
            (occupant, epoch),
            prepared.message,
            prepared.attachments,
            prepared.snapshot,
        ),
        Some(SubmissionTarget::Wake) => Submission::wake(
            id,
            fanout,
            lane,
            prepared.message,
            prepared.attachments,
            prepared.snapshot,
        ),
        None => return false,
    };
    if !queue_submission(sender, submissions, submission) {
        return false;
    }
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
    let Some(prepared) = drafts.0.prepare_primary(source_key) else {
        return false;
    };
    let message = crate::composer_markdown::mark_broadcast(&prepared.message);
    let Ok(fanout) = random_uuid() else {
        return false;
    };
    let payload = Submission::payload(fanout, message, prepared.attachments);
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
    drafts.dispatch(ComposerAction::ClearPrimary {
        lane: source_key.to_owned(),
    });
    true
}
