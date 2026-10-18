//! One lane's activity trace inside its team pip.
//!
//! Notes and markers are spans positioned in percentages of the pane, so the
//! stylesheet owns pixel minimums and snapping; the thread is one stretched
//! path whose stroke does not scale. The app serves `style-src 'self'`, which
//! refuses inline style attributes, so each span's position reaches CSS as
//! custom properties written through the CSSOM after every render.

use std::fmt::Write as _;
use std::rc::Rc;

use gloo_timers::callback::Timeout;
use wasm_bindgen::JsCast;
use yew::prelude::*;

use super::super::clock::now_millis;
use crate::watchband::{TRACE_STEP_MILLIS, TRACE_WINDOW_MILLIS, Trace, TraceActivity, trace};

#[derive(Properties, PartialEq)]
pub(super) struct ActivityTraceProps {
    /// The lane's activities, in any order.
    pub(super) activities: Rc<[TraceActivity]>,
}

/// One lane's retained activity within the trace window. Lane activity
/// excludes operator direction, a card in the receiver's plane, so the trace
/// marks direction from that card.
pub(super) fn lane_activities(
    runtime: &crate::AppRuntime,
    lane: wire::CanonicalUuid,
) -> Rc<[TraceActivity]> {
    let since = now_millis() - TRACE_WINDOW_MILLIS;
    let directions = runtime
        .messages()
        .iter()
        .filter(|message| {
            message.lane == lane
                && message.activity == wire::AgentActivityKind::Direction
                && message.display_millis() >= since
        })
        .map(|message| TraceActivity {
            kind: message.activity,
            millis: message.display_millis(),
            bytes: 0,
        });
    // Retained newest first; oldest first keeps equal instants in arrival order.
    runtime
        .activities()
        .iter()
        .rev()
        .filter(|activity| activity.lane() == lane && activity.display_millis() >= since)
        .map(|activity| TraceActivity {
            kind: activity.kind(),
            millis: activity.display_millis(),
            bytes: activity.detail().len(),
        })
        .chain(directions)
        .collect()
}

/// Least distance between neighbouring notes in a row, in CSS pixels: a
/// one-pixel note and the one-pixel gap the stylesheet leaves before the next.
const NOTE_PITCH_PX: f64 = 2.0;

/// Redraws on new activity and, while anything is shown, once per trace step
/// so the window slides with time and the note pitch follows the pane's width.
#[function_component(ActivityTrace)]
pub(super) fn activity_trace(props: &ActivityTraceProps) -> Html {
    let tick = use_state(|| 0_u64);
    let pane = use_node_ref();
    // The pane's CSS width as last drawn; unknown until the first draw.
    let width = use_state_eq(|| 0.0_f64);
    let pitch = if *width > 0.0 {
        NOTE_PITCH_PX / *width
    } else {
        0.0
    };
    let placed = Rc::new(trace(&props.activities, now_millis(), pitch));
    {
        let pane = pane.clone();
        let placed = placed.clone();
        let width = width.clone();
        use_effect(move || {
            if let Some(drawn) = position(&pane, &placed) {
                width.set(drawn);
            }
        });
    }
    {
        let tick = tick.clone();
        use_effect_with((!placed.is_empty(), *tick), move |&(shown, generation)| {
            let timeout = shown.then(|| {
                Timeout::new(TRACE_STEP_MILLIS, move || {
                    tick.set(generation.wrapping_add(1));
                })
            });
            move || drop(timeout)
        });
    }
    if placed.is_empty() {
        return Html::default();
    }
    // The path's box spans the pane: x in percent, y in rows, centred per row.
    let mut thread = String::new();
    for note in &placed.notes {
        let x = note.x * 100.0;
        let y = note.row.index() as f64 + 0.5;
        if thread.is_empty() {
            let _ = write!(thread, "M{x:.2} {y}");
        } else {
            let _ = write!(thread, "H{x:.2}V{y}");
        }
    }
    if !thread.is_empty() {
        let _ = write!(thread, "H{:.2}", placed.thread_end * 100.0);
    }
    html! {
        <span ref={pane} class="activity-trace" aria-hidden="true">
            if !thread.is_empty() {
                <svg viewBox="0 0 100 3" preserveAspectRatio="none">
                    <path d={thread} />
                </svg>
            }
            {for placed.notes.iter().map(|note| html! {
                <span data-trace-note={note.row.key()} data-trace-overflow={note.overflow.then_some("")}></span>
            })}
            {for placed.markers.iter().map(|marker| html! {
                <span data-trace-marker={marker.kind.key()}></span>
            })}
        </span>
    }
}

/// Write each rendered note's and marker's place, in render order, and answer
/// the pane's CSS width, or nothing when no trace is drawn.
fn position(pane: &NodeRef, placed: &Trace) -> Option<f64> {
    let pane = pane.cast::<web_sys::Element>()?;
    let notes = placed.notes.iter().map(|note| {
        [
            ("--x", format!("{:.2}", note.x * 100.0)),
            ("--length", format!("{:.2}", note.length * 100.0)),
            ("--room", format!("{:.2}", note.room * 100.0)),
            ("--row", note.row.index().to_string()),
        ]
    });
    write(&pane, "[data-trace-note]", notes);
    let markers = placed
        .markers
        .iter()
        .map(|marker| [("--x", format!("{:.2}", marker.x * 100.0))]);
    write(&pane, "[data-trace-marker]", markers);
    Some(pane.get_bounding_client_rect().width())
}

fn write<const N: usize>(
    pane: &web_sys::Element,
    selector: &str,
    places: impl Iterator<Item = [(&'static str, String); N]>,
) {
    let spans = pane
        .query_selector_all(selector)
        .expect("the trace selectors are valid");
    for (index, place) in (0..spans.length()).zip(places) {
        let style = spans
            .item(index)
            .and_then(|span| span.dyn_into::<web_sys::HtmlElement>().ok())
            .expect("a trace span is an HTML element")
            .style();
        for (property, value) in place {
            style
                .set_property(property, &value)
                .unwrap_or_else(|_| panic!("the activity trace writes {property}"));
        }
    }
}
