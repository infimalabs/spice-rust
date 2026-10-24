//! A working group's toolbar, the team toolbar's twin read from Designs: an
//! open-task total and one tab per agent currently working in the group's
//! Designs, one standing pip per Design, and a ticker of the newest words
//! said across them, as a team's ticker shows its newest words. Like a
//! team toolbar it shows no name; this browser's placement of the console
//! lives in the panel's arrangement menu, where a team's arrangement stands.
//! Everything renders from the board, the room projection, and lane topology
//! on each read.

use wire::rooms::RoomAuthorityStamp;
use yew::prelude::*;

use super::{Channel, GroupRender};
use crate::browser::clock::Age;
use crate::browser::{navigation, view};
use crate::room_views::GroupView;
use crate::runtime::{DesignStanding, RoomEvent};
use crate::watchband;
use crate::work::{WorkCounts, WorkScope};

pub(super) fn render(render: &GroupRender<'_, '_>, view: &GroupView, channels: &[Channel]) -> Html {
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
                {render_ticker(render, channels, &standings)}
                <span
                    class="team-lights"
                    role="group"
                    aria-label={format!("{} designs", channels.len())}
                    data-team-size={channels.len().to_string()}
                >
                    {for channels.iter().zip(&standings).map(|(channel, standing)| {
                        super::super::render_design_pip(
                            &channel.stamp,
                            channel.accent,
                            &channel.label,
                            *standing,
                            super::super::PipSize::Status,
                        )
                    })}
                </span>
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

/// The fixed total and active-agent scroller. As on a team rail, the total
/// holds the right edge and the agents flow leftward from it, in the order of
/// the design composers below, each over the first design its work serves.
fn render_rail(render: &GroupRender<'_, '_>, view: &GroupView, channels: &[Channel]) -> Html {
    let (counts, mut workers) = render
        .work
        .map(|work| work.group_worker_strip(&scope(view, channels)))
        .unwrap_or_default();
    workers.sort_by_key(|(_, task)| {
        let statement = &task.row().statement.body;
        channels
            .iter()
            .position(|channel| statement.is_associated_with_design(&channel.stamp))
            .unwrap_or(channels.len())
    });
    html! {
        <div class="team-task-rail">
            if !workers.is_empty() {
                {render_total(render, view, channels, counts)}
                <nav class="app-horn-tabs team-tasks" aria-label="Agents currently working in this group's designs">
                    {for workers.into_iter().map(|(lane, _)| render_worker(render, lane))}
                </nav>
            }
        </div>
    }
}

/// The trailing tab: a badge counting every active task the rail shows, opening
/// the overview with exactly that current selection.
fn render_total(
    render: &GroupRender<'_, '_>,
    view: &GroupView,
    channels: &[Channel],
    counts: WorkCounts,
) -> Html {
    let title = view::chrome::task_count_title("Open work active in this group", counts);
    let open_group = {
        let navigate = render.navigate.clone();
        let scope = scope(view, channels);
        Callback::from(move |_| navigate.emit(navigation::Intent::Work(scope.clone())))
    };
    html! {
        <span
            key="group-total"
            class="app-horn-tab app-horn-tab--up app-horn-tab--group"
            data-work-scope="group"
            data-work-mark={view::chrome::work_mark(counts)}
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
/// One agent tab in that lane's canonical accent. A currently admitted Chat
/// member links to its composer; a lane absent from Chat stays a plain label.
fn render_worker(render: &GroupRender<'_, '_>, lane: wire::CanonicalUuid) -> Html {
    let lanes = render.runtime.lanes();
    let member = lanes.and_then(|lanes| lanes.member(lane));
    let label = crate::work::owner_label(&wire::Actor::Lane(lane), member, None);
    let accent = lanes.and_then(|lanes| watchband::lane_accent(&lanes.lanes, lane));
    let attributes = classes!("app-horn-tab", "app-horn-tab--up", "app-horn-tab__handle");
    let focus_key = format!("group-lane:{lane}");
    let lane_text = lane.to_string();
    let href = super::super::super::watchband::lane_composer_href(lanes, lane);
    if let Some(href) = href {
        html! {
            <a
                key={lane_text.clone()}
                class={attributes}
                title={format!("Open {label} in Chat")}
                aria-label={format!("Open {label} in Chat")}
                data-lane={lane_text}
                data-accent={accent.map(|accent| accent.to_string())}
                data-sidebar-focus-key={focus_key}
                {href}
            >{label}</a>
        }
    } else {
        html! {
            <span
                key={lane_text.clone()}
                class={attributes}
                title={label.clone()}
                data-lane={lane_text}
                data-accent={accent.map(|accent| accent.to_string())}
                data-sidebar-focus-key={focus_key}
            >{label}</span>
        }
    }
}

/// The newest words said across the group's Designs, aged and coloured by the
/// Design they landed in. A departure says nothing, so it never displaces
/// the latest message.
fn render_ticker(
    render: &GroupRender<'_, '_>,
    channels: &[Channel],
    standings: &[DesignStanding],
) -> Html {
    let newest = channels
        .iter()
        .zip(standings)
        .filter_map(|(channel, standing)| {
            let (publication, event, _) = super::newest_words(render.runtime, &channel.stamp)?;
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
