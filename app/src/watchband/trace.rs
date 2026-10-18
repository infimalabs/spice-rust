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

/// Kernel half-width as a fraction of the ten-minute window. It is fixed in
/// wall time so the same activity has the same height at every pip width.
const KERNEL_HALF_WINDOW: f64 = 1.5 / 8.0;

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

/// Map one Design-room event onto the same activity vocabulary as a team pip.
///
/// Publication authorship matters: a lane's publication is agent prose, while
/// operator and human publications are direction. Resident-authored
/// bookkeeping carries neither kind of authored activity.
#[must_use]
pub fn room_activity(event: &crate::runtime::RoomEventKind) -> Option<(AgentActivityKind, usize)> {
    use crate::runtime::RoomEventKind;

    match event {
        RoomEventKind::Messaged { source, .. } => {
            Some((AgentActivityKind::Prose, hunk_bytes(source)))
        }
        RoomEventKind::Contributed { text, .. } => Some((AgentActivityKind::Tool, text.len())),
        RoomEventKind::Published { author, content } => match author {
            wire::Actor::Lane(_) => Some((AgentActivityKind::Prose, publication_bytes(content))),
            wire::Actor::Operator | wire::Actor::Human(_) => {
                Some((AgentActivityKind::Direction, 0))
            }
            wire::Actor::Spice => None,
        },
        RoomEventKind::Posted { .. } => Some((AgentActivityKind::Direction, 0)),
        RoomEventKind::Coordination { .. } => Some((AgentActivityKind::Turn, 0)),
        RoomEventKind::Left { .. } => Some((AgentActivityKind::Compaction, 0)),
        RoomEventKind::PutAside { .. } => None,
    }
}

fn publication_bytes(content: &wire::rooms::RoomPublicationContent) -> usize {
    match content {
        wire::rooms::RoomPublicationContent::Authored { text } => text.len(),
        wire::rooms::RoomPublicationContent::Linked {
            link: wire::rooms::RoomSource::Transcript(source),
        } => hunk_bytes(source),
        wire::rooms::RoomPublicationContent::Linked { .. } => 0,
    }
}

fn hunk_bytes(source: &wire::rooms::TranscriptHunk) -> usize {
    usize::try_from(source.text.end.saturating_sub(source.text.start)).unwrap_or(usize::MAX)
}

/// Trace one lane's activities, given in any order, through the window ending
/// at `now`, sampling each curve at no fewer than `samples` instants.
///
/// The grid is always odd so the window start, midpoint, and now are shared
/// samples at every browser-selected density. A sample's level is its kind's
/// weighted rate near that instant: each activity spreads through a triangular
/// kernel of fixed wall-time width, and where the window's edge cuts the kernel
/// short the rate is taken from the part inside, so the level at now reports
/// the current rate rather than half of it.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn trace(activities: &[TraceActivity], now: i64, samples: usize) -> Trace {
    let samples = samples.max(2);
    let samples = samples + usize::from(samples.is_multiple_of(2));
    let since = now - TRACE_WINDOW_MILLIS;
    let window = TRACE_WINDOW_MILLIS as f64;
    let x_of = |millis: i64| ((millis - since) as f64 / window).clamp(0.0, 1.0);
    let half_width = KERNEL_HALF_WINDOW;
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
    fn one_fixed_rate_scale_is_shared_by_every_browser_sample_count() {
        let mut midpoint = None;
        let mut edge = None;
        for samples in 5..=9 {
            let middle = trace(&[at(AgentActivityKind::Tool, 300)], NOW, samples);
            let middle = levels(&middle, TraceKind::Tool);
            let current = trace(&[at(AgentActivityKind::Tool, 0)], NOW, samples);
            let current = levels(&current, TraceKind::Tool);
            let observed = (middle[middle.len() / 2], current[current.len() - 1]);
            let expected = (
                *midpoint.get_or_insert(observed.0),
                *edge.get_or_insert(observed.1),
            );
            assert!((observed.0 - expected.0).abs() < 1e-9, "{samples}");
            assert!((observed.1 - expected.1).abs() < 1e-9, "{samples}");
        }
    }

    #[test]
    fn room_publication_authorship_preserves_prose_and_direction_semantics() {
        let lane = "018f82d4-6f59-7d06-92e3-bce9131163e5"
            .parse()
            .expect("lane");
        let human = "018f82d4-6f59-7d06-92e3-bce9131163e6"
            .parse()
            .expect("human");
        let lane_text = "A lane-authored room observation.".to_owned();
        let lane_publication = crate::runtime::RoomEventKind::Published {
            author: wire::Actor::Lane(lane),
            content: wire::rooms::RoomPublicationContent::Authored {
                text: lane_text.clone(),
            },
        };
        let human_publication = crate::runtime::RoomEventKind::Published {
            author: wire::Actor::Human(human),
            content: wire::rooms::RoomPublicationContent::Authored {
                text: "Operator observation.".to_owned(),
            },
        };

        assert_eq!(
            room_activity(&lane_publication),
            Some((AgentActivityKind::Prose, lane_text.len())),
        );
        assert_eq!(
            room_activity(&human_publication),
            Some((AgentActivityKind::Direction, 0)),
        );
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
        assert_eq!(peak, Some(tool.len() / 2));
        assert!(tool[0].abs() < f64::EPSILON && tool[tool.len() - 1].abs() < f64::EPSILON);

        let flood = (0..600)
            .map(|second| at(AgentActivityKind::Tool, second))
            .collect::<Vec<_>>();
        for samples in 5..=9 {
            let flooded = trace(&flood, NOW, samples);
            assert!(
                levels(&flooded, TraceKind::Tool)
                    .iter()
                    .all(|level| (level - 1.0).abs() < f64::EPSILON),
                "{samples}",
            );
        }
    }

    #[test]
    fn a_steady_rate_reads_the_same_at_now_as_mid_window_at_every_density() {
        // One reasoning step every twenty seconds: three a minute.
        let steady = (0..30)
            .map(|step| at(AgentActivityKind::Reasoning, step * 20 + 5))
            .collect::<Vec<_>>();
        for samples in 5..=9 {
            let placed = trace(&steady, NOW, samples);
            let reasoning = levels(&placed, TraceKind::Reasoning);
            let middle = reasoning[reasoning.len() / 2];
            assert!(middle > 0.0 && middle < 1.0);
            assert!(
                (reasoning[reasoning.len() - 1] - middle).abs() / middle < 0.2,
                "{samples}",
            );
        }
    }
}
