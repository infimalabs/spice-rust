//! What one lane has been doing lately, as its team pip draws it.
//!
//! A trace is a fixed window of wall time ending at the instant it is drawn,
//! shared by every lane so neighbouring pips compare honestly. Work becomes one
//! curve per kind, reasoning, tool, and prose, whose height is that kind's
//! recent rate on one fixed scale; lifecycle activity becomes markers. The
//! curves are sampled at evenly spaced instants through a kernel that slides
//! with time, so a redraw moves them smoothly instead of jumping between
//! buckets. Positions and levels are fractions; the browser owns every pixel.

use wire::AgentActivityKind;

/// Wall time one trace spans, ending at the instant it is drawn.
pub const TRACE_WINDOW_MILLIS: i64 = 10 * 60 * 1_000;

/// Redraw cadence of a non-empty trace: about one device pixel of drift at the
/// widest pip on a 2x display.
pub const TRACE_STEP_MILLIS: u32 = 4_000;

/// Text length at which an activity reaches full weight.
const FULL_WEIGHT_BYTES: usize = 1_600;

/// Weight every activity carries before its text, so a terse call still counts.
const BASE_WEIGHT: f64 = 0.35;

/// Weighted activities per minute at which a kind's curve reaches the top.
const FULL_RATE_PER_MINUTE: f64 = 3.0;

/// Kernel half-width in sample spacings: wide enough that neighbouring
/// samples share each activity and the curve stays smooth.
const KERNEL_SPACINGS: f64 = 1.5;

/// Level below which a kind draws no curve.
const VISIBLE_LEVEL: f64 = 0.02;

/// The part of one retained activity a trace draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceActivity {
    pub kind: AgentActivityKind,
    /// Display instant in epoch milliseconds.
    pub millis: i64,
    /// Length of the activity's text, which weights it.
    pub bytes: usize,
}

/// Kind of work a curve shows, in drawing order: prose paints last, on top.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceKind {
    Reasoning,
    Tool,
    Prose,
}

impl TraceKind {
    const ALL: [Self; 3] = [Self::Reasoning, Self::Tool, Self::Prose];

    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Reasoning => "reasoning",
            Self::Tool => "tool",
            Self::Prose => "prose",
        }
    }
}

/// One kind's rate through the window.
#[derive(Clone, Debug, PartialEq)]
pub struct TraceCurve {
    pub kind: TraceKind,
    /// Levels in `0..=1` at evenly spaced instants, the window's start first
    /// and now last.
    pub levels: Vec<f64>,
}

/// Lifecycle activity drawn as a marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceMarkerKind {
    Turn,
    Direction,
    Failure,
    Compaction,
    Session,
    SignOut,
}

impl TraceMarkerKind {
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Turn => "turn",
            Self::Direction => "direction",
            Self::Failure => "failure",
            Self::Compaction => "compaction",
            Self::Session => "session",
            Self::SignOut => "sign-out",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TraceMarker {
    pub kind: TraceMarkerKind,
    /// Fraction of the window, its start at 0 and now at 1.
    pub x: f64,
}

/// One lane's trace at one instant.
#[derive(Clone, Debug, PartialEq)]
pub struct Trace {
    /// Kinds with visible activity, in drawing order.
    pub curves: Vec<TraceCurve>,
    /// Markers in time order.
    pub markers: Vec<TraceMarker>,
}

impl Trace {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.curves.is_empty() && self.markers.is_empty()
    }
}

enum Placement {
    Curve(TraceKind),
    Marker(TraceMarkerKind),
}

const fn placement(kind: AgentActivityKind) -> Placement {
    match kind {
        AgentActivityKind::Prose => Placement::Curve(TraceKind::Prose),
        AgentActivityKind::Tool => Placement::Curve(TraceKind::Tool),
        AgentActivityKind::Reasoning => Placement::Curve(TraceKind::Reasoning),
        AgentActivityKind::Turn => Placement::Marker(TraceMarkerKind::Turn),
        AgentActivityKind::Direction => Placement::Marker(TraceMarkerKind::Direction),
        AgentActivityKind::Failure => Placement::Marker(TraceMarkerKind::Failure),
        AgentActivityKind::Compaction => Placement::Marker(TraceMarkerKind::Compaction),
        AgentActivityKind::Session => Placement::Marker(TraceMarkerKind::Session),
        AgentActivityKind::SignOut => Placement::Marker(TraceMarkerKind::SignOut),
    }
}

/// Trace one lane's activities, given in any order, through the window ending
/// at `now`, sampling each curve at `samples` instants (at least two).
///
/// A sample's level is its kind's weighted rate near that instant: each
/// activity spreads through a triangular kernel, and where the window's edge
/// cuts the kernel short the rate is taken from the part inside, so the
/// level at now reports the current rate rather than half of it.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn trace(activities: &[TraceActivity], now: i64, samples: usize) -> Trace {
    let samples = samples.max(2);
    let since = now - TRACE_WINDOW_MILLIS;
    let window = TRACE_WINDOW_MILLIS as f64;
    let x_of = |millis: i64| ((millis - since) as f64 / window).clamp(0.0, 1.0);
    let half_width = KERNEL_SPACINGS / (samples - 1) as f64;
    let mut sums = [(); 3].map(|()| vec![0.0_f64; samples]);
    let mut markers = Vec::new();
    for activity in activities
        .iter()
        .filter(|activity| activity.millis >= since)
    {
        let x = x_of(activity.millis);
        match placement(activity.kind) {
            Placement::Curve(kind) => {
                let weight = BASE_WEIGHT + text_weight(activity.bytes);
                let sums = &mut sums[kind as usize];
                for (index, sum) in sums.iter_mut().enumerate() {
                    let at = index as f64 / (samples - 1) as f64;
                    *sum += weight * (1.0 - (at - x).abs() / half_width).max(0.0);
                }
            }
            Placement::Marker(kind) => markers.push(TraceMarker { kind, x }),
        }
    }
    markers.sort_by(|a, b| a.x.total_cmp(&b.x));
    let window_minutes = window / 60_000.0;
    let curves = TraceKind::ALL
        .into_iter()
        .zip(sums)
        .map(|(kind, sums)| {
            let levels = sums
                .iter()
                .enumerate()
                .map(|(index, sum)| {
                    let at = index as f64 / (samples - 1) as f64;
                    // Kernel area inside the window, in window fractions.
                    let area = inside(at, half_width);
                    let per_minute = sum / (area * window_minutes);
                    (per_minute / FULL_RATE_PER_MINUTE).min(1.0)
                })
                .collect::<Vec<_>>();
            TraceCurve { kind, levels }
        })
        .filter(|curve| curve.levels.iter().any(|level| *level >= VISIBLE_LEVEL))
        .collect();
    Trace { curves, markers }
}

/// Area of a unit-height triangular kernel of half-width `half` centred at
/// `at` that lies within `0..=1`.
fn inside(at: f64, half: f64) -> f64 {
    // Area of the kernel beyond an edge `reach` away from its centre.
    let cut = |reach: f64| {
        let over = (half - reach).max(0.0);
        over * over / (2.0 * half)
    };
    half - cut(at) - cut(1.0 - at)
}

/// Weight grows with the square root of text length, so a long reply counts
/// for more without drowning out a run of terse calls.
#[allow(clippy::cast_precision_loss)]
fn text_weight(bytes: usize) -> f64 {
    (bytes.min(FULL_WEIGHT_BYTES) as f64 / FULL_WEIGHT_BYTES as f64).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;
    const SAMPLES: usize = 9;

    fn at(kind: AgentActivityKind, seconds_ago: i64) -> TraceActivity {
        TraceActivity {
            kind,
            millis: NOW - seconds_ago * 1_000,
            bytes: 400,
        }
    }

    fn levels(placed: &Trace, kind: TraceKind) -> &[f64] {
        &placed
            .curves
            .iter()
            .find(|curve| curve.kind == kind)
            .expect("the kind is drawn")
            .levels
    }

    #[test]
    fn the_window_ends_now_and_drops_older_activity() {
        let placed = trace(
            &[
                at(AgentActivityKind::Tool, 601),
                at(AgentActivityKind::Turn, 601),
                at(AgentActivityKind::Turn, 150),
                at(AgentActivityKind::Reasoning, 300),
            ],
            NOW,
            SAMPLES,
        );
        assert_eq!(
            placed
                .curves
                .iter()
                .map(|curve| curve.kind)
                .collect::<Vec<_>>(),
            [TraceKind::Reasoning]
        );
        assert_eq!(placed.markers.len(), 1);
        assert!((placed.markers[0].x - 0.75).abs() < 1e-9);
    }

    #[test]
    fn a_curve_peaks_at_its_activity_and_stays_inside_the_scale() {
        let placed = trace(&[at(AgentActivityKind::Tool, 300)], NOW, SAMPLES);
        let tool = levels(&placed, TraceKind::Tool);
        let peak = tool
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(index, _)| index);
        assert_eq!(peak, Some(SAMPLES / 2));
        assert!(tool[0].abs() < f64::EPSILON && tool[SAMPLES - 1].abs() < f64::EPSILON);

        let flood = (0..600)
            .map(|second| at(AgentActivityKind::Tool, second))
            .collect::<Vec<_>>();
        let flooded = trace(&flood, NOW, SAMPLES);
        assert!(
            levels(&flooded, TraceKind::Tool)
                .iter()
                .all(|level| (level - 1.0).abs() < f64::EPSILON)
        );
    }

    #[test]
    fn a_steady_rate_reads_the_same_at_now_as_mid_window() {
        // One reasoning step every twenty seconds: three a minute.
        let steady = (0..30)
            .map(|step| at(AgentActivityKind::Reasoning, step * 20 + 5))
            .collect::<Vec<_>>();
        let placed = trace(&steady, NOW, SAMPLES);
        let reasoning = levels(&placed, TraceKind::Reasoning);
        let middle = reasoning[SAMPLES / 2];
        assert!(middle > 0.0 && middle < 1.0);
        assert!((reasoning[SAMPLES - 1] - middle).abs() / middle < 0.2);
    }
}
