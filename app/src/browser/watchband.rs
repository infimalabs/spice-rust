//! Predecessor-style lane composer rendered inside the Agents surface.

use pulldown_cmark::{CowStr, Event as MarkdownEvent, Options, Parser, Tag};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{ClipboardEvent, DragEvent, File, FileList, FileReader, HtmlInputElement};

use super::*;
use crate::composer_drafts::ComposerDraft;
use crate::watchband::{self, Standing};

mod controls;
mod state;

pub(super) use controls::{LineageControlAction, LineageControls};
use controls::{LineageLife, LineageSpeech, render_lineage_controls};
use state::ComposerAction;
pub(super) use state::ComposerDrafts;

const MAX_ATTACHMENTS: usize = 8;
const MAX_ATTACHMENT_BYTES: f64 = 8.0 * 1_024.0 * 1_024.0;

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
    if lanes.lanes.is_empty() {
        let empty_lineage = crate::runtime::LaneRow {
            lane: lanes.epoch,
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
                                    surface,
                                    runtime,
                                    lineage,
                                    &lineage_standings,
                                    lanes.epoch,
                                    can_send,
                                    sender,
                                    drafts,
                                    mode,
                                    lineage_controls,
                                    can_configure,
                                    controls,
                                ),
                                LineageMode::Filters | LineageMode::Metrics | LineageMode::Info => html! {
                                    <>
                                        {view::chrome::render_ticker(surface)}
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
    surface: &SurfaceData,
    runtime: &CanvasRuntime,
    lineage: &crate::runtime::LaneRow,
    standings: &[Standing],
    epoch: CanonicalUuid,
    can_send: bool,
    sender: &SenderHandle,
    drafts: &UseReducerHandle<ComposerDrafts>,
    mode: &UseStateHandle<LineageMode>,
    lineage_controls: &UseReducerHandle<LineageControls>,
    can_configure: bool,
    controls: &view::AgentControls,
) -> Html {
    if lineage.members.is_empty() {
        return empty("No agents are assigned. Open Info to add one.");
    }
    let server_life = lineage
        .host()
        .map_or("steer", |member| member.life.as_str());
    let control = lineage_controls.current(lineage.lane, server_life);
    let submit = submit_lineage_callback(&lineage.members, epoch, sender.clone(), drafts.clone());
    let ready = can_send
        && !control.pending_life
        && lineage.members.iter().any(|member| {
            member.occupant.is_some()
                && drafts
                    .get(&member.lane.to_string())
                    .is_some_and(|draft| !compose_markdown(draft).trim().is_empty())
        });
    html! {
        <div class="team-compose">
            <form class="team-composer" onsubmit={submit.clone()}>
                <div class="team-agents">
                    {for lineage.members.iter().zip(standings.iter()).map(|(member, standing)| {
                        render_agent_composer(
                            member,
                            standing,
                            epoch,
                            standing.pending,
                            can_send,
                            sender,
                            drafts,
                            mode,
                        )
                    })}
                </div>
                {render_lineage_controls(
                    lineage.lane,
                    server_life,
                    &control,
                    lineage_controls,
                    sender,
                    can_configure,
                    ready,
                )}
            </form>
            {view::chrome::render_ticker(surface)}
            {render_activity(runtime, lineage, drafts, controls, control.speech, control.life)}
        </div>
    }
}

fn render_activity(
    runtime: &CanvasRuntime,
    lineage: &crate::runtime::LaneRow,
    drafts: &UseReducerHandle<ComposerDrafts>,
    controls: &view::AgentControls,
    speech: LineageSpeech,
    life: LineageLife,
) -> Html {
    let member_targets = lineage
        .members
        .iter()
        .map(|member| member.lane.to_string())
        .collect::<Vec<_>>();
    let cards = runtime
        .messages()
        .iter()
        .filter_map(|message| {
            let member = lineage.member(message.lane)?;
            let producer = lineage
                .members
                .iter()
                .find(|candidate| candidate.occupant == Some(message.author))
                .map(|candidate| candidate.lane.to_string());
            let quote_target =
                crate::composer_drafts::quote_target(&member_targets, producer.as_deref())?;
            let quote_lane = lineage
                .members
                .iter()
                .find(|candidate| candidate.lane.to_string() == quote_target)?
                .lane;
            Some(ActivityCard::Agent {
                member: member.clone(),
                quote_lane,
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
        .collect::<Vec<_>>();
    let empty = cards.is_empty();
    let speak = if speech == LineageSpeech::Quiet {
        Callback::from(|_: String| {})
    } else {
        controls.speak.clone()
    };
    html! {
        <div
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
                {cards}
                drafts={drafts.clone()}
                {speak}
                {speech}
            />
        </div>
    }
}

#[derive(Clone, PartialEq)]
enum ActivityCard {
    Agent {
        member: crate::runtime::LaneMember,
        quote_lane: CanonicalUuid,
        message: wire::AgentMessage,
    },
    Directive {
        lane: CanonicalUuid,
        directive: wire::intent::PendingDirective,
    },
}

#[derive(Properties, PartialEq)]
struct MosaicPlaneProps {
    lane: CanonicalUuid,
    life: LineageLife,
    members: usize,
    cards: Vec<ActivityCard>,
    drafts: UseReducerHandle<ComposerDrafts>,
    speak: Callback<String>,
    speech: LineageSpeech,
}

/// One component per lineage lane is deliberate: every lane owns one width,
/// candidate stream, engine call, and row floor for all of its worktree
/// members. Sibling lineages can never carry a skyline into one another.
#[function_component(MosaicPlane)]
fn mosaic_plane(properties: &MosaicPlaneProps) -> Html {
    let host = use_node_ref();
    let width = use_element_width(host.clone());
    let narration = use_mut_ref(|| (false, BTreeMap::<String, ()>::new()));
    {
        let narration = narration.clone();
        let speak = properties.speak.clone();
        let speech = properties.speech;
        let messages = properties
            .cards
            .iter()
            .filter_map(|card| match card {
                ActivityCard::Agent { message, .. } => Some((
                    format!(
                        "{}:{}:{}:{}:{}",
                        message.lane,
                        message.locus.member,
                        message.locus.offset,
                        message.locus.length,
                        message.locus.event,
                    ),
                    message.text.clone(),
                )),
                ActivityCard::Directive { .. } => None,
            })
            .collect::<Vec<_>>();
        use_effect_with((speech, messages), move |(speech, messages)| {
            let mut narration = narration.borrow_mut();
            if !narration.0 {
                narration.0 = true;
                for (key, _) in messages {
                    narration.1.insert(key.clone(), ());
                }
                return;
            }
            let newest = messages
                .iter()
                .rev()
                .find_map(|(key, text)| (!narration.1.contains_key(key)).then_some(text.clone()));
            for (key, _) in messages {
                narration.1.insert(key.clone(), ());
            }
            if *speech == LineageSpeech::Narrate
                && let Some(text) = newest
            {
                speak.emit(text);
            }
        });
    }
    let geometry = crate::mosaic::Geometry::new(16.0, *width);
    let candidate_sets = properties
        .cards
        .iter()
        .map(|card| activity_candidates(card, &geometry))
        .collect::<Vec<_>>();
    let plane = crate::mosaic_plane::arrange(&candidate_sets);
    let rendered_cards = properties
        .cards
        .iter()
        .zip(plane.cards.iter())
        .enumerate()
        .map(|(order, (activity, card))| match activity {
            ActivityCard::Agent {
                member,
                quote_lane,
                message,
            } => render_agent_message_card(
                member,
                *quote_lane,
                message,
                &properties.drafts,
                &properties.speak,
                order as u64,
                card,
                plane.max_row,
            ),
            ActivityCard::Directive { lane, directive } => render_directive_card(
                *lane,
                directive,
                &lane.to_string(),
                &properties.drafts,
                &properties.speak,
                order as u64,
                card,
                plane.max_row,
            ),
        })
        .collect::<Html>();
    html! {
        <section
            ref={host}
            class="lane-mosaic-plane"
            data-agent-plane={properties.lane.to_string()}
            data-lineage-policy={properties.life.label()}
            data-lineage-members={properties.members.to_string()}
            data-lanes={geometry.lane_count.to_string()}
            data-plane-rows={plane.rows().to_string()}
            aria-label={format!(
                "Packed lineage plane: {} policy, {} {}",
                properties.life.label(),
                properties.members,
                if properties.members == 1 { "member" } else { "members" },
            )}
        >
            {rendered_cards}
        </section>
    }
}

fn activity_candidates(
    activity: &ActivityCard,
    geometry: &crate::mosaic::Geometry,
) -> Vec<crate::mosaic::Candidate> {
    let (text, attachment_height) = match activity {
        ActivityCard::Agent { message, .. } => (message.text.as_str(), 0.0),
        ActivityCard::Directive { directive, .. } => {
            let height = directive
                .attachments
                .iter()
                .filter(|attachment| !directive.text.contains(&attachment.data_url))
                .map(|attachment| {
                    if attachment.content_type.starts_with("image/") {
                        crate::mosaic::Reservation::Image.pixels(16.0) + 10.0
                    } else {
                        42.0
                    }
                })
                .sum::<f64>();
            (directive.text.as_str(), height)
        }
    };
    crate::mosaic::span_candidates(geometry, |span| {
        let width = f64::from(geometry.card_width(0, span).max(160));
        let characters_per_line = (width / 7.2).floor().max(18.0) as usize;
        let text_lines = text
            .lines()
            .map(|line| line.chars().count().max(1).div_ceil(characters_per_line))
            .sum::<usize>()
            .max(1);
        54.0 + text_lines as f64 * 19.0 + attachment_height
    })
}

#[hook]
fn use_element_width(host: NodeRef) -> UseStateHandle<f64> {
    let width = use_state_eq(|| 0.0);
    {
        let width = width.clone();
        use_effect_with(host, move |host| {
            let registration = host.cast::<web_sys::Element>().and_then(|element| {
                width.set(f64::from(element.client_width()));
                let observed = element.clone();
                let width = width.clone();
                let callback = Closure::<dyn FnMut(js_sys::Array, web_sys::ResizeObserver)>::new(
                    move |_, _| width.set(f64::from(observed.client_width())),
                );
                let observer =
                    web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref()).ok()?;
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
    width
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
            title={standing_title(standing)}
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
    sender: &SenderHandle,
    drafts: &UseReducerHandle<ComposerDrafts>,
    mode: &UseStateHandle<LineageMode>,
) -> Html {
    let lane_id = member.lane;
    let lane_key = lane_id.to_string();
    let draft = drafts.get(&lane_key).cloned().unwrap_or_default();
    let occupant = member.occupant;
    let send_enabled = can_send && occupant.is_some();
    let markdown = compose_markdown(&draft);
    let oninput = update_body(lane_key.clone(), drafts.clone());
    let onkeydown = keyboard_submit(
        lane_id,
        occupant,
        epoch,
        lane_key.clone(),
        sender.clone(),
        drafts.clone(),
    );
    let onpaste = paste_images(lane_key.clone(), drafts.clone());
    let ondragover = Callback::from(move |event: DragEvent| {
        event.prevent_default();
        if let Some(transfer) = event.data_transfer() {
            transfer.set_drop_effect("copy");
        }
    });
    let ondrop = drop_on_lane(
        lane_id,
        occupant,
        epoch,
        lane_key.clone(),
        sender.clone(),
        drafts.clone(),
    );
    let input_id = format!("mosaic-images-{lane_id}");
    let primary_id = primary_textarea_id(&lane_key);
    let onchange = choose_images(lane_key.clone(), drafts.clone());
    let activity = standing
        .headline
        .as_deref()
        .or(standing.handle.as_deref())
        .unwrap_or(&standing.disposition);
    let open = mode.clone();
    html! {
        <section
            key={lane_key.clone()}
            class={classes!(
                "team-agent",
                "composer-shard",
                (!send_enabled).then_some("team-agent--vacant"),
            )}
            data-agent-lane={lane_id.to_string()}
            data-shard-target-id={lane_id.to_string()}
            title={standing_title(standing)}
            {ondragover}
            {ondrop}
        >
            {render_quote_bands(&lane_key, &draft, drafts, onkeydown.clone())}
            <div class="mosaic-composer">
                <header class="composer-shard-heading">
                    <span class="composer-shard-age">{
                        standing
                            .held
                            .map(watchband::elapsed_label)
                            .unwrap_or_else(|| "now".to_owned())
                    }</span>
                    <strong title={activity.to_owned()}>{standing.label.clone()}</strong>
                    <button
                        type="button"
                        aria-label={format!("Details for {}", standing.label)}
                        onclick={Callback::from(move |_| open.set(LineageMode::Info))}
                    >{"☰"}</button>
                </header>
                    <div class="composer-shard-standing">
                        <span>{format!("{} pending", pending)}</span>
                        <span>{standing.liveness.as_str()}</span>
                    </div>
                    <textarea
                        id={primary_id}
                        aria-label={format!("Message for {lane_id}")}
                        data-primary-composer-target-id={lane_id.to_string()}
                        rows="4"
                        value={draft.body.clone()}
                        placeholder={composer_placeholder(standing, pending)}
                        {oninput}
                        {onkeydown}
                        {onpaste}
                    ></textarea>
                    {render_attachments(&lane_key, &draft, drafts)}
                    if let Some(issue) = &draft.issue {
                        <p class="composer-issue" role="status">{issue.clone()}</p>
                    }
                    if !markdown.is_empty() {
                        <div class="composer-markdown markdown-body" aria-label="Markdown preview">
                            {render_markdown(&markdown)}
                        </div>
                    }
                    <div class="mosaic-composer-actions">
                        <input
                            id={input_id.clone()}
                            class="visually-hidden"
                            type="file"
                            accept="image/*"
                            multiple=true
                            {onchange}
                        />
                        <label class="image-picker" for={input_id}>{"Images"}</label>
                        <span>{format!("{}/{}", draft.attachments.len(), MAX_ATTACHMENTS)}</span>
                    </div>
            </div>
        </section>
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

fn standing_title(standing: &Standing) -> String {
    format!(
        "{}\n{}\n{} pending\n{}",
        standing.label,
        standing.worktree,
        standing.pending,
        standing.liveness.as_str(),
    )
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

fn update_body(lane_key: String, drafts: UseReducerHandle<ComposerDrafts>) -> Callback<InputEvent> {
    Callback::from(move |event: InputEvent| {
        let input: web_sys::HtmlTextAreaElement = event.target_unchecked_into();
        drafts.dispatch(ComposerAction::SetBody {
            lane: lane_key.clone(),
            body: input.value(),
        });
    })
}

fn keyboard_submit(
    lane: CanonicalUuid,
    occupant: Option<CanonicalUuid>,
    epoch: CanonicalUuid,
    lane_key: String,
    sender: SenderHandle,
    drafts: UseReducerHandle<ComposerDrafts>,
) -> Callback<KeyboardEvent> {
    Callback::from(move |event: KeyboardEvent| {
        if event.key() == "Enter" && (event.meta_key() || event.ctrl_key()) {
            event.prevent_default();
            if submit_draft(lane, occupant, epoch, &lane_key, &sender, &drafts) {
                focus_after_render(primary_textarea_id(&lane_key));
            }
        }
    })
}

fn primary_textarea_id(lane_key: &str) -> String {
    format!("composer-primary-{lane_key}")
}

fn quote_textarea_id(lane_key: &str, identity: &str) -> String {
    format!("composer-quote-{lane_key}-{identity}")
}

fn focus_after_render(id: String) {
    spawn_local(async move {
        TimeoutFuture::new(0).await;
        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            return;
        };
        let Some(textarea) = document
            .get_element_by_id(&id)
            .and_then(|element| element.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
        else {
            return;
        };
        let _ = textarea.focus();
    });
}

fn render_agent_message_card(
    member: &crate::runtime::LaneMember,
    quote_lane: CanonicalUuid,
    message: &wire::AgentMessage,
    drafts: &UseReducerHandle<ComposerDrafts>,
    speak: &Callback<String>,
    order: u64,
    card: &crate::mosaic::Card,
    max_row: i32,
) -> Html {
    let drag_text = message.text.clone();
    let ondragstart = Callback::from(move |event: DragEvent| {
        if let Some(transfer) = event.data_transfer() {
            let _ = transfer.set_data("text/plain", &drag_text);
            transfer.set_effect_allowed("copy");
        }
    });
    let lane_key = quote_lane.to_string();
    let quote_key = lane_key.clone();
    let quote_drafts = drafts.clone();
    let quote_text = message.text.clone();
    let quote_identity = format!(
        "agent-{}-{}-{}-{}-{}",
        message.lane,
        message.locus.member,
        message.locus.offset,
        message.locus.length,
        message.locus.event,
    );
    let focus_identity = quote_identity.clone();
    let focus_lane = lane_key.clone();
    let producer = format!(
        "{} · {}",
        watchband::leaf(&member.worktree),
        watchband::short(message.author),
    );
    let correspondent = view::chrome::Correspondent {
        name: producer.clone(),
        accent: watchband::accent(member.lane),
        speak: speak.clone(),
        quote: Callback::from(move |()| {
            quote_drafts.dispatch(ComposerAction::AddQuote {
                lane: quote_key.clone(),
                identity: quote_identity.clone(),
                order,
                quote: quote_text.clone(),
                preview: None,
            });
            focus_after_render(quote_textarea_id(&focus_lane, &focus_identity));
        }),
    };
    let locus = format!(
        "tape {}:{}+{}#{}",
        message.locus.member, message.locus.offset, message.locus.length, message.locus.event,
    );
    html! {
        <article
            key={format!(
                "agent-{}-{}-{}",
                message.locus.member,
                message.locus.offset,
                message.locus.event,
            )}
            class={classes!("mosaic-message-card", card.lifecycle.as_str())}
            draggable="true"
            aria-label={format!("Agent message from {producer}")}
            data-card-kind="agent-message"
            data-agent-lane={member.lane.to_string()}
            data-producer-target-id={quote_lane.to_string()}
            data-accent={correspondent.accent.to_string()}
            data-accent-slot={correspondent.accent.to_string()}
            data-state={card.lifecycle.as_str()}
            data-track={card.track.to_string()}
            data-span={card.span.to_string()}
            data-base={card.base.to_string()}
            data-row={crate::mosaic::css_row_start(card, max_row).to_string()}
            data-rows={card.rows.to_string()}
            {ondragstart}
        >
            <div class="message-body markdown-body">{render_markdown(&message.text)}</div>
            <footer class="message-meta">
                <span class="message-key">{"agent"}</span>
                <span class="message-age" title={locus.clone()}>{locus}</span>
                <span class="message-author">{producer}</span>
                {view::chrome::render_card_actions(&message.text, &correspondent)}
            </footer>
        </article>
    }
}

fn render_directive_card(
    lane: CanonicalUuid,
    directive: &wire::intent::PendingDirective,
    lane_key: &str,
    drafts: &UseReducerHandle<ComposerDrafts>,
    speak: &Callback<String>,
    order: u64,
    card: &crate::mosaic::Card,
    max_row: i32,
) -> Html {
    let drag_text = directive.text.clone();
    let ondragstart = Callback::from(move |event: DragEvent| {
        if let Some(transfer) = event.data_transfer() {
            let _ = transfer.set_data("text/plain", &drag_text);
            transfer.set_effect_allowed("copy");
        }
    });
    let quote_key = lane_key.to_owned();
    let quote_drafts = drafts.clone();
    let quote_preview = directive.preview.clone();
    let quote_text = directive.text.clone();
    let quote_identity = format!("directive-{}", directive.directive);
    let focus_identity = quote_identity.clone();
    let focus_lane = lane_key.to_owned();
    let correspondent = view::chrome::Correspondent {
        name: watchband::short(lane),
        accent: watchband::accent(lane),
        speak: speak.clone(),
        quote: Callback::from(move |()| {
            quote_drafts.dispatch(ComposerAction::AddQuote {
                lane: quote_key.clone(),
                identity: quote_identity.clone(),
                order,
                quote: quote_text.clone(),
                preview: quote_preview.clone(),
            });
            focus_after_render(quote_textarea_id(&focus_lane, &focus_identity));
        }),
    };
    let remaining = directive.expires.get().as_millisecond() - js_sys::Date::now() as i64;
    let unlinked_attachments = directive
        .attachments
        .iter()
        .filter(|attachment| !directive.text.contains(&attachment.data_url))
        .collect::<Vec<_>>();
    html! {
        <article
            key={directive.directive.to_string()}
            class={classes!("mosaic-message-card", card.lifecycle.as_str())}
            draggable="true"
            data-directive={directive.directive.to_string()}
            data-agent-lane={lane.to_string()}
            data-state={card.lifecycle.as_str()}
            data-track={card.track.to_string()}
            data-span={card.span.to_string()}
            data-base={card.base.to_string()}
            data-row={crate::mosaic::css_row_start(card, max_row).to_string()}
            data-rows={card.rows.to_string()}
            {ondragstart}
        >
            <div class="message-body markdown-body">{render_markdown(&directive.text)}</div>
            if !unlinked_attachments.is_empty() {
                <div class="message-attachments">
                    {for unlinked_attachments.into_iter().map(|attachment| html! {
                        <img
                            src={attachment.data_url.clone()}
                            alt={attachment.name.clone()}
                            title={attachment.name.clone()}
                        />
                    })}
                </div>
            }
            <footer class="message-meta">
                <span class="message-key">{directive.key.clone()}</span>
                <span class="message-age" title={directive.expires.to_string()}>
                    {format!("{} left", watchband::elapsed_label(remaining))}
                </span>
                <span class="message-author">{correspondent.name.clone()}</span>
                {view::chrome::render_card_actions(&directive.text, &correspondent)}
            </footer>
        </article>
    }
}

fn render_quote_bands(
    lane_key: &str,
    draft: &ComposerDraft,
    drafts: &UseReducerHandle<ComposerDrafts>,
    target_submit: Callback<KeyboardEvent>,
) -> Html {
    html! {
        <div class="composer-quote-stack" data-composer-quote-stack-target-id={lane_key.to_owned()}>
            {for draft.quotes.iter().map(|quote| {
                let remove_key = lane_key.to_owned();
                let remove_drafts = drafts.clone();
                let remove_identity = quote.identity.clone();
                let remove = Callback::from(move |_| {
                    remove_drafts.dispatch(ComposerAction::RemoveQuote {
                        lane: remove_key.clone(),
                        identity: remove_identity.clone(),
                    });
                });
                let update_key = lane_key.to_owned();
                let update_drafts = drafts.clone();
                let update_identity = quote.identity.clone();
                let update = Callback::from(move |event: InputEvent| {
                    let input: web_sys::HtmlTextAreaElement = event.target_unchecked_into();
                    update_drafts.dispatch(ComposerAction::SetQuoteBody {
                        lane: update_key.clone(),
                        identity: update_identity.clone(),
                        body: input.value(),
                    });
                });
                let shown = crate::composer_markdown::quote_label(quote.preview.as_deref());
                let textarea_id = quote_textarea_id(lane_key, &quote.identity);
                html! {
                    <div
                        key={quote.identity.clone()}
                        class="composer-quote-band"
                        title={quote.quote.clone()}
                        data-quote-identity={quote.identity.clone()}
                        data-quote-target-id={lane_key.to_owned()}
                    >
                        <header>
                            <span>{shown}</span>
                            <button
                                type="button"
                                aria-label="Remove quoted composer"
                                onclick={remove}
                            >{"×"}</button>
                        </header>
                        <textarea
                            id={textarea_id}
                            aria-label="Reply to quoted message"
                            data-quote-draft-id={quote.identity.clone()}
                            rows="2"
                            value={quote.body.clone()}
                            placeholder="Reply to this quoted message"
                            oninput={update}
                            onkeydown={target_submit.clone()}
                        ></textarea>
                    </div>
                }
            })}
        </div>
    }
}

fn render_attachments(
    lane_key: &str,
    draft: &ComposerDraft,
    drafts: &UseReducerHandle<ComposerDrafts>,
) -> Html {
    html! {
        <div class="composer-attachment-list">
            {for draft.attachments.iter().enumerate().map(|(index, attachment)| {
                let remove_key = lane_key.to_owned();
                let remove_drafts = drafts.clone();
                let remove = Callback::from(move |_| {
                    remove_drafts.dispatch(ComposerAction::RemoveAttachment {
                        lane: remove_key.clone(),
                        index,
                    });
                });
                html! {
                    <span class="composer-attachment-chip" title={attachment.name.clone()}>
                        <img src={attachment.data_url.clone()} alt={attachment.name.clone()} />
                        <button
                            type="button"
                            aria-label="Remove image"
                            onclick={remove}
                        >{"×"}</button>
                    </span>
                }
            })}
        </div>
    }
}

fn choose_images(lane_key: String, drafts: UseReducerHandle<ComposerDrafts>) -> Callback<Event> {
    Callback::from(move |event: Event| {
        let input: HtmlInputElement = event.target_unchecked_into();
        if let Some(files) = input.files() {
            add_image_files(&drafts, &lane_key, &files);
        }
        input.set_value("");
    })
}

fn paste_images(lane_key: String, drafts: UseReducerHandle<ComposerDrafts>) -> Callback<Event> {
    Callback::from(move |event: Event| {
        let clipboard: ClipboardEvent = event.clone().unchecked_into();
        let Some(transfer) = clipboard.clipboard_data() else {
            return;
        };
        let Some(files) = transfer.files() else {
            return;
        };
        if has_image(&files) {
            clipboard.prevent_default();
            add_image_files(&drafts, &lane_key, &files);
        }
    })
}

fn drop_on_lane(
    lane: CanonicalUuid,
    occupant: Option<CanonicalUuid>,
    epoch: CanonicalUuid,
    lane_key: String,
    sender: SenderHandle,
    drafts: UseReducerHandle<ComposerDrafts>,
) -> Callback<DragEvent> {
    Callback::from(move |event: DragEvent| {
        event.prevent_default();
        let Some(transfer) = event.data_transfer() else {
            return;
        };
        if let Some(files) = transfer.files()
            && has_image(&files)
        {
            add_image_files(&drafts, &lane_key, &files);
            return;
        }
        let Ok(text) = transfer.get_data("text/plain") else {
            return;
        };
        let Some(occupant) = occupant else {
            return;
        };
        let message = crate::composer_markdown::block_quote(&text);
        if message.is_empty() {
            return;
        }
        send_action(
            &sender,
            ClientAction::Steer {
                lane,
                occupant,
                epoch,
                message,
                attachments: Vec::new(),
                resolve: None,
            },
        );
    })
}

fn has_image(files: &FileList) -> bool {
    (0..files.length()).any(|index| {
        files
            .get(index)
            .is_some_and(|file| file.type_().starts_with("image/"))
    })
}

fn add_image_files(drafts: &UseReducerHandle<ComposerDrafts>, lane_key: &str, files: &FileList) {
    for index in 0..files.length() {
        let Some(file) = files.get(index) else {
            continue;
        };
        if !file.type_().starts_with("image/") {
            continue;
        }
        if file.size() > MAX_ATTACHMENT_BYTES {
            drafts.dispatch(ComposerAction::SetIssue {
                lane: lane_key.to_owned(),
                issue: "Image is over 8 MB.".to_owned(),
            });
            continue;
        }
        read_image(drafts.clone(), lane_key.to_owned(), file);
    }
}

fn read_image(drafts: UseReducerHandle<ComposerDrafts>, lane_key: String, file: File) {
    let Ok(reader) = FileReader::new() else {
        drafts.dispatch(ComposerAction::SetIssue {
            lane: lane_key,
            issue: "Image reader is unavailable.".to_owned(),
        });
        return;
    };
    let result_reader = reader.clone();
    let name = if file.name().is_empty() {
        "pasted-image.png".to_owned()
    } else {
        file.name()
    };
    let content_type = if file.type_().is_empty() {
        "image/png".to_owned()
    } else {
        file.type_()
    };
    let onload = Closure::<dyn FnMut(web_sys::ProgressEvent)>::new(move |_| {
        let Ok(value) = result_reader.result() else {
            return;
        };
        let Some(data_url) = value.as_string() else {
            return;
        };
        drafts.dispatch(ComposerAction::AddAttachment {
            lane: lane_key.clone(),
            attachment: DirectiveAttachment {
                name: name.clone(),
                content_type: content_type.clone(),
                data_url,
            },
        });
    });
    reader.set_onload(Some(onload.as_ref().unchecked_ref()));
    if reader.read_as_data_url(&file).is_ok() {
        onload.forget();
    }
}

/// Delegate preview composition to the same target-neutral model sends use.
fn compose_markdown(draft: &ComposerDraft) -> String {
    crate::composer_drafts::compose(draft)
}

fn render_markdown(markdown: &str) -> Html {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_SMART_PUNCTUATION
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM;
    let events = Parser::new_ext(markdown, options).map(sanitize_markdown_event);
    let mut output = String::new();
    pulldown_cmark::html::push_html(&mut output, events);
    Html::from_html_unchecked(AttrValue::from(output))
}

fn sanitize_markdown_event(event: MarkdownEvent<'_>) -> MarkdownEvent<'_> {
    match event {
        MarkdownEvent::Start(tag) => MarkdownEvent::Start(sanitize_markdown_tag(tag)),
        MarkdownEvent::Html(raw) | MarkdownEvent::InlineHtml(raw) => MarkdownEvent::Text(raw),
        other => other,
    }
}

fn sanitize_markdown_tag(tag: Tag<'_>) -> Tag<'_> {
    match tag {
        Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        } => Tag::Link {
            link_type,
            dest_url: sanitize_markdown_url(dest_url, false),
            title,
            id,
        },
        Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        } => Tag::Image {
            link_type,
            dest_url: sanitize_markdown_url(dest_url, true),
            title,
            id,
        },
        other => other,
    }
}

fn sanitize_markdown_url(url: CowStr<'_>, image: bool) -> CowStr<'_> {
    let candidate = url.trim().to_ascii_lowercase();
    let safe_scheme = candidate.starts_with("https://")
        || candidate.starts_with("http://")
        || candidate.starts_with("mailto:")
        || (image && candidate.starts_with("data:image/"));
    let relative = !candidate.starts_with("//")
        && !candidate
            .split(['/', '#', '?'])
            .next()
            .is_some_and(|head| head.contains(':'));
    if safe_scheme || relative {
        url
    } else {
        CowStr::Borrowed("")
    }
}
