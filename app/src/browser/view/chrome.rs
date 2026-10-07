//! The chrome standing above the feed.
//!
//! One bar of stem tabs and the connectivity-bearing Spice button across the top, one
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
use crate::watchband;
use crate::work::{WorkCounts, WorkProjection, WorkScope};

/// Characters the ticker's `aria-live` label announces.
const TICKER_SPOKEN_BUDGET: usize = 160;

/// The bar of view pills, each wearing what its axis currently counts.
pub(super) struct CommandBarRender<'a> {
    pub(super) connection: &'a UseStateHandle<Connection>,
    pub(super) destination: AppDestination,
    pub(super) navigate: &'a Callback<navigation::Intent>,
    pub(super) menu_open: &'a UseStateHandle<bool>,
    pub(super) work: Option<&'a WorkProjection<'a>>,
    pub(super) identity: &'a UseStateHandle<Option<AppIdentity>>,
    pub(super) app_update: &'a UseStateHandle<Option<AppUpdate>>,
    pub(super) surface: &'a SurfaceData,
    pub(super) cursor: &'a str,
    pub(super) sender: &'a SenderHandle,
    pub(super) submissions: &'a SubmissionStatus,
}

pub(super) fn render_command_bar(render: CommandBarRender<'_>) -> Html {
    let surface = render.surface.read();
    let runtime = surface.as_ref();
    let stems = render
        .work
        .into_iter()
        .flat_map(WorkProjection::stems)
        .filter(|stem| stem.counts().has_open())
        .collect::<Vec<_>>();
    // The strip rolls up every open task, team-associated or not; the total
    // leads it whenever any open work exists.
    let all_work_pill = render
        .work
        .map(WorkProjection::counts)
        .filter(|counts| counts.has_open())
        .map(|counts| render_aggregate_pill(counts, render.navigate));
    let operator_link = render_operator_link(&render, runtime);
    let connection_label: AttrValue = render.connection.label().into();
    html! {
        <header class="commandbar commandbar--mosaic">
            <div
                class="app-disclosure"
                data-expanded={(**render.menu_open).to_string()}
            >
                {operator_link}
                <nav class="app-horn-tabs app-global-work-tabs" aria-label="Open task stems">
                    {all_work_pill}
                    {for stems.into_iter().map(|stem| {
                        render_stem_pill(stem, render.navigate)
                    })}
                </nav>
                // Connection is routine state: the Spice button, tab title, and
                // icon carry it visibly; only assistive technology hears it.
                <span class="visually-hidden" role="status">{connection_label.clone()}</span>
                {render_app_menu(&render, runtime, &connection_label)}
            </div>
        </header>
    }
}

/// The operator's face at the head of the bar opens durable Dash; the aggregate
/// count beside it opens the browser-local all-work overview. The face keeps
/// its place and size while identity loads, so the pills after it never shift,
/// and it looks the same on every route.
fn render_operator_link(render: &CommandBarRender<'_>, runtime: Option<&AppRuntime>) -> Html {
    let operator = runtime
        .and_then(AppRuntime::settings)
        .map(|settings| &settings.operator);
    let on_dash = render.destination == AppDestination::Dash;
    let name = operator
        .and_then(|operator| operator.name.as_deref())
        .unwrap_or("Operator");
    let label = format!("{name}: open Dash");
    let navigate = render.navigate.clone();
    html! {
        <button
            class="app-operator-link"
            type="button"
            title={label.clone()}
            aria-label={label}
            aria-current={on_dash.then_some("page")}
            data-operator-state={if operator.is_some() { "ready" } else { "loading" }}
            onclick={Callback::from(move |_| navigate.emit(navigation::Intent::Dash))}
        >
            if let Some(operator) = operator {
                {super::render_operator_avatar(operator)}
            } else {
                <span class="operator-avatar" data-avatar-source="loading" aria-hidden="true">
                    {"…"}
                </span>
            }
        </button>
    }
}

/// The destination rail atop the Spice menu.
///
/// The open menu frame lies over the team surface beneath it, so this rail
/// lands on the same compact control band.
fn render_navigation(render: &CommandBarRender<'_>, runtime: Option<&AppRuntime>) -> Html {
    html! {
        <nav class="app-navigation app-control-rail app-option-rail" aria-label="Views">
            {navigation_option(
                render,
                "Dash",
                AppDestination::Dash.description(),
                navigation::Intent::Dash,
                render.destination == AppDestination::Dash,
                watchband::badge(axis_count(Axis::Topology, runtime)),
            )}
            {navigation_option(
                render,
                "Work",
                "Work across tasks, claims, deferrals, and reviews",
                navigation::Intent::Work(WorkScope::AllOpen),
                false,
                watchband::badge(axis_count(Axis::Flow, runtime)),
            )}
            {navigation_option(
                render,
                "Chat",
                AppDestination::Chat.description(),
                navigation::Intent::Chat,
                render.destination == AppDestination::Chat,
                watchband::badge(waiting_directives(runtime, render.submissions)),
            )}
        </nav>
    }
}

fn navigation_option(
    render: &CommandBarRender<'_>,
    label: &'static str,
    title: &'static str,
    intent: navigation::Intent,
    selected: bool,
    badge: Option<String>,
) -> Html {
    let navigate = render.navigate.clone();
    let menu_open = render.menu_open.clone();
    html! {
        <button
            class={classes!("app-option", "app-option--counted", selected.then_some("selected"))}
            type="button"
            aria-current={selected.then_some("page")}
            {title}
            onclick={Callback::from(move |_| {
                navigate.emit(intent.clone());
                app_menu::close(&menu_open);
            })}
        >
            <span class="app-option-label">{label}</span>
            if let Some(count) = badge {
                <span class="app-count-badge app-count-badge--option">{count}</span>
            }
        </button>
    }
}

fn render_app_menu(
    render: &CommandBarRender<'_>,
    runtime: Option<&AppRuntime>,
    connection_label: &str,
) -> Html {
    let fast_mode = runtime
        .and_then(AppRuntime::settings)
        .is_some_and(|settings| settings.fast_mode);
    let menu_open = render.menu_open.clone();
    let update = (**render.app_update).as_ref();
    let trigger_label = match (update, fast_mode) {
        (Some(_), true) => "Open spice menu - update available; fast mode on",
        (Some(_), false) => "Open spice menu - update available",
        (None, true) => "Open spice menu - fast mode on",
        (None, false) => "Open spice menu",
    };
    let quota = quota_level(runtime);
    let edge_accent = match (fast_mode, update.is_some()) {
        (false, false) => None,
        (true, false) => Some(2),
        (false, true) => Some(1),
        (true, true) => Some(3),
    };
    let trigger_label: AttrValue = match &quota {
        Some(quota) => format!("{trigger_label} - {connection_label} - {}", quota.label),
        None => format!("{trigger_label} - {connection_label}"),
    }
    .into();
    let toggle = Callback::from(move |event: MouseEvent| {
        event.prevent_default();
        menu_open.set(!*menu_open);
    });
    html! {
        <details class="app-menu" open={**render.menu_open}>
            <summary
                data-accent={edge_accent.map(|accent| accent.to_string())}
                data-connection={render.connection.indicator_state()}
                data-quota-stale={quota.as_ref().is_some_and(|quota| quota.stale).then_some("true")}
                aria-label={trigger_label.clone()}
                aria-haspopup="menu"
                aria-expanded={(**render.menu_open).to_string()}
                title={trigger_label}
                onclick={toggle}
            >
                <span class="app-menu-well" aria-hidden="true">
                    {quota.as_ref().map(QuotaLevel::render)}
                </span>
                <span class="app-menu-pepper" aria-hidden="true">{"🌶️"}</span>
            </summary>
            <div class="app-menu-frame">
                {render_navigation(render, runtime)}
                <div class="app-menu-panel">
                    {super::team_menu::render(
                        render.destination,
                        runtime,
                        &super::team_menu::Context {
                            cursor: render.cursor,
                            identity: (**render.identity).as_ref(),
                            update,
                            sender: render.sender,
                            submissions: render.submissions,
                        },
                    )}
                </div>
            </div>
        </details>
    }
}

/// Plan-weighted weekly quota drawn as account liquid layers, ordered by
/// remaining capacity, each in the accent its capacity order assigns.
struct QuotaLevel<'a> {
    stale: bool,
    stack: crate::quota::WeeklyQuotaStack<'a>,
    label: String,
}

impl QuotaLevel<'_> {
    /// SVG geometry attributes, unlike inline style, survive the app's
    /// `style-src 'self'` policy.
    fn render(&self) -> Html {
        let mut surface = 100.0;
        let layers = self
            .stack
            .layers()
            .into_iter()
            .map(|layer| {
                let depth = layer.available / self.stack.capacity * 100.0;
                surface = (surface - depth).max(0.0);
                html! {
                    <rect
                        class="app-menu-quota-account"
                        data-accent={(layer.accent % watchband::ACCENTS).to_string()}
                        x="0"
                        y={format!("{surface:.3}")}
                        width="100"
                        height={format!("{depth:.3}")}
                    />
                }
            })
            .collect::<Html>();
        html! {
            <svg class="app-menu-quota" viewBox="0 0 100 100" preserveAspectRatio="none" focusable="false">
                {layers}
            </svg>
        }
    }
}

/// Only a failed refresh marks the level stale.
fn quota_level(runtime: Option<&AppRuntime>) -> Option<QuotaLevel<'_>> {
    let generation = runtime.and_then(AppRuntime::quota)?;
    let stack = crate::quota::weekly_quota_stack(generation.last_good.as_deref()?)?;
    let stale = crate::quota::last_good_stale(generation);
    let percentage = stack.remaining * 100.0;
    let label = if stale {
        format!(
            "weekly quota {percentage:.0}% remaining ({})",
            crate::quota::Condition::StaleLastGood.label()
        )
    } else {
        format!("weekly quota {percentage:.0}% remaining")
    };
    Some(QuotaLevel {
        stale,
        stack,
        label,
    })
}

/// One neutral open-task count badge shared by global and team-local roll-ups.
pub(in crate::browser) fn render_task_count(counts: WorkCounts) -> Html {
    html! {
        <span class="app-count-badge">{counts.open().to_string()}</span>
    }
}

/// The label-less total leading the global strip, opening all open Work. Like
/// the team total, its badge is one open-task count.
fn render_aggregate_pill(counts: WorkCounts, navigate: &Callback<navigation::Intent>) -> Html {
    let open = counts.open();
    let title = format!(
        "All open work: {open} {}; {} ready, {} active or in flight, {} blocked or deferred",
        if open == 1 { "task" } else { "tasks" },
        counts.ready(),
        counts.in_flight(),
        counts.unavailable(),
    );
    let navigate = navigate.clone();
    html! {
        <button
            key="global-work"
            class="app-horn-tab app-horn-tab--down"
            type="button"
            title={title.clone()}
            aria-label={title}
            data-work-scope="all"
            data-sidebar-focus-key="global-work"
            onclick={Callback::from(move |_| navigate.emit(navigation::Intent::Work(WorkScope::AllOpen)))}
        >
            {render_task_count(counts)}
        </button>
    }
}

/// One global stem tab opening that stem's Work, the scope its counts
/// describe.
fn render_stem_pill(stem: &crate::work::WorkStem, navigate: &Callback<navigation::Intent>) -> Html {
    let identity = stem.identity();
    let counts = stem.counts();
    let scope = WorkScope::Stem(identity.clone());
    let focus_key = format!("global-work:{identity}");
    let title = format!(
        "{identity}: {} ready, {} active or in flight, {} blocked or deferred",
        counts.ready(),
        counts.in_flight(),
        counts.unavailable(),
    );
    let navigate = navigate.clone();
    html! {
        <button
            key={identity.to_string()}
            class="app-horn-tab app-horn-tab--down"
            type="button"
            title={title.clone()}
            aria-label={title}
            data-task-stem={identity.stem().to_owned()}
            data-task-visibility={if identity.hidden() { "hidden" } else { "public" }}
            data-sidebar-focus-key={focus_key}
            onclick={Callback::from(move |_| navigate.emit(navigation::Intent::Work(scope.clone())))}
        >
            <span class="app-horn-tab__label">{identity.stem().to_owned()}</span>
            {render_task_count(counts)}
        </button>
    }
}

/// Directives waiting across every lane, which is what the feed is for.
fn waiting_directives(runtime: Option<&AppRuntime>, submissions: &SubmissionStatus) -> usize {
    let waiting = runtime
        .and_then(AppRuntime::inbox)
        .map(|inbox| inbox.lanes.as_slice())
        .unwrap_or(&[]);
    submissions.total_pending(waiting.iter().map(|row| (row.lane, row.pending.as_slice())))
}

/// What one axis currently counts, or zero when its answer has not arrived.
///
/// Each count is the size of what that axis's own panes render, so a badge
/// never advertises more than the pill leads to.
pub(super) fn axis_count(axis: Axis, runtime: Option<&AppRuntime>) -> usize {
    match axis {
        Axis::Flow => runtime
            .and_then(AppRuntime::board)
            .map_or(0, |board| board.active_public_tasks().count()),
        Axis::Topology => runtime
            .and_then(AppRuntime::lanes)
            .map_or(0, |lanes| lanes.lanes.len()),
    }
}

/// Newest activity from any member of one team.
pub(in crate::browser) fn render_team_ticker(
    runtime: &AppRuntime,
    team: &crate::runtime::LaneRow,
    standings: &[watchband::Standing],
) -> Html {
    let Some(activity) = runtime.newest_team_activity(team) else {
        return render_empty_team_ticker();
    };
    let standing = standings
        .iter()
        .find(|standing| standing.lane == activity.lane())
        .expect("a team activity belongs to one rendered standing");
    let member = team
        .member(activity.lane())
        .expect("a team activity belongs to one rendered member");
    ticker(activity, standing, member.status(standing.waking))
}

/// The ticker row before any activity, or with no team at all, keeps the
/// team surface's pane where the first activity or team leaves it.
pub(in crate::browser) fn render_empty_team_ticker() -> Html {
    html! {
        <div class="app-status-band app-status-text ticker" aria-live="polite" data-empty="true"></div>
    }
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
    let (prefix, detail) =
        watchband::ticker_chunks(visible_prefix, source, watchband::STATUS_LINE_DOM_LIMIT);
    let separator = if activity.class() == "prose" || class == "tool" || detail.is_empty() {
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
        watchband::ticker_accessible_label(&standing.label, class, &line, TICKER_SPOKEN_BUDGET);
    html! {
        <div
            class={classes!("app-status-band", "app-status-text", "ticker", activity.class())}
            aria-live="polite"
            aria-label={accessible_label}
            title={line}
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

/// Agent-card actions and the colour of its producing worktree.
#[derive(Clone, PartialEq)]
pub(in crate::browser) struct Correspondent {
    pub(in crate::browser) accent: usize,
    pub(in crate::browser) speak: Callback<(SpeechControl, String)>,
    pub(in crate::browser) speech_owner: Option<SpeechOwner>,
    pub(in crate::browser) message_key: String,
    pub(in crate::browser) quote: Option<Callback<()>>,
}

fn action_speech_text(event: &MouseEvent, fallback: &str) -> String {
    let Some(hunk) = event
        .current_target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        .and_then(|target| target.closest(".message-hunk").ok().flatten())
        .and_then(|hunk| hunk.clone_node_with_deep(true).ok())
        .and_then(|hunk| hunk.dyn_into::<web_sys::Element>().ok())
    else {
        return fallback.to_owned();
    };
    if let Ok(nodes) = hunk.query_selector_all(
        ".message-controls, .message-attribution, .message-image-status, .message-quote-task-tail",
    ) {
        for index in 0..nodes.length() {
            if let Some(element) = nodes
                .item(index)
                .and_then(|node| node.dyn_into::<web_sys::Element>().ok())
            {
                element.remove();
            }
        }
    }
    hunk.text_content()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| fallback.to_owned())
}

#[derive(Properties, PartialEq)]
struct CardActionsProps {
    text: AttrValue,
    correspondent: Correspondent,
}

#[function_component(CardActions)]
fn card_actions(props: &CardActionsProps) -> Html {
    let control = use_context::<SpeechControl>().unwrap_or_default();
    let speak = props.correspondent.speak.clone();
    let quote = props.correspondent.quote.clone();
    let fallback = props.text.to_string();
    let speaking = props
        .correspondent
        .speech_owner
        .as_ref()
        .is_some_and(|owner| owner.is_control(&props.correspondent.message_key, control));
    let speech_label = if speaking {
        "Stop playback"
    } else {
        "Speak this message"
    };
    html! {
        <span class="message-actions app-control-rail">
            <button
                class={classes!(
                    "app-card-action",
                    "app-card-action--speak",
                    speaking.then_some("active"),
                )}
                type="button" title={speech_label}
                aria-label={speech_label}
                onclick={Callback::from(move |event: MouseEvent| {
                    speak.emit((control, action_speech_text(&event, &fallback)));
                })}
            >{
                if speaking {
                    html! {
                        <svg
                            class="app-card-action__icon"
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
                            class="app-card-action__icon"
                            data-card-action-icon="play"
                            width="15" height="15" viewBox="0 0 24 24"
                            aria-hidden="true" focusable="false"
                        >
                            <path d="M8 5v14l11-7Z" fill="currentColor" />
                        </svg>
                    }
                }
            }</button>
            if let Some(quote) = quote {
                <button
                    class="app-card-action app-card-action--quote" type="button" title="Quote this message"
                    aria-label="Quote this message"
                    onclick={Callback::from(move |_| quote.emit(()))}
                ><span class="app-card-action__icon" aria-hidden="true">{"❝"}</span></button>
            }
        </span>
    }
}

pub(in crate::browser) fn render_card_actions(text: &str, correspondent: &Correspondent) -> Html {
    html! {
        <CardActions
            text={AttrValue::from(text.to_owned())}
            correspondent={correspondent.clone()}
        />
    }
}

#[derive(Properties, PartialEq)]
pub(in crate::browser) struct SettingToggleProps {
    pub(in crate::browser) label: AttrValue,
    pub(in crate::browser) value: AttrValue,
    pub(in crate::browser) checked: bool,
    pub(in crate::browser) disabled: bool,
    #[prop_or_default]
    pub(in crate::browser) checkbox: bool,
    #[prop_or_default]
    pub(in crate::browser) title: Option<AttrValue>,
    #[prop_or_default]
    pub(in crate::browser) onclick: Option<Callback<MouseEvent>>,
}

/// One shared two-column setting toggle for global and member controls.
#[function_component(SettingToggle)]
pub(in crate::browser) fn setting_toggle(props: &SettingToggleProps) -> Html {
    let state = props.checked.to_string();
    html! {
        <button
            class="app-setting-row app-setting-toggle app-setting-toggle--fast"
            type="button"
            role={props.checkbox.then_some("menuitemcheckbox")}
            aria-checked={props.checkbox.then(|| state.clone())}
            aria-pressed={(!props.checkbox).then_some(state)}
            disabled={props.disabled}
            title={props.title.clone()}
            onclick={props.onclick.clone()}
        >
            <strong>{props.label.clone()}</strong>
            <span>{props.value.clone()}</span>
        </button>
    }
}

#[derive(Properties, PartialEq)]
pub(in crate::browser) struct CopyButtonProps {
    /// Text put on the clipboard.
    pub(in crate::browser) text: AttrValue,
    /// What the toast names as copied: "Copied {subject}".
    pub(in crate::browser) subject: AttrValue,
    #[prop_or_default]
    pub(in crate::browser) class: Classes,
    #[prop_or_default]
    pub(in crate::browser) title: Option<AttrValue>,
    /// Accessible name, when the visible content does not already name it.
    #[prop_or_default]
    pub(in crate::browser) label: Option<AttrValue>,
    #[prop_or_default]
    pub(in crate::browser) children: Html,
}

/// A button that copies text and reports the outcome through the root toast.
#[function_component(CopyButton)]
pub(in crate::browser) fn copy_button(props: &CopyButtonProps) -> Html {
    let toaster = crate::browser::toast::use_toaster();
    let onclick = {
        let text = props.text.clone();
        let subject = props.subject.clone();
        Callback::from(move |_| {
            let toaster = toaster.clone();
            let subject = subject.clone();
            crate::browser::clipboard::copy_text(&text, move |result| {
                toaster.settled(match result {
                    Ok(()) => format!("Copied {subject}"),
                    Err(error) => format!("Copy failed: {error}"),
                });
            });
        })
    };
    html! {
        <button
            class={props.class.clone()} type="button"
            title={props.title.clone()} aria-label={props.label.clone()}
            {onclick}
        >{props.children.clone()}</button>
    }
}
