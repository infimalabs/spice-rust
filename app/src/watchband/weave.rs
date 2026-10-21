//! A team or working group drawn as one weave: one thread per channel, each in
//! its own row of the panel, over the trace's shared ten-minute window.
//!
//! A channel is a set of lanes. A team row is its one lane and a working-group
//! row is the distinct lanes working its Design, so both panels draw through
//! this one model. A thread's thickness is its channel's summed reasoning,
//! tool, and prose rate from [`trace`], on that calculation's fixed scale. The
//! thread ends where its channel was last heard; the browser runs a silence
//! trail from there to now. Where lanes in two different rows interacted, the
//! two threads bow toward each other and meet at that instant, so collaboration
//! reads as a knot. Positions are fractions of the window across and of the
//! panel's height down; the browser owns every pixel.

use wire::{AgentActivityKind, CanonicalUuid};

use super::trace::{
    TRACE_WINDOW_MILLIS, TraceActivity, TraceMarkerKind, TraceOff, TraceSpan, drawn_spans, grid,
    trace,
};

/// Half-width of a knot as a fraction of the window: thirty seconds either
/// side, fixed in wall time so a knot keeps its width at every panel size. Two
/// interactions between the same rows closer than this are one knot, so a
/// packet seen by both its sender and its recipient knots once.
pub const KNOT_HALF_WINDOW: f64 = 0.05;

/// One row's input: the lanes it gathers and their retained record.
#[derive(Clone, Copy, Debug)]
pub struct Channel<'a> {
    pub lanes: &'a [CanonicalUuid],
    pub activities: &'a [TraceActivity],
    /// Intervals the row was off; a working-group row has none.
    pub off: &'a [TraceOff],
}

/// One lane addressing another at an instant, in epoch milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interaction {
    pub from: CanonicalUuid,
    pub to: CanonicalUuid,
    pub millis: i64,
}

/// One row's thread at one instant.
#[derive(Clone, Debug, PartialEq)]
pub struct Thread {
    /// Summed work rate in `0..=1` at each sample, the window's start first.
    pub levels: Vec<f64>,
    /// Vertical centre at each sample as a fraction of the panel's height.
    pub centers: Vec<f64>,
    /// Fraction of the window at which the row was last heard, if within it.
    pub heard: Option<f64>,
    /// Spans through which the thread draws, as [`drawn_spans`] computes them.
    pub spans: Vec<TraceSpan>,
    /// Operator directions, as fractions of the window, in time order.
    pub directions: Vec<f64>,
}

/// Two rows' threads meeting at one instant; `rows.0 < rows.1`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Knot {
    pub rows: (usize, usize),
    pub x: f64,
}

/// The panel edge an operator direction drops from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
}

impl Edge {
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Weave {
    pub threads: Vec<Thread>,
    pub knots: Vec<Knot>,
}

/// The edge nearer a row, so a direction takes the shorter path: rows in the
/// upper half drop from the top, rows in the lower half rise from the bottom,
/// and the middle row of an odd-sized panel drops from the top.
#[must_use]
pub const fn direction_edge(row: usize, rows: usize) -> Edge {
    if 2 * row < rows {
        Edge::Top
    } else {
        Edge::Bottom
    }
}

/// A row's resting centre: the rows divide the panel's height equally.
#[allow(clippy::cast_precision_loss)]
fn home(row: usize, rows: usize) -> f64 {
    (row as f64 + 0.5) / rows as f64
}

/// The newest retained instant at which a row was heard: anything its lanes
/// did, which excludes operator direction reaching them.
#[must_use]
pub fn last_heard(activities: &[TraceActivity]) -> Option<i64> {
    activities
        .iter()
        .filter(|activity| activity.kind != AgentActivityKind::Direction)
        .map(|activity| activity.millis)
        .max()
}

/// Knots among `channels` from `interactions` in the window ending at `now`,
/// in time order. An interaction from a lane in one row to a lane in another
/// knots that pair; one inside a single row, or touching a lane no row holds,
/// knots nothing. One stamped after `now`, by a clock running ahead of this
/// one, knots at now, as [`trace`] places such activity.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn knots(channels: &[Channel<'_>], interactions: &[Interaction], now: i64) -> Vec<Knot> {
    let since = now - TRACE_WINDOW_MILLIS;
    let mut placed = interactions
        .iter()
        .filter(|interaction| interaction.millis > since)
        .flat_map(|interaction| {
            let x = ((interaction.millis - since) as f64 / TRACE_WINDOW_MILLIS as f64).min(1.0);
            let rows_of = |lane| {
                channels
                    .iter()
                    .enumerate()
                    .filter(move |(_, channel)| channel.lanes.contains(&lane))
                    .map(|(row, _)| row)
            };
            rows_of(interaction.from)
                .flat_map(move |from| rows_of(interaction.to).map(move |to| (from, to)))
                .filter(|(from, to)| from != to)
                .map(move |(from, to)| Knot {
                    rows: (from.min(to), from.max(to)),
                    x,
                })
        })
        .collect::<Vec<_>>();
    placed.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.rows.cmp(&b.rows)));
    let mut kept: Vec<Knot> = Vec::with_capacity(placed.len());
    for knot in placed {
        let merged = kept
            .iter()
            .rev()
            .take_while(|earlier| knot.x - earlier.x < KNOT_HALF_WINDOW)
            .any(|earlier| earlier.rows == knot.rows);
        if !merged {
            kept.push(knot);
        }
    }
    kept
}

/// How strongly a knot at `x` pulls at `at`: one at the knot, easing to zero
/// a half-width away.
fn pull(at: f64, x: f64) -> f64 {
    let distance = (at - x).abs() / KNOT_HALF_WINDOW;
    if distance >= 1.0 {
        0.0
    } else {
        0.5 * (1.0 + (std::f64::consts::PI * distance).cos())
    }
}

/// Weave `channels`, one row each in order, through the window ending at
/// `now`, sampling every thread at no fewer than `samples` instants.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn weave(
    channels: &[Channel<'_>],
    interactions: &[Interaction],
    now: i64,
    samples: usize,
) -> Weave {
    let rows = channels.len();
    let since = now - TRACE_WINDOW_MILLIS;
    let knots = knots(channels, interactions, now);
    let threads = channels
        .iter()
        .enumerate()
        .map(|(row, channel)| {
            let traced = trace(channel.activities, now, samples);
            let count = grid(samples);
            let mut levels = vec![0.0_f64; count];
            for curve in &traced.curves {
                for (level, add) in levels.iter_mut().zip(&curve.levels) {
                    *level = (*level + add).min(1.0);
                }
            }
            let rest = home(row, rows);
            let centers = (0..count)
                .map(|index| {
                    let at = index as f64 / (count - 1) as f64;
                    let (offset, weight) = knots
                        .iter()
                        .filter_map(|knot| {
                            let other = match knot.rows {
                                (upper, lower) if upper == row => lower,
                                (upper, lower) if lower == row => upper,
                                _ => return None,
                            };
                            let strength = pull(at, knot.x);
                            let meet = f64::midpoint(rest, home(other, rows));
                            Some((strength * (meet - rest), strength))
                        })
                        .fold((0.0, 0.0), |(offset, weight), (o, w)| {
                            (offset + o, weight + w)
                        });
                    rest + if weight > 1.0 {
                        offset / weight
                    } else {
                        offset
                    }
                })
                .collect();
            let heard = last_heard(channel.activities)
                .filter(|millis| *millis > since)
                .map(|millis| ((millis - since) as f64 / TRACE_WINDOW_MILLIS as f64).min(1.0));
            let directions = traced
                .markers
                .iter()
                .filter(|marker| marker.kind == TraceMarkerKind::Direction)
                .map(|marker| marker.x)
                .collect();
            Thread {
                levels,
                centers,
                heard,
                spans: drawn_spans(channel.off, now, count),
                directions,
            }
        })
        .collect();
    Weave { threads, knots }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;
    const MINUTE: i64 = 60_000;

    fn lane(n: u8) -> CanonicalUuid {
        format!("00000000-0000-4000-8000-{n:012}")
            .parse()
            .expect("fixture lane")
    }

    fn prose(minutes_ago: i64) -> TraceActivity {
        TraceActivity {
            kind: AgentActivityKind::Prose,
            millis: NOW - minutes_ago * MINUTE,
            bytes: 800,
        }
    }

    fn channel<'a>(lanes: &'a [CanonicalUuid], activities: &'a [TraceActivity]) -> Channel<'a> {
        Channel {
            lanes,
            activities,
            off: &[],
        }
    }

    fn told(from: CanonicalUuid, to: CanonicalUuid, millis: i64) -> Interaction {
        Interaction { from, to, millis }
    }

    #[test]
    fn a_thread_ends_where_its_row_was_last_heard_and_direction_is_not_hearing() {
        let lanes = [lane(1)];
        let direction = TraceActivity {
            kind: AgentActivityKind::Direction,
            millis: NOW - MINUTE,
            bytes: 0,
        };
        let heard = [prose(5), prose(3), direction];
        let woven = weave(&[channel(&lanes, &heard)], &[], NOW, 9);
        let thread = &woven.threads[0];
        assert!((thread.heard.expect("heard in window") - 0.7).abs() < 1e-9);
        assert_eq!(thread.directions.len(), 1);
        assert!((thread.directions[0] - 0.9).abs() < 1e-9);

        let stale = [prose(11)];
        let silent = weave(&[channel(&lanes, &stale)], &[], NOW, 9);
        assert_eq!(silent.threads[0].heard, None);
        assert_eq!(last_heard(&stale), Some(NOW - 11 * MINUTE));
    }

    #[test]
    fn rows_divide_the_panel_and_a_knot_meets_two_threads_at_its_instant() {
        let (a, b, c) = (lane(1), lane(2), lane(3));
        let (ra, rb, rc) = ([a], [b], [c]);
        let channels = [channel(&ra, &[]), channel(&rb, &[]), channel(&rc, &[])];
        let quiet = weave(&channels, &[], NOW, 9);
        for (row, thread) in quiet.threads.iter().enumerate() {
            let rest = [1.0 / 6.0, 0.5, 5.0 / 6.0][row];
            assert!(
                thread
                    .centers
                    .iter()
                    .all(|center| (center - rest).abs() < 1e-9)
            );
        }

        // Five minutes ago is the midpoint sample of every grid.
        let woven = weave(&channels, &[told(a, c, NOW - 5 * MINUTE)], NOW, 9);
        assert_eq!(
            woven.knots,
            [Knot {
                rows: (0, 2),
                x: 0.5
            }]
        );
        let mid = woven.threads[0].centers.len() / 2;
        assert!((woven.threads[0].centers[mid] - 0.5).abs() < 1e-9);
        assert!((woven.threads[2].centers[mid] - 0.5).abs() < 1e-9);
        assert!((woven.threads[1].centers[mid] - 0.5).abs() < 1e-9);
        // Far from the knot, every thread rests in its own row.
        for (row, thread) in woven.threads.iter().enumerate() {
            let rest = home(row, 3);
            assert!((thread.centers[0] - rest).abs() < 1e-9);
            assert!((thread.centers[thread.centers.len() - 1] - rest).abs() < 1e-9);
        }
    }

    #[test]
    fn one_interaction_knots_each_row_pair_once_whichever_side_retained_it() {
        let (a, b, c, outside) = (lane(1), lane(2), lane(3), lane(9));
        let (ra, rb) = ([a], [b]);
        let teams = [channel(&ra, &[]), channel(&rb, &[])];
        let sent = NOW - 4 * MINUTE;
        // The sender's card and the recipient's directive both carry the packet.
        let seen_twice = [told(a, b, sent), told(a, b, sent + 4_000)];
        assert_eq!(knots(&teams, &seen_twice, NOW).len(), 1);
        // A reply a minute later is a second knot.
        let replied = [told(a, b, sent), told(b, a, sent + MINUTE)];
        assert_eq!(knots(&teams, &replied, NOW).len(), 2);
        // Within one row, to a lane outside the panel, or outside the window: none.
        let unrelated = [
            told(a, a, sent),
            told(a, outside, sent),
            told(a, b, NOW - 11 * MINUTE),
        ];
        assert!(knots(&teams, &unrelated, NOW).is_empty());
        // A packet stamped by a clock running ahead knots at now.
        assert_eq!(
            knots(&teams, &[told(a, b, NOW + 2_000)], NOW),
            [Knot {
                rows: (0, 1),
                x: 1.0
            }]
        );

        // Design rows share lanes: a knot joins rows holding either end, never
        // a row to itself.
        let (design_one, design_two) = ([a, b], [b, c]);
        let designs = [channel(&design_one, &[]), channel(&design_two, &[])];
        assert_eq!(
            knots(&designs, &[told(a, c, sent)], NOW),
            [Knot {
                rows: (0, 1),
                x: 0.6
            }]
        );
        assert_eq!(knots(&designs, &[told(a, b, sent)], NOW).len(), 1);
    }

    #[test]
    fn a_direction_takes_the_shorter_path_with_the_middle_row_from_the_top() {
        use Edge::{Bottom, Top};
        let expected: [&[Edge]; 6] = [
            &[Top],
            &[Top, Bottom],
            &[Top, Top, Bottom],
            &[Top, Top, Bottom, Bottom],
            &[Top, Top, Top, Bottom, Bottom],
            &[Top, Top, Top, Bottom, Bottom, Bottom],
        ];
        for (size, edges) in (1..=6).zip(expected) {
            let actual = (0..size)
                .map(|row| direction_edge(row, size))
                .collect::<Vec<_>>();
            assert_eq!(actual, edges, "{size} rows");
        }
    }
}
