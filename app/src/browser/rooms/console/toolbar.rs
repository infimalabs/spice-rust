//! A working group's toolbar, the team toolbar's twin read from Designs: an
//! open-task total and one tab per task associated with the group's
//! Designs, one standing light per Design, and a ticker of the newest room
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
use crate::work::{WorkCounts, WorkScope, WorkTask};

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

/// The scope that selects this group's associated work.
fn scope(view: &GroupView, channels: &[Channel]) -> WorkScope {
    WorkScope::Group {
        group: view.group.clone(),
        designs: channels
            .iter()
            .map(|channel| channel.stamp.clone())
            .collect(),
    }
}

/// The fixed total and the task scroller.
fn render_rail(render: &GroupRender<'_, '_>, view: &GroupView, channels: &[Channel]) -> Html {
    let tasks = render
        .work
        .map(|work| work.scope_strip(&scope(view, channels)))
        .unwrap_or_default();
    html! {
        <div class="team-task-rail">
            if !tasks.is_empty() {
                {render_total(render, view, channels, &tasks)}
                <nav class="app-horn-tabs team-tasks" aria-label="Open tasks associated with this group's designs">
                    {for tasks.iter().copied().map(|task| render_handle(render, channels, task))}
                </nav>
            }
        </div>
    }
}

/// The leading tab: a badge counting every task the rail shows, opening the
/// overview with the group's whole associated selection.
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
        "Open all of this group's work: {open} {}",
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

/// One task-detail target in the accent of the Design it serves.
fn render_handle(render: &GroupRender<'_, '_>, channels: &[Channel], task: &WorkTask<'_>) -> Html {
    let statement = &task.row().statement.body;
    let availability = task.availability();
    let accent = channels
        .iter()
        .find(|channel| crate::runtime::associated(statement, &channel.stamp))
        .map(|channel| channel.accent);
    let label = format!(
        "Open {}: {}, {}",
        statement.handle,
        crate::composer_markdown::inline::plain_text(task.row().presentation_title()),
        availability.key(),
    );
    let open_task = {
        let select = render.select_task.clone();
        let born = statement.born.clone();
        Callback::from(move |_| select.emit(born.clone()))
    };
    html! {
        <button
            key={statement.born.to_string()}
            class={classes!(
                "app-horn-tab",
                "app-horn-tab--up",
                "app-horn-tab__handle",
                (!availability.moving()).then_some("dormant"),
            )}
            type="button"
            title={label.clone()}
            aria-label={label}
            data-availability={availability.key()}
            data-accent={accent.map(|accent| accent.to_string())}
            data-sidebar-focus-key={format!("group-task:{}", statement.born)}
            onclick={open_task}
        >{statement.handle.to_string()}</button>
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
