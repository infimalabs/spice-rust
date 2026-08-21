//! The chrome standing above the feed.
//!
//! One bar of pills across the top, one band of lane standings under it, one
//! line of the newest command outcome under that, and the card the feed packs.
//! All four are placement and markup only: every number they show is derived
//! in [`crate::watchband`], which a native test can reach, so what is left here
//! is the part that genuinely needs a browser.

use super::*;
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
    if **render.mosaic {
        return render_mosaic_command_bar(&render, runtime);
    }
    html! {
        <header class="commandbar">
            <div class="brand">
                {render_agent_roster(runtime, render.submissions)}
                <span class="brand-name">{"Spice Canvas"}</span>
            </div>
            <nav class="pills" aria-label="Canvas views">
                {render_pill(
                    "Agents",
                    "Converse with seated agents",
                    **render.mosaic,
                    watchband::badge(waiting_directives(runtime, render.submissions)),
                    {
                        let mosaic = render.mosaic.clone();
                        Callback::from(move |_| mosaic.set(true))
                    },
                )}
                {for Axis::ALL.iter().copied().map(|candidate| {
                    let axis = render.axis.clone();
                    let mosaic = render.mosaic.clone();
                    render_pill(
                        &title(candidate.as_str()),
                        candidate.question(),
                        !**render.mosaic && candidate == render.active_axis,
                        watchband::badge(axis_count(candidate, runtime)),
                        Callback::from(move |_| {
                            axis.set(candidate);
                            mosaic.set(false);
                        }),
                    )
                })}
            </nav>
            <span class="bar-meta">{format!("cursor {}", render.cursor)}</span>
            {render_canvas_menu(&render, runtime)}
        </header>
    }
}

#[derive(Default)]
struct TaskPill {
    stem: String,
    hidden: bool,
    ready: usize,
    active: usize,
    unavailable: usize,
}

impl TaskPill {
    fn count(&self) -> String {
        if self.hidden {
            (self.ready + self.active + self.unavailable).to_string()
        } else {
            format!("{}·{}·{}", self.ready, self.active, self.unavailable)
        }
    }

    fn title(&self) -> String {
        if self.hidden {
            return format!("{} deferred or system tasks", self.count());
        }
        format!(
            "{} ready, {} active or in flight, {} blocked or deferred across {}.*",
            self.ready, self.active, self.unavailable, self.stem
        )
    }
}

fn render_mosaic_command_bar(
    render: &CommandBarRender<'_>,
    runtime: Option<&CanvasRuntime>,
) -> Html {
    let task_pills = task_pills(runtime);
    html! {
        <header class="commandbar commandbar--mosaic">
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
            <span class="bar-meta">{format!("cursor {}", render.cursor)}</span>
            <nav class="canvas-shortcuts" aria-label="Canvas context views">
                {render_pill(
                    "Agents",
                    "Converse with seated agents",
                    **render.mosaic,
                    watchband::badge(waiting_directives(runtime, render.submissions)),
                    {
                        let mosaic = render.mosaic.clone();
                        Callback::from(move |_| mosaic.set(true))
                    },
                )}
                {for [Axis::Topology, Axis::Chronology].map(|candidate| {
                    let axis = render.axis.clone();
                    let mosaic = render.mosaic.clone();
                    render_pill(
                        &title(candidate.as_str()),
                        candidate.question(),
                        !**render.mosaic && candidate == render.active_axis,
                        watchband::badge(axis_count(candidate, runtime)),
                        Callback::from(move |_| {
                            axis.set(candidate);
                            mosaic.set(false);
                        }),
                    )
                })}
            </nav>
            {render_canvas_menu(render, runtime)}
        </header>
    }
}

fn render_canvas_menu(render: &CommandBarRender<'_>, runtime: Option<&CanvasRuntime>) -> Html {
    html! {
        <details class="canvas-menu">
            <summary aria-label="Open Spice Canvas menu">
                <span aria-hidden="true">{"🌶️"}</span>
                <span>{"Spice"}</span>
            </summary>
            <div class="canvas-menu-panel">
                {super::lineage_menu::render(
                    **render.mosaic,
                    render.active_axis,
                    axis_count(render.active_axis, runtime),
                    runtime,
                    render.sender,
                    render.lineage_controls,
                )}
                <nav class="canvas-menu-views" aria-label="More Canvas views">
                    {for [Axis::Magnitude, Axis::Flow].map(|candidate| {
                        let axis = render.axis.clone();
                        let mosaic = render.mosaic.clone();
                        render_pill(
                            &title(candidate.as_str()),
                            candidate.question(),
                            !**render.mosaic && candidate == render.active_axis,
                            watchband::badge(axis_count(candidate, runtime)),
                            Callback::from(move |_| {
                                axis.set(candidate);
                                mosaic.set(false);
                            }),
                        )
                    })}
                </nav>
            </div>
        </details>
    }
}

fn render_agent_roster(runtime: Option<&CanvasRuntime>, submissions: &SubmissionStatus) -> Html {
    let members = runtime
        .and_then(|runtime| {
            let lanes = runtime.lanes()?;
            let waiting = runtime
                .inbox()
                .map(|inbox| inbox.lanes.as_slice())
                .unwrap_or(&[]);
            Some(
                lanes
                    .lanes
                    .iter()
                    .flat_map(|lineage| lineage.members.iter())
                    .map(|member| {
                        let backend = waiting
                            .iter()
                            .find(|row| row.lane == member.lane)
                            .map(|row| row.pending.as_slice())
                            .unwrap_or(&[]);
                        let pending = submissions.pending(member.lane, backend);
                        (member, pending)
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .unwrap_or_default();
    html! {
        <span
            class="brand-dots"
            role="group"
            aria-label={format!("{} admitted agents", members.len())}
        >
            {for members.chunks(2).map(|column| html! {
                <span class="brand-dot-column">
                    {for column.iter().map(|(member, pending)| {
                        let status = member.status(*pending);
                        let label = member.name.as_deref().unwrap_or_else(|| {
                            member
                                .worktree
                                .rsplit('/')
                                .find(|segment| !segment.is_empty())
                                .unwrap_or(&member.worktree)
                        });
                        html! {
                            <span
                                class={classes!("brand-dot", status.key())}
                                role="img"
                                aria-label={format!("{label}: {}", status.label())}
                                title={format!("{label}: {}", status.label())}
                                data-agent-lane={member.lane.to_string()}
                                data-agent-status={status.key()}
                            ></span>
                        }
                    })}
                </span>
            })}
        </span>
    }
}

fn task_pills(runtime: Option<&CanvasRuntime>) -> Vec<TaskPill> {
    let Some(board) = runtime.and_then(CanvasRuntime::board) else {
        return Vec::new();
    };
    let mut pills = BTreeMap::<String, TaskPill>::new();
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
        let pill = pills.entry(stem.clone()).or_insert_with(|| TaskPill {
            stem,
            hidden,
            ..TaskPill::default()
        });
        if task
            .claim
            .as_ref()
            .is_some_and(|claim| claim.body.disposition == wire::head::ClaimDisposition::Active)
        {
            pill.active += 1;
        } else if workflow.deferral.is_some()
            || task.statement.body.dependencies.iter().any(|dependency| {
                board.tasks.iter().any(|candidate| {
                    candidate.statement.body.born == *dependency
                        && candidate.workflow.body.completion.is_none()
                })
            })
        {
            pill.unavailable += 1;
        } else {
            pill.ready += 1;
        }
    }
    pills.into_values().collect()
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
) -> Html {
    let Some(activity) = runtime.newest_lineage_activity(lineage) else {
        return html! {
            <div class="ticker" aria-live="polite" data-empty="true"></div>
        };
    };
    ticker(activity.class(), activity.title(), activity.detail())
}

fn ticker(class: &'static str, title: &str, detail: &str) -> Html {
    let detail = if detail.is_empty() { title } else { detail };
    html! {
        <div class={classes!("ticker", class)} aria-live="polite">
            {watchband::ticker_line(title, detail, TICKER_BUDGET)}
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
    pub(in crate::browser) name: String,
    pub(in crate::browser) accent: usize,
    pub(in crate::browser) speak: Callback<String>,
    pub(in crate::browser) quote: Callback<()>,
}

pub(in crate::browser) fn render_card_actions(text: &str, correspondent: &Correspondent) -> Html {
    let speak = correspondent.speak.clone();
    let quote = correspondent.quote.clone();
    let spoken = text.to_owned();
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
                onclick={Callback::from(move |_| quote.emit(()))}
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
