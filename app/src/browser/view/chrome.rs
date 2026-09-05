//! The chrome standing above the feed.
//!
//! One bar of pills across the top, one band of lane standings under it, one
//! line of the newest command outcome under that, and the card the feed packs.
//! All four are placement and markup only: every number they show is derived
//! in [`crate::watchband`], which a native test can reach, so what is left here
//! is the part that genuinely needs a browser.

use super::*;
use crate::browser::clock::Age;
use crate::task_pill::{TaskPill, TaskPillAvailability, TaskPillIndex};
use crate::watchband;

/// Characters the ticker line is allowed.
///
/// The ticker is one line of monospace across the full width, and a command
/// echo runs far past that; the budget cuts it here rather than letting the
/// row grow and push the feed down.
const TICKER_BUDGET: usize = 160;

/// The bar of view pills, each wearing what its axis currently counts.
pub(super) struct CommandBarRender<'a> {
    pub(super) axis: &'a UseStateHandle<Axis>,
    pub(super) mosaic: &'a UseStateHandle<bool>,
    pub(super) menu_open: &'a UseStateHandle<bool>,
    pub(super) active_axis: Axis,
    pub(super) surface: &'a SurfaceData,
    pub(super) cursor: &'a str,
    pub(super) sender: &'a SenderHandle,
    pub(super) submissions: &'a SubmissionStatus,
    pub(super) lineage_controls: &'a UseReducerHandle<LineageControls>,
}

pub(super) fn render_command_bar(render: CommandBarRender<'_>) -> Html {
    let surface = render.surface.read();
    let runtime = surface.as_ref();
    let task_pills = task_pills(runtime);
    html! {
        <header class="commandbar commandbar--mosaic">
            if **render.menu_open {
                {render_navigation(&render, runtime)}
            } else {
                <nav class="task-pill-strip" aria-label="Open task projects">
                    if task_pills.is_empty() {
                        <span class="task-pill-empty">{"No open tasks"}</span>
                    } else {
                        {for task_pills.into_iter().map(|pill| {
                            let axis = render.axis.clone();
                            let mosaic = render.mosaic.clone();
                            html! {
                                <button
                                    class={classes!(
                                        "task-pill",
                                        pill.hidden.then_some("system"),
                                        (pill.active > 0).then_some("active"),
                                        (pill.ready == 0 && pill.active == 0).then_some("dormant"),
                                    )}
                                    type="button"
                                    title={pill.title()}
                                    data-task-stem={pill.stem.clone()}
                                    data-ready={pill.ready.to_string()}
                                    data-active={pill.active.to_string()}
                                    data-unavailable={pill.unavailable.to_string()}
                                    onclick={Callback::from(move |_| {
                                        axis.set(Axis::Flow);
                                        mosaic.set(false);
                                    })}
                                >
                                    <span>{pill.stem.clone()}</span>
                                    <span class="task-pill-count">{pill.count()}</span>
                                </button>
                            }
                        })}
                    }
                </nav>
            }
            {render_canvas_menu(&render, runtime)}
        </header>
    }
}

fn render_navigation(render: &CommandBarRender<'_>, runtime: Option<&CanvasRuntime>) -> Html {
    let destinations = [
        Axis::Flow,
        Axis::Magnitude,
        Axis::Chronology,
        Axis::Topology,
    ];
    html! {
        <nav class="canvas-navigation" aria-label="Canvas views">
            {for destinations.map(|candidate| {
                let axis = render.axis.clone();
                let mosaic = render.mosaic.clone();
                let menu_open = render.menu_open.clone();
                let sender = render.sender.clone();
                render_pill(
                    super::axis_label(candidate),
                    candidate.question(),
                    !**render.mosaic && candidate == render.active_axis,
                    watchband::badge(axis_count(candidate, runtime)),
                    Callback::from(move |_| {
                        if candidate == Axis::Magnitude {
                            send_action(&sender, ClientAction::Effort);
                        }
                        axis.set(candidate);
                        mosaic.set(false);
                        menu_open.set(false);
                    }),
                )
            })}
            {render_pill(
                "Agents",
                "Converse with seated agents",
                **render.mosaic,
                watchband::badge(waiting_directives(runtime, render.submissions)),
                {
                    let mosaic = render.mosaic.clone();
                    let menu_open = render.menu_open.clone();
                    Callback::from(move |_| {
                        mosaic.set(true);
                        menu_open.set(false);
                    })
                },
            )}
        </nav>
    }
}

fn render_canvas_menu(render: &CommandBarRender<'_>, runtime: Option<&CanvasRuntime>) -> Html {
    let fast_mode = runtime
        .and_then(CanvasRuntime::settings)
        .is_some_and(|settings| settings.fast_mode);
    let menu_open = render.menu_open.clone();
    let toggle = Callback::from(move |event: MouseEvent| {
        event.prevent_default();
        menu_open.set(!*menu_open);
    });
    html! {
        <details class="canvas-menu" open={**render.menu_open}>
            <summary
                class={fast_mode.then_some("canvas-menu-trigger--fast")}
                aria-label="Open Spice Canvas menu"
                aria-haspopup="menu"
                aria-expanded={(**render.menu_open).to_string()}
                title={if fast_mode {
                    "Open Spice Canvas menu - fast mode on"
                } else {
                    "Open Spice Canvas menu"
                }}
                onclick={toggle}
            >
                <span class="canvas-menu-flame" aria-hidden="true">{"🔥"}</span>
                <span class="canvas-menu-pepper" aria-hidden="true">{"🌶️"}</span>
            </summary>
            <div class="canvas-menu-panel">
                {super::lineage_menu::render(
                    **render.mosaic,
                    render.active_axis,
                    axis_count(render.active_axis, runtime),
                    runtime,
                    render.cursor,
                    render.sender,
                    render.lineage_controls,
                )}
            </div>
        </details>
    }
}

fn task_pills(runtime: Option<&CanvasRuntime>) -> Vec<TaskPill> {
    let Some(board) = runtime.and_then(CanvasRuntime::board) else {
        return Vec::new();
    };
    let mut pills = TaskPillIndex::default();
    for task in &board.tasks {
        let workflow = &task.workflow.body;
        if workflow.status != "pending" || workflow.completion.is_some() {
            continue;
        }
        let hidden = task.visibility.is_hidden();
        let stem = task
            .visibility
            .hidden_stem()
            .map(str::to_owned)
            .unwrap_or_else(|| {
                task.statement
                    .body
                    .project
                    .trim_start_matches('.')
                    .split('.')
                    .next()
                    .unwrap_or("task")
                    .to_owned()
            });
        let availability = if task
            .claim
            .as_ref()
            .is_some_and(|claim| claim.body.disposition == wire::head::ClaimDisposition::Active)
        {
            TaskPillAvailability::Active
        } else if workflow.deferral.is_some()
            || task.statement.body.dependencies.iter().any(|dependency| {
                board.tasks.iter().any(|candidate| {
                    candidate.statement.body.born == *dependency
                        && candidate.workflow.body.completion.is_none()
                })
            })
        {
            TaskPillAvailability::Unavailable
        } else {
            TaskPillAvailability::Ready
        };
        pills.record(hidden, stem, availability);
    }
    pills.into_pills()
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
fn waiting_directives(runtime: Option<&CanvasRuntime>, submissions: &SubmissionStatus) -> usize {
    let waiting = runtime
        .and_then(CanvasRuntime::inbox)
        .map(|inbox| inbox.lanes.as_slice())
        .unwrap_or(&[]);
    submissions.total_pending(waiting.iter().map(|row| (row.lane, row.pending.as_slice())))
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

/// Newest activity from any member of one lineage.
///
/// Team strips never consult command receipts. Those remain visible in the
/// chronology pane, while this line follows captured transcript activity
/// exclusively.
pub(in crate::browser) fn render_lineage_ticker(
    runtime: &CanvasRuntime,
    lineage: &crate::runtime::LaneRow,
    standings: &[watchband::Standing],
) -> Html {
    let Some(activity) = runtime.newest_lineage_activity(lineage) else {
        return html! {
            <div class="ticker" aria-live="polite" data-empty="true"></div>
        };
    };
    let standing = standings
        .iter()
        .find(|standing| standing.lane == activity.lane())
        .expect("a lineage activity belongs to one rendered standing");
    let member = lineage
        .member(activity.lane())
        .expect("a lineage activity belongs to one rendered member");
    ticker(activity, standing, member.status(standing.pending))
}

fn ticker(
    activity: &crate::runtime::LaneActivity,
    standing: &watchband::Standing,
    status: crate::runtime::LaneStatus,
) -> Html {
    let title = activity.title();
    let detail = if activity.detail().is_empty() {
        title
    } else {
        activity.detail()
    };
    let line = watchband::ticker_line(title, detail, TICKER_BUDGET);
    let prefix = format!("{title}:");
    let detail = line
        .strip_prefix(&prefix)
        .map(str::trim_start)
        .unwrap_or_default();
    html! {
        <div
            class={classes!("ticker", activity.class())}
            aria-live="polite"
            aria-label={format!("{}: {line}", standing.label)}
            data-agent-lane={activity.lane().to_string()}
            data-accent={standing.accent.to_string()}
            data-lane-status={status.key()}
        >
            <Age
                authored={activity.display_millis()}
                class={classes!("ticker-age", status.key())}
                title={format!("{} {title}", standing.label)}
            />
            {" "}
            <strong class="ticker-prefix">{prefix}</strong>
            if !detail.is_empty() {
                {" "}
                <span class="ticker-detail">{detail}</span>
            }
        </div>
    }
}

/// Who the card is addressed to, and the colour that says so.
///
/// For assistant prose this is the immutable producing worktree. A pending
/// direction carries no agent author, so its correspondent is the member the
/// operator addressed.
#[derive(Clone, PartialEq)]
pub(in crate::browser) struct Correspondent {
    pub(in crate::browser) accent: usize,
    pub(in crate::browser) speak: Callback<String>,
    pub(in crate::browser) speaking: bool,
    pub(in crate::browser) quote: Callback<()>,
}

pub(in crate::browser) fn render_card_actions(text: &str, correspondent: &Correspondent) -> Html {
    let speak = correspondent.speak.clone();
    let quote = correspondent.quote.clone();
    let spoken = text.to_owned();
    let copied = text.to_owned();
    let speech_label = if correspondent.speaking {
        "Stop playback"
    } else {
        "Speak this message"
    };
    html! {
        <span class="message-actions">
            <button
                class={classes!(
                    "card-action",
                    "card-action--speak",
                    correspondent.speaking.then_some("active"),
                )}
                type="button" title={speech_label}
                aria-label={speech_label}
                onclick={Callback::from(move |_| speak.emit(spoken.clone()))}
            >{
                if correspondent.speaking {
                    html! {
                        <svg
                            class="card-action-icon"
                            data-card-action-icon="stop"
                            width="15" height="15" viewBox="0 0 24 24"
                            aria-hidden="true" focusable="false"
                        >
                            <rect x="7" y="7" width="10" height="10" rx="1.5" fill="currentColor" />
                        </svg>
                    }
                } else {
                    html! {
                        <svg
                            class="card-action-icon"
                            data-card-action-icon="play"
                            width="15" height="15" viewBox="0 0 24 24"
                            aria-hidden="true" focusable="false"
                        >
                            <path d="M8 5v14l11-7Z" fill="currentColor" />
                        </svg>
                    }
                }
            }</button>
            <button
                class="card-action card-action--quote" type="button" title="Quote this message"
                aria-label="Quote this message"
                onclick={Callback::from(move |_| quote.emit(()))}
            ><span class="card-action-icon" aria-hidden="true">{"❝"}</span></button>
            <button
                class="card-action card-action--copy" type="button" title="Copy this message"
                aria-label="Copy this message"
                onclick={Callback::from(move |_| copy_text(&copied))}
            ><span class="card-action-icon" aria-hidden="true">{"⧉"}</span></button>
        </span>
    }
}

/// Hand `text` to the platform clipboard, ignoring a refusal.
///
/// Reached reflectively rather than through a typed binding: `web-sys` gates
/// the clipboard behind an unstable-API flag that would have to be set for the
/// whole build, and the call is three property reads either way.
pub(in crate::browser) fn copy_text(text: &str) {
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
