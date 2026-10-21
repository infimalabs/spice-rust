//! A team's or working group's lights panel drawn as one weave: a thread per
//! row in the row's accent, as thick as its channel's work rate, ending where
//! it was last heard, with a dotted silence trail in its status colour running
//! from there to now. Threads bow together where lanes in two rows interacted,
//! and operator direction drops onto its row's thread from the nearer edge.
//! Everything is SVG geometry in the pane's measured CSS pixels, which the
//! app's `style-src 'self'` policy admits where inline styles are refused;
//! colours and glow stay in the stylesheet.

use std::fmt::Write as _;
use std::rc::Rc;

use gloo_timers::callback::Timeout;
use wire::{Actor, CanonicalUuid, QuoteAuthor};
use yew::prelude::*;

use super::super::clock::now_millis;
use crate::watchband::{
    Channel, Edge, Interaction, TRACE_STEP_MILLIS, Thread, TraceActivity, TraceOff, TraceSpan,
    direction_edge, elapsed_label, last_heard, weave,
};

/// One row of a weave: a team lane, or a Design and the lanes working it.
#[derive(Clone, PartialEq)]
pub(crate) struct WeaveRow {
    /// Lane or Design identity, unique on the page.
    pub(crate) key: AttrValue,
    /// Who the row is and its status or standing.
    pub(crate) label: AttrValue,
    pub(crate) accent: usize,
    /// Status or standing key, which colours the row's trail and rail.
    pub(crate) state: &'static str,
    /// Whether the row wears its accent; an empty team seat drains to gray.
    pub(crate) seated: bool,
    pub(crate) lanes: Rc<[CanonicalUuid]>,
    pub(crate) activities: Rc<[TraceActivity]>,
    pub(crate) off: Rc<[TraceOff]>,
}

#[derive(Properties, PartialEq)]
pub(crate) struct WeaveProps {
    /// Rows from top to bottom.
    pub(crate) rows: Rc<[WeaveRow]>,
    pub(crate) interactions: Rc<[Interaction]>,
}

/// Every lane-to-lane packet the browser retains: each packet a sender's card
/// presents, and each packet a recipient's card quotes. One packet seen from
/// both sides is one knot; the weave merges them.
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

/// Space the status rail needs inside the pane's right edge, in CSS pixels.
const RAIL: f64 = 1.5;

/// Thread thickness while heard but idle, in CSS pixels.
const HAIRLINE: f64 = 0.6;

/// Share of its row a thread fills at the full rate.
const FILL: f64 = 0.85;

/// Room a clip keeps beyond the pane's outer edges, so a thread's glow there
/// is not shaved; only a gap's or silence's own edges cut it.
const CLIP_BLEED: f64 = 3.0;

/// A row last heard less than this long ago was heard just now.
const JUST_NOW_MILLIS: i64 = 1_000;

/// Samples per thread: about one per two pixels, so a knot thirty seconds
/// either side still bends smoothly in a narrow panel.
fn samples(width: f64) -> usize {
    // Truncation is the intent: whole two-pixel columns.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let columns = (width / 2.0).floor() as usize;
    columns.clamp(9, 121)
}

/// Redraws on new activity and once per trace step, so the window keeps
/// sliding, silence trails lengthen, and labels age without a new frame; the
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
        .rows
        .iter()
        .map(|row| Channel {
            lanes: &row.lanes,
            activities: &row.activities,
            off: &row.off,
        })
        .collect::<Vec<_>>();
    let woven = weave(&channels, &props.interactions, now, samples(width));
    let pane_size = Pane {
        width,
        height,
        rows: props.rows.len(),
    };
    let drawn = width > RAIL && height > RAIL;
    html! {
        <span ref={pane} class="team-weave">
            if drawn {
                <svg viewBox={format!("0 0 {width:.2} {height:.2}")} aria-hidden="true">
                    {for props.rows.iter().zip(&woven.threads).enumerate().map(|(index, (row, thread))| {
                        render_row(pane_size, index, row, thread)
                    })}
                </svg>
            }
            {for props.rows.iter().map(|row| html! {
                <span class="visually-hidden" data-weave-label={row.key.clone()}>{label(row, now)}</span>
            })}
        </span>
    }
}

/// A row's accessible name with how long ago it was last heard. Under a
/// second, or stamped by a clock running ahead, it was heard just now.
fn label(row: &WeaveRow, now: i64) -> String {
    match last_heard(&row.activities) {
        None => format!("{}, not heard", row.label),
        Some(heard) if now - heard < JUST_NOW_MILLIS => format!("{}, heard just now", row.label),
        Some(heard) => format!("{}, heard {} ago", row.label, elapsed_label(now - heard)),
    }
}

#[derive(Clone, Copy)]
struct Pane {
    width: f64,
    height: f64,
    rows: usize,
}

impl Pane {
    /// Pixels across for a fraction of the window, short of the rail.
    fn x(self, at: f64) -> f64 {
        at * (self.width - RAIL)
    }

    #[allow(clippy::cast_precision_loss)]
    fn row_height(self) -> f64 {
        self.height / self.rows as f64
    }
}

/// A thread's centre and thickness in pixels at a fraction of the window,
/// interpolated between its samples.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn at(pane: Pane, thread: &Thread, fraction: f64) -> (f64, f64) {
    let last = thread.centers.len() - 1;
    let position = fraction.clamp(0.0, 1.0) * last as f64;
    let index = (position.floor() as usize).min(last.saturating_sub(1));
    let blend = position - index as f64;
    let mix =
        |values: &[f64]| values[index] + (values[(index + 1).min(last)] - values[index]) * blend;
    (
        mix(&thread.centers) * pane.height,
        thickness(pane, mix(&thread.levels)),
    )
}

fn thickness(pane: Pane, level: f64) -> f64 {
    HAIRLINE + level * pane.row_height() * FILL
}

/// Clip rectangles for the thread: its drawn spans, ended where it was last
/// heard. A row not heard in the window draws no thread.
fn thread_rects(pane: Pane, spans: &[TraceSpan], heard: f64) -> Vec<(f64, f64)> {
    spans
        .iter()
        .filter_map(|&TraceSpan { from, to }| {
            let left = if from <= 0.0 {
                -CLIP_BLEED
            } else {
                pane.x(from)
            };
            let right = if to >= 1.0 && heard >= 1.0 {
                pane.width + CLIP_BLEED
            } else {
                pane.x(to.min(heard))
            };
            (right > left).then_some((left, right))
        })
        .collect()
}

#[allow(clippy::cast_precision_loss)]
fn render_row(pane: Pane, index: usize, row: &WeaveRow, thread: &Thread) -> Html {
    let last = thread.centers.len() - 1;
    let points = (0..=last)
        .map(|sample| {
            let x = pane.x(sample as f64 / last as f64);
            let center = thread.centers[sample] * pane.height;
            (x, center, thickness(pane, thread.levels[sample]))
        })
        .collect::<Vec<_>>();
    let thread_shape = thread.heard.map(|heard| {
        let mut d = String::new();
        for (sample, (x, center, thick)) in points.iter().enumerate() {
            let verb = if sample == 0 { 'M' } else { 'L' };
            let _ = write!(d, "{verb}{x:.2},{:.2} ", center - thick / 2.0);
        }
        for (x, center, thick) in points.iter().rev() {
            let _ = write!(d, "L{x:.2},{:.2} ", center + thick / 2.0);
        }
        d.push('Z');
        let clip = format!("weave-{}-heard", row.key);
        let (heard_x, heard_center) = (pane.x(heard), at(pane, thread, heard).0);
        html! {
            <>
                <defs>
                    <@{"clipPath"} id={clip.clone()}>
                        {for thread_rects(pane, &thread.spans, heard).into_iter().map(|(left, right)| html! {
                            <rect
                                x={format!("{left:.2}")}
                                y={format!("{:.2}", -CLIP_BLEED)}
                                width={format!("{:.2}", right - left)}
                                height={format!("{:.2}", pane.height + 2.0 * CLIP_BLEED)}
                            />
                        })}
                    </@>
                </defs>
                <path class="weave-thread" d={d} clip-path={format!("url(#{clip})")} />
                <circle class="weave-heard" cx={format!("{heard_x:.2}")} cy={format!("{heard_center:.2}")} r="1" />
            </>
        }
    });
    // Silence runs from the last-heard instant, or the window's start, to now.
    let from = thread.heard.unwrap_or(0.0);
    let trail = (from < 1.0).then(|| {
        let (start_center, _) = at(pane, thread, from);
        let mut d = format!("M{:.2},{start_center:.2}", pane.x(from));
        for (sample, (x, center, _)) in points.iter().enumerate() {
            if sample as f64 / last as f64 > from {
                let _ = write!(d, " L{x:.2},{center:.2}");
            }
        }
        let _ = write!(d, " L{:.2},{:.2}", pane.width, points[last].1);
        html! { <path class="weave-trail" d={d} /> }
    });
    let (_, now_center, now_thick) = points[last];
    let rail_height = if thread.heard.is_some() {
        now_thick.max(2.0)
    } else {
        2.0
    };
    let edge = direction_edge(index, pane.rows);
    let directions = thread.directions.iter().map(|&x| {
        let (center, thick) = at(pane, thread, x);
        let (from, to) = match edge {
            Edge::Top => (0.0, center - thick / 2.0),
            Edge::Bottom => (pane.height, center + thick / 2.0),
        };
        let x = pane.x(x);
        html! {
            <g class="weave-direction" data-weave-edge={edge.key()}>
                <line x1={format!("{x:.2}")} y1={format!("{from:.2}")} x2={format!("{x:.2}")} y2={format!("{to:.2}")} />
                <circle cx={format!("{x:.2}")} cy={format!("{to:.2}")} r=".9" />
            </g>
        }
    });
    html! {
        <g
            key={row.key.to_string()}
            class={classes!("weave-row", row.state)}
            data-weave-row={row.key.clone()}
            data-accent={row.accent.to_string()}
            data-lane-seated={row.seated.to_string()}
        >
            <title>{row.label.clone()}</title>
            {trail}
            {thread_shape}
            <rect
                class="weave-rail"
                x={format!("{:.2}", pane.width - 1.0)}
                y={format!("{:.2}", now_center - rail_height / 2.0)}
                width="1"
                height={format!("{rail_height:.2}")}
            />
            {for directions}
        </g>
    }
}
