//! A team or working group drawn as log-time kind violins: one column per
//! channel, time flowing down from now at the top over the trace's shared
//! ten-minute window.
//!
//! A channel is a set of lanes. A team column is its one lane and a
//! working-group column is the distinct lanes working its Design, so both
//! panels draw through this one model. The axis is log-recent: depth down the
//! panel grows with the logarithm of age, so the last minute takes about the
//! top third and ten minutes reaches the floor. Each channel's activity is a
//! violin whose width is its reasoning, tool, and prose rate on the trace's
//! fixed scale, smoothed through a kernel that widens with age, so recent
//! seconds stay sharp and older minutes become texture. A violin runs from
//! where the channel was last heard down to the floor and breaks where the
//! channel was off. Packets between channels and operator directions are
//! placed at their depth. Depths are fractions of the axis and levels are
//! fractions of the scale; the browser owns every pixel.

use wire::{AgentActivityKind, CanonicalUuid};

use super::trace::{TRACE_WINDOW_MILLIS, TraceActivity, TraceOff, activity_weight};

/// Age the log-recent axis is scaled by: depth is `ln(1 + age / tau)`
/// normalised so the whole window reaches the floor. Forty seconds puts one
/// minute about a third of the way down.
pub const AXIS_TAU_MILLIS: f64 = 40_000.0;

/// Smoothing half-width at now, in milliseconds.
const KERNEL_FLOOR_MILLIS: f64 = 12_000.0;

/// Growth of the smoothing half-width per millisecond of age, so a sample ten
/// minutes old is smoothed over about two minutes either side.
const KERNEL_SLOPE: f64 = 0.2;

/// Weighted activities per minute at which a violin fills its column, shared
/// by every column and panel. The kernel near now is seconds wide, so it
/// reports bursts the composer rail's two-minute average flattens; this scale
/// lets a steadily busy lane fill its column without saturating on every
/// burst.
const FULL_RATE_PER_MINUTE: f64 = 12.0;

/// Two packets from one channel to another closer than this are one packet,
/// so a packet seen by both its sender's card and its recipient's quote draws
/// once.
pub const PACKET_MERGE_MILLIS: i64 = 10_000;

/// One column's input: the lanes it gathers and their retained record.
#[derive(Clone, Copy, Debug)]
pub struct Channel<'a> {
    pub lanes: &'a [CanonicalUuid],
    pub activities: &'a [TraceActivity],
    /// Intervals the channel was off; a working-group column has none.
    pub off: &'a [TraceOff],
}

/// One lane addressing another at an instant, in epoch milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interaction {
    pub from: CanonicalUuid,
    pub to: CanonicalUuid,
    pub millis: i64,
}

/// A violin's width and kind split at one depth.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    /// Fraction of the axis, now at 0 and the window's floor at 1.
    pub depth: f64,
    /// Summed work rate in `0..=1`.
    pub level: f64,
    /// Reasoning's share of the level in `0..=1`.
    pub reasoning: f64,
    /// Tool work's share of the level in `0..=1`; prose is the remainder.
    pub tool: f64,
}

/// One unbroken stretch of a channel's violin, shallowest sample first.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub samples: Vec<Sample>,
    /// Whether the window's floor cuts the run, rather than the channel
    /// having stopped there.
    pub reaches_floor: bool,
}

/// One channel's column.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    /// Live stretches from the top down; empty when the channel was not heard
    /// in the window.
    pub runs: Vec<Run>,
    /// Depth at which the channel was last heard, if within the window.
    pub heard: Option<f64>,
    /// Depths of operator directions reaching the channel, newest first.
    pub directions: Vec<f64>,
}

/// One MSG packet from a lane in one column to a lane in another.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Packet {
    pub from: usize,
    pub to: usize,
    pub depth: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Weave {
    pub columns: Vec<Column>,
    /// Newest first.
    pub packets: Vec<Packet>,
}

#[allow(clippy::cast_precision_loss)]
const WINDOW: f64 = TRACE_WINDOW_MILLIS as f64;

/// Rates are per minute.
const MINUTE_MILLIS: f64 = 60_000.0;

fn axis_span() -> f64 {
    (1.0 + WINDOW / AXIS_TAU_MILLIS).ln()
}

/// Depth down the axis of an instant `age_millis` ago: now at 0, the window's
/// floor at 1. An instant stamped ahead of now sits at the top, and one older
/// than the window sits on the floor.
#[must_use]
pub fn depth(age_millis: f64) -> f64 {
    (1.0 + age_millis.clamp(0.0, WINDOW) / AXIS_TAU_MILLIS).ln() / axis_span()
}

/// The age, in milliseconds, at a depth down the axis; the inverse of
/// [`depth`] within the window.
#[must_use]
pub fn age(depth: f64) -> f64 {
    AXIS_TAU_MILLIS * (depth.clamp(0.0, 1.0) * axis_span()).exp_m1()
}

/// The newest retained instant at which a channel was heard: anything its
/// lanes did, which excludes operator direction reaching them.
#[must_use]
pub fn last_heard(activities: &[TraceActivity]) -> Option<i64> {
    activities
        .iter()
        .filter(|activity| activity.kind != AgentActivityKind::Direction)
        .map(|activity| activity.millis)
        .max()
}

/// Packets among `channels` from `interactions` in the window ending at
/// `now`, newest first. An interaction from a lane in one column to a lane in
/// another is a packet between them; one inside a single column, or touching
/// a lane no column holds, draws nothing. One stamped after `now`, by a clock
/// running ahead of this one, sits at the top.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn packets(channels: &[Channel<'_>], interactions: &[Interaction], now: i64) -> Vec<Packet> {
    let since = now - TRACE_WINDOW_MILLIS;
    let mut placed = interactions
        .iter()
        .filter(|interaction| interaction.millis > since)
        .flat_map(|interaction| {
            let millis = interaction.millis.min(now);
            let columns_of = |lane| {
                channels
                    .iter()
                    .enumerate()
                    .filter(move |(_, channel)| channel.lanes.contains(&lane))
                    .map(|(column, _)| column)
            };
            columns_of(interaction.from)
                .flat_map(move |from| columns_of(interaction.to).map(move |to| (from, to)))
                .filter(|(from, to)| from != to)
                .map(move |(from, to)| (millis, from, to))
        })
        .collect::<Vec<_>>();
    placed.sort_unstable_by(|a, b| b.cmp(a));
    let mut kept: Vec<(i64, usize, usize)> = Vec::with_capacity(placed.len());
    for packet in placed {
        let merged = kept
            .iter()
            .rev()
            .take_while(|earlier| earlier.0 - packet.0 < PACKET_MERGE_MILLIS)
            .any(|earlier| (earlier.1, earlier.2) == (packet.1, packet.2));
        if !merged {
            kept.push(packet);
        }
    }
    kept.into_iter()
        .map(|(millis, from, to)| Packet {
            from,
            to,
            depth: depth((now - millis) as f64),
        })
        .collect()
}

/// Smoothing half-width at an age: sharp at now, widening with age so that
/// older minutes become texture.
fn bandwidth(age_millis: f64) -> f64 {
    KERNEL_FLOOR_MILLIS + KERNEL_SLOPE * age_millis
}

/// Raised-cosine kernel of half-width `half` at `offset` from its centre.
fn kernel(offset: f64, half: f64) -> f64 {
    if offset.abs() >= half {
        0.0
    } else {
        f64::midpoint(1.0, (std::f64::consts::PI * offset / half).cos())
    }
}

/// Area of the kernel of half-width `half` centred at age `at` that lies
/// within the window, in milliseconds.
fn inside(at: f64, half: f64) -> f64 {
    let area = |t: f64| {
        f64::midpoint(
            t,
            half / std::f64::consts::PI * (std::f64::consts::PI * t / half).sin(),
        )
    };
    area((WINDOW - at).min(half)) - area((-at).max(-half))
}

/// Whether `millis` falls in an interval the channel was off.
fn off_at(off: &[TraceOff], millis: i64) -> bool {
    off.iter().any(|interval| {
        interval.from.is_none_or(|from| from <= millis)
            && interval.until.is_none_or(|until| millis < until)
    })
}

/// A channel's violin at the sample `age_millis` ago: its summed level and
/// kind shares, each kind's rate being its weighted activities through the
/// kernel for that age, normalised to the part of the kernel inside the
/// window so the level at now reports the current rate rather than half of it.
fn sample(activities: &[TraceActivity], now: i64, depth: f64, age_millis: f64) -> Sample {
    let half = bandwidth(age_millis);
    let (mut reasoning, mut tool, mut prose) = (0.0_f64, 0.0_f64, 0.0_f64);
    for activity in activities {
        let rate = match activity.kind {
            AgentActivityKind::Reasoning => &mut reasoning,
            AgentActivityKind::Tool => &mut tool,
            AgentActivityKind::Prose => &mut prose,
            _ => continue,
        };
        #[allow(clippy::cast_precision_loss)]
        let at = (now - activity.millis).max(0) as f64;
        if at >= WINDOW {
            continue;
        }
        *rate += activity_weight(activity.bytes) * kernel(at - age_millis, half);
    }
    let scale = MINUTE_MILLIS / inside(age_millis, half) / FULL_RATE_PER_MINUTE;
    let (reasoning, tool, prose) = (reasoning * scale, tool * scale, prose * scale);
    let total = reasoning + tool + prose;
    if total <= 0.0 {
        return Sample {
            depth,
            level: 0.0,
            reasoning: 0.0,
            tool: 1.0,
        };
    }
    Sample {
        depth,
        level: total.min(1.0),
        reasoning: reasoning / total,
        tool: tool / total,
    }
}

/// Weave `channels`, one column each in order, through the window ending at
/// `now`, sampling every violin at `samples` depths spaced evenly down the
/// axis so the curve is equally smooth everywhere. A sample is live when the
/// channel had been heard by then and was not off; runs of live samples are
/// violins, so silence above the last-heard instant and off spans leave real
/// gaps, and a run of one sample is too brief to draw.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn weave(
    channels: &[Channel<'_>],
    interactions: &[Interaction],
    now: i64,
    samples: usize,
) -> Weave {
    let since = now - TRACE_WINDOW_MILLIS;
    let samples = samples.max(2);
    let columns = channels
        .iter()
        .map(|channel| {
            let heard_millis = last_heard(channel.activities).filter(|millis| *millis > since);
            let heard_age = heard_millis.map(|millis| (now - millis).max(0));
            let mut runs = Vec::new();
            let mut run: Vec<Sample> = Vec::new();
            for index in 0..samples {
                let depth = index as f64 / (samples - 1) as f64;
                let age_millis = age(depth);
                // Truncation is the intent: ages are whole milliseconds.
                #[allow(clippy::cast_possible_truncation)]
                let instant = now - age_millis as i64;
                let live = heard_age.is_some_and(|heard| now - instant >= heard)
                    && !off_at(channel.off, instant);
                if live {
                    run.push(sample(channel.activities, now, depth, age_millis));
                } else if !run.is_empty() {
                    runs.push(Run {
                        samples: std::mem::take(&mut run),
                        reaches_floor: false,
                    });
                }
            }
            if !run.is_empty() {
                runs.push(Run {
                    samples: run,
                    reaches_floor: true,
                });
            }
            runs.retain(|run| run.samples.len() > 1);
            let directions = {
                let mut directions = channel
                    .activities
                    .iter()
                    .filter(|activity| {
                        activity.kind == AgentActivityKind::Direction && activity.millis > since
                    })
                    .map(|activity| depth((now - activity.millis) as f64))
                    .collect::<Vec<_>>();
                directions.sort_by(f64::total_cmp);
                directions
            };
            Column {
                runs,
                heard: heard_age.map(|age| depth(age as f64)),
                directions,
            }
        })
        .collect();
    Weave {
        columns,
        packets: packets(channels, interactions, now),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;
    const SECOND: i64 = 1_000;
    const MINUTE: i64 = 60 * SECOND;
    const SAMPLES: usize = 97;

    fn lane(n: u8) -> CanonicalUuid {
        format!("00000000-0000-4000-8000-{n:012}")
            .parse()
            .expect("fixture lane")
    }

    fn activity(kind: AgentActivityKind, seconds_ago: i64) -> TraceActivity {
        TraceActivity {
            kind,
            millis: NOW - seconds_ago * SECOND,
            bytes: 800,
        }
    }

    fn prose(seconds_ago: i64) -> TraceActivity {
        activity(AgentActivityKind::Prose, seconds_ago)
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

    fn seconds(depth: f64) -> f64 {
        age(depth) / 1_000.0
    }

    #[test]
    fn the_axis_is_log_recent_from_now_at_the_top_to_ten_minutes_at_the_floor() {
        assert_eq!(depth(0.0), 0.0);
        assert_eq!(depth(-5_000.0), 0.0, "a clock running ahead sits at now");
        assert_eq!(depth(600_000.0), 1.0);
        assert_eq!(
            depth(700_000.0),
            1.0,
            "older than the window sits on the floor"
        );
        let minute = depth(60_000.0);
        assert!((0.30..=0.36).contains(&minute), "one minute at {minute}");
        let mut previous = -1.0;
        for step in 0..=600 {
            let now = depth(f64::from(step) * 1_000.0);
            assert!(now > previous || (step == 0 && now == 0.0));
            previous = now;
        }
        for age_millis in [0.0, 1_000.0, 40_000.0, 299_000.0, 600_000.0] {
            assert!((age(depth(age_millis)) - age_millis).abs() < 1e-6);
        }
    }

    #[test]
    fn a_violin_runs_from_last_heard_to_the_floor_and_breaks_where_the_channel_was_off() {
        let lanes = [lane(1)];
        let heard = [prose(500), prose(300), prose(60)];
        let (off_from, off_until): (i32, i32) = (360, 240);
        let off = [TraceOff {
            from: Some(NOW - i64::from(off_from) * SECOND),
            until: Some(NOW - i64::from(off_until) * SECOND),
        }];
        let woven = weave(
            &[Channel {
                lanes: &lanes,
                activities: &heard,
                off: &off,
            }],
            &[],
            NOW,
            SAMPLES,
        );
        let column = &woven.columns[0];
        assert!((seconds(column.heard.expect("heard")) - 60.0).abs() < 1e-6);
        assert_eq!(column.runs.len(), 2, "{:?}", column.runs);
        let (upper, lower) = (&column.runs[0], &column.runs[1]);
        let first = seconds(upper.samples[0].depth);
        assert!(
            (60.0..66.0).contains(&first),
            "starts just below last heard at {first}"
        );
        let last_above = seconds(upper.samples.last().expect("samples").depth);
        assert!(last_above < f64::from(off_until));
        assert!(!upper.reaches_floor);
        assert!(seconds(lower.samples[0].depth) >= f64::from(off_until));
        assert!(lower.reaches_floor);
        assert_eq!(lower.samples.last().expect("samples").depth, 1.0);
        let in_gap = column
            .runs
            .iter()
            .flat_map(|run| &run.samples)
            .any(|sample| {
                (f64::from(off_until)..f64::from(off_from)).contains(&seconds(sample.depth))
            });
        assert!(!in_gap);

        // A seat vacant since before the record, taken while the window was
        // young, stops the violin there rather than letting it reach the floor.
        let (heard_ago, taken_ago): (i32, i32) = (20, 400);
        let vacant_before = [TraceOff {
            from: None,
            until: Some(NOW - i64::from(taken_ago) * SECOND),
        }];
        let ended = weave(
            &[Channel {
                lanes: &lanes,
                activities: &[prose(i64::from(heard_ago)), prose(200)],
                off: &vacant_before,
            }],
            &[],
            NOW,
            SAMPLES,
        );
        let runs = &ended.columns[0].runs;
        assert_eq!(runs.len(), 1);
        assert!(seconds(runs[0].samples[0].depth) >= f64::from(heard_ago));
        let last = seconds(runs[0].samples.last().expect("samples").depth);
        assert!(last < f64::from(taken_ago));
        assert!(!runs[0].reaches_floor);

        // Nothing heard in the window: no violin, no last-heard depth.
        let stale = weave(&[channel(&lanes, &[prose(700)])], &[], NOW, SAMPLES);
        assert_eq!(stale.columns[0].runs, []);
        assert_eq!(stale.columns[0].heard, None);
    }

    #[test]
    fn last_heard_ignores_direction_and_clamps_a_clock_running_ahead_to_now() {
        let lanes = [lane(1)];
        let direction = activity(AgentActivityKind::Direction, 10);
        let heard = [prose(180), direction, activity(AgentActivityKind::Tool, 90)];
        let woven = weave(&[channel(&lanes, &heard)], &[], NOW, SAMPLES);
        let column = &woven.columns[0];
        assert!((seconds(column.heard.expect("heard")) - 90.0).abs() < 1e-6);
        assert_eq!(column.directions.len(), 1);
        assert!((seconds(column.directions[0]) - 10.0).abs() < 1e-6);
        assert_eq!(last_heard(&heard), Some(NOW - 90 * SECOND));

        let ahead = [prose(-3)];
        let woven = weave(&[channel(&lanes, &ahead)], &[], NOW, SAMPLES);
        assert_eq!(woven.columns[0].heard, Some(0.0));
        assert_eq!(woven.columns[0].runs[0].samples[0].depth, 0.0);
    }

    #[test]
    fn a_violin_is_widest_at_its_activity_and_splits_by_kind() {
        let lanes = [lane(1)];
        let tool = [activity(AgentActivityKind::Tool, 120)];
        let woven = weave(&[channel(&lanes, &tool)], &[], NOW, SAMPLES);
        let samples = &woven.columns[0].runs[0].samples;
        let widest = samples
            .iter()
            .max_by(|a, b| a.level.total_cmp(&b.level))
            .expect("samples");
        assert!(widest.level > 0.0);
        let (event_ago, tolerance) = (120.0, 10.0);
        assert!(
            (seconds(widest.depth) - event_ago).abs() < tolerance,
            "{widest:?}"
        );
        assert_eq!((widest.reasoning, widest.tool), (0.0, 1.0));
        let far = samples.last().expect("samples");
        assert_eq!(far.level, 0.0, "ten minutes away is quiet");
        for sample in samples {
            assert!((0.0..=1.0).contains(&sample.level));
        }

        let busy = (0..120)
            .map(|step| activity(AgentActivityKind::Reasoning, 5 * step))
            .collect::<Vec<_>>();
        let woven = weave(&[channel(&lanes, &busy)], &[], NOW, SAMPLES);
        let top = woven.columns[0].runs[0].samples[0];
        assert_eq!(top.level, 1.0, "a burst saturates the scale");
        assert_eq!((top.reasoning, top.tool), (1.0, 0.0));
    }

    #[test]
    fn one_packet_rungs_each_column_pair_once_whichever_side_retained_it() {
        let (a, b, c, outside) = (lane(1), lane(2), lane(3), lane(9));
        let (ra, rb) = ([a], [b]);
        let teams = [channel(&ra, &[]), channel(&rb, &[])];
        let sent = NOW - 4 * MINUTE;
        // The sender's card and the recipient's directive both carry the packet.
        let seen_twice = [told(a, b, sent), told(a, b, sent + 4_000)];
        assert_eq!(packets(&teams, &seen_twice, NOW).len(), 1);
        // A reply a minute later is a second rung, pointing the other way.
        let replied = [told(a, b, sent), told(b, a, sent + MINUTE)];
        let rungs = packets(&teams, &replied, NOW);
        assert_eq!(rungs.len(), 2);
        assert_eq!((rungs[0].from, rungs[0].to), (1, 0), "newest first");
        assert!(rungs[0].depth < rungs[1].depth);
        assert!((seconds(rungs[1].depth) - 240.0).abs() < 1e-6);
        // Within one column, to a lane outside the panel, or outside the window: none.
        let unrelated = [
            told(a, a, sent),
            told(a, outside, sent),
            told(a, b, NOW - 11 * MINUTE),
        ];
        assert!(packets(&teams, &unrelated, NOW).is_empty());
        // A packet stamped by a clock running ahead sits at now.
        assert_eq!(
            packets(&teams, &[told(a, b, NOW + 2_000)], NOW),
            [Packet {
                from: 0,
                to: 1,
                depth: 0.0
            }]
        );

        // Design columns share lanes: a packet joins columns holding either
        // end, never a column to itself.
        let (design_one, design_two) = ([a, b], [b, c]);
        let designs = [channel(&design_one, &[]), channel(&design_two, &[])];
        let across = packets(&designs, &[told(a, c, sent)], NOW);
        assert_eq!(across.len(), 1);
        assert_eq!((across[0].from, across[0].to), (0, 1));
        assert_eq!(packets(&designs, &[told(a, b, sent)], NOW).len(), 1);
    }
}
