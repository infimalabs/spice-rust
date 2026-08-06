//! The lane mosaic: the surface work is directed from rather than read on.
//!
//! Every axis renders one derivation over what the resident holds. This renders
//! the resident itself: the lanes as columns, what each is carrying, and the
//! events arriving behind them. The join from a lane to the work it holds is
//! [`wire::ClaimHead::lane`] against [`crate::runtime::LaneRow::lane`], which is
//! the same identity the allocator binds a claim with, so a column shows the
//! task the lane actually holds rather than one inferred from a name.

use wire::intent::BoardTask;

use super::*;
use crate::runtime::{LaneRow, LaneView};

pub(super) fn render_mosaic(surface: &SurfaceData, connection: &Connection) -> Html {
    let Some(runtime) = surface.as_ref() else {
        return empty("No admitted mosaic is available.");
    };
    let Some(lanes) = runtime.lanes() else {
        return empty("No lane snapshot is available.");
    };
    html! {
        <>
            {render_lane_strip(runtime, lanes)}
            {render_live_status(lanes, connection)}
            {render_event_mosaic(runtime)}
        </>
    }
}

/// One column per seated lane, carrying what that lane currently holds.
fn render_lane_strip(runtime: &CanvasRuntime, lanes: &LaneView) -> Html {
    let held = held_by_lane(runtime);
    html! {
        <section class="section mosaic-strip-section">
            <div class="section-heading">
                <h2>{"Lanes"}</h2>
                <span class="section-count">{format!("{} seated", lanes.lanes.len())}</span>
            </div>
            if lanes.lanes.is_empty() {
                {empty("No lanes are seated.")}
            } else {
                <div class="mosaic-strip">
                    {for lanes.lanes.iter().map(|lane| render_lane_column(lane, &held))}
                </div>
            }
        </section>
    }
}

/// The work each lane holds, keyed by the lane its claim names.
///
/// A lane with no active claim is absent rather than present and empty, so the
/// column below states that it is idle instead of rendering a blank handle.
fn held_by_lane(runtime: &CanvasRuntime) -> BTreeMap<CanonicalUuid, &BoardTask> {
    let Some(board) = runtime.board() else {
        return BTreeMap::new();
    };
    board
        .active_public_tasks()
        .filter_map(|task| task.claim.as_ref().map(|claim| (claim.body.lane, task)))
        .collect()
}

fn render_lane_column(lane: &LaneRow, held: &BTreeMap<CanonicalUuid, &BoardTask>) -> Html {
    let holding = held.get(&lane.lane);
    html! {
        <article class="mosaic-lane" data-disposition={lane.disposition.clone()}>
            <header class="mosaic-lane-head">
                <span class="mosaic-lane-name">{lane.worktree.clone()}</span>
                <span class="tag active">{lane.disposition.clone()}</span>
            </header>
            <div class="mosaic-lane-body">
                if let Some(task) = holding {
                    <span class="mosaic-lane-handle mono">
                        {task.statement.body.handle.to_string()}
                    </span>
                    <span class="mosaic-lane-phase">
                        {task.workflow.body.phase.to_string()}
                    </span>
                    <span class="mosaic-lane-title">{task.presentation_title.clone()}</span>
                } else {
                    <span class="mosaic-lane-idle">{"holding nothing"}</span>
                }
            </div>
            <footer class="mosaic-lane-foot">
                <span class="mosaic-lane-vendor">{lane.vendor.to_string()}</span>
                <span class="mono">{
                    lane.occupant
                        .map(|value| short(&value.to_string()))
                        .unwrap_or_else(|| "vacant".to_owned())
                }</span>
            </footer>
        </article>
    }
}

/// The one live line: what the resident is, and whether this app is hearing it.
fn render_live_status(lanes: &LaneView, connection: &Connection) -> Html {
    let (state, detail) = match connection {
        Connection::Starting => ("starting", "the surface has not opened yet".to_owned()),
        Connection::Connecting => ("connecting", "the surface is opening".to_owned()),
        Connection::Online { actor, epoch } => (
            "online",
            format!("{} at epoch {}", short(actor), short(epoch)),
        ),
        Connection::Offline(reason) => ("offline", reason.clone()),
    };
    html! {
        <div class="mosaic-live" data-state={state}>
            <span class="mosaic-live-state">{state}</span>
            <span class="mosaic-live-resident">{format!("resident {:?}", lanes.state)}</span>
            <span class="mosaic-live-detail">{detail}</span>
        </div>
    }
}

/// The arriving events, laid out as cards rather than as a table.
///
/// The axes already carry the tabular readings. This is the same evidence in
/// the shape an operator scans rather than queries, which is the whole reason
/// the mosaic is a surface of its own.
fn render_event_mosaic(runtime: &CanvasRuntime) -> Html {
    let events = runtime.events();
    html! {
        <section class="section">
            <div class="section-heading">
                <h2>{"Arriving"}</h2>
                <span class="section-count">{format!("{} events", events.len())}</span>
            </div>
            if events.is_empty() {
                {empty("Nothing has arrived on this connection yet.")}
            } else {
                <div class="mosaic-cards">
                    {for events.iter().map(|event| html! {
                        <article class="mosaic-card" data-class={event.class()}>
                            <header class="mosaic-card-head">
                                <span class="tag active">{event.class()}</span>
                                <span class="mosaic-card-title">{event.title().to_owned()}</span>
                            </header>
                            <p class="mosaic-card-detail">{event.detail().to_owned()}</p>
                        </article>
                    })}
                </div>
            }
        </section>
    }
}
