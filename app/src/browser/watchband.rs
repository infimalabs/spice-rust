//! Predecessor-style lane composer rendered inside the Agents surface.

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{DragEvent, HtmlInputElement};

use super::clock::Age;
use super::*;
use crate::composer_drafts::{ComposerDraft, compose};
use crate::watchband::{self, Standing, TeamTaskSectionState};

mod composer;
mod controls;
mod fleet;
mod lineage_menu;
mod mosaic_view;
mod state;

use composer::{
    AgentMessageCard, agent_message_source_id, drop_on_lane, focus_after_render, keyboard_submit,
    paste_images, primary_textarea_id, render_image_indicators, render_quote_shards, update_body,
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
const RENEWAL_REQUEST: &str = "Renew your current task lease now.";
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

/// Render every live lineage as one packed surface.
///
/// This is the sole Agents conversation path: each lineage owns exactly one
/// composer and activity plane, however many independently steerable members
/// the lineage contains.
pub(super) fn render_lineage(
    surface: &SurfaceData,
    sender: &SenderHandle,
    drafts: &UseReducerHandle<ComposerDrafts>,
    submissions: &SubmissionHandle,
    lineage_controls: &UseReducerHandle<LineageControls>,
    controls: &view::AgentControls,
) -> Html {
    let surface = surface.read();
    let Some(runtime) = surface.as_ref() else {
        return empty("No admitted live team is available.");
    };
    let Some(lanes) = runtime.lanes() else {
        return empty("No lane snapshot is available.");
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
    // Wake-and-send needs both halves: lifecycle authority and the fresh lane
    // read that establishes the new residency witness before steering.
    let can_wake = runtime.offers(CommandKind::LaneState) && runtime.offers(CommandKind::LaneRead);
    let can_configure = runtime.offers(CommandKind::LineageConfigure);
    // Two drags, two laws: a card moves one lane, a grip restates the whole
    // topology. Each gesture is offered only when its own command is.
    let can_arrange_lanes = runtime.offers(CommandKind::LaneArrange);
    let can_arrange_lineages = runtime.offers(CommandKind::LineageArrange);
    let topology = watchband::topology(&lanes.lanes);
    if lanes.lanes.is_empty() {
        return render_empty_fleet(runtime, lanes.epoch, lineage_controls, controls);
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
    render_populated_fleet(&lanes.lanes, &standings, &render)
}

#[allow(
    clippy::too_many_arguments,
    reason = "one lineage renderer needs the admitted surface and its interactive control handles"
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
        return empty("No agents are assigned. Open Info to add one.");
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
    let ready = can_send
        && control.pending_life.is_none()
        && lineage.members.iter().any(|member| {
            (member.occupant.is_some() || (member.bound && can_wake))
                && drafts
                    .get(&member.lane.to_string())
                    .is_some_and(|draft| !compose(draft).trim().is_empty())
        });
    let cards = activity_cards(runtime, lineage);
    let history_targets = lineage
        .members
        .iter()
        .filter_map(|member| {
            runtime
                .message_history_target(member.lane)
                .map(|before| (member.lane, before))
        })
        .collect();
    html! {
        <div class="team-compose">
            <form class="team-composer" onsubmit={submit.clone()}>
                <div class="team-agents">
                    {for lineage.members.iter().zip(standings.iter()).map(|(member, standing)| {
                        let header = primary_composer_header(
                            &cards,
                            lineage,
                            member.lane,
                            standing,
                        );
                        render_agent_composer(
                            member,
                            standing,
                            epoch,
                            topology_revision,
                            standing.pending,
                            can_send,
                            can_wake,
                            can_arrange,
                            sender,
                            drafts,
                            submissions,
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
                    sender,
                    can_configure,
                    ready,
                )}
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

/// Every stable assistant-authored record the lineage can render in total chronology.
///
/// Observation time is the primary display order. The admitted lane and source
/// locus break equal-time ties without parsing a rendered DOM identity. The
/// persistent plane uses this complete order to distinguish edge additions
/// from structural middle insertions during staged hydration.
fn activity_cards(runtime: &CanvasRuntime, lineage: &crate::runtime::LaneRow) -> Vec<ActivityCard> {
    runtime
        .messages()
        .iter()
        .filter_map(|message| {
            let member = lineage.member(message.lane)?;
            Some(ActivityCard::Agent {
                member: member.clone(),
                quote_lane: crate::composer_drafts::quote_lane(&lineage.members, message),
                message: Rc::clone(message),
            })
        })
        .collect()
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
            class="team-activity-grid"
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
    targets: BTreeMap<CanonicalUuid, wire::MessageLocus>,
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
            "team-detail-pane",
            (mode == LineageMode::Info).then_some("with-seat")
        )}>
            <div class="team-detail-grid">
                {for lineage.members.iter().zip(standings.iter()).map(|(member, standing)| {
                    render_agent_details(member, standing, mode)
                })}
            </div>
            if mode == LineageMode::Info {
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
    let active_state = TeamTaskSectionState::active(runtime.board().is_some(), active.len());
    let direction_state = TeamTaskSectionState::direction(runtime.inbox().is_some(), queued.len());
    let policy = LineageLife::from(lineage.subscription).label();
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
            <section class="team-task-section" aria-labelledby={format!("active-tasks-{}", lineage.lane)}>
                <header>
                    <h3 id={format!("active-tasks-{}", lineage.lane)}>{"Active tasks"}</h3>
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
            <section class="team-task-section" aria-labelledby={format!("direction-queue-{}", lineage.lane)}>
                <header>
                    <h3 id={format!("direction-queue-{}", lineage.lane)}>{"Direction queue"}</h3>
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
    let renewed = lease_age(standing.renewed_at, "team-task-age", "unavailable");
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
                <span class="team-task-renewed">{"renewed "}{renewed}</span>
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
///
/// The vacant spelling stays a caller's choice because the two sites mean
/// different things by it: a metrics row is stating that a measurement is
/// absent, while a composer heading is stating that the agent's activity is
/// current. Both render into the same fixed slot, so a lane taking or releasing
/// a claim swaps text in a box that was already the right size.
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

fn render_agent_details(
    member: &crate::runtime::LaneMember,
    standing: &Standing,
    mode: LineageMode,
) -> Html {
    let occupant = member
        .occupant
        .map(|value| value.to_string())
        .unwrap_or_else(|| "vacant".to_owned());
    let held = lease_age(standing.held_since, "claim-age", "—");
    let renewed = lease_age(standing.renewed_at, "claim-age", "—");
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
                LineageMode::Metrics => html! {
                    <dl>
                        <div><dt>{"Pending"}</dt><dd>{standing.pending}</dd></div>
                        <div><dt>{"Claim age"}</dt><dd>{held}</dd></div>
                        <div><dt>{"Renewed"}</dt><dd>{renewed}</dd></div>
                        <div>
                            <dt>{"Liveness"}</dt>
                            <dd>{standing.liveness.as_str()}</dd>
                        </div>
                    </dl>
                },
                LineageMode::Info => html! {
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
                LineageMode::Compose | LineageMode::Tasks => Html::default(),
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
/// Tool previews and compaction boundaries remain useful activity cards, but
/// neither is the assistant prose printed beside this age. Selecting them here
/// would make the clock describe a different event from the text it labels.
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
    }
}

fn renewal_callback(
    lane: CanonicalUuid,
    occupant: Option<CanonicalUuid>,
    epoch: CanonicalUuid,
    sender: &SenderHandle,
) -> Callback<MouseEvent> {
    let sender = sender.clone();
    Callback::from(move |_| {
        if let Some(occupant) = occupant {
            let _sent = send_action(
                &sender,
                ClientAction::Steer {
                    lane,
                    occupant,
                    epoch,
                    message: RENEWAL_REQUEST.to_owned(),
                    attachments: Vec::new(),
                },
            );
        }
    })
}

fn copy_dragover() -> Callback<DragEvent> {
    Callback::from(move |event: DragEvent| {
        if let Some(transfer) = event.data_transfer() {
            // A team in flight is passing over on its way to a team surface;
            // claiming it here would drop a whole team onto one composer.
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
        if enabled && source != lane {
            let _sent = send_action(
                &sender,
                ClientAction::ArrangeLane {
                    topology_revision,
                    lane: source,
                    before: lane,
                },
            );
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
    member: &crate::runtime::LaneMember,
    standing: &Standing,
    epoch: CanonicalUuid,
    topology_revision: u64,
    pending: usize,
    can_send: bool,
    can_wake: bool,
    can_arrange: bool,
    sender: &SenderHandle,
    drafts: &UseReducerHandle<ComposerDrafts>,
    submissions: &SubmissionHandle,
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
    let renew_enabled = can_send && occupant.is_some();
    let renew = renewal_callback(lane_id, occupant, epoch, sender);
    let onkeydown = keyboard_submit(
        lane_id,
        target,
        lane_key.clone(),
        sender.clone(),
        drafts.clone(),
        submissions.clone(),
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
                    standing,
                    status,
                    pending,
                    drafts,
                    onkeydown.clone(),
                    renew_enabled,
                    renew,
                    stream,
                    prose,
                    dragging,
                )}
                {render_quote_shards(&lane_key, &draft, status, drafts, onkeydown)}
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
    standing: &Standing,
    status: crate::runtime::LaneStatus,
    pending: usize,
    drafts: &UseReducerHandle<ComposerDrafts>,
    onkeydown: Callback<KeyboardEvent>,
    renew_enabled: bool,
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
                lane_key,
                standing,
                status,
                renew_enabled,
                renew,
                stream,
                prose,
                render_image_indicators(lane_key, &draft.attachments, None, drafts),
                drag_enabled,
                drag_start,
            )}
            <textarea
                id={primary_textarea_id(lane_key)}
                aria-label={format!("Message for {lane}")}
                data-primary-composer-target-id={lane.to_string()}
                rows="4"
                value={draft.body.clone()}
                placeholder={composer_placeholder(standing, pending)}
                {oninput}
                {onkeydown}
                {onpaste}
            ></textarea>
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
    lane_key: &str,
    standing: &Standing,
    status: crate::runtime::LaneStatus,
    renew_enabled: bool,
    renew: Callback<MouseEvent>,
    stream: ComposerStreamLink,
    prose: String,
    indicators: Html,
    drag_enabled: bool,
    drag_start: Callback<DragEvent>,
) -> Html {
    let focus_lane = lane_key.to_owned();
    let focus = Callback::from(move |event: MouseEvent| {
        close_agent_action_menu(&event);
        focus_after_render(primary_textarea_id(&focus_lane));
    });
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
                <summary aria-label={format!("Actions for {}", standing.label)}>{"☰"}</summary>
                <div
                    class="agent-action-drawer"
                    data-agent-drawer-target-id={lane.to_string()}
                >
                    <button type="button" onclick={focus}>{"Focus message editor"}</button>
                    <button
                        class="agent-renew-request"
                        type="button"
                        disabled={!renew_enabled}
                        title="Ask this agent to renew its current task lease"
                        onclick={renew}
                    >
                        <strong>{"Renew this agent"}</strong>
                        <span>{"request renewal"}</span>
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

fn composer_placeholder(standing: &Standing, pending: usize) -> String {
    let mut lines = vec![
        format!(
            "{}, {}",
            standing.vendor,
            watchband::truncate(&standing.label, 8)
        ),
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
