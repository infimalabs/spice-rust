//! A working group's toolbar, the team toolbar's twin read from Designs: an
//! open-task total closing a rail of the lanes working the group's Designs,
//! one standing light per Design, and a ticker of the newest room
//! event across them. Only the name is the group's own; this browser's
//! placement of the console lives in the panel's arrangement menu, where a
//! team's arrangement stands. Everything renders from the board, the room
//! projection, and lane topology on each read.

use wire::rooms::RoomAuthorityStamp;
use yew::prelude::*;

use super::{Channel, GroupRender};
use crate::browser::clock::Age;
use crate::browser::{navigation, view};
use crate::room_views::GroupView;
use crate::runtime::{DesignStanding, RoomEvent};
use crate::watchband;
use crate::work::{ScopeLane, WorkCounts, WorkScope, WorkTask};

pub(super) fn render(
    render: &GroupRender<'_, '_>,
    view: &GroupView,
    name: &str,
    channels: &[Channel],
    loaded: bool,
) -> Html {
    let standings = channels
        .iter()
        .map(|channel| {
            render
                .runtime
                .room_projection()
                .design_standing(&channel.stamp, render.runtime.board())
        })
        .collect::<Vec<_>>();
    html! {
        <header class="team-toolbar">
            {render_rail(render, view, channels)}
            <div class="app-glass-control-surface app-status-band team-status-row">
                <span
                    class="team-lights"
                    role="group"
                    aria-label={format!("{} designs", channels.len())}
                    data-team-size={channels.len().to_string()}
                >
                    {for channels.iter().zip(&standings).map(|(channel, standing)| render_light(channel, *standing))}
                </span>
                {render_ticker(render, channels, &standings)}
                <strong class="group-surface-name" aria-busy={(!loaded).to_string()}>{name.to_owned()}</strong>
            </div>
        </header>
    }
}

/// The scope that selects work active in this group's associated Designs.
fn scope(view: &GroupView, channels: &[Channel]) -> WorkScope {
    WorkScope::Group {
        group: view.group.clone(),
        designs: channels
            .iter()
            .map(|channel| channel.stamp.clone())
            .collect(),
    }
}

/// The rail mirrors the Design globals: the fixed total closes it and the
/// lanes working the group stack toward the start beside it.
fn render_rail(render: &GroupRender<'_, '_>, view: &GroupView, channels: &[Channel]) -> Html {
    let tasks = render
        .work
        .map(|work| work.scope_strip(&scope(view, channels)))
        .unwrap_or_default();
    let lanes = ScopeLane::working(&tasks);
    html! {
        <div class="team-task-rail">
            if !tasks.is_empty() {
                {render_total(render, view, channels, &tasks)}
                <nav class="app-horn-tabs team-tasks" aria-label="Lanes working this group's designs">
                    {for lanes.iter().map(|working| render_lane_tab(render, channels, working))}
                </nav>
            }
        </div>
    }
}

/// The closing tab: a badge counting every active task in the group's work,
/// opening the overview with exactly that current selection.
fn render_total(
    render: &GroupRender<'_, '_>,
    view: &GroupView,
    channels: &[Channel],
    tasks: &[&WorkTask<'_>],
) -> Html {
    let counts = tasks
        .iter()
        .map(|task| task.availability())
        .collect::<WorkCounts>();
    let open = counts.open();
    let title = format!(
        "Open work active in this group: {open} {}",
        if open == 1 { "task" } else { "tasks" }
    );
    let open_group = {
        let navigate = render.navigate.clone();
        let scope = scope(view, channels);
        Callback::from(move |_| navigate.emit(navigation::Intent::Work(scope.clone())))
    };
    html! {
        <span
            key="group-total"
            class="app-horn-tab app-horn-tab--up app-horn-tab--wide app-horn-tab--group"
            data-work-scope="group"
        >
            <button
                class="app-horn-tab__target"
                type="button"
                title={title.clone()}
                aria-label={title}
                data-sidebar-focus-key={format!("group-work:{}:all", view.group)}
                onclick={open_group}
            >
                {view::chrome::render_task_count(counts)}
            </button>
        </span>
    }
}

/// One lane working the group, the twin of a team's task tab: named as its
/// cards sign themselves, in the accent of the Design its first task serves.
/// A seated lane opens its composer in Chat, where its history lives; a lane
/// with no Chat seat reads as a plain label.
fn render_lane_tab(
    render: &GroupRender<'_, '_>,
    channels: &[Channel],
    working: &ScopeLane<'_, '_>,
) -> Html {
    let lanes = render.runtime.lanes();
    let lane = working.lane();
    let label = crate::work::owner_label(
        &wire::Actor::Lane(lane),
        lanes.and_then(|lanes| lanes.member(lane)),
    );
    let accent = working
        .tasks()
        .iter()
        .find_map(|task| {
            channels.iter().find(|channel| {
                crate::runtime::associated(&task.row().statement.body, &channel.stamp)
            })
        })
        .map(|channel| channel.accent.to_string());
    let handles = working
        .tasks()
        .iter()
        .map(|task| task.row().statement.body.handle.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let key = lane.to_string();
    let classes = classes!("app-horn-tab", "app-horn-tab--up", "app-horn-tab__handle");
    if lanes
        .and_then(|lanes| lanes.chat_target_for_lane(lane))
        .is_some()
    {
        let title = format!("Open {label}'s composer in Chat, working {handles}");
        let href = crate::view::card_href(&crate::browser::watchband::primary_textarea_id(&key));
        html! {
            <a
                key={key.clone()}
                class={classes}
                {href}
                title={title.clone()}
                aria-label={title}
                data-accent={accent}
                data-group-lane={key}
            >{label}</a>
        }
    } else {
        html! {
            <span
                key={key.clone()}
                class={classes}
                title={format!("{label}, working {handles}")}
                data-accent={accent}
                data-group-lane={key}
            >{label}</span>
        }
    }
}

/// One Design's light: its accent, shaped by its standing.
fn render_light(channel: &Channel, standing: DesignStanding) -> Html {
    let label = format!("{}: {}", channel.label, standing.label());
    html! {
        <span
            key={channel.stamp.to_string()}
            class={classes!("app-presence-pip", "app-presence-pip--status", "group-design-pip", standing.key())}
            role="img"
            data-design={channel.stamp.to_string()}
            data-accent={channel.accent.to_string()}
            data-design-standing={standing.key()}
            aria-label={label.clone()}
            title={label}
        ></span>
    }
}

/// The newest room publication or coordination event across the group's
/// Designs, aged and coloured by the Design it landed in.
fn render_ticker(
    render: &GroupRender<'_, '_>,
    channels: &[Channel],
    standings: &[DesignStanding],
) -> Html {
    let projection = render.runtime.room_projection();
    let newest = channels
        .iter()
        .zip(standings)
        .filter_map(|(channel, standing)| {
            let (publication, event) = projection
                .room(&channel.stamp)?
                .events()
                .iter()
                .max_by_key(|(_, event)| event.position)?;
            Some((channel, *standing, publication, event))
        })
        .max_by_key(|(_, _, _, event)| event.position);
    let Some((channel, standing, publication, event)) = newest else {
        return view::chrome::render_empty_team_ticker();
    };
    ticker(render, channel, standing, publication, event)
}

fn ticker(
    render: &GroupRender<'_, '_>,
    channel: &Channel,
    standing: DesignStanding,
    publication: &RoomAuthorityStamp,
    event: &RoomEvent,
) -> Html {
    let (speaker, words) = super::super::event_headline(render.runtime, publication, event);
    let (prefix, detail) =
        watchband::ticker_chunks(&speaker, &words, watchband::STATUS_LINE_DOM_LIMIT);
    let line = if detail.is_empty() {
        prefix.clone()
    } else {
        format!("{prefix}: {detail}")
    };
    let accessible_label = watchband::ticker_accessible_label(
        &channel.label,
        "",
        &line,
        view::chrome::TICKER_SPOKEN_BUDGET,
    );
    html! {
        <div
            class="app-status-band app-status-text ticker"
            aria-live="polite"
            aria-label={accessible_label}
            title={line}
            data-design={channel.stamp.to_string()}
            data-publication={publication.to_string()}
            data-accent={channel.accent.to_string()}
            data-design-standing={standing.key()}
        >
            <Age
                authored={publication.micros() / 1_000}
                class={classes!("ticker-age", standing.key())}
                title={format!("{} · {publication}", channel.label)}
            />
            {" "}
            <strong class="ticker-prefix">{if detail.is_empty() { prefix } else { format!("{prefix}:") }}</strong>
            if !detail.is_empty() {
                {" "}
                <span class="ticker-detail">{detail}</span>
            }
        </div>
    }
}
