//! A Design's room read in place: its standing, preparation authority, and
//! room history, for open and settled Designs alike. The view holds no
//! control that writes, and showing it joins, groups, or wakes nothing.

use wire::TaskStamp;
use yew::prelude::*;

use super::{RoomProjectionStatus, render_event, render_preparation};
use crate::AppRuntime;

/// The read view of one Design's room, addressed by its birth stamp.
pub(in crate::browser) fn render_design_room(runtime: &AppRuntime, design: &TaskStamp) -> Html {
    let projection = runtime.room_projection();
    let body = match projection.status() {
        RoomProjectionStatus::Loading => {
            html! { <p class="room-empty" aria-busy="true">{"Loading room…"}</p> }
        }
        RoomProjectionStatus::RequiresSnapshot => {
            html! { <p class="room-unavailable">{"Room authority unavailable until reconnect."}</p> }
        }
        RoomProjectionStatus::Available => {
            let events = projection.room(design).map(|room| room.events());
            html! {
                <>
                    {super::standing::render_standing(runtime, design)}
                    {render_preparation(runtime, design)}
                    <section class="room-history" aria-label="Room history">
                        <header>
                            <strong>{"Room history"}</strong>
                            <span>{events.map_or(0, |events| events.len())}</span>
                        </header>
                        if let Some(events) = events.filter(|events| !events.is_empty()) {
                            {for events.iter().rev().map(|(publication, event)| {
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
