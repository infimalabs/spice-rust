//! A team's or working group's lights panel drawn as log-time kind violins:
//! one fixed-width column per channel, painted from the right edge in channel
//! order so each column stands over its composer, with time flowing down from
//! now at the top. Each channel's work is a symmetric violin split side by side into
//! reasoning, tool, and prose tones of its accent, closing with a rounded
//! taper where it truly stopped. A status cap sits in the column's top row,
//! a dotted silence trail in the same status colour runs from it down to the
//! last-heard dot, MSG packets are accent rungs with an arrowhead at the
//! recipient, and operator directions are ink notches on the receiving
//! column's left edge. Everything is SVG geometry in the pane's measured CSS
//! pixels, which the app's `style-src 'self'` policy admits where inline
//! styles are refused; colours and glow stay in the stylesheet.

use std::fmt::Write as _;
use std::rc::Rc;

use gloo_timers::callback::Timeout;
use wire::{Actor, CanonicalUuid, QuoteAuthor};
use yew::prelude::*;

use super::super::clock::now_millis;
use crate::watchband::{
    Channel, Column, Interaction, Packet, Run, TRACE_STEP_MILLIS, TraceActivity, TraceOff, depth,
    elapsed_label, last_heard, weave,
};

/// One column of a weave: a team lane, or a Design and the lanes working it.
#[derive(Clone, PartialEq)]
pub(crate) struct WeaveChannel {
    /// Lane or Design identity, unique on the page.
    pub(crate) key: AttrValue,
    /// Who the channel is and its status or standing.
    pub(crate) label: AttrValue,
    pub(crate) accent: usize,
    /// Status or standing key, which colours the column's cap and trail.
    pub(crate) state: &'static str,
    /// Whether the column wears its accent; an empty team seat drains to gray.
    pub(crate) seated: bool,
    pub(crate) lanes: Rc<[CanonicalUuid]>,
    pub(crate) activities: Rc<[TraceActivity]>,
    pub(crate) off: Rc<[TraceOff]>,
}

#[derive(Properties, PartialEq)]
pub(crate) struct WeaveProps {
    /// Channels from left to right.
    pub(crate) channels: Rc<[WeaveChannel]>,
    pub(crate) interactions: Rc<[Interaction]>,
}

/// Every lane-to-lane packet the browser retains: each packet a sender's card
/// presents, and each packet a recipient's card quotes. One packet seen from
/// both sides draws once; the weave merges them.
pub(crate) fn lane_interactions(runtime: &crate::AppRuntime) -> Rc<[Interaction]> {
    let mut interactions = Vec::new();
    for message in runtime.messages() {
        let millis = message.display_millis();
        for send in &message.sends {
            interactions.extend(send.recipients.iter().map(|recipient| Interaction {
                from: send.from,
                to: recipient.lane,
                millis,
            }));
        }
        for directive in &message.directive_contexts {
            let from = match &directive.attribution.author {
                QuoteAuthor::Directive {
                    actor: Actor::Lane(lane),
                } if directive.attribution.packet => *lane,
                _ => continue,
            };
            let millis = directive.authored.get().as_millisecond();
            interactions.extend(directive.attribution.recipients.iter().map(|recipient| {
                Interaction {
                    from,
                    to: recipient.lane,
                    millis,
                }
            }));
        }
    }
    interactions.into()
}

/// Space kept inside the pane's left and right edges, in CSS pixels.
const PAD: f64 = 1.5;

/// Gutter between two columns.
const GUTTER: f64 = 1.4;

/// The status cap's row at the top of each column: flush with the pane top,
/// ending above the silence trail's start.
const CAP_TOP: f64 = 0.0;
const CAP_HEIGHT: f64 = 2.0;

/// Share of its column the cap spans, and the narrowest cap drawn.
const CAP_SHARE: f64 = 0.7;
const CAP_MIN: f64 = 2.0;

/// Where now sits: the top of the time axis, under the cap row.
const AXIS_TOP: f64 = 2.6;

/// Room the axis keeps above the pane's bottom edge for the floor.
const FLOOR_INSET: f64 = 0.8;

/// How far above the axis top the silence trail starts, so it meets the cap.
const TRAIL_LEAD: f64 = 0.3;

/// A trail shorter than this is hidden under its own dot.
const TRAIL_MIN: f64 = 1.0;

/// Share of its column a violin fills at the full rate.
const FILL: f64 = 0.92;

/// Violin width while heard but idle.
const HAIRLINE: f64 = 0.6;

/// Distance over which a violin rounds off where its channel truly stopped.
const TAPER: f64 = 0.9;

const HEARD_RADIUS: f64 = 0.95;

/// A rung's arrowhead: how far back from the recipient's centre it opens and
/// its half-height.
const RUNG_HEAD: f64 = 1.2;
const RUNG_HALF: f64 = 0.8;

/// A direction notch: how far it reaches into the column from its left edge,
/// how far it backs out past that edge, and its half-height.
const NOTCH_REACH: f64 = 1.25;
const NOTCH_BACK: f64 = 0.2;
const NOTCH_HALF: f64 = 0.95;

/// The dotted rule marking one minute ago.
const MINUTE_MILLIS: f64 = 60_000.0;

/// A channel last heard less than this long ago was heard just now.
const JUST_NOW_MILLIS: i64 = 1_000;

/// Samples per violin: about three per pixel of axis, so the sharp top of a
/// fresh burst still curves rather than kinks.
fn samples(height: f64) -> usize {
    // Truncation is the intent: whole samples.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let count = ((height - AXIS_TOP - FLOOR_INSET) * 3.0).floor().max(0.0) as usize;
    count.clamp(25, 161)
}

/// Redraws on new activity and once per trace step, so time keeps flowing
/// down, silence trails lengthen, and labels age without a new frame; the
/// pane's size is measured after each draw.
#[function_component(TeamWeave)]
pub(crate) fn team_weave(props: &WeaveProps) -> Html {
    let tick = use_state(|| 0_u64);
    let pane = use_node_ref();
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
    {
        let tick = tick.clone();
        use_effect_with(*tick, move |&generation| {
            let timeout = Timeout::new(TRACE_STEP_MILLIS, move || {
                tick.set(generation.wrapping_add(1));
            });
            move || drop(timeout)
        });
    }
    let (width, height) = *size;
    let now = now_millis();
    let channels = props
        .channels
        .iter()
        .map(|channel| Channel {
            lanes: &channel.lanes,
            activities: &channel.activities,
            off: &channel.off,
        })
        .collect::<Vec<_>>();
    let woven = weave(&channels, &props.interactions, now, samples(height));
    let pane_size = Pane {
        width,
        height,
        columns: props.channels.len(),
    };
    let drawn = !props.channels.is_empty()
        && pane_size.column_width() >= CAP_MIN
        && pane_size.floor() - AXIS_TOP >= TAPER;
    let minute = pane_size.y(depth(MINUTE_MILLIS));
    html! {
        <span ref={pane} class="team-weave">
            if drawn {
                <svg viewBox={format!("0 0 {width:.2} {height:.2}")} aria-hidden="true">
                    <line class="weave-minute" x1="0" x2={format!("{width:.2}")} y1={format!("{minute:.2}")} y2={format!("{minute:.2}")} />
                    {for props.channels.iter().zip(&woven.columns).enumerate().map(|(index, (channel, column))| {
                        render_column(pane_size, index, channel, column)
                    })}
                    {for woven.packets.iter().map(|packet| render_packet(pane_size, &props.channels, packet))}
                </svg>
            }
            {for props.channels.iter().map(|channel| html! {
                <span class="visually-hidden" data-weave-label={channel.key.clone()}>{label(channel, now)}</span>
            })}
        </span>
    }
}

/// A channel's accessible name with how long ago it was last heard. Under a
/// second, or stamped by a clock running ahead, it was heard just now.
fn label(channel: &WeaveChannel, now: i64) -> String {
    match last_heard(&channel.activities) {
        None => format!("{}, not heard", channel.label),
        Some(heard) if now - heard < JUST_NOW_MILLIS => {
            format!("{}, heard just now", channel.label)
        }
        Some(heard) => format!(
            "{}, heard {} ago",
            channel.label,
            elapsed_label(now - heard)
        ),
    }
}

#[derive(Clone, Copy)]
struct Pane {
    width: f64,
    height: f64,
    columns: usize,
}

impl Pane {
    /// Where the ten-minute floor sits.
    fn floor(self) -> f64 {
        self.height - FLOOR_INSET
    }

    /// Pixels down for a depth of the axis.
    fn y(self, depth: f64) -> f64 {
        AXIS_TOP + (self.floor() - AXIS_TOP) * depth
    }

    fn gutter(self) -> f64 {
        if self.columns > 1 { GUTTER } else { 0.0 }
    }

    #[allow(clippy::cast_precision_loss)]
    fn column_width(self) -> f64 {
        let columns = self.columns.max(1) as f64;
        (self.width - 2.0 * PAD - self.gutter() * (columns - 1.0)) / columns
    }

    /// Columns run from the right edge in channel order, as the composers
    /// below them paint canonical member zero at the control-adjacent edge.
    #[allow(clippy::cast_precision_loss)]
    fn left(self, index: usize) -> f64 {
        let from_right = self.columns.saturating_sub(index + 1) as f64;
        PAD + from_right * (self.column_width() + self.gutter())
    }

    fn center(self, index: usize) -> f64 {
        self.left(index) + self.column_width() / 2.0
    }
}

/// Appends a Catmull-Rom spline through `count` points as cubic segments.
fn curve(d: &mut String, count: usize, at: impl Fn(usize) -> (f64, f64)) {
    for index in 0..count.saturating_sub(1) {
        let before = at(index.saturating_sub(1));
        let from = at(index);
        let to = at(index + 1);
        let after = at((index + 2).min(count - 1));
        let _ = write!(
            d,
            " C{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}",
            from.0 + (to.0 - before.0) / 6.0,
            from.1 + (to.1 - before.1) / 6.0,
            to.0 - (after.0 - from.0) / 6.0,
            to.1 - (after.1 - from.1) / 6.0,
            to.0,
            to.1,
        );
    }
}

/// A closed band between two edges, each `count` points from the top down:
/// down the left edge and back up the right.
fn band(
    count: usize,
    left: impl Fn(usize) -> (f64, f64),
    right: impl Fn(usize) -> (f64, f64),
) -> String {
    let mut d = String::new();
    let (x, y) = left(0);
    let _ = write!(d, "M{x:.2},{y:.2}");
    curve(&mut d, count, &left);
    let (x, y) = right(count - 1);
    let _ = write!(d, " L{x:.2},{y:.2}");
    curve(&mut d, count, |index| right(count - 1 - index));
    d.push('Z');
    d
}

/// One violin: its three kind bands side by side, reasoning, tool, then
/// prose from left to right, each as wide as that kind's share of the level.
/// The violin rounds off over the taper distance at an end where the channel
/// truly stopped and is cut square only by the floor.
fn render_run(pane: Pane, index: usize, run: &Run) -> Html {
    let width = pane.column_width();
    let center = pane.center(index);
    let top = pane.y(run.samples[0].depth);
    let bottom = pane.y(run.samples[run.samples.len() - 1].depth);
    let edges = run
        .samples
        .iter()
        .map(|sample| {
            let y = pane.y(sample.depth);
            let below = if run.reaches_floor {
                f64::INFINITY
            } else {
                bottom - y
            };
            let taper = ((y - top).min(below) / TAPER).min(1.0).sqrt();
            let half = (sample.level * FILL * width / 2.0).max(HAIRLINE / 2.0) * taper;
            let full = 2.0 * half;
            (
                y,
                [
                    center - half,
                    center - half + full * sample.reasoning,
                    center - half + full * (sample.reasoning + sample.tool),
                    center + half,
                ],
            )
        })
        .collect::<Vec<_>>();
    let count = edges.len();
    let edges = edges.as_slice();
    let edge = |side: usize| move |index: usize| (edges[index].1[side], edges[index].0);
    html! {
        <g class="weave-violin">
            <path class="weave-kind-reasoning" d={band(count, edge(0), edge(1))} />
            <path class="weave-kind-tool" d={band(count, edge(1), edge(2))} />
            <path class="weave-kind-prose" d={band(count, edge(2), edge(3))} />
        </g>
    }
}

fn render_column(pane: Pane, index: usize, channel: &WeaveChannel, column: &Column) -> Html {
    let width = pane.column_width();
    let left = pane.left(index);
    let center = pane.center(index);
    let trail_top = AXIS_TOP - TRAIL_LEAD;
    let trail_end = column.heard.map_or(pane.floor(), |heard| pane.y(heard));
    let trail = (trail_end - trail_top >= TRAIL_MIN).then(|| {
        html! {
            <line
                class="weave-trail"
                x1={format!("{center:.2}")}
                y1={format!("{trail_top:.2}")}
                x2={format!("{center:.2}")}
                y2={format!("{trail_end:.2}")}
            />
        }
    });
    let heard = column.heard.map(|heard| {
        html! {
            <circle
                class="weave-heard"
                cx={format!("{center:.2}")}
                cy={format!("{:.2}", pane.y(heard))}
                r={format!("{HEARD_RADIUS:.2}")}
            />
        }
    });
    let cap = (width * CAP_SHARE).max(CAP_MIN).min(width);
    let notches = column.directions.iter().map(|&direction| {
        let y = pane.y(direction);
        let d = format!(
            "M{:.2},{:.2} L{:.2},{y:.2} L{:.2},{:.2}Z",
            left - NOTCH_BACK,
            y - NOTCH_HALF,
            left + NOTCH_REACH,
            left - NOTCH_BACK,
            y + NOTCH_HALF,
        );
        html! { <path class="weave-direction" d={d} /> }
    });
    html! {
        <g
            key={channel.key.to_string()}
            class={classes!("weave-column", channel.state)}
            data-weave-column={channel.key.clone()}
            data-accent={channel.accent.to_string()}
            data-lane-seated={channel.seated.to_string()}
        >
            <title>{channel.label.clone()}</title>
            {for column.runs.iter().map(|run| render_run(pane, index, run))}
            {trail}
            {heard}
            <rect
                class="weave-cap"
                x={format!("{:.2}", center - cap / 2.0)}
                y={format!("{CAP_TOP:.2}")}
                width={format!("{cap:.2}")}
                height={format!("{CAP_HEIGHT:.2}")}
                rx={format!("{:.2}", CAP_HEIGHT / 2.0)}
            />
            {for notches}
        </g>
    }
}

/// A packet: a rung in the sender's accent from its column's centre to the
/// recipient's, with an open arrowhead at the recipient.
fn render_packet(pane: Pane, channels: &[WeaveChannel], packet: &Packet) -> Html {
    let sender = &channels[packet.from];
    let y = pane.y(packet.depth);
    let from = pane.center(packet.from);
    let to = pane.center(packet.to);
    let back = if to > from { RUNG_HEAD } else { -RUNG_HEAD };
    let head = format!(
        "M{:.2},{:.2} L{to:.2},{y:.2} L{:.2},{:.2}",
        to - back,
        y - RUNG_HALF,
        to - back,
        y + RUNG_HALF,
    );
    html! {
        <g
            class="weave-packet"
            data-accent={sender.accent.to_string()}
            data-lane-seated={sender.seated.to_string()}
        >
            <line x1={format!("{from:.2}")} y1={format!("{y:.2}")} x2={format!("{to:.2}")} y2={format!("{y:.2}")} />
            <path d={head} />
        </g>
    }
}
