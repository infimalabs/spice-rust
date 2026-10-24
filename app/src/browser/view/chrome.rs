//! The chrome standing above the feed.
//!
//! Two global totals beside the Spice launcher across the top, one band of
//! lane standings under them, one line of the newest command outcome under
//! that, and the card the feed packs. All four are placement and markup only:
//! every number they show is derived in [`crate::watchband`], which a native
//! test can reach, so what is left here is the part that genuinely needs a
//! browser.
//!
//! The connection state lives in this bar because this bar is the one piece of
//! chrome every layout renders. The surface keeps the last admitted runtime on
//! screen through an outage and through a restarted resident's hydration, so a
//! layout that hid the state would show a live-looking surface whose composers
//! accept nothing.

use super::*;
use crate::browser::clock::Age;
use crate::watchband;
use crate::work::{WorkCounts, WorkMark, WorkProjection, WorkScope};
/// Characters the ticker's `aria-live` label announces.
pub(in crate::browser) const TICKER_SPOKEN_BUDGET: usize = 160;

/// The fixed command bar and its browser-local sidebar launcher.
pub(super) struct CommandBarRender<'a> {
    pub(super) connection: &'a UseStateHandle<Connection>,
    pub(super) navigate: &'a Callback<navigation::Intent>,
    pub(super) spice_sidebar_open: bool,
    pub(super) open_spice_sidebar: &'a Callback<web_sys::Element>,
    pub(super) work: Option<&'a WorkProjection<'a>>,
    pub(super) app_update: &'a UseStateHandle<Option<AppUpdate>>,
    pub(super) surface: &'a SurfaceData,
}

pub(super) fn render_command_bar(render: CommandBarRender<'_>) -> Html {
    let surface = render.surface.read();
    let runtime = surface.as_ref();
    let connection_label: AttrValue = render.connection.label().into();
    html! {
        <header class="commandbar commandbar--mosaic">
            <div class="app-commandbar-content">
                // Connection is routine state: the Spice button, tab title, and
                // icon carry it visibly; only assistive technology hears it.
                <span class="visually-hidden" role="status">{connection_label.clone()}</span>
                <nav class="app-work-totals" aria-label="Open work">
                    {render_work_total(render.work, render.navigate, GlobalWorkRail::Execution)}
                    {render_work_total(render.work, render.navigate, GlobalWorkRail::Design)}
                </nav>
                {render_spice_button(SpiceButton {
                    connection: render.connection,
                    app_update: (**render.app_update).as_ref(),
                    runtime,
                    place: SpicePlace::Launcher {
                        expanded: render.spice_sidebar_open,
                    },
                    node_ref: NodeRef::default(),
                    onclick: {
                        let open = render.open_spice_sidebar.clone();
                        Callback::from(move |event: MouseEvent| {
                            if let Some(target) = event
                                .current_target()
                                .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                            {
                                open.emit(target);
                            }
                        })
                    },
                })}
            </div>
        </header>
    }
}

/// One Spice button: the command bar's launcher, or the open Spice sidebar's
/// close control standing at that same place.
pub(super) struct SpiceButton<'a> {
    pub(super) connection: &'a Connection,
    pub(super) app_update: Option<&'a AppUpdate>,
    pub(super) runtime: Option<&'a AppRuntime>,
    pub(super) place: SpicePlace,
    pub(super) node_ref: NodeRef,
    pub(super) onclick: Callback<MouseEvent>,
}

#[derive(Clone, Copy)]
pub(super) enum SpicePlace {
    /// In the command bar, opening the sidebar and receiving focus back.
    Launcher { expanded: bool },
    /// In the open sidebar over the launcher's place, closing it.
    Anchor,
}

pub(super) fn render_spice_button(button: SpiceButton<'_>) -> Html {
    let fast_mode = button
        .runtime
        .and_then(AppRuntime::settings)
        .is_some_and(|settings| settings.fast_mode);
    let (verb, expanded, focus_key) = match button.place {
        SpicePlace::Launcher { expanded } => ("Open", expanded, Some("spice")),
        SpicePlace::Anchor => ("Close", true, None),
    };
    let trigger_label = match (button.app_update, fast_mode) {
        (Some(_), true) => format!("{verb} Spice - update available; fast mode on"),
        (Some(_), false) => format!("{verb} Spice - update available"),
        (None, true) => format!("{verb} Spice - fast mode on"),
        (None, false) => format!("{verb} Spice"),
    };
    let quota = quota_level(button.runtime);
    let edge_accent = match (fast_mode, button.app_update.is_some()) {
        (false, false) => None,
        (true, false) => Some(3),
        (false, true) => Some(1),
        (true, true) => Some(3),
    };
    let connection_label = button.connection.label();
    let mut trigger_label = match &quota {
        Some(quota) => format!("{trigger_label} - {connection_label} - {}", quota.label),
        None => format!("{trigger_label} - {connection_label}"),
    };
    let waiting = button.runtime.map_or(0, waiting_messages);
    match waiting {
        0 => {}
        1 => trigger_label.push_str(" - 1 message waiting for you"),
        count => trigger_label.push_str(&format!(" - {count} messages waiting for you")),
    }
    let trigger_label: AttrValue = trigger_label.into();
    html! {
        <button
            ref={button.node_ref}
            class="app-spice-button"
            type="button"
            data-accent={edge_accent.map(|accent| accent.to_string())}
            data-connection={button.connection.indicator_state()}
            data-quota-stale={quota.as_ref().is_some_and(|quota| quota.stale).then_some("true")}
            data-inbox-waiting={(waiting > 0).then(|| waiting.to_string())}
            data-sidebar-focus-key={focus_key}
            aria-label={trigger_label.clone()}
            aria-haspopup="dialog"
            aria-expanded={expanded.to_string()}
            aria-controls="app-sidebar-title"
            title={trigger_label}
            onclick={button.onclick}
        >
            <span class="app-spice-well" aria-hidden="true">
                {quota.as_ref().map(QuotaLevel::render)}
            </span>
            <span class="app-spice-pepper" aria-hidden="true">{"🌶️"}</span>
        </button>
    }
}

/// Open items in the connected principal's personal inbox, counted only while
/// the room projection that places them is available, as the inbox itself
/// counts them.
fn waiting_messages(runtime: &AppRuntime) -> usize {
    if runtime.room_projection().status() == crate::runtime::RoomProjectionStatus::Available {
        runtime.operator_inbox().len()
    } else {
        0
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
                        class="app-spice-quota-account"
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
            <svg class="app-spice-quota-well" viewBox="0 0 100 100" preserveAspectRatio="none" focusable="false">
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

/// One neutral open-task count badge shared by global and local roll-ups.
pub(in crate::browser) fn render_task_count(counts: WorkCounts) -> Html {
    html! {
        <span class="app-count-badge">{counts.open().to_string()}</span>
    }
}

/// Every open category a Work count title names, in task status's terms.
fn count_breakdown(counts: WorkCounts) -> String {
    format!(
        "{} ready, {} review, {} active or in flight, {} stale, {} blocked, {} waiting or set aside",
        counts.ready(),
        counts.review(),
        counts.in_flight(),
        counts.stale(),
        counts.blocked(),
        counts.waiting(),
    )
}

/// A Work tab's mark: the most actionable state the partition it opens holds,
/// or none.
pub(in crate::browser) fn work_mark(counts: WorkCounts) -> Option<&'static str> {
    counts.mark().map(|(mark, _)| mark.key())
}

/// A Work tab's title and accessible name, led by the count of tasks in the
/// state its mark shows, so that state never rests on the mark's colour alone.
pub(in crate::browser) fn task_count_title(label: &str, counts: WorkCounts) -> String {
    let open = counts.open();
    let title = format!(
        "{label}: {open} {}; {}",
        if open == 1 { "task" } else { "tasks" },
        count_breakdown(counts),
    );
    match counts.mark() {
        None => title,
        Some((WorkMark::Blocked, count)) => format!("{count} need you; {title}"),
        Some((WorkMark::Decision, count)) => {
            format!("{count} awaiting your decision; {title}")
        }
        Some((WorkMark::Proposal, count)) => format!("{count} ready for proposal; {title}"),
        Some((WorkMark::Deferred, count)) => format!("{count} deferred; {title}"),
    }
}

/// Which global total a button counts: all open execution work, or all open
/// Design work.
#[derive(Clone, Copy)]
enum GlobalWorkRail {
    Execution,
    Design,
}

impl GlobalWorkRail {
    fn scope(self) -> WorkScope {
        match self {
            Self::Execution => WorkScope::AllOpen,
            Self::Design => WorkScope::planning(),
        }
    }

    const fn focus_key(self) -> &'static str {
        match self {
            Self::Execution => "global-work",
            Self::Design => "design-work",
        }
    }

    const fn aggregate_title(self) -> &'static str {
        match self {
            Self::Execution => "All open execution work",
            Self::Design => "All open Design work",
        }
    }

    const fn data_scope(self) -> &'static str {
        match self {
            Self::Execution => "all",
            Self::Design => "design",
        }
    }
}

/// The label-less total for one global rail, the same hanging tab as before
/// seated beside the Spice launcher; nothing while that rail holds no open
/// work. Its badge is one open-task count, matching every team total, and it
/// opens the rail's whole scope.
fn render_work_total(
    work: Option<&WorkProjection<'_>>,
    navigate: &Callback<navigation::Intent>,
    rail: GlobalWorkRail,
) -> Html {
    let scope = rail.scope();
    let Some(counts) = work
        .map(|work| work.scope_counts(&scope))
        .filter(|counts| counts.has_open())
    else {
        return Html::default();
    };
    let base_accent =
        matches!(rail, GlobalWorkRail::Design).then(|| watchband::DESIGN_BASE.to_string());
    let title = task_count_title(rail.aggregate_title(), counts);
    let focus_key = rail.focus_key();
    let navigate = navigate.clone();
    html! {
        <button
            key={focus_key}
            class="app-horn-tab app-horn-tab--down"
            type="button"
            title={title.clone()}
            aria-label={title}
            data-work-scope={rail.data_scope()}
            data-work-mark={work_mark(counts)}
            data-base-flavor={base_accent.as_ref().map(|_| "accent")}
            data-accent={base_accent}
            data-sidebar-focus-key={focus_key}
            onclick={Callback::from(move |_| navigate.emit(navigation::Intent::Work(scope.clone())))}
        >
            {render_task_count(counts)}
        </button>
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
    pub(in crate::browser) title: Option<AttrValue>,
    #[prop_or_default]
    pub(in crate::browser) onclick: Option<Callback<MouseEvent>>,
}

/// One two-column setting toggle for a member's own fast-mode flag.
#[function_component(SettingToggle)]
pub(in crate::browser) fn setting_toggle(props: &SettingToggleProps) -> Html {
    html! {
        <button
            class="app-setting-row app-setting-toggle app-setting-toggle--fast"
            type="button"
            aria-pressed={props.checked.to_string()}
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
pub(in crate::browser) struct TurboButtonProps {
    #[prop_or_default]
    pub(in crate::browser) class: Classes,
    #[prop_or_default]
    pub(in crate::browser) label: Option<AttrValue>,
    pub(in crate::browser) engaged: bool,
    pub(in crate::browser) disabled: bool,
    pub(in crate::browser) title: AttrValue,
    pub(in crate::browser) onclick: Callback<MouseEvent>,
}

/// Shared stable TURBO lettering; pressed state reflects team or global fast
/// mode. The word never changes: engaged, the button's glass backlights in
/// amber the way a lit slider does, and the pressed state and label stay the
/// accessible truth.
#[function_component(TurboButton)]
pub(in crate::browser) fn turbo_button(props: &TurboButtonProps) -> Html {
    let label = props
        .label
        .clone()
        .unwrap_or_else(|| AttrValue::from("Turbo"));
    html! {
        <button
            type="button"
            class={classes!("app-turbo-button", props.class.clone())}
            data-lit={props.engaged.to_string()}
            aria-label={label}
            aria-pressed={props.engaged.to_string()}
            title={props.title.clone()}
            disabled={props.disabled}
            onclick={props.onclick.clone()}
        >
            {fit_word("Turbo")}
        </button>
    }
}

/// One control word drawn to fill its box exactly: every word occupies the
/// same physical width and height, squeezed or stretched to fit.
pub(in crate::browser) fn fit_word(word: &'static str) -> Html {
    html! {
        <svg class="fit-word" viewBox="0 0 100 16" preserveAspectRatio="none" aria-hidden="true">
            <text x="0" y="13" font-size="15" textLength="100" lengthAdjust="spacingAndGlyphs">
                {word}
            </text>
        </svg>
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
