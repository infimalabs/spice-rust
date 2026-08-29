//! Predecessor-style lane composer rendered inside the Agents surface.

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{DragEvent, HtmlInputElement};

use super::clock::Age;
use super::*;
use crate::composer_drafts::{ComposerDraft, compose};
use crate::watchband::{self, Standing};

mod composer;
mod controls;
mod fleet;
mod lineage_menu;
mod mosaic_view;
mod state;

use composer::{
    agent_message_source_id, directive_source_id, drop_on_lane, focus_after_render,
    keyboard_submit, paste_images, primary_textarea_id, render_agent_message_card,
    render_directive_card, render_image_indicators, render_quote_shards, update_body,
};
pub(super) use controls::{LineageControlAction, LineageControls};
use controls::{LineageLife, LineageSpeech, render_lineage_controls};
use fleet::{
    FleetRender, LineageMode, carries_lineage, render_empty_fleet, render_populated_fleet,
};
use mosaic_view::{ActivityCard, CardLayout, MosaicPlane};
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
    let wake_ready = can_send
        && can_wake
        && lineage.members.iter().any(|member| {
            member.occupant.is_none()
                && member.bound
                && drafts
                    .get(&member.lane.to_string())
                    .is_some_and(|draft| !compose(draft).trim().is_empty())
        });
    let ready = can_send
        && control.pending_life.is_none()
        && lineage.members.iter().any(|member| {
            (member.occupant.is_some() || (member.bound && can_wake))
                && drafts
                    .get(&member.lane.to_string())
                    .is_some_and(|draft| !compose(draft).trim().is_empty())
        });
    let cards = activity_cards(runtime, lineage);
    html! {
        <div class="team-compose">
            <form class="team-composer" onsubmit={submit.clone()}>
                <div class="team-agents">
                    {for lineage.members.iter().zip(standings.iter()).map(|(member, standing)| {
                        let stream = primary_stream_link(&cards, lineage, member.lane, standing);
                        let prose = latest_agent_prose(&cards, member.lane);
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
                            stream,
                            prose,
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
                    wake_ready,
                )}
            </form>
            {view::chrome::render_lineage_ticker(runtime, lineage)}
            {render_activity(
                lineage,
                &cards,
                drafts,
                controls,
                control.speech,
                control.life,
            )}
        </div>
    }
}

/// Every activity the lineage can render, messages first and pending
/// directives after.
///
/// That order reaches the plane only as the order *unseen* keys are offered:
/// the plane keeps its own creation order and `render_activity_cards` renders
/// from that, so this chain is not the order an operator sees. A message
/// arriving while a directive is pending therefore appends to the lattice tail
/// rather than displacing the directive, which is decided behavior — see the
/// tail-by-decision case in `app/tests/mosaic_plane.rs` for why honoring this
/// order instead would repack the plane on its ordinary path.
fn activity_cards(runtime: &CanvasRuntime, lineage: &crate::runtime::LaneRow) -> Vec<ActivityCard> {
    runtime
        .messages()
        .iter()
        .filter_map(|message| {
            let member = lineage.member(message.lane)?;
            Some(ActivityCard::Agent {
                member: member.clone(),
                quote_lane: crate::composer_drafts::quote_lane(&lineage.members, message),
                message: message.clone(),
            })
        })
        .chain(lineage.members.iter().flat_map(|member| {
            let lane = member.lane;
            runtime
                .inbox()
                .and_then(|inbox| inbox.lanes.iter().find(|row| row.lane == lane))
                .map(|row| row.pending.as_slice())
                .unwrap_or(&[])
                .iter()
                .cloned()
                .map(move |directive| ActivityCard::Directive { lane, directive })
        }))
        .collect()
}

fn render_activity(
    lineage: &crate::runtime::LaneRow,
    cards: &[ActivityCard],
    drafts: &UseReducerHandle<ComposerDrafts>,
    controls: &view::AgentControls,
    speech: LineageSpeech,
    life: LineageLife,
) -> Html {
    let empty = cards.is_empty();
    let narrate = controls.narrate.clone();
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
            />
        </div>
    }
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
    let claim = standing
        .handle
        .as_deref()
        .zip(standing.phase.as_deref())
        .map_or_else(
            || "no active claim".to_owned(),
            |(handle, phase)| format!("{handle} / {phase}"),
        );
    let held = lease_age(standing.held_since, "claim-age", "—");
    let renewed = lease_age(standing.renewed_at, "claim-age", "—");
    let headline = standing
        .headline
        .clone()
        .unwrap_or_else(|| "clear".to_owned());
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
                LineageMode::Filters => html! {
                    <dl>
                        <div><dt>{"Task scope"}</dt><dd>{claim}</dd></div>
                        <div>
                            <dt>{"Queue"}</dt>
                            <dd>{format!("{} pending", standing.pending)}</dd>
                        </div>
                        <div><dt>{"Life"}</dt><dd>{standing.life.clone()}</dd></div>
                        <div>
                            <dt>{"Activity"}</dt>
                            <dd>{headline}</dd>
                        </div>
                    </dl>
                },
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
                            <dt>{"Vendor / life"}</dt>
                            <dd>{format!("{} / {}", standing.vendor, standing.life)}</dd>
                        </div>
                    </dl>
                },
                LineageMode::Compose => Html::default(),
            }}
        </article>
    }
}

struct ComposerStreamLink {
    target: String,
    label: String,
    authored: Option<i64>,
}

fn primary_stream_link(
    cards: &[ActivityCard],
    lineage: &crate::runtime::LaneRow,
    lane: CanonicalUuid,
    standing: &Standing,
) -> ComposerStreamLink {
    let source = cards.iter().rev().find_map(|card| match card {
        ActivityCard::Agent {
            member, message, ..
        } if member.lane == lane => Some((
            agent_message_source_id(message),
            source_authored_millis(message),
        )),
        ActivityCard::Agent { .. } | ActivityCard::Directive { .. } => None,
    });
    source.map_or_else(
        || ComposerStreamLink {
            target: format!("lineage-stream-{}", lineage.lane),
            label: "Jump to lineage activity".to_owned(),
            authored: None,
        },
        |(target, authored)| ComposerStreamLink {
            target,
            label: format!("Jump to latest {} activity", standing.label),
            authored,
        },
    )
}

fn source_authored_millis(message: &wire::AgentMessage) -> Option<i64> {
    (!message.authored_synthetic).then(|| message.authored.get().as_millisecond())
}

fn latest_agent_prose(cards: &[ActivityCard], lane: CanonicalUuid) -> String {
    cards
        .iter()
        .rev()
        .find_map(|card| match card {
            ActivityCard::Agent {
                member, message, ..
            } if member.lane == lane => watchband::first_nonquoted_prose(&message.text),
            ActivityCard::Agent { .. } | ActivityCard::Directive { .. } => None,
        })
        .unwrap_or_else(|| "No agent prose yet".to_owned())
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
                    member.status(standing.pending),
                    pending,
                    drafts,
                    onkeydown.clone(),
                    renew_enabled,
                    renew,
                    stream,
                    prose,
                    dragging,
                )}
                {render_quote_shards(&lane_key, &draft, drafts, onkeydown)}
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
    let onpaste = paste_images(lane_key.to_owned(), drafts.clone());
    html! {
        <div
            class="mosaic-composer composer-shard composer-shard--primary"
            data-shard-kind="primary"
            data-shard-target-id={lane.to_string()}
            data-primary-composer-draggable={dragging.enabled.to_string()}
            data-draft-unsafe={(
                !draft.body.is_empty()
                    || !draft.quotes.is_empty()
                    || !draft.attachments.is_empty()
            ).to_string()}
            draggable={dragging.enabled.to_string()}
            ondragstart={dragging.start}
            ondragover={dragging.over}
            ondrop={dragging.drop}
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
                render_image_indicators(lane_key, draft, drafts),
            )}
            <div class="composer-shard-standing">
                <span>{format!("{pending} pending")}</span>
                <span>{standing.liveness.as_str()}</span>
            </div>
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
        authored,
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
            >{lease_age(authored, "composer-shard-age-label", "—")}</a>
            <strong class="composer-shard-prose" title={prose.clone()}>{prose}</strong>
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
