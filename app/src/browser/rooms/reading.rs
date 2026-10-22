//! A task's room read in place: a Design's Standing beside its Maturity and
//! provenance beneath the room heading, the composer that publishes into the
//! room, the Design's positive conclusion control, immutable proposal versions
//! and exact decisions, and the room's conversation newest first, for open and
//! settled tasks alike. Showing the view joins, groups, or wakes nothing.

use wire::TaskStamp;
use wire::intent::BoardTask;
use wire::rooms::{DesignItemInput, DesignMaturity, DesignMaturityHead, DesignMaturityState};
use yew::prelude::*;

use super::{Controls, RoomProjectionStatus, card};
use crate::AppRuntime;
use crate::browser::sender::SenderHandle;

/// The read view of one task's room around `composer`. `detail` is the
/// task's row from its task detail; the board's row wins while the board
/// carries it, since only the board follows working-group moves. A Design's
/// room pairs its Standing with a column of Maturity over `aside` (the
/// sheet's provenance facts) above the composer, so the shorter column fills
/// the height the lane list takes, and adds its Design authority above the
/// same conversation. While the room is still loading, `aside` follows the
/// loading notice so the facts never disappear.
pub(in crate::browser) fn render_task_room(
    runtime: &AppRuntime,
    controls: &Controls,
    sender: &SenderHandle,
    open_task: &Callback<(TaskStamp, web_sys::Element)>,
    detail: &BoardTask,
    composer: Html,
    aside: Html,
) -> Html {
    let room = &detail.statement.body.born;
    let row = runtime
        .board()
        .and_then(|board| board.row(room))
        .unwrap_or(detail);
    let design = row.design_standing().is_some();
    let projection = runtime.room_projection();
    let (pair, body) = match projection.status() {
        RoomProjectionStatus::Loading => (
            aside,
            html! { <p class="room-empty" aria-busy="true">{"Loading room…"}</p> },
        ),
        RoomProjectionStatus::RequiresSnapshot => (
            aside,
            html! { <p class="room-unavailable">{"Room authority unavailable until reconnect."}</p> },
        ),
        RoomProjectionStatus::Available => {
            let has_current_listener = projection.has_current_listener(room, runtime.board());
            let mut cards = projection
                .room(room)
                .map(|room| room.in_journal_order())
                .unwrap_or_default()
                .into_iter()
                .map(|(publication, event)| {
                    let card = card::render_card(
                        runtime,
                        None,
                        open_task,
                        room,
                        has_current_listener,
                        publication,
                        event,
                    );
                    (event.position, card)
                })
                .chain(
                    projection
                        .preparation()
                        .candidates
                        .values()
                        .filter(|proposal| proposal.value.candidate.controller == *room)
                        .map(|proposal| {
                            let card = super::approval::render_proposal_card(
                                runtime, open_task, room, proposal,
                            );
                            (proposal.position, card)
                        }),
                )
                .collect::<Vec<_>>();
            cards.sort_by_key(|(position, _)| std::cmp::Reverse(*position));
            let pair = if design {
                html! {
                    <div class="room-design-pair">
                        {super::standing::render_standing(runtime, room, Some(row))}
                        <div class="room-design-aside">
                            {render_maturity(
                                projection.maturity(room),
                                room,
                                &row.statement.body.acceptance,
                            )}
                            {aside}
                        </div>
                    </div>
                }
            } else {
                aside
            };
            let body = html! {
                <>
                    if design {
                        {super::conclusion::render_conclusion(runtime, controls, sender, row)}
                        {super::approval::render_approval(runtime, controls, sender, open_task, room)}
                    }
                    <section class="room-history" aria-label="Room conversation">
                        if cards.is_empty() {
                            <p class="room-empty">{"No room publications."}</p>
                        } else {
                            <header>
                                <h4>{"Conversation"}</h4>
                                <span>{cards.len()}</span>
                            </header>
                            {for cards.into_iter().map(|(_, card)| card)}
                        }
                    </section>
                </>
            };
            (pair, body)
        }
    };
    html! {
        <section
            class="app-sheet-section room-reading"
            data-task-room={room.to_string()}
            data-design-room={design.then(|| room.to_string())}
        >
            <div class="app-section-heading">
                if design {
                    <h3>{"Design room"}</h3>
                    <a class="app-compact-control" href={crate::view::design_href(room)}>{"Permalink"}</a>
                } else {
                    <h3>{"Room"}</h3>
                }
            </div>
            {pair}
            {composer}
            {body}
        </section>
    }
}

/// `design`'s maturity judged against `acceptance`, its current statement's
/// question items; each item's own standing reads in the Contract list.
fn render_maturity(
    maturity: Option<&DesignMaturity>,
    design: &TaskStamp,
    acceptance: &[String],
) -> Html {
    let empty = DesignMaturity::default();
    let maturity = maturity.unwrap_or(&empty);
    let judged = maturity.state(acceptance);
    let state = match judged {
        DesignMaturityState::Fresh => "fresh",
        DesignMaturityState::Steeping { .. } => "steeping",
        DesignMaturityState::Steeped { .. } => "steeped",
    };
    html! {
        <section
            class="room-maturity"
            aria-label="Design maturity"
            data-design={design.to_string()}
            data-maturity-state={state}
        >
            <header>
                <h4>{"Maturity"}</h4>
                <span class="room-maturity-state">{state}</span>
            </header>
            <p>{summary(&judged)}</p>
            if !maturity.recent_heads.is_empty() {
                <div class="room-maturity-heads" aria-label="Maturity by statement">
                    {for maturity.recent_heads.iter().rev().map(render_head)}
                </div>
            }
            if maturity.older_heads.statement_heads > 0 {
                <p class="room-maturity-folded">
                    {format!(
                        "{} older statement heads folded · {} asked lanes contributed · {} declined",
                        maturity.older_heads.statement_heads,
                        maturity.older_heads.requested_contributor_lane_heads,
                        maturity.older_heads.requested_decliner_lane_heads,
                    )}
                </p>
            }
            if maturity.unrecorded_head.contributions > 0 {
                <p class="room-maturity-folded">
                    {format!(
                        "{} historical contributions have an unrecorded statement head",
                        maturity.unrecorded_head.contributions,
                    )}
                </p>
            }
        </section>
    }
}

/// One statement head's answers from the lanes it asked.
fn render_head(head: &DesignMaturityHead) -> Html {
    html! {
        <p>
            <code>{head.statement.to_string()}</code>
            <span>
                {format!(
                    "{} asked · {} contributed · {} declined · {} unanswered{}",
                    bounded_count(head.requested_lanes.len(), head.requested_lanes_truncated),
                    head.requested_contributors(),
                    head.requested_decliners(),
                    head.requested_unanswered(),
                    if head.yield_truncated() { " (bounded)" } else { "" },
                )}
            </span>
        </p>
    }
}

/// Where the Design stands, naming the vote a steeping item still needs.
fn summary(state: &DesignMaturityState) -> String {
    let DesignMaturityState::Steeping {
        next: Some(need),
        quiet,
    } = state
    else {
        return state.summary();
    };
    format!(
        "steeping; item {} is {} and needs {}; {quiet} {} quiet",
        need.question,
        need.item.label(),
        vote_needed(need.needs),
        if *quiet == 1 { "item" } else { "items" },
    )
}

/// The vote that would move an item toward concurrence.
const fn vote_needed(input: DesignItemInput) -> &'static str {
    match input {
        DesignItemInput::AnyCharge => "any vote",
        DesignItemInput::PositiveLanes => "more lanes for",
        DesignItemInput::NegativeLanes => "more lanes against",
        DesignItemInput::PositiveAgreement => {
            "a vote for from a lane that last voted against, or more lanes for"
        }
        DesignItemInput::NegativeAgreement => {
            "a vote against from a lane that last voted for, or more lanes against"
        }
        DesignItemInput::PositiveStrength => "a strongly-for vote",
        DesignItemInput::NegativeStrength => "a strongly-against vote",
    }
}

fn bounded_count(count: usize, truncated: bool) -> String {
    format!("{count}{}", if truncated { "+" } else { "" })
}
