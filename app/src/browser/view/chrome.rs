//! The chrome standing above the feed.
//!
//! One bar of project pills and the connectivity-bearing Spice button across the top, one
//! band of lane standings under it, one line of the newest command outcome
//! under that, and the card the feed packs. All four are placement and markup
//! only: every number they show is derived in [`crate::watchband`], which a
//! native test can reach, so what is left here is the part that genuinely
//! needs a browser.
//!
//! The connection state lives in this bar because this bar is the one piece of
//! chrome every layout renders. The surface keeps the last admitted runtime on
//! screen through an outage and through a restarted resident's hydration, so a
//! layout that hid the state would show a live-looking surface whose composers
//! accept nothing.

use super::*;
use crate::browser::clock::Age;
use crate::task_pill::TaskPill;
use crate::watchband;
use crate::work::WorkProjection;

/// Characters the ticker line is allowed.
///
/// The ticker is one line of monospace across the full width, and a command
/// echo runs far past that; the budget cuts it here rather than letting the
/// row grow and push the feed down.
const TICKER_BUDGET: usize = 160;

/// The bar of view pills, each wearing what its axis currently counts.
pub(super) struct CommandBarRender<'a> {
    pub(super) connection: &'a UseStateHandle<Connection>,
    pub(super) destination: CanvasDestination,
    pub(super) navigate: &'a Callback<navigation::Intent>,
    pub(super) menu_open: &'a UseStateHandle<bool>,
    pub(super) work_scope: &'a UseStateHandle<WorkScope>,
    pub(super) work: Option<&'a WorkProjection<'a>>,
    pub(super) identity: &'a UseStateHandle<Option<CanvasIdentity>>,
    pub(super) canvas_update: &'a UseStateHandle<Option<CanvasUpdate>>,
    pub(super) surface: &'a SurfaceData,
    pub(super) cursor: &'a str,
    pub(super) sender: &'a SenderHandle,
    pub(super) submissions: &'a SubmissionStatus,
    pub(super) team_controls: &'a UseReducerHandle<TeamControls>,
}

pub(super) fn render_command_bar(render: CommandBarRender<'_>) -> Html {
    let surface = render.surface.read();
    let runtime = surface.as_ref();
    let task_pills = task_pills(render.work);
    let connection_label: AttrValue = render.connection.label().into();
    html! {
        <header class="commandbar commandbar--mosaic">
            <div
                class="canvas-disclosure"
                data-expanded={(**render.menu_open).to_string()}
            >
                <nav class="task-pill-strip" aria-label="Open task projects">
                    {for task_pills.into_iter().map(|pill| {
                        let navigate = render.navigate.clone();
                        let identity = pill.identity.clone();
                        let selected = render.destination == CanvasDestination::Work
                            && render.work_scope.stem() == Some(&pill.identity);
                        let key = identity.to_string();
                        let title = pill.title();
                        html! {
                            <button
                                key={key}
                                class={classes!(
                                    "task-pill",
                                    pill.hidden.then_some("system"),
                                    (pill.active > 0).then_some("active"),
                                    (pill.ready == 0 && pill.active == 0).then_some("dormant"),
                                    selected.then_some("selected"),
                                )}
                                type="button"
                                title={title.clone()}
                                aria-label={title}
                                aria-pressed={selected.to_string()}
                                aria-current={selected.then_some("page")}
                                data-task-stem={pill.stem.clone()}
                                data-task-visibility={if pill.hidden { "hidden" } else { "public" }}
                                data-ready={pill.ready.to_string()}
                                data-active={pill.active.to_string()}
                                data-unavailable={pill.unavailable.to_string()}
                                onclick={Callback::from(move |_| {
                                    navigate.emit(navigation::Intent::Work(WorkScope::Stem(
                                        identity.clone(),
                                    )));
                                })}
                            >
                                <span>{pill.stem.clone()}</span>
                                <span class="task-pill-count">{pill.count()}</span>
                            </button>
                        }
                    })}
                </nav>
                <span class="visually-hidden" role="status">{connection_label.clone()}</span>
                {render_canvas_menu(&render, runtime, &connection_label)}
            </div>
        </header>
    }
}

/// The destination rail atop the Spice menu.
///
/// The open menu frame lies over the team surface beneath it, so this rail
/// wears the team toolbar's own rail classes and lands on the same band.
fn render_navigation(render: &CommandBarRender<'_>, runtime: Option<&CanvasRuntime>) -> Html {
    html! {
        <nav class="canvas-navigation canvas-control-rail team-mode-rail" aria-label="Canvas views">
            {for CanvasDestination::ALL.map(|destination| {
                let navigate = render.navigate.clone();
                let selected = destination == render.destination;
                let intent = match destination {
                    CanvasDestination::Info => navigation::Intent::Info,
                    CanvasDestination::Work => navigation::Intent::Work(WorkScope::AllOpen),
                    CanvasDestination::Chat => navigation::Intent::Chat,
                };
                let badge = navigation_badge(destination, runtime, render.submissions);
                html! {
                    <button
                        class={classes!("team-mode", selected.then_some("selected"))}
                        type="button"
                        aria-current={selected.then_some("page")}
                        title={destination.description()}
                        onclick={Callback::from(move |_| navigate.emit(intent.clone()))}
                    >
                        <span class="team-mode-label">{destination.label()}</span>
                        if let Some(count) = badge {
                            <span class="team-mode-badge">{count}</span>
                        }
                    </button>
                }
            })}
        </nav>
    }
}

fn navigation_badge(
    destination: CanvasDestination,
    runtime: Option<&CanvasRuntime>,
    submissions: &SubmissionStatus,
) -> Option<String> {
    destination.axis().map_or_else(
        || watchband::badge(waiting_directives(runtime, submissions)),
        |axis| watchband::badge(axis_count(axis, runtime)),
    )
}

fn render_canvas_menu(
    render: &CommandBarRender<'_>,
    runtime: Option<&CanvasRuntime>,
    connection_label: &str,
) -> Html {
    let fast_mode = runtime
        .and_then(CanvasRuntime::settings)
        .is_some_and(|settings| settings.fast_mode);
    let menu_open = render.menu_open.clone();
    let update = (**render.canvas_update).as_ref();
    let trigger_label = match (update, fast_mode) {
        (Some(_), true) => "Open Spice Canvas menu - update available; fast mode on",
        (Some(_), false) => "Open Spice Canvas menu - update available",
        (None, true) => "Open Spice Canvas menu - fast mode on",
        (None, false) => "Open Spice Canvas menu",
    };
    let trigger_label: AttrValue = format!("{trigger_label} - {connection_label}").into();
    let toggle = Callback::from(move |event: MouseEvent| {
        event.prevent_default();
        menu_open.set(!*menu_open);
    });
    html! {
        <details class="canvas-menu" open={**render.menu_open}>
            <summary
                class={classes!(
                    fast_mode.then_some("canvas-menu-trigger--fast"),
                    update.is_some().then_some("canvas-menu-trigger--update"),
                )}
                data-connection={render.connection.indicator_state()}
                aria-label={trigger_label.clone()}
                aria-haspopup="menu"
                aria-expanded={(**render.menu_open).to_string()}
                title={trigger_label}
                onclick={toggle}
            >
                <span class="canvas-menu-flame" aria-hidden="true">{"🔥"}</span>
                <span class="canvas-menu-pepper" aria-hidden="true">{"🌶️"}</span>
            </summary>
            <div class="canvas-menu-frame">
                {render_navigation(render, runtime)}
                <div class="canvas-menu-panel">
                    {super::team_menu::render(
                        render.destination,
                        runtime,
                        &super::team_menu::Context {
                            cursor: render.cursor,
                            identity: (**render.identity).as_ref(),
                            update,
                            sender: render.sender,
                            team_controls: render.team_controls,
                            submissions: render.submissions,
                        },
                    )}
                </div>
            </div>
        </details>
    }
}

fn task_pills(work: Option<&WorkProjection<'_>>) -> Vec<TaskPill> {
    work.into_iter()
        .flat_map(WorkProjection::stems)
        .filter(|stem| stem.counts().has_open())
        .map(TaskPill::from_stem)
        .collect()
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
pub(super) fn axis_count(axis: Axis, runtime: Option<&CanvasRuntime>) -> usize {
    match axis {
        Axis::Flow => runtime
            .and_then(CanvasRuntime::board)
            .map_or(0, |board| board.active_public_tasks().count()),
        Axis::Topology => runtime
            .and_then(CanvasRuntime::lanes)
            .map_or(0, |lanes| lanes.lanes.len()),
    }
}

/// Newest activity from any member of one team.
pub(in crate::browser) fn render_team_ticker(
    runtime: &CanvasRuntime,
    team: &crate::runtime::LaneRow,
    standings: &[watchband::Standing],
) -> Html {
    let Some(activity) = runtime.newest_team_activity(team) else {
        return html! {
            <div class="ticker" aria-live="polite" data-empty="true"></div>
        };
    };
    let standing = standings
        .iter()
        .find(|standing| standing.lane == activity.lane())
        .expect("a team activity belongs to one rendered standing");
    let member = team
        .member(activity.lane())
        .expect("a team activity belongs to one rendered member");
    ticker(activity, standing, member.status(standing.pending))
}

fn ticker(
    activity: &crate::runtime::LaneActivity,
    standing: &watchband::Standing,
    status: crate::runtime::LaneStatus,
) -> Html {
    let class = activity.title();
    let visible_prefix = if activity.class() == "prose" {
        standing.label.as_str()
    } else {
        class
    };
    let source = if activity.detail().is_empty() && class == "tool" {
        visible_prefix
    } else {
        activity.detail()
    };
    let (prefix, detail) = watchband::ticker_chunks(visible_prefix, source, TICKER_BUDGET);
    let separator = if class == "tool" || detail.is_empty() {
        ""
    } else {
        ":"
    };
    let line = if detail.is_empty() {
        format!("{prefix}{separator}")
    } else {
        format!("{prefix}{separator} {detail}")
    };
    let accessible_label =
        watchband::ticker_accessible_label(&standing.label, class, &line, TICKER_BUDGET);
    html! {
        <div
            class={classes!("ticker", activity.class())}
            aria-live="polite"
            aria-label={accessible_label}
            data-agent-lane={activity.lane().to_string()}
            data-accent={standing.accent.to_string()}
            data-lane-status={status.key()}
        >
            <Age
                authored={activity.display_millis()}
                class={classes!("ticker-age", status.key())}
                title={format!("{} {class}", standing.label)}
            />
            {" "}
            <strong class="ticker-prefix">{format!("{prefix}{separator}")}</strong>
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
