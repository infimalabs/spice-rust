//! The activity trace across the foot of a lane's composer: a glowing curve per
//! kind of work over a fading fill, a dot where each curve meets now, and
//! lifecycle markers. Everything is SVG geometry in the pane's measured CSS
//! pixels, which the app's `style-src 'self'` policy admits where inline styles
//! are refused; colours and glow stay in the stylesheet.

use std::fmt::Write as _;
use std::rc::Rc;

use gloo_timers::callback::Timeout;
use yew::prelude::*;

use super::super::clock::now_millis;
use crate::watchband::{
    Occupancy, TRACE_STEP_MILLIS, TraceActivity, TraceMarker, TraceMarkerKind, TraceOff, TraceSpan,
    drawn_spans, off_intervals, trace,
};

#[derive(Properties, PartialEq)]
pub(crate) struct ActivityTraceProps {
    /// Names the trace's gradients, unique on the page: the lane.
    pub(crate) id: AttrValue,
    /// The traced activities, in any order.
    pub(crate) activities: Rc<[TraceActivity]>,
    /// Intervals the traced lane was off, which its curves leave undrawn.
    #[prop_or_default]
    pub(crate) off: Rc<[TraceOff]>,
}

/// One lane's retained activity, which its trace keeps to the window. Agent
/// activity carries reasoning, tool, and prose rates; operator direction is
/// retained on message cards rather than in that activity stream, so it joins
/// here as a marker.
fn lane_activities(runtime: &crate::AppRuntime, lane: wire::CanonicalUuid) -> Rc<[TraceActivity]> {
    let directions = runtime
        .messages()
        .iter()
        .filter(|message| {
            message.lane == lane && message.activity == wire::AgentActivityKind::Direction
        })
        .map(|message| TraceActivity {
            kind: message.activity,
            millis: message.display_millis(),
            bytes: 0,
        });
    runtime
        .activities()
        .iter()
        .filter(|activity| activity.lane() == lane)
        .map(|activity| TraceActivity {
            kind: activity.kind(),
            millis: activity.display_millis(),
            bytes: activity.text_bytes(),
        })
        .chain(directions)
        .collect()
}

/// The intervals one lane was off, from its retained occupancy transitions and
/// whether it is `seated_now`.
fn lane_off(
    runtime: &crate::AppRuntime,
    lane: wire::CanonicalUuid,
    seated_now: bool,
) -> Rc<[TraceOff]> {
    off_intervals(
        runtime.lane_presence_transitions(lane).map(|transition| {
            let occupancy = match (transition.occupant(), transition.renewed()) {
                (None, _) => Occupancy::Vacant,
                (Some(_), false) => Occupancy::Started,
                (Some(_), true) => Occupancy::Renewed,
            };
            (transition.display_millis(), occupancy)
        }),
        seated_now,
    )
    .into()
}

/// One lane's trace across the foot of its primary composer, drawn at the
/// rail's own size.
pub(super) fn render_composer_rail(
    runtime: &crate::AppRuntime,
    member: &crate::runtime::LaneMember,
) -> Html {
    let seated = member.occupant.is_some();
    let activities = lane_activities(runtime, member.lane);
    let off = lane_off(runtime, member.lane, seated);
    html! {
        <span class="composer-trace-rail" data-lane-seated={seated.to_string()} aria-hidden="true">
            <ActivityTrace id={format!("composer-{}", member.lane)} {activities} {off} />
        </span>
    }
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

/// Redraws on new activity and, while anything is shown, once per trace step,
/// so the window keeps sliding with time and old activity leaves without a new
/// frame; the pane's size is measured after each draw.
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
    let now = now_millis();
    let placed = trace(&props.activities, now, samples(width));
    let spans = drawn_spans(&props.off, now, samples(width));
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
    let whole = matches!(spans.as_slice(), [only] if only.from <= 0.0 && only.to >= 1.0);
    let clip = (!whole).then(|| format!("trace-{}-on", props.id));
    html! {
        <span ref={pane} class="activity-trace" aria-hidden="true">
            if drawn {
                <svg viewBox={format!("0 0 {width:.2} {height:.2}")}>
                    <defs>
                        // A literal tag is normalised to lowercase, and SVG knows
                        // no `lineargradient`; the dynamic form keeps its case.
                        {for placed.curves.iter().map(|curve| html! {
                            <@{"linearGradient"}
                                id={gradient(curve.kind.key())}
                                data-trace-kind={curve.kind.key()}
                                x1="0" y1="0" x2="0" y2="1"
                            >
                                <stop offset="0" stop-opacity=".42" />
                                <stop offset="1" stop-opacity="0" />
                            </@>
                        })}
                        if let Some(clip) = &clip {
                            // `html!` lowercases a literal tag, which SVG does not
                            // recognise as a clip; a dynamic tag keeps its case.
                            <@{"clipPath"} id={clip.clone()}>
                                {for on_rects(&spans, width, height).into_iter().map(|(x, y, w, h)| html! {
                                    <rect x={format!("{x:.2}")} y={format!("{y:.2}")} width={format!("{w:.2}")} height={format!("{h:.2}")} />
                                })}
                            </@>
                        }
                    </defs>
                    <g clip-path={clip.as_ref().map(|clip| format!("url(#{clip})"))}>
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
                    </g>
                    {for placed.markers.iter().map(|marker| render_marker(*marker, width, height))}
                </svg>
            }
        </span>
    }
}

/// Room the clip keeps beyond the pane's outer edges, so a line's stroke and
/// glow there are not shaved; only a gap's own edges cut the curves.
const CLIP_BLEED: f64 = 3.0;

/// The drawn spans as rectangles in pane pixels: time maps to x as the curve
/// points map it, and the span reaching now keeps the now-dot's room.
fn on_rects(spans: &[TraceSpan], width: f64, height: f64) -> Vec<(f64, f64, f64, f64)> {
    let span = width - DOT_ROOM;
    spans
        .iter()
        .map(|&TraceSpan { from, to }| {
            let left = if from <= 0.0 {
                -CLIP_BLEED
            } else {
                from * span
            };
            let right = if to >= 1.0 {
                width + CLIP_BLEED
            } else {
                to * span
            };
            (left, -CLIP_BLEED, right - left, height + 2.0 * CLIP_BLEED)
        })
        .collect()
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
    // A one-pixel rule covering the device pixels from its snapped x: whole
    // pixels on a 2x display, and on 1x whenever x snapped to a whole pixel.
    let rule = x + 0.5;
    match marker.kind {
        TraceMarkerKind::Direction => html! {
            <circle data-trace-marker={key} cx={format!("{rule:.2}")} cy="2" r="1.8" />
        },
        TraceMarkerKind::Failure => html! {
            <rect data-trace-marker={key} x={format!("{x:.2}")} y="0" width="1" height={format!("{height:.2}")} />
        },
        // A fresh session ticks forward from its rule; a sign-out ticks back.
        TraceMarkerKind::Session | TraceMarkerKind::SignOut => {
            let tick = if marker.kind == TraceMarkerKind::Session {
                2.5
            } else {
                -2.5
            };
            html! {
                <path
                    data-trace-marker={key}
                    d={format!("M{rule:.2},{height:.2} V0.5 H{:.2}", rule + tick)}
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
