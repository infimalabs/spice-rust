//! Predecessor-style lane composer rendered inside the Agents surface.

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{DragEvent, HtmlInputElement};

use super::*;
use crate::composer_drafts::{ComposerDraft, compose};
use crate::watchband::{self, Standing};

mod composer;
mod controls;
mod mosaic_view;
mod state;

use composer::{
    agent_message_source_id, directive_source_id, drop_on_lane, focus_after_render,
    keyboard_submit, paste_images, primary_textarea_id, render_agent_message_card,
    render_directive_card, render_image_indicators, render_quote_shards, update_body,
};
pub(super) use controls::{LineageControlAction, LineageControls};
use controls::{LineageLife, LineageSpeech, render_lineage_controls};
use mosaic_view::{ActivityCard, CardLayout, MosaicPlane};
use state::ComposerAction;
pub(super) use state::ComposerDrafts;

const MAX_ATTACHMENTS: usize = 8;
const MAX_ATTACHMENT_BYTES: f64 = 8.0 * 1_024.0 * 1_024.0;
const RENEWAL_REQUEST: &str = "Renew your current task lease now.";
const LANE_DRAG_MIME: &str = "application/x-spice-lane";

/// The four one-click views of one live team.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LineageMode {
    Compose,
    Filters,
    Metrics,
    Info,
}

impl LineageMode {
    const ALL: [Self; 4] = [Self::Compose, Self::Filters, Self::Metrics, Self::Info];

    const fn label(self) -> &'static str {
        match self {
            Self::Compose => "Compose",
            Self::Filters => "Filters",
            Self::Metrics => "Metrics",
            Self::Info => "Info",
        }
    }
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
    mode: &UseStateHandle<LineageMode>,
    lineage_controls: &UseReducerHandle<LineageControls>,
    controls: &view::AgentControls,
) -> Html {
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
    let standings = watchband::standings(&lanes.lanes, waiting, tasks, js_sys::Date::now() as i64);
    let can_send = runtime.offers(CommandKind::SteerSend);
    let can_configure = runtime.offers(CommandKind::LineageConfigure);
    let can_arrange = runtime.offers(CommandKind::LaneArrange);
    if lanes.lanes.is_empty() {
        let empty_lineage = crate::runtime::LaneRow {
            lane: lanes.epoch,
            subscription: wire::intent::LineageLife::Steer,
            members: Vec::new(),
        };
        return html! {
            <div class="lineage-fleet" data-pane="agent-console" data-view="mosaic">
                <section class="team-surface" data-lineage-weight="1">
                    {render_lineage_toolbar(mode, &empty_lineage, &[])}
                    <div class="team-pane" data-team-mode={(**mode).label()}>
                        {match **mode {
                            LineageMode::Compose => empty("No agents are assigned. Open Info to add one."),
                            LineageMode::Filters | LineageMode::Metrics | LineageMode::Info => {
                                render_details(runtime, &empty_lineage, &[], **mode, controls)
                            }
                        }}
                    </div>
                </section>
            </div>
        };
    }
    html! {
        <div class="lineage-fleet" data-pane="agent-console" data-view="mosaic">
            {for lanes.lanes.iter().map(|lineage| {
                let lineage_standings = standings
                    .iter()
                    .filter(|standing| standing.lineage_id == lineage.lane)
                    .cloned()
                    .collect::<Vec<_>>();
                html! {
                    <section
                        class="team-surface"
                        data-lineage-id={lineage.lane.to_string()}
                        data-lineage-weight={lineage_standings.len().max(1).to_string()}
                    >
                        {render_lineage_toolbar(mode, lineage, &lineage_standings)}
                        <div class="team-pane" data-team-mode={(**mode).label()}>
                            {match **mode {
                                LineageMode::Compose => render_compose(
                                    runtime,
                                    lineage,
                                    &lineage_standings,
                                    lanes.epoch,
                                    can_send,
                                    can_arrange,
                                    sender,
                                    drafts,
                                    lineage_controls,
                                    can_configure,
                                    controls,
                                ),
                                LineageMode::Filters | LineageMode::Metrics | LineageMode::Info => html! {
                                    <>
                                        {view::chrome::render_lineage_ticker(runtime, lineage)}
                                        {render_details(
                                            runtime,
                                            lineage,
                                            &lineage_standings,
                                            **mode,
                                            controls,
                                        )}
                                    </>
                                },
                            }}
                        </div>
                    </section>
                }
            })}
        </div>
    }
}

fn render_lineage_toolbar(
    mode: &UseStateHandle<LineageMode>,
    lineage: &crate::runtime::LaneRow,
    standings: &[Standing],
) -> Html {
    html! {
        <header class="team-toolbar">
            <span
                class="team-lights"
                role="group"
                aria-label={format!("{} admitted agents", standings.len())}
            >
                {for standings.iter().filter_map(|standing| {
                    let member = lineage.member(standing.lane)?;
                    let status = member.status(standing.pending);
                    Some(html! {
                        <span
                            class={classes!("team-light", status.key())}
                            role="img"
                            data-lane-light-target-id={member.lane.to_string()}
                            data-lane-status={status.key()}
                            aria-label={format!("{}: {}", standing.label, status.label())}
                        ></span>
                    })
                })}
            </span>
            <nav class="team-mode-rail" role="tablist" aria-label="Live team views">
                {for LineageMode::ALL.map(|candidate| {
                    let selected = candidate == **mode;
                    let choose = mode.clone();
                    let badge = mode_badge(candidate, standings);
                    html! {
                        <button
                            type="button"
                            role="tab"
                            class={classes!("team-mode", selected.then_some("selected"))}
                            aria-selected={selected.to_string()}
                            onclick={Callback::from(move |_| choose.set(candidate))}
                        >
                            <span>{candidate.label()}</span>
                            if let Some(badge) = badge {
                                <span class="team-mode-badge">{badge}</span>
                            }
                        </button>
                    }
                })}
            </nav>
            <button
                class="team-menu"
                type="button"
                aria-label="Team and agent details"
                title="Team and agent details"
                onclick={{
                    let mode = mode.clone();
                    Callback::from(move |_| mode.set(LineageMode::Info))
                }}
            ><span aria-hidden="true">{"☰"}</span></button>
        </header>
    }
}

fn mode_badge(mode: LineageMode, standings: &[Standing]) -> Option<String> {
    let count = match mode {
        LineageMode::Compose => standings.iter().map(|standing| standing.pending).sum(),
        LineageMode::Filters => standings
            .iter()
            .filter(|standing| standing.handle.is_some())
            .count(),
        LineageMode::Metrics => standings
            .iter()
            .filter(|standing| standing.held.is_some())
            .count(),
        LineageMode::Info => standings.len(),
    };
    watchband::badge(count)
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
    can_send: bool,
    can_arrange: bool,
    sender: &SenderHandle,
    drafts: &UseReducerHandle<ComposerDrafts>,
    lineage_controls: &UseReducerHandle<LineageControls>,
    can_configure: bool,
    controls: &view::AgentControls,
) -> Html {
    if lineage.members.is_empty() {
        return empty("No agents are assigned. Open Info to add one.");
    }
    let authoritative_life = lineage.subscription;
    let control = lineage_controls.current(lineage.lane, authoritative_life);
    let submit = submit_lineage_callback(&lineage.members, epoch, sender.clone(), drafts.clone());
    let ready = can_send
        && !control.pending_life
        && lineage.members.iter().any(|member| {
            member.occupant.is_some()
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
                            standing.pending,
                            can_send,
                            can_arrange,
                            sender,
                            drafts,
                            stream,
                            prose,
                        )
                    })}
                </div>
                {render_lineage_controls(
                    lineage.lane,
                    authoritative_life,
                    &control,
                    lineage_controls,
                    sender,
                    can_configure,
                    ready,
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
    let speak = if speech == LineageSpeech::Quiet {
        Callback::from(|_: String| {})
    } else {
        controls.speak.clone()
    };
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
                drafts={drafts.clone()}
                {speak}
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
    let held = standing
        .held
        .map(watchband::elapsed_label)
        .unwrap_or_else(|| "—".to_owned());
    let renewed = standing
        .renewed
        .map(watchband::elapsed_label)
        .unwrap_or_else(|| "—".to_owned());
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
        } if member.lane == lane => Some(agent_message_source_id(message)),
        ActivityCard::Directive {
            lane: card_lane,
            directive,
        } if *card_lane == lane => Some(directive_source_id(directive)),
        ActivityCard::Agent { .. } | ActivityCard::Directive { .. } => None,
    });
    source.map_or_else(
        || ComposerStreamLink {
            target: format!("lineage-stream-{}", lineage.lane),
            label: "Jump to lineage activity".to_owned(),
        },
        |target| ComposerStreamLink {
            target,
            label: format!("Jump to latest {} activity", standing.label),
        },
    )
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
                    resolve: None,
                },
            );
        }
    })
}

fn copy_dragover() -> Callback<DragEvent> {
    Callback::from(move |event: DragEvent| {
        event.prevent_default();
        if let Some(transfer) = event.data_transfer() {
            transfer.set_drop_effect("copy");
        }
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

fn primary_composer_dragging(
    lane: CanonicalUuid,
    enabled: bool,
    sender: &SenderHandle,
) -> PrimaryComposerDragging {
    let start = Callback::from(move |event: DragEvent| {
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
    pending: usize,
    can_send: bool,
    can_arrange: bool,
    sender: &SenderHandle,
    drafts: &UseReducerHandle<ComposerDrafts>,
    stream: ComposerStreamLink,
    prose: String,
) -> Html {
    let lane_id = member.lane;
    let lane_key = lane_id.to_string();
    let draft = drafts.get(&lane_key).cloned().unwrap_or_default();
    let occupant = member.occupant;
    let send_enabled = can_send && occupant.is_some();
    let renew_enabled = send_enabled;
    let renew = renewal_callback(lane_id, occupant, epoch, sender);
    let onkeydown = keyboard_submit(
        lane_id,
        occupant,
        epoch,
        lane_key.clone(),
        sender.clone(),
        drafts.clone(),
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
    let dragging = primary_composer_dragging(lane_id, can_arrange, sender);
    html! {
        <section
            key={lane_key.clone()}
            class={classes!(
                "team-agent",
                (!send_enabled).then_some("team-agent--vacant"),
            )}
            data-agent-lane={lane_id.to_string()}
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
            draggable={dragging.enabled.to_string()}
            ondragstart={dragging.start}
            ondragover={dragging.over}
            ondrop={dragging.drop}
        >
            {render_composer_heading(
                lane,
                lane_key,
                standing,
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
    renew_enabled: bool,
    renew: Callback<MouseEvent>,
    stream: ComposerStreamLink,
    prose: String,
    indicators: Html,
) -> Html {
    let focus_lane = lane_key.to_owned();
    let focus = Callback::from(move |_| {
        focus_after_render(primary_textarea_id(&focus_lane));
    });
    html! {
        <header class="composer-shard-heading">
            <a
                class="composer-shard-link composer-shard-age"
                href={format!("#{}", stream.target)}
                aria-label={stream.label}
            >{
                standing
                    .held
                    .map(watchband::elapsed_label)
                    .unwrap_or_else(|| "now".to_owned())
            }</a>
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
    sender: SenderHandle,
    drafts: UseReducerHandle<ComposerDrafts>,
) -> Callback<SubmitEvent> {
    let targets = members
        .iter()
        .map(|member| (member.lane, member.occupant, member.lane.to_string()))
        .collect::<Vec<_>>();
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        for (lane, occupant, lane_key) in &targets {
            submit_draft(*lane, *occupant, epoch, lane_key, &sender, &drafts);
        }
    })
}

fn submit_draft(
    lane: CanonicalUuid,
    occupant: Option<CanonicalUuid>,
    epoch: CanonicalUuid,
    lane_key: &str,
    sender: &SenderHandle,
    drafts: &UseReducerHandle<ComposerDrafts>,
) -> bool {
    let Some(occupant) = occupant else {
        return false;
    };
    let Some(prepared) = drafts.0.prepare(lane_key) else {
        return false;
    };
    let restore_drafts = drafts.clone();
    let restore_lane = lane_key.to_owned();
    let snapshot = prepared.snapshot;
    let resolve = Callback::from(move |accepted: bool| {
        if !accepted {
            restore_drafts.dispatch(ComposerAction::Restore {
                lane: restore_lane.clone(),
                draft: snapshot.clone(),
            });
        }
    });
    if !send_action(
        sender,
        ClientAction::Steer {
            lane,
            occupant,
            epoch,
            message: prepared.message,
            attachments: prepared.attachments,
            resolve: Some(resolve),
        },
    ) {
        return false;
    }
    drafts.dispatch(ComposerAction::Clear {
        lane: lane_key.to_owned(),
    });
    true
}
