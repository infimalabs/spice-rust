//! Predecessor-style lane composer rendered inside the additive Mosaic tab.

use std::{collections::BTreeMap, rc::Rc};

use pulldown_cmark::{CowStr, Event as MarkdownEvent, Options, Parser, Tag};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{ClipboardEvent, DragEvent, File, FileList, FileReader, HtmlInputElement};

use super::*;
use crate::watchband::{self, Standing};

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

/// One policy shared by every agent member in the visual lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LineageLife {
    Steer,
    Drive,
    Drain,
}

impl LineageLife {
    const ALL: [Self; 3] = [Self::Steer, Self::Drive, Self::Drain];

    pub(super) fn from_wire(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.label().eq_ignore_ascii_case(value))
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Steer => "Steer",
            Self::Drive => "Drive",
            Self::Drain => "Drain",
        }
    }

    const fn help(self) -> &'static str {
        match self {
            Self::Steer => "Manual task filters",
            Self::Drive => "Auto-subscribe to projects this team creates or claims",
            Self::Drain => "Boundary dissolved: see all assignable work",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Steer => 0,
            Self::Drive => 1,
            Self::Drain => 2,
        }
    }
}

#[derive(Clone, Default, PartialEq)]
pub(super) struct ComposerDraft {
    body: String,
    quotes: Vec<QuoteDraft>,
    attachments: Vec<DirectiveAttachment>,
    issue: Option<String>,
}

#[derive(Clone, PartialEq)]
struct QuoteDraft {
    quote: String,
    preview: Option<String>,
    body: String,
}

#[derive(Clone, Default, PartialEq)]
pub(super) struct ComposerDrafts(BTreeMap<String, ComposerDraft>);

pub(super) enum ComposerAction {
    SetBody {
        lane: String,
        body: String,
    },
    AddQuote {
        lane: String,
        quote: String,
        preview: Option<String>,
    },
    RemoveQuote {
        lane: String,
        index: usize,
    },
    SetQuoteBody {
        lane: String,
        index: usize,
        body: String,
    },
    AddAttachment {
        lane: String,
        attachment: DirectiveAttachment,
    },
    RemoveAttachment {
        lane: String,
        index: usize,
    },
    SetIssue {
        lane: String,
        issue: String,
    },
    Clear {
        lane: String,
    },
}

impl Reducible for ComposerDrafts {
    type Action = ComposerAction;

    fn reduce(self: Rc<Self>, action: Self::Action) -> Rc<Self> {
        let mut next = (*self).clone();
        match action {
            ComposerAction::SetBody { lane, body } => next.draft_mut(lane).body = body,
            ComposerAction::AddQuote {
                lane,
                quote,
                preview,
            } => {
                let draft = next.draft_mut(lane);
                if !draft.quotes.iter().any(|draft| draft.quote == quote) {
                    draft.quotes.push(QuoteDraft {
                        quote,
                        preview,
                        body: String::new(),
                    });
                }
            }
            ComposerAction::RemoveQuote { lane, index } => {
                let draft = next.draft_mut(lane);
                if index < draft.quotes.len() {
                    draft.quotes.remove(index);
                }
            }
            ComposerAction::SetQuoteBody { lane, index, body } => {
                let draft = next.draft_mut(lane);
                if let Some(quote) = draft.quotes.get_mut(index) {
                    quote.body = body;
                }
            }
            ComposerAction::AddAttachment { lane, attachment } => {
                let draft = next.draft_mut(lane);
                if draft.attachments.len() >= MAX_ATTACHMENTS {
                    draft.issue = Some("Maximum images attached.".to_owned());
                } else {
                    draft.attachments.push(attachment);
                    draft.issue = None;
                }
            }
            ComposerAction::RemoveAttachment { lane, index } => {
                let draft = next.draft_mut(lane);
                if index < draft.attachments.len() {
                    draft.attachments.remove(index);
                }
            }
            ComposerAction::SetIssue { lane, issue } => {
                next.draft_mut(lane).issue = Some(issue);
            }
            ComposerAction::Clear { lane } => {
                next.0.remove(&lane);
            }
        }
        Rc::new(next)
    }
}

impl ComposerDrafts {
    fn draft_mut(&mut self, lane: String) -> &mut ComposerDraft {
        self.0.entry(lane).or_default()
    }

    fn get(&self, lane: &str) -> Option<&ComposerDraft> {
        self.0.get(lane)
    }
}

/// Render the one live-team surface.
///
/// This is the sole Mosaic execution path: the rail changes which facet of
/// the same four lanes is visible instead of mounting a second conversation
/// UI underneath it.
pub(super) fn render_lineage(
    surface: &SurfaceData,
    sender: &SenderHandle,
    drafts: &UseReducerHandle<ComposerDrafts>,
    mode: &UseStateHandle<LineageMode>,
    life: &UseStateHandle<LineageLife>,
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
    let lineage_weight = standings.len().max(1);
    let can_send = runtime.offers(CommandKind::SteerSend);
    html! {
        <section
            class="team-surface"
            data-pane="agent-console"
            data-view="mosaic"
            data-lineage-weight={lineage_weight.to_string()}
        >
            {render_lineage_toolbar(mode, &standings)}
            <div class="team-pane" data-team-mode={(**mode).label()}>
                {match **mode {
                    LineageMode::Compose => render_compose(
                        surface,
                        runtime,
                        &lanes.lanes,
                        &standings,
                        lanes.epoch,
                        can_send,
                        sender,
                        drafts,
                        mode,
                        life,
                        controls,
                    ),
                    LineageMode::Filters | LineageMode::Metrics | LineageMode::Info => html! {
                        <>
                            {view::chrome::render_ticker(surface)}
                            {render_details(runtime, &lanes.lanes, &standings, **mode, controls)}
                        </>
                    },
                }}
            </div>
        </section>
    }
}

fn render_lineage_toolbar(mode: &UseStateHandle<LineageMode>, standings: &[Standing]) -> Html {
    html! {
        <header class="team-toolbar">
            <span class="team-lights" aria-label={format!("{} live agents", standings.len())}>
                {for (0..4).map(|index| html! {
                    <span class={classes!(
                        "team-light",
                        (index >= standings.len()).then_some("vacant")
                    )}></span>
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

fn render_compose(
    surface: &SurfaceData,
    runtime: &CanvasRuntime,
    lanes: &[crate::runtime::LaneRow],
    standings: &[Standing],
    epoch: CanonicalUuid,
    can_send: bool,
    sender: &SenderHandle,
    drafts: &UseReducerHandle<ComposerDrafts>,
    mode: &UseStateHandle<LineageMode>,
    life: &UseStateHandle<LineageLife>,
    controls: &view::AgentControls,
) -> Html {
    if lanes.is_empty() {
        return empty("No agents are assigned. Open Info to add one.");
    }
    let submit = submit_lineage_callback(lanes, epoch, sender.clone(), drafts.clone());
    let ready = lanes.iter().any(|lane| {
        lane.occupant.is_some()
            && drafts
                .get(&lane.lane.to_string())
                .is_some_and(|draft| !compose_markdown(draft).trim().is_empty())
    });
    html! {
        <div class="team-compose">
            <form class="team-composer" onsubmit={submit.clone()}>
                <div class="team-agents">
                    {for lanes.iter().zip(standings.iter()).map(|(lane, standing)| {
                        let pending = runtime
                            .inbox()
                            .and_then(|inbox| inbox.lanes.iter().find(|row| row.lane == lane.lane))
                            .map_or(0, |row| row.pending.len());
                        render_agent_composer(
                            lane,
                            standing,
                            epoch,
                            pending,
                            can_send,
                            sender,
                            drafts,
                            submit.clone(),
                            mode,
                        )
                    })}
                </div>
                {render_lineage_controls(life, ready)}
            </form>
            {view::chrome::render_ticker(surface)}
            {render_activity(runtime, lanes, drafts, controls)}
        </div>
    }
}

fn render_activity(
    runtime: &CanvasRuntime,
    lanes: &[crate::runtime::LaneRow],
    drafts: &UseReducerHandle<ComposerDrafts>,
    controls: &view::AgentControls,
) -> Html {
    let cards = lanes.iter().flat_map(|lane| {
        runtime
            .inbox()
            .and_then(|inbox| inbox.lanes.iter().find(|row| row.lane == lane.lane))
            .map(|row| row.pending.as_slice())
            .unwrap_or(&[])
            .iter()
            .map(move |directive| (lane.lane, directive))
    });
    let cards = cards.collect::<Vec<_>>();
    html! {
        <div class="team-activity-grid" aria-live="polite" aria-label="Live agent activity">
            if cards.is_empty() {
                <div class="mosaic-message-empty">{"No pending direction."}</div>
            } else {
                {for cards.into_iter().map(|(lane, directive)| {
                    render_directive_card(lane, directive, &lane.to_string(), drafts, controls)
                })}
            }
        </div>
    }
}

fn render_details(
    runtime: &CanvasRuntime,
    lanes: &[crate::runtime::LaneRow],
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
                {for lanes.iter().zip(standings.iter()).map(|(lane, standing)| {
                    render_agent_details(lane, standing, mode)
                })}
            </div>
            if mode == LineageMode::Info {
                {view::render_seat_panel(Some(runtime), controls)}
            }
        </div>
    }
}

fn render_agent_details(
    lane: &crate::runtime::LaneRow,
    standing: &Standing,
    mode: LineageMode,
) -> Html {
    let occupant = lane
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

fn render_agent_composer(
    lane: &crate::runtime::LaneRow,
    standing: &Standing,
    epoch: CanonicalUuid,
    pending: usize,
    can_send: bool,
    sender: &SenderHandle,
    drafts: &UseReducerHandle<ComposerDrafts>,
    team_submit: Callback<SubmitEvent>,
    mode: &UseStateHandle<LineageMode>,
) -> Html {
    let lane_id = lane.lane;
    let lane_key = lane_id.to_string();
    let draft = drafts.get(&lane_key).cloned().unwrap_or_default();
    let occupant = lane.occupant;
    let send_enabled = can_send && occupant.is_some();
    let markdown = compose_markdown(&draft);
    let oninput = update_body(lane_key.clone(), drafts.clone());
    let onkeydown = keyboard_submit(team_submit.clone());
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
    let onchange = choose_images(lane_key.clone(), drafts.clone());
    let activity = standing
        .headline
        .as_deref()
        .or(standing.handle.as_deref())
        .unwrap_or(&standing.disposition);
    let open = mode.clone();
    html! {
        <section
            class={classes!("team-agent", (!send_enabled).then_some("team-agent--vacant"))}
            data-agent-lane={lane_id.to_string()}
            title={standing_title(standing)}
            {ondragover}
            {ondrop}
        >
            {render_quote_bands(&lane_key, &draft, drafts, team_submit.clone())}
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
                        aria-label={format!("Message for {lane_id}")}
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

fn render_lineage_controls(life: &UseStateHandle<LineageLife>, ready: bool) -> Html {
    let selected = **life;
    let update = life.clone();
    html! {
        <aside class="team-composer-controls">
            <label class={classes!(
                "team-lifetime",
                (selected == LineageLife::Steer).then_some("armed"),
                (selected == LineageLife::Drain).then_some("maxed"),
            )}>
                <input
                    type="range"
                    min="0"
                    max="2"
                    step="1"
                    value={selected.index().to_string()}
                    aria-label={format!("Task subscription policy: {}", selected.help())}
                    title={selected.help()}
                    oninput={Callback::from(move |event: InputEvent| {
                        let input: HtmlInputElement = event.target_unchecked_into();
                        let index = input.value().parse::<usize>().unwrap_or(0).min(2);
                        update.set(LineageLife::ALL[index]);
                    })}
                />
                <span>{selected.label()}</span>
            </label>
            <button
                class={classes!(
                    "team-send",
                    (selected == LineageLife::Steer).then_some("armed"),
                    (selected == LineageLife::Drain).then_some("maxed"),
                )}
                type="submit"
                disabled={!ready}
                title={format!("Send with {}: {}", selected.label(), selected.help())}
            >{selected.label()}</button>
        </aside>
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
    lanes: &[crate::runtime::LaneRow],
    epoch: CanonicalUuid,
    sender: SenderHandle,
    drafts: UseReducerHandle<ComposerDrafts>,
) -> Callback<SubmitEvent> {
    let targets = lanes
        .iter()
        .map(|lane| (lane.lane, lane.occupant, lane.lane.to_string()))
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
) {
    let Some(occupant) = occupant else {
        return;
    };
    let Some(draft) = drafts.get(lane_key).cloned() else {
        return;
    };
    let message = compose_markdown(&draft);
    if message.trim().is_empty() {
        return;
    }
    send_action(
        sender,
        ClientAction::Steer {
            lane,
            occupant,
            epoch,
            message,
            attachments: draft.attachments,
        },
    );
    drafts.dispatch(ComposerAction::Clear {
        lane: lane_key.to_owned(),
    });
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

fn keyboard_submit(submit: Callback<SubmitEvent>) -> Callback<KeyboardEvent> {
    Callback::from(move |event: KeyboardEvent| {
        if event.key() == "Enter" && (event.meta_key() || event.ctrl_key()) {
            event.prevent_default();
            let event = SubmitEvent::new("submit").expect("submit event construction succeeds");
            submit.emit(event);
        }
    })
}

fn render_directive_card(
    lane: CanonicalUuid,
    directive: &wire::intent::PendingDirective,
    lane_key: &str,
    drafts: &UseReducerHandle<ComposerDrafts>,
    controls: &view::AgentControls,
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
    let correspondent = view::chrome::Correspondent {
        name: watchband::short(lane),
        accent: watchband::accent(lane),
        speak: controls.speak.clone(),
        quote: Callback::from(move |text: String| {
            quote_drafts.dispatch(ComposerAction::AddQuote {
                lane: quote_key.clone(),
                quote: text,
                preview: quote_preview.clone(),
            });
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
            class="mosaic-message-card"
            draggable="true"
            data-directive={directive.directive.to_string()}
            data-agent-lane={lane.to_string()}
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
    team_submit: Callback<SubmitEvent>,
) -> Html {
    html! {
        <div class="composer-quote-stack">
            {for draft.quotes.iter().enumerate().map(|(index, quote)| {
                let remove_key = lane_key.to_owned();
                let remove_drafts = drafts.clone();
                let remove = Callback::from(move |_| {
                    remove_drafts.dispatch(ComposerAction::RemoveQuote {
                        lane: remove_key.clone(),
                        index,
                    });
                });
                let update_key = lane_key.to_owned();
                let update_drafts = drafts.clone();
                let update = Callback::from(move |event: InputEvent| {
                    let input: web_sys::HtmlTextAreaElement = event.target_unchecked_into();
                    update_drafts.dispatch(ComposerAction::SetQuoteBody {
                        lane: update_key.clone(),
                        index,
                        body: input.value(),
                    });
                });
                let shown = crate::composer_markdown::quote_label(quote.preview.as_deref());
                html! {
                    <div class="composer-quote-band" title={quote.quote.clone()}>
                        <header>
                            <span>{shown}</span>
                            <button
                                type="button"
                                aria-label="Remove quoted composer"
                                onclick={remove}
                            >{"×"}</button>
                        </header>
                        <textarea
                            aria-label="Reply to quoted message"
                            rows="2"
                            value={quote.body.clone()}
                            placeholder="Reply to this quoted message"
                            oninput={update}
                            onkeydown={keyboard_submit(team_submit.clone())}
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
        let message = markdown_block_quote(&text);
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

fn compose_markdown(draft: &ComposerDraft) -> String {
    let mut parts = Vec::new();
    let body = draft.body.trim();
    if !body.is_empty() {
        parts.push(body.to_owned());
    }
    parts.extend(draft.quotes.iter().filter_map(|draft| {
        let quote = markdown_block_quote(&draft.quote);
        let body = draft.body.trim();
        let part = [quote.as_str(), body]
            .into_iter()
            .filter(|part| !part.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        (!part.is_empty()).then_some(part)
    }));
    parts.extend(draft.attachments.iter().map(|attachment| {
        let name = attachment
            .name
            .replace(['\r', '\n'], " ")
            .replace(']', "\\]");
        format!("![{name}]({})", attachment.data_url)
    }));
    parts.join("\n\n")
}

fn markdown_block_quote(raw: &str) -> String {
    raw.replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
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
