//! A Design's room read in place: its standing, conclusion control, approval
//! control, preparation authority, kind-grouped contributions, and room
//! history, for open and concluded Designs alike. The controls that write are
//! the operator's conclusion of an open Design and review evidence over a
//! retained candidate; showing the view joins, groups, or wakes nothing.

use wire::intent::BoardTask;
use yew::prelude::*;

use super::{Controls, RoomProjectionStatus, render_event, render_preparation};
use crate::AppRuntime;
use crate::browser::sender::SenderHandle;

/// The read view of one Design's room. `detail` is the Design's row from its
/// task detail; the board's row wins while the board carries it, since only
/// the board follows working-group moves.
pub(in crate::browser) fn render_design_room(
    runtime: &AppRuntime,
    controls: &Controls,
    sender: &SenderHandle,
    detail: &BoardTask,
) -> Html {
    let design = &detail.statement.body.born;
    let row = runtime
        .board()
        .and_then(|board| board.row(design))
        .unwrap_or(detail);
    let projection = runtime.room_projection();
    let body = match projection.status() {
        RoomProjectionStatus::Loading => {
            html! { <p class="room-empty" aria-busy="true">{"Loading room…"}</p> }
        }
        RoomProjectionStatus::RequiresSnapshot => {
            html! { <p class="room-unavailable">{"Room authority unavailable until reconnect."}</p> }
        }
        RoomProjectionStatus::Available => {
            let events = projection.room(design).map(|room| room.in_journal_order());
            html! {
                <>
                    {super::standing::render_standing(runtime, design, Some(row))}
                    {super::conclusion::render_conclusion(runtime, controls, sender, row)}
                    {super::approval::render_approval(runtime, controls, sender, design)}
                    {render_preparation(runtime, design)}
                    {super::contributions::render_groups(runtime, design)}
                    {super::contribute::render_composer(runtime, controls, sender, row)}
                    <section class="room-history" aria-label="Room history">
                        <header>
                            <strong>{"Room history"}</strong>
                            <span>{events.as_ref().map_or(0, Vec::len)}</span>
                        </header>
                        if let Some(events) = events.filter(|events| !events.is_empty()) {
                            {for events.into_iter().rev().map(|(publication, event)| {
                                render_event(runtime, None, publication, event)
                            })}
                        } else {
                            <p class="room-empty">{"No room publications."}</p>
                        }
                    </section>
                </>
            }
        }
    };
    html! {
        <section class="app-sheet-section room-reading" data-design-room={design.to_string()}>
            <div class="app-section-heading">
                <h3>{"Design room"}</h3>
                <a href={crate::view::design_href(design)}>{"Permalink"}</a>
            </div>
            {body}
        </section>
    }
}
