//! A task's room read in place: the composer that publishes into it, a
//! Design's authority — its standing, positive conclusion control, immutable
//! proposal versions and exact decisions, and kind-grouped contributions —
//! and the room's conversation newest first, for open and settled tasks
//! alike. Showing the view joins, groups, or wakes nothing.

use wire::TaskStamp;
use wire::intent::BoardTask;
use wire::rooms::{DesignMaturity, DesignMaturityHead, DesignMaturityState};
use yew::prelude::*;

use super::{Controls, RoomProjectionStatus, card};
use crate::AppRuntime;
use crate::browser::sender::SenderHandle;

/// The read view of one task's room beneath `composer`. `detail` is the
/// task's row from its task detail; the board's row wins while the board
/// carries it, since only the board follows working-group moves. A Design's
/// room adds its Design authority above the same conversation.
pub(in crate::browser) fn render_task_room(
    runtime: &AppRuntime,
    controls: &Controls,
    sender: &SenderHandle,
    detail: &BoardTask,
    composer: Html,
) -> Html {
    let room = &detail.statement.body.born;
    let row = runtime
        .board()
        .and_then(|board| board.row(room))
        .unwrap_or(detail);
    let design = row.design_standing().is_some();
    let projection = runtime.room_projection();
    let body = match projection.status() {
        RoomProjectionStatus::Loading => {
            html! { <p class="room-empty" aria-busy="true">{"Loading room…"}</p> }
        }
        RoomProjectionStatus::RequiresSnapshot => {
            html! { <p class="room-unavailable">{"Room authority unavailable until reconnect."}</p> }
        }
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
                            let card =
                                super::approval::render_proposal_card(runtime, room, proposal);
                            (proposal.position, card)
                        }),
                )
                .collect::<Vec<_>>();
            cards.sort_by_key(|(position, _)| std::cmp::Reverse(*position));
            html! {
                <>
                    if design {
                        {super::standing::render_standing(runtime, room, Some(row))}
                        {render_maturity(projection.maturity(room), room)}
                        {super::conclusion::render_conclusion(runtime, controls, sender, row)}
                        {super::approval::render_approval(runtime, controls, sender, room)}
                        {super::contributions::render_groups(runtime, room)}
                        {super::contribute::render_note(row)}
                    }
                    <section class="room-history" aria-label="Room conversation">
                        <header>
                            <strong>{"Conversation"}</strong>
                            <span>{cards.len()}</span>
                        </header>
                        if cards.is_empty() {
                            <p class="room-empty">{"No room publications."}</p>
                        } else {
                            {for cards.into_iter().map(|(_, card)| card)}
                        }
                    </section>
                </>
            }
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
                    <a href={crate::view::design_href(room)}>{"Permalink"}</a>
                } else {
                    <h3>{"Room"}</h3>
                }
            </div>
            {composer}
            {body}
        </section>
    }
}

fn render_maturity(maturity: Option<&DesignMaturity>, design: &TaskStamp) -> Html {
    let empty = DesignMaturity::default();
    let maturity = maturity.unwrap_or(&empty);
    let (state, summary) = match maturity.state() {
        DesignMaturityState::Fresh => ("fresh", "No contribution has been recorded.".to_owned()),
        DesignMaturityState::Steeped => (
            "steeped",
            "Substantive evidence spans three or more contributors.".to_owned(),
        ),
        DesignMaturityState::Steeping { unmet } => (
            "steeping",
            format!(
                "Still needs {}.",
                unmet
                    .into_iter()
                    .map(|requirement| requirement.label())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ),
    };
    html! {
        <section
            class="room-maturity"
            aria-label="Design maturity"
            data-design={design.to_string()}
            data-maturity-state={state}
        >
            <header>
                <strong>{"Maturity"}</strong>
                <span class="room-maturity-state">{state}</span>
            </header>
            <p>{summary}</p>
            <dl>
                <div>
                    <dt>{"Contributors"}</dt>
                    <dd>{bounded_count(maturity.contributing_lanes.len(), maturity.contributing_lanes_truncated)}</dd>
                </div>
                <div>
                    <dt>{"For"}</dt>
                    <dd>{maturity.contributions.reason_for}</dd>
                </div>
                <div>
                    <dt>{"Against"}</dt>
                    <dd>{maturity.contributions.reason_against}</dd>
                </div>
                <div>
                    <dt>{"Evidence"}</dt>
                    <dd>{maturity.contributions.evidence}</dd>
                </div>
                <div>
                    <dt>{"Questions"}</dt>
                    <dd>{maturity.contributions.question}</dd>
                </div>
                <div>
                    <dt>{"Requested reach"}</dt>
                    <dd>
                        {format!(
                            "{} requests · {} lanes",
                            maturity.requests,
                            bounded_count(
                                maturity.requested_lanes.len(),
                                maturity.requested_lanes_truncated,
                            ),
                        )}
                    </dd>
                </div>
            </dl>
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
            <small>{"Advisory only; this neither blocks nor authorizes a Design decision."}</small>
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

fn bounded_count(count: usize, truncated: bool) -> String {
    format!("{count}{}", if truncated { "+" } else { "" })
}
