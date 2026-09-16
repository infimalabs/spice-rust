//! Predecessor-style lane composer rendered inside the Lanes surface.

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{DragEvent, HtmlInputElement};

use super::clock::{Age, Countdown};
use super::*;
use crate::composer_drafts::{ComposerDraft, compose};
use crate::watchband::{self, OrgTaskSectionState, Standing};

mod command_preview;
mod composer;
mod controls;
mod fleet;
mod lineage_menu;
mod mosaic_view;
mod state;

pub(super) use command_preview::use_command_modifier_preview;
use composer::{
    AgentMessageCard, agent_message_source_id, drop_on_lane, keyboard_submit, paste_images,
    primary_textarea_id, render_image_indicators, render_quote_shards, update_body,
};
pub(super) use controls::{LineageControlAction, LineageControls, LineageSpeech};
use controls::{LineageLife, render_lineage_controls};
use fleet::{
    FleetRender, LineageMode, carries_lineage, render_empty_fleet, render_populated_fleet,
};
use mosaic_view::{ActivityCard, CardLayout, HistorySender, MosaicPlane};
pub(in crate::browser) use state::{ComposerAction, ComposerDrafts};

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

/// Render every live lineage as one packed surface.
///
/// This is the sole Lanes conversation path: each lineage owns exactly one
/// composer and activity plane, however many independently steerable members
/// the lineage contains.
pub(super) fn render_lineage(
    surface: &SurfaceData,
    sender: &SenderHandle,
    drafts: &UseReducerHandle<ComposerDrafts>,
    submissions: &SubmissionHandle,
    lineage_fleet: &NodeRef,
    lineage_controls: &UseReducerHandle<LineageControls>,
    controls: &view::AgentControls,
) -> Html {
    let surface = surface.read();
    let Some(runtime) = surface.as_ref() else {
        return empty("No admitted live organization is available.");
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
    let can_configure = runtime.offers(CommandKind::LineageConfigure);
    let can_arrange_lanes = runtime.offers(CommandKind::LaneArrange);
    let can_arrange_lineages = runtime.offers(CommandKind::LineageArrange);
    let topology = watchband::topology(&lanes.lanes);
    if lanes.lanes.is_empty() {
        return render_empty_fleet(
            runtime,
            lanes.epoch,
            lineage_fleet,
            lineage_controls,
            controls,
        );
    }
    let render = FleetRender {
        runtime,
        epoch: lanes.epoch,
        topology_revision: lanes.topology_revision,
        can_send,
        can_wake,
        can_arrange_lanes,
        can_arrange_lineages,
        can_configure,
        topology,
        sender,
        drafts,
        submissions,
        lineage_controls,
        controls,
    };
    render_populated_fleet(&lanes.lanes, &standings, lineage_fleet, &render)
}

#[hook]
pub(super) fn use_lineage_fleet_right_edge(host: NodeRef, lineage_count: usize) {
    use_effect_with((host, lineage_count), move |(host, _)| {
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
    reason = "one lineage render owns its controls"
)]
fn render_compose(
    runtime: &CanvasRuntime,
    lineage: &crate::runtime::LaneRow,
    standings: &[Standing],
    epoch: CanonicalUuid,
    topology_revision: u64,
    can_send: bool,
    can_wake: bool,
    can_arrange: bool,
    sender: &SenderHandle,
    drafts: &UseReducerHandle<ComposerDrafts>,
    submissions: &SubmissionHandle,
    lineage_controls: &UseReducerHandle<LineageControls>,
    can_configure: bool,
    controls: &view::AgentControls,
) -> Html {
    if lineage.members.is_empty() {
        return empty("No agents are assigned. Open Team to add one.");
    }
    let authoritative_life = lineage.subscription;
    let control = lineage_controls.current(lineage.lane, authoritative_life);
    let submit = submit_lineage_callback(
        &lineage.members,
        epoch,
        can_wake,
        sender.clone(),
        drafts.clone(),
        submissions.clone(),
    );
    let broadcast_targets = broadcast_targets(&lineage.members, epoch, can_send);
    let broadcast_help = format!("composer-broadcast-help-{}", lineage.lane);
    let ready = can_send
        && control.pending_life.is_none()
        && lineage.members.iter().any(|member| {
            (member.occupant.is_some() || (member.bound && can_wake))
                && drafts
                    .get(&member.lane.to_string())
                    .is_some_and(|draft| !compose(draft).trim().is_empty())
        });
    let cards = activity_cards(runtime, lineage);
    let dismiss_agent_menus = Callback::from(dismiss_other_agent_action_menus);
    let history_targets = lineage
        .members
        .iter()
        .filter_map(|member| history_target(runtime, member.lane))
        .collect();
    html! {
        <div class="organization-compose" onclick={dismiss_agent_menus}>
            <form class="organization-composer" onsubmit={submit.clone()}>
                <div class="organization-agents">
                    {for lineage.members.iter().zip(standings.iter()).map(|(member, standing)| {
                        let header = primary_composer_header(
                            &cards,
                            lineage,
                            member.lane,
                            standing,
                        );
                        render_agent_composer(
                            lineage.lane,
                            member,
                            standing,
                            epoch,
                            topology_revision,
                            runtime
                                .lanes()
                                .and_then(|lanes| lanes.renewal(lineage.lane, member.lane)),
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
                {render_lineage_controls(
                    lineage.lane,
                    authoritative_life,
                    topology_revision,
                    &control,
                    lineage_controls,
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
                lineage,
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

/// Every stable assistant-authored record the lineage can render in total chronology.
/// Observation time is the primary display order. The admitted lane and source
/// locus break equal-time ties without parsing a rendered DOM identity. The
/// persistent plane uses this complete order to distinguish edge additions
/// from structural middle insertions during staged hydration.
fn activity_cards(runtime: &CanvasRuntime, lineage: &crate::runtime::LaneRow) -> Vec<ActivityCard> {
    let mut cards = runtime
        .messages()
        .iter()
        .filter_map(|message| {
            let (position, member) = lineage
                .members
                .iter()
                .enumerate()
                .find(|(_, member)| member.lane == message.lane)?;
            Some(ActivityCard::Agent {
                accent: watchband::accent(lineage.members.len(), position),
                member: member.clone(),
                quote_lane: crate::composer_drafts::quote_lane(&lineage.members, message),
                message: Rc::clone(message),
            })
        })
        .collect::<Vec<_>>();
    cards.extend(
        runtime
            .lifecycle_boundaries()
            .iter()
            .filter_map(|boundary| {
                let (position, member) = lineage
                    .members
                    .iter()
                    .enumerate()
                    .find(|(_, member)| member.lane == boundary.lane())?;
                Some(ActivityCard::Lifecycle {
                    accent: watchband::accent(lineage.members.len(), position),
                    member: member.clone(),
                    boundary: boundary.clone(),
                })
            }),
    );
    cards.sort_by_key(ActivityCard::chronology_key);
    cards
}

fn render_activity(
    lineage: &crate::runtime::LaneRow,
    cards: &[ActivityCard],
    drafts: &UseReducerHandle<ComposerDrafts>,
    controls: &view::AgentControls,
    speech: LineageSpeech,
    life: LineageLife,
    history: HistoryPaging<'_>,
) -> Html {
    let empty = cards.is_empty();
    let narrate = controls.narrate.clone();
    let history_sender = HistorySender::new(history.sender.clone());
    html! {
        <div
            id={format!("lineage-stream-{}", lineage.lane)}
            class="organization-activity-grid"
            aria-live="polite"
            aria-label="Live agent activity"
            data-empty={empty.to_string()}
        >
            if empty {
                <div class="mosaic-message-empty">{"No pending direction."}</div>
            }
            <MosaicPlane
                key={lineage.lane.to_string()}
                lane={lineage.lane}
                {life}
                members={lineage.members.len()}
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
    lineage: &crate::runtime::LaneRow,
    standings: &[Standing],
    mode: LineageMode,
    controls: &view::AgentControls,
) -> Html {
    html! {
        <div class={classes!(
            "organization-detail-pane",
            (mode == LineageMode::Org).then_some("with-seat")
        )}>
            <div class="organization-detail-grid">
                {for lineage.members.iter().zip(standings.iter()).map(|(member, standing)| {
                    render_agent_details(member, standing, mode)
                })}
            </div>
            if mode == LineageMode::Org {
                {view::render_seat_panel(Some(runtime), controls)}
            }
        </div>
    }
}

fn render_tasks(
    runtime: &CanvasRuntime,
    lineage: &crate::runtime::LaneRow,
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
    let active_state = OrgTaskSectionState::active(runtime.board().is_some(), active.len());
    let direction_state = OrgTaskSectionState::direction(runtime.inbox().is_some(), queued.len());
    let policy = LineageLife::from(lineage.subscription).label();
    let active_summary = runtime.board().map_or_else(
        || "Task state unavailable".to_owned(),
        |_| format!("{} active", active.len()),
    );
    html! {
        <main class="organization-task-workspace" aria-label="Organization tasks">
            <header class="organization-task-summary">
                <div>
                    <span class="organization-task-eyebrow">{"Team work"}</span>
                    <strong>{active_summary}</strong>
                </div>
                <div
                    class="organization-task-policy"
                    data-task-policy-extension=""
                    aria-label={format!("Task policy: {policy}")}
                >
                    <span>{"Task policy"}</span>
                    <strong>{policy}</strong>
                </div>
            </header>
            <section class="organization-task-section" aria-labelledby={format!("active-tasks-{}", lineage.lane)}>
                <header>
                    <h3 id={format!("active-tasks-{}", lineage.lane)}>{"Active tasks"}</h3>
                    if !active.is_empty() {
                        <span>{active.len()}</span>
                    }
                </header>
                if let Some(message) = active_state.message() {
                    <p class="organization-task-empty" role={active_state.unavailable().then_some("status")}>{message}</p>
                } else {
                    <ol class="organization-task-list">
                        {for active.into_iter().map(render_task_row)}
                    </ol>
                }
            </section>
            <section class="organization-task-section" aria-labelledby={format!("direction-queue-{}", lineage.lane)}>
                <header>
                    <h3 id={format!("direction-queue-{}", lineage.lane)}>{"Direction queue"}</h3>
                    if !queued.is_empty() {
                        <span>{queued.iter().map(|standing| standing.pending).sum::<usize>()}</span>
                    }
                </header>
                if let Some(message) = direction_state.message() {
                    <p class="organization-task-empty" role={direction_state.unavailable().then_some("status")}>{message}</p>
                } else {
                    <ol class="organization-direction-list">
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
    let expiry = lease_countdown(standing.expires_at, "organization-task-age", "unavailable");
    html! {
        <li
            class="organization-task-row"
            data-task-handle={handle.clone()}
            data-agent-lane={standing.lane.to_string()}
            data-accent={standing.accent.to_string()}
        >
            <div class="organization-task-row-main">
                <code>{handle}</code>
                <strong class="organization-task-title">{title}</strong>
            </div>
            <div class="organization-task-row-meta">
                <span class="organization-task-member">
                    <span class="agent-presence" aria-hidden="true"></span>
                    <span class="organization-task-member-name">{standing.label.clone()}</span>
                </span>
                <span class="organization-task-phase">{phase}</span>
                <span class="organization-task-renewed">{"expires "}{expiry}</span>
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
            class="organization-direction-row"
            data-agent-lane={standing.lane.to_string()}
            data-accent={standing.accent.to_string()}
        >
            <span class="organization-task-member">
                <span class="agent-presence" aria-hidden="true"></span>
                <strong class="organization-task-member-name">{standing.label.clone()}</strong>
            </span>
            <span class="organization-direction-preview">{preview}</span>
            <span class="organization-direction-count">{format!("{} pending", standing.pending)}</span>
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
    mode: LineageMode,
) -> Html {
    let occupant = member
        .occupant
        .map(|value| value.to_string())
        .unwrap_or_else(|| "vacant".to_owned());
    html! {
        <article
            class="organization-detail-card"
            data-accent={standing.accent.to_string()}
            title={watchband::standing_title(standing)}
        >
            <header>
                <span class="agent-presence"></span>
                <strong>{standing.label.clone()}</strong>
                <span>{standing.vendor.clone()}</span>
            </header>
            {match mode {
                LineageMode::Org => html! {
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
                        <div>
                            <dt>{"Vendor / residency policy"}</dt>
                            <dd>{format!("{} / {}", standing.vendor, standing.life)}</dd>
                        </div>
                    </dl>
                },
                LineageMode::Lanes | LineageMode::Tasks => Html::default(),
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
/// labels.
fn primary_composer_header(
    cards: &[ActivityCard],
    lineage: &crate::runtime::LaneRow,
    lane: CanonicalUuid,
    standing: &Standing,
) -> ComposerHeader {
    let own_source = cards
        .iter()
        .rev()
        .find_map(|card| visible_agent_prose(card, Some(lane)));
    let is_own_source = own_source.is_some();
    let source = own_source.or_else(|| {
        cards
            .iter()
            .rev()
            .find_map(|card| visible_agent_prose(card, None))
    });
    source.map_or_else(
        || ComposerHeader {
            stream: ComposerStreamLink {
                target: format!("lineage-stream-{}", lineage.lane),
                label: "Jump to lineage activity".to_owned(),
                display_millis: None,
            },
            prose: "No agent prose yet".to_owned(),
        },
        |(message, prose)| ComposerHeader {
            stream: ComposerStreamLink {
                target: agent_message_source_id(message),
                label: if is_own_source {
                    format!("Jump to latest {} message", standing.label)
                } else {
                    "Jump to latest lineage message".to_owned()
                },
                display_millis: Some(crate::runtime::agent_message_display_millis(message)),
            },
            prose,
        },
    )
}

fn visible_agent_prose(
    card: &ActivityCard,
    lane: Option<CanonicalUuid>,
) -> Option<(&wire::AgentMessage, String)> {
    match card {
        ActivityCard::Agent {
            member, message, ..
        } if lane.is_none_or(|lane| lane == member.lane)
            && message.activity == wire::AgentActivityKind::Prose =>
        {
            watchband::first_nonquoted_prose(&message.text).map(|prose| (message.as_ref(), prose))
        }
        ActivityCard::Agent { .. } => None,
        ActivityCard::Lifecycle { .. } => None,
    }
}

fn renewal_callback(
    topology_revision: u64,
    lineage: CanonicalUuid,
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
                lineage,
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
            // An organization in flight is passing over on its way to an organization surface;
            // claiming it here would drop a whole organization onto one composer.
            if carries_lineage(&transfer) {
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
    reason = "one member composer needs its standing and shared lineage interaction handles"
)]
fn render_agent_composer(
    lineage: CanonicalUuid,
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
        lineage,
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
                "organization-agent",
                occupant.is_none().then_some("organization-agent--vacant"),
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
                        data-renewal-lineage={standing.lineage_id.to_string()}
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
    if let Some(handle) = &standing.handle {
        lines.push(match &standing.phase {
            Some(phase) => format!("{handle}, {phase}"),
            None => handle.clone(),
        });
    }
    lines.join("\n")
}

fn render_composer_vendor_icon(standing: &Standing) -> Html {
    let Some(class) = watchband::composer_vendor_icon_class(&standing.vendor) else {
        return Html::default();
    };
    let path = match standing.vendor.as_str() {
        "claude" => {
            "m3.127 10.604 3.135-1.76.053-.153-.053-.085H6.11l-.525-.032-1.791-.048-1.554-.065-1.505-.08-.38-.081L0 7.832l.036-.234.32-.214.455.04 1.009.069 1.513.105 1.097.064 1.626.17h.259l.036-.105-.089-.065-.068-.064-1.566-1.062-1.695-1.121-.887-.646-.48-.327-.243-.306-.104-.67.435-.48.585.04.15.04.593.456 1.267.981 1.654 1.218.242.202.097-.068.012-.049-.109-.181-.9-1.626-.96-1.655-.428-.686-.113-.411a2 2 0 0 1-.068-.484l.496-.674L4.446 0l.662.089.279.242.411.94.666 1.48 1.033 2.014.302.597.162.553.06.17h.105v-.097l.085-1.134.157-1.392.154-1.792.052-.504.25-.605.497-.327.387.186.319.456-.045.294-.19 1.23-.37 1.93-.243 1.29h.142l.161-.16.654-.868 1.097-1.372.484-.545.565-.601.363-.287h.686l.505.751-.226.775-.707.895-.585.759-.839 1.13-.524.904.048.072.125-.012 1.897-.403 1.024-.186 1.223-.21.553.258.06.263-.218.536-1.307.323-1.533.307-2.284.54-.028.02.032.04 1.029.098.44.024h1.077l2.005.15.525.346.315.424-.053.323-.807.411-3.631-.863-.872-.218h-.12v.073l.726.71 1.331 1.202 1.667 1.55.084.383-.214.302-.226-.032-1.464-1.101-.565-.497-1.28-1.077h-.084v.113l.295.432 1.557 2.34.08.718-.112.234-.404.141-.444-.08-.911-1.28-.94-1.44-.759-1.291-.093.053-.448 4.821-.21.246-.484.186-.403-.307-.214-.496.214-.98.258-1.28.21-1.016.19-1.263.112-.42-.008-.028-.092.012-.953 1.307-1.448 1.957-1.146 1.227-.274.109-.477-.247.045-.44.266-.39 1.586-2.018.956-1.25.617-.723-.004-.105h-.036l-4.212 2.736-.75.096-.324-.302.04-.496.154-.162 1.267-.871z"
        }
        "codex" | "openai" => {
            "M14.949 6.547a3.94 3.94 0 0 0-.348-3.273 4.11 4.11 0 0 0-4.4-1.934A4.1 4.1 0 0 0 8.423.2 4.15 4.15 0 0 0 6.305.086a4.1 4.1 0 0 0-1.891.948 4.04 4.04 0 0 0-1.158 1.753 4.1 4.1 0 0 0-1.563.679A4 4 0 0 0 .554 4.72a3.99 3.99 0 0 0 .502 4.731 3.94 3.94 0 0 0 .346 3.274 4.11 4.11 0 0 0 4.402 1.933c.382.425.852.764 1.377.995.526.231 1.095.35 1.67.346 1.78.002 3.358-1.132 3.901-2.804a4.1 4.1 0 0 0 1.563-.68 4 4 0 0 0 1.14-1.253 3.99 3.99 0 0 0-.506-4.716m-6.097 8.406a3.05 3.05 0 0 1-1.945-.694l.096-.054 3.23-1.838a.53.53 0 0 0 .265-.455v-4.49l1.366.778q.02.011.025.035v3.722c-.003 1.653-1.361 2.992-3.037 2.996m-6.53-2.75a2.95 2.95 0 0 1-.36-2.01l.095.057L5.29 12.09a.53.53 0 0 0 .527 0l3.949-2.246v1.555a.05.05 0 0 1-.022.041L6.473 13.3c-1.454.826-3.311.335-4.15-1.098m-.85-6.94A3.02 3.02 0 0 1 3.07 3.949v3.785a.51.51 0 0 0 .262.451l3.93 2.237-1.366.779a.05.05 0 0 1-.048 0L2.585 9.342a2.98 2.98 0 0 1-1.113-4.094zm11.216 2.571L8.747 5.576l1.362-.776a.05.05 0 0 1 .048 0l3.265 1.86a3 3 0 0 1 1.173 1.207 2.96 2.96 0 0 1-.27 3.2 3.05 3.05 0 0 1-1.36.997V8.279a.52.52 0 0 0-.276-.445m1.36-2.015-.097-.057-3.226-1.855a.53.53 0 0 0-.53 0L6.249 6.153V4.598a.04.04 0 0 1 .019-.04L9.533 2.7a3.07 3.07 0 0 1 3.257.139c.474.325.843.778 1.066 1.303.223.526.289 1.103.191 1.664zM5.503 8.575 4.139 7.8a.05.05 0 0 1-.026-.037V4.049c0-.57.166-1.127.476-1.607s.752-.864 1.275-1.105a3.08 3.08 0 0 1 3.234.41l-.096.054-3.23 1.838a.53.53 0 0 0 .265.455zm.742-1.577 1.758-1 1.762 1v2l-1.755 1-1.762-1z"
        }
        // oh-my-pi's pi with its plug, drawn in one colour.
        "omp" => "M1.3 3h13.4v1.6H1.3zM3.3 4.6h1.6v8.3H3.3zM10 4.6h1.6v6H10zM9.5 9.3h2.7v2.2H9.5z",
        _ => unreachable!("recognized composer vendor was checked above"),
    };
    let title = watchband::standing_title(standing);
    html! {
        <span
            class={classes!("composer-vendor-icon", class)}
            data-composer-vendor-icon={standing.vendor.clone()}
            title={title.clone()}
            aria-label={title}
            role="img"
        >
            <svg viewBox="0 0 16 16" aria-hidden="true" focusable="false">
                <path d={path}></path>
            </svg>
        </span>
    }
}

fn submit_lineage_callback(
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
