//! The activity trace inside a team or working-group pip: a glowing curve per
//! kind of work over a fading fill, a dot where each curve meets now, and
//! lifecycle markers. A team's pip traces its lane; a group's pip traces its
//! Design's room, through the same trace. Everything is SVG geometry in the
//! pane's measured CSS pixels, which the app's `style-src 'self'` policy admits
//! where inline styles are refused; colours and glow stay in the stylesheet.

use std::fmt::Write as _;
use std::rc::Rc;

use gloo_timers::callback::Timeout;
use yew::prelude::*;

use super::super::clock::now_millis;
use crate::watchband::{
    TRACE_STEP_MILLIS, TRACE_WINDOW_MILLIS, TraceActivity, TraceMarker, TraceMarkerKind,
    room_activity, trace,
};

#[derive(Properties, PartialEq)]
pub(crate) struct ActivityTraceProps {
    /// Names the trace's gradients, unique on the page: the lane or Design.
    pub(crate) id: AttrValue,
    /// The traced activities, in any order.
    pub(crate) activities: Rc<[TraceActivity]>,
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
    runtime
        .activities()
        .iter()
        .filter(|activity| activity.lane() == lane && activity.display_millis() >= since)
        .map(|activity| TraceActivity {
            kind: activity.kind(),
            millis: activity.display_millis(),
            bytes: activity.text_bytes(),
        })
        .chain(directions)
        .collect()
}

/// One Design room's events within the trace window, read as a lane's
/// activity: an agent's words are prose, a lane's contribution is work landing
/// in the Design, a human's words are direction, a coordination opens a turn,
/// and a lane's departure divides the room as a compaction divides a lane.
pub(crate) fn design_activities(
    runtime: &crate::AppRuntime,
    design: &wire::TaskStamp,
) -> Rc<[TraceActivity]> {
    let since = now_millis() - TRACE_WINDOW_MILLIS;
    let Some(room) = runtime.room_projection().room(design) else {
        return Rc::from([]);
    };
    room.events()
        .iter()
        .filter_map(|(publication, event)| {
            let millis = publication.micros() / 1_000;
            let (kind, bytes) = room_activity(&event.kind)?;
            (millis >= since).then_some(TraceActivity {
                kind,
                millis,
                bytes,
            })
        })
        .collect()
}

/// Space the now-dot needs inside the pane's right edge, in CSS pixels.
const DOT_ROOM: f64 = 1.5;

/// Samples per curve: about one per six pixels, between five and nine, so a
/// narrow pip still bends and a wide one stays smooth.
fn samples(width: f64) -> usize {
    // Truncation is the intent: whole three-pixel columns.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let columns = ((width / 3.0).floor() as usize).clamp(6, 16);
    (columns.div_ceil(2) + 1).max(5)
}

/// Redraws on new activity and, while anything is shown, once per trace step
/// so the window slides with time; the pane's size is measured after each draw.
#[function_component(ActivityTrace)]
pub(crate) fn activity_trace(props: &ActivityTraceProps) -> Html {
    let tick = use_state(|| 0_u64);
    let pane = use_node_ref();
    // The pane's CSS size as last drawn; unknown until the first draw.
    let size = use_state_eq(|| (0.0_f64, 0.0_f64));
    {
        let pane = pane.clone();
        let size = size.clone();
        use_effect(move || {
            if let Some(pane) = pane.cast::<web_sys::Element>() {
                let rect = pane.get_bounding_client_rect();
                size.set((rect.width(), rect.height()));
            }
        });
    }
    let (width, height) = *size;
    let placed = trace(&props.activities, now_millis(), samples(width));
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
    let drawn = width > DOT_ROOM && height > DOT_ROOM;
    let gradient = |key: &str| format!("trace-{}-{key}", props.id);
    html! {
        <span ref={pane} class="activity-trace" aria-hidden="true">
            if drawn {
                <svg viewBox={format!("0 0 {width:.2} {height:.2}")}>
                    <defs>
                        {for placed.curves.iter().map(|curve| html! {
                            <linearGradient
                                id={gradient(curve.kind.key())}
                                data-trace-kind={curve.kind.key()}
                                x1="0" y1="0" x2="0" y2="1"
                            >
                                <stop offset="0" stop-opacity=".42" />
                                <stop offset="1" stop-opacity="0" />
                            </linearGradient>
                        })}
                    </defs>
                    {for placed.curves.iter().map(|curve| {
                        let points = points(&curve.levels, width, height);
                        let line = smooth(&points, height);
                        let (x, y) = points[points.len() - 1];
                        html! {
                            <g data-trace-kind={curve.kind.key()}>
                                <path
                                    class="trace-fill"
                                    d={format!("{line} L{x:.2},{height:.2} L0,{height:.2} Z")}
                                    fill={format!("url(#{})", gradient(curve.kind.key()))}
                                />
                                <g class="trace-lit">
                                    <path class="trace-line" d={line} />
                                    <circle class="trace-now" cx={format!("{x:.2}")} cy={format!("{y:.2}")} r="1.1" />
                                </g>
                            </g>
                        }
                    })}
                    {for placed.markers.iter().map(|marker| render_marker(*marker, width, height))}
                </svg>
            }
        </span>
    }
}

/// Curve points in pane pixels: time runs across to the now-dot's room, level
/// rises from the floor.
fn points(levels: &[f64], width: f64, height: f64) -> Vec<(f64, f64)> {
    #[allow(clippy::cast_precision_loss)]
    let last = (levels.len() - 1) as f64;
    levels
        .iter()
        .enumerate()
        .map(|(index, level)| {
            #[allow(clippy::cast_precision_loss)]
            let x = index as f64 * (width - DOT_ROOM) / last;
            (x, height - 0.5 - level * (height - DOT_ROOM))
        })
        .collect()
}

/// A Catmull-Rom curve through the points as cubic segments, its control
/// points held inside the pane so a sharp rise cannot overshoot the edge.
fn smooth(points: &[(f64, f64)], height: f64) -> String {
    let clamp = |y: f64| y.clamp(0.25, height - 0.25);
    let mut d = format!("M{:.2},{:.2}", points[0].0, points[0].1);
    for index in 0..points.len() - 1 {
        let p0 = points[index.saturating_sub(1)];
        let p1 = points[index];
        let p2 = points[index + 1];
        let p3 = points[(index + 2).min(points.len() - 1)];
        let c1 = (
            p1.0 + (p2.0 - p0.0) / 6.0,
            clamp(p1.1 + (p2.1 - p0.1) / 6.0),
        );
        let c2 = (
            p2.0 - (p3.0 - p1.0) / 6.0,
            clamp(p2.1 - (p3.1 - p1.1) / 6.0),
        );
        let _ = write!(
            d,
            " C{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}",
            c1.0, c1.1, c2.0, c2.1, p2.0, p2.1
        );
    }
    d
}

/// Snap to the half pixel, a whole device pixel on a 2x display.
fn snap(value: f64) -> f64 {
    (value * 2.0).round() / 2.0
}

fn render_marker(marker: TraceMarker, width: f64, height: f64) -> Html {
    let key = marker.kind.key();
    let x = snap(marker.x * (width - DOT_ROOM));
    // A half-pixel rule centred inside its device pixel.
    let rule = x + 0.25;
    match marker.kind {
        TraceMarkerKind::Direction => html! {
            <circle data-trace-marker={key} cx={format!("{x:.2}")} cy="1.25" r=".9" />
        },
        TraceMarkerKind::Failure => html! {
            <rect data-trace-marker={key} x={format!("{x:.2}")} y="0" width="1" height={format!("{height:.2}")} />
        },
        // A fresh session ticks forward from its rule; a sign-out ticks back.
        TraceMarkerKind::Session | TraceMarkerKind::SignOut => {
            let tick = if marker.kind == TraceMarkerKind::Session {
                1.5
            } else {
                -1.5
            };
            html! {
                <path
                    data-trace-marker={key}
                    d={format!("M{rule:.2},{height:.2} V0.25 H{:.2}", rule + tick)}
                />
            }
        }
        TraceMarkerKind::Turn | TraceMarkerKind::Compaction => html! {
            <line
                data-trace-marker={key}
                x1={format!("{rule:.2}")} y1="0"
                x2={format!("{rule:.2}")} y2={format!("{height:.2}")}
            />
        },
    }
}
