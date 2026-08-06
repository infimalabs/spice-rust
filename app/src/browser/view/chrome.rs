//! The chrome standing above the feed.
//!
//! One bar of pills across the top, one band of lane standings under it, one
//! line of the newest command outcome under that, and the card the feed packs.
//! All four are placement and markup only: every number they show is derived
//! in [`crate::watchband`], which a native test can reach, so what is left here
//! is the part that genuinely needs a browser.

use super::*;
use crate::watchband::{self, Standing};

/// Characters the ticker line is allowed.
///
/// The ticker is one line of monospace across the full width, and a command
/// echo runs far past that; the budget cuts it here rather than letting the
/// row grow and push the feed down.
const TICKER_BUDGET: usize = 160;

/// Milliseconds since the epoch, as the browser reads them.
fn now_millis() -> i64 {
    js_sys::Date::now() as i64
}

/// The bar of view pills, each wearing what its axis currently counts.
pub(super) fn render_command_bar(
    axis: &UseStateHandle<Axis>,
    mosaic: &UseStateHandle<bool>,
    active_axis: Axis,
    surface: &SurfaceData,
    cursor: &str,
) -> Html {
    let runtime = surface.as_ref();
    html! {
        <header class="commandbar">
            <div class="brand">
                <span class="brand-dots" aria-hidden="true">
                    {for (0..4).map(|_| html! { <span class="brand-dot"></span> })}
                </span>
                <span class="brand-name">{"Spice Canvas"}</span>
            </div>
            <nav class="pills" aria-label="Canvas views">
                {render_pill(
                    "Mosaic",
                    "Converse with seated agents",
                    **mosaic,
                    watchband::badge(waiting_directives(runtime)),
                    {
                        let mosaic = mosaic.clone();
                        Callback::from(move |_| mosaic.set(true))
                    },
                )}
                {for Axis::ALL.iter().copied().map(|candidate| {
                    let axis = axis.clone();
                    let mosaic = mosaic.clone();
                    render_pill(
                        &title(candidate.as_str()),
                        candidate.question(),
                        !*mosaic && candidate == active_axis,
                        watchband::badge(axis_count(candidate, runtime)),
                        Callback::from(move |_| {
                            axis.set(candidate);
                            mosaic.set(false);
                        }),
                    )
                })}
            </nav>
            <span class="bar-meta">{format!("cursor {cursor}")}</span>
        </header>
    }
}

fn render_pill(
    label: &str,
    question: &str,
    selected: bool,
    badge: Option<String>,
    choose: Callback<MouseEvent>,
) -> Html {
    html! {
        <button
            class={classes!("axis-button", "pill", selected.then_some("selected"))}
            type="button"
            aria-current={selected.then_some("page")}
            title={question.to_owned()}
            onclick={choose}
        >
            <span class="pill-label">{label.to_owned()}</span>
            if let Some(count) = badge {
                <span class="pill-badge">{count}</span>
            }
        </button>
    }
}

/// Directives waiting across every lane, which is what the feed is for.
fn waiting_directives(runtime: Option<&CanvasRuntime>) -> usize {
    runtime.and_then(CanvasRuntime::inbox).map_or(0, |inbox| {
        inbox.lanes.iter().map(|lane| lane.pending.len()).sum()
    })
}

/// What one axis currently counts, or zero when its answer has not arrived.
///
/// Each count is the size of what that axis's own panes render, so a badge
/// never advertises more than the pill leads to.
fn axis_count(axis: Axis, runtime: Option<&CanvasRuntime>) -> usize {
    match axis {
        Axis::Magnitude => runtime
            .and_then(CanvasRuntime::policy)
            .map_or(0, |policy| policy.rules.len()),
        Axis::Flow => runtime
            .and_then(CanvasRuntime::board)
            .map_or(0, |board| board.active_public_tasks().count()),
        Axis::Topology => runtime
            .and_then(CanvasRuntime::lanes)
            .map_or(0, |lanes| lanes.lanes.len()),
        Axis::Chronology => runtime.map_or(0, |runtime| runtime.events().len()),
    }
}

/// The band of lane standings, one card per seated lane.
///
/// The cards remain the tablist that selects a conversation; what changed is
/// how much each one says while doing it.
pub(super) fn render_watchband(surface: &SurfaceData, controls: &AgentControls) -> Html {
    let runtime = surface.as_ref();
    let lanes = runtime
        .and_then(CanvasRuntime::lanes)
        .map(|lanes| lanes.lanes.as_slice())
        .unwrap_or(&[]);
    let waiting = runtime
        .and_then(CanvasRuntime::inbox)
        .map(|inbox| inbox.lanes.as_slice())
        .unwrap_or(&[]);
    let tasks = runtime
        .and_then(CanvasRuntime::board)
        .map(|board| board.tasks.as_slice())
        .unwrap_or(&[]);
    let standings = watchband::standings(lanes, waiting, tasks, now_millis());
    let selected = selected_agent(lanes, &controls.selected_lane).map(|lane| lane.lane);
    html! {
        <div class="agent-tabs watchband" role="tablist" aria-label="Seated agents">
            if standings.is_empty() {
                <span class="agent-tab-empty">{"No agents seated"}</span>
            } else {
                {for standings.iter().map(|standing| {
                    render_standing(
                        standing,
                        selected == Some(standing.lane),
                        controls.selected_lane.clone(),
                    )
                })}
            }
        </div>
    }
}

fn render_standing(standing: &Standing, selected: bool, choose: UseStateHandle<String>) -> Html {
    let lane = standing.lane.to_string();
    let label = format!(
        "Open {} lane {} in {}",
        standing.vendor, standing.name, standing.worktree
    );
    let holding = standing.handle.as_ref().zip(standing.phase.as_ref());
    html! {
        <button
            class={classes!("agent-tab", "watchband-card", selected.then_some("selected"))}
            type="button" role="tab"
            aria-selected={selected.to_string()} aria-label={label}
            data-lane={lane.clone()} data-accent={standing.accent.to_string()}
            title={standing.headline.clone().unwrap_or_else(|| standing.disposition.clone())}
            onclick={Callback::from(move |_| choose.set(lane.clone()))}
        >
            <span class="watchband-line">
                <span class="agent-presence"></span>
                <span class="watchband-name">{watchband::truncate(&standing.label, watchband::FIELD_BUDGET)}</span>
                <span class="tag vendor">{standing.vendor.clone()}</span>
            </span>
            <span class="watchband-line muted">
                {format!("{} pending", standing.pending)}
                {", "}
                {standing.disposition.clone()}
                if let Some(held) = standing.held {
                    {format!(" {}", watchband::elapsed_label(held))}
                }
            </span>
            <span class="watchband-line mono">
                if let Some((handle, phase)) = holding {
                    {format!("{handle}, {phase}")}
                } else {
                    {"no claim"}
                }
            </span>
            if let Some(renewed) = standing.renewed {
                <span class="watchband-line muted">
                    {format!("renewed {}", watchband::elapsed_label(renewed))}
                </span>
            }
        </button>
    }
}

/// The newest command outcome, folded onto one line.
pub(super) fn render_ticker(surface: &SurfaceData) -> Html {
    let Some(event) = surface.as_ref().and_then(|runtime| runtime.events().last()) else {
        return Html::default();
    };
    let detail = if event.detail().is_empty() {
        event.title()
    } else {
        event.detail()
    };
    html! {
        <div class={classes!("ticker", event.class())} aria-live="polite">
            {watchband::ticker_line(event.title(), detail, TICKER_BUDGET)}
        </div>
    }
}

/// One packed card: the lattice decides where, this says what.
pub(super) fn render_mosaic_card(
    (directive, card): (&wire::intent::PendingDirective, &crate::mosaic::Card),
    max_row: i32,
    correspondent: &Correspondent,
) -> Html {
    let state = card.lifecycle.as_str();
    let text = directive.text.clone();
    html! {
        <article
            class={classes!("message", "outgoing", "mosaic-card", state)}
            data-state={state}
            data-key={directive.key.clone()}
            data-accent={correspondent.accent.to_string()}
            data-track={card.track.to_string()}
            data-span={card.span.to_string()}
            data-base={card.base.to_string()}
            data-row={crate::mosaic::css_row_start(card, max_row).to_string()}
            data-rows={card.rows.to_string()}
        >
            <p>{text.clone()}</p>
            {render_attachments(&directive.attachments)}
            <footer class="message-meta">
                <span class="message-key">{directive.key.clone()}</span>
                <span class="message-age" title={directive.expires.to_string()}>
                    {format!(
                        "{} left",
                        watchband::elapsed_label(
                            directive.expires.get().as_millisecond() - now_millis(),
                        ),
                    )}
                </span>
                <span class="message-author">{correspondent.name.clone()}</span>
                {render_card_actions(&text, correspondent)}
            </footer>
            <span class="accent-rail" aria-hidden="true"></span>
        </article>
    }
}

/// Who the card is addressed to, and the colour that says so.
///
/// A pending directive carries no author of its own: the operator wrote every
/// one of them. What distinguishes cards is the lane they are waiting on, so
/// that is what the accent and the footer name.
#[derive(Clone, PartialEq)]
pub(super) struct Correspondent {
    pub(super) name: String,
    pub(super) accent: usize,
    pub(super) speak: Callback<String>,
    pub(super) quote: Callback<String>,
}

fn render_card_actions(text: &str, correspondent: &Correspondent) -> Html {
    let speak = correspondent.speak.clone();
    let quote = correspondent.quote.clone();
    let spoken = text.to_owned();
    let quoted = text.to_owned();
    let copied = text.to_owned();
    html! {
        <span class="message-actions">
            <button
                class="card-action" type="button" title="Speak this message"
                aria-label="Speak this message"
                onclick={Callback::from(move |_| speak.emit(spoken.clone()))}
            >{"▶"}</button>
            <button
                class="card-action" type="button" title="Quote this message"
                aria-label="Quote this message"
                onclick={Callback::from(move |_| quote.emit(quoted.clone()))}
            >{"❞"}</button>
            <button
                class="card-action" type="button" title="Copy this message"
                aria-label="Copy this message"
                onclick={Callback::from(move |_| copy_text(&copied))}
            >{"⧉"}</button>
        </span>
    }
}

/// Hand `text` to the platform clipboard, ignoring a refusal.
///
/// Reached reflectively rather than through a typed binding: `web-sys` gates
/// the clipboard behind an unstable-API flag that would have to be set for the
/// whole build, and the call is three property reads either way.
fn copy_text(text: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let clipboard = js_sys::Reflect::get(&window, &JsValue::from_str("navigator"))
        .and_then(|navigator| js_sys::Reflect::get(&navigator, &JsValue::from_str("clipboard")));
    let Ok(clipboard) = clipboard else {
        return;
    };
    let write = js_sys::Reflect::get(&clipboard, &JsValue::from_str("writeText"))
        .ok()
        .and_then(|write| write.dyn_into::<js_sys::Function>().ok());
    if let Some(write) = write {
        let _ = write.call1(&clipboard, &JsValue::from_str(text));
    }
}

pub(super) fn render_attachments(attachments: &[wire::body::DirectiveAttachment]) -> Html {
    if attachments.is_empty() {
        return Html::default();
    }
    html! {
        <div class="message-attachments">
            {for attachments.iter().map(|attachment| {
                if attachment.content_type.starts_with("image/") {
                    html! {
                        <figure>
                            <img src={attachment.data_url.clone()} alt={attachment.name.clone()} />
                            <figcaption>{attachment.name.clone()}</figcaption>
                        </figure>
                    }
                } else {
                    html! { <span class="message-attachment">{attachment.name.clone()}</span> }
                }
            })}
        </div>
    }
}
