//! Where one lane's recent activity lands in its team pip.
//!
//! A trace is a fixed window of wall time ending at the instant it is drawn,
//! shared by every lane so neighbouring pips compare honestly. Work becomes
//! notes in three rows, prose above tool above reasoning, joined in time order
//! by one step thread whose final run shows what the lane did most recently.
//! Lifecycle activity becomes full-height markers. Positions are fractions of
//! the trace width; the browser supplies only the least pitch between
//! neighbouring notes, so a burst stays countable at any width while every
//! pixel minimum stays a stylesheet concern.

use wire::AgentActivityKind;

/// Wall time one trace spans, ending at the instant it is drawn.
pub const TRACE_WINDOW_MILLIS: i64 = 10 * 60 * 1_000;

/// Redraw cadence of a non-empty trace: about one device pixel of drift at the
/// widest pip on a 2x display.
pub const TRACE_STEP_MILLIS: u32 = 4_000;

/// Note length at full weight, as a fraction of the trace width.
const NOTE_SPAN: f64 = 0.045;

/// Text length at which a note reaches full weight.
const FULL_NOTE_BYTES: usize = 1_600;

/// The part of one retained activity a trace draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceActivity {
    pub kind: AgentActivityKind,
    /// Display instant in epoch milliseconds.
    pub millis: i64,
    /// Length of the activity's text, which sets its note's length.
    pub bytes: usize,
}

/// Row of a work note, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceRow {
    Prose,
    Tool,
    Reasoning,
}

impl TraceRow {
    /// Zero-based row from the top.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Prose => 0,
            Self::Tool => 1,
            Self::Reasoning => 2,
        }
    }

    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Prose => "prose",
            Self::Tool => "tool",
            Self::Reasoning => "reasoning",
        }
    }
}

/// One work activity drawn as a note.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TraceNote {
    pub row: TraceRow,
    /// Left edge.
    pub x: f64,
    /// Length the activity's text earns.
    pub length: f64,
    /// Distance to the next note in the same row, or to the trace's right
    /// edge. A note stops short of it so a burst stays countable.
    pub room: f64,
}

/// Lifecycle activity drawn as a full-height marker.
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
    pub x: f64,
}

/// One lane's trace at one instant.
#[derive(Clone, Debug, PartialEq)]
pub struct Trace {
    /// Notes in time order; the thread visits them in this order.
    pub notes: Vec<TraceNote>,
    pub markers: Vec<TraceMarker>,
    /// Where the thread's final run stops: a sign-out after the last note,
    /// otherwise now.
    pub thread_end: f64,
}

impl Trace {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty() && self.markers.is_empty()
    }
}

enum Placement {
    Note(TraceRow),
    Marker(TraceMarkerKind),
}

const fn placement(kind: AgentActivityKind) -> Placement {
    match kind {
        AgentActivityKind::Prose => Placement::Note(TraceRow::Prose),
        AgentActivityKind::Tool => Placement::Note(TraceRow::Tool),
        AgentActivityKind::Reasoning => Placement::Note(TraceRow::Reasoning),
        AgentActivityKind::Turn => Placement::Marker(TraceMarkerKind::Turn),
        AgentActivityKind::Direction => Placement::Marker(TraceMarkerKind::Direction),
        AgentActivityKind::Failure => Placement::Marker(TraceMarkerKind::Failure),
        AgentActivityKind::Compaction => Placement::Marker(TraceMarkerKind::Compaction),
        AgentActivityKind::Session => Placement::Marker(TraceMarkerKind::Session),
        AgentActivityKind::SignOut => Placement::Marker(TraceMarkerKind::SignOut),
    }
}

/// Place one lane's activities, given in any order, in the window ending at
/// `now`. Activities at equal instants keep the order given.
///
/// Neighbouring notes in a row stand at least `pitch` apart, a fraction of the
/// trace width: the newest note of a closer run keeps its instant and each
/// older one steps back from its successor, so the trace's recent edge stays
/// true while every note in a burst stays separately visible.
#[must_use]
pub fn trace(activities: &[TraceActivity], now: i64, pitch: f64) -> Trace {
    let since = now - TRACE_WINDOW_MILLIS;
    let mut recent = activities
        .iter()
        .filter(|activity| activity.millis >= since)
        .collect::<Vec<_>>();
    recent.sort_by_key(|activity| activity.millis);
    // A display instant ahead of this browser's clock lands at now.
    #[allow(clippy::cast_precision_loss)]
    let x_of = |millis: i64| 1.0 - (now - millis).max(0) as f64 / TRACE_WINDOW_MILLIS as f64;
    let mut notes = Vec::new();
    let mut markers = Vec::new();
    for activity in recent {
        let x = x_of(activity.millis);
        match placement(activity.kind) {
            Placement::Note(row) => notes.push(TraceNote {
                row,
                x,
                length: weight(activity.bytes) * NOTE_SPAN,
                room: 1.0 - x,
            }),
            Placement::Marker(kind) => markers.push(TraceMarker { kind, x }),
        }
    }
    let mut following = [None::<f64>; 3];
    for note in notes.iter_mut().rev() {
        let next = &mut following[note.row.index()];
        if let Some(next) = *next {
            note.x = note.x.min(next - pitch);
            note.room = next - note.x;
        }
        *next = Some(note.x);
    }
    let thread_end = notes.last().map_or(1.0, |last| {
        markers
            .iter()
            .rev()
            .find(|marker| marker.kind == TraceMarkerKind::SignOut && marker.x >= last.x)
            .map_or(1.0, |sign_out| sign_out.x)
    });
    Trace {
        notes,
        markers,
        thread_end,
    }
}

/// Weight grows with the square root of text length, so a terse call still
/// reads while a long reply cannot crowd out its neighbours.
#[allow(clippy::cast_precision_loss)]
fn weight(bytes: usize) -> f64 {
    (bytes.min(FULL_NOTE_BYTES) as f64 / FULL_NOTE_BYTES as f64).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;
    /// Two pixels across a hundred-pixel trace.
    const PITCH: f64 = 0.02;

    fn at(kind: AgentActivityKind, seconds_ago: i64) -> TraceActivity {
        TraceActivity {
            kind,
            millis: NOW - seconds_ago * 1_000,
            bytes: 400,
        }
    }

    #[test]
    fn the_window_ends_now_and_drops_older_activity() {
        let placed = trace(
            &[
                at(AgentActivityKind::Tool, 601),
                at(AgentActivityKind::Tool, 600),
                at(AgentActivityKind::Tool, 150),
                at(AgentActivityKind::Tool, -5),
            ],
            NOW,
            PITCH,
        );
        let xs = placed.notes.iter().map(|note| note.x).collect::<Vec<_>>();
        assert_eq!(xs, [0.0, 0.75, 1.0]);
    }

    #[test]
    fn a_note_stops_at_the_next_note_in_its_own_row() {
        let placed = trace(
            &[
                at(AgentActivityKind::Tool, 300),
                at(AgentActivityKind::Reasoning, 299),
                at(AgentActivityKind::Tool, 240),
                at(AgentActivityKind::Prose, 60),
            ],
            NOW,
            PITCH,
        );
        let rooms = placed
            .notes
            .iter()
            .map(|note| (note.row, (note.room * 100.0).round()))
            .collect::<Vec<_>>();
        assert_eq!(
            rooms,
            [
                (TraceRow::Tool, 10.0),
                (TraceRow::Reasoning, 50.0),
                (TraceRow::Tool, 40.0),
                (TraceRow::Prose, 10.0),
            ]
        );
    }

    #[test]
    fn a_burst_steps_back_from_its_newest_note_without_touching_other_rows() {
        let mut activities = (0..12)
            .map(|step| at(AgentActivityKind::Tool, 300 - step * 4))
            .collect::<Vec<_>>();
        activities.push(at(AgentActivityKind::Reasoning, 290));
        let placed = trace(&activities, NOW, PITCH);
        let tools = placed
            .notes
            .iter()
            .filter(|note| note.row == TraceRow::Tool)
            .collect::<Vec<_>>();
        assert_eq!(tools.len(), 12);
        let newest = 1.0 - 256.0 / 600.0;
        assert!((tools[11].x - newest).abs() < 1e-9);
        for pair in tools.windows(2) {
            assert!(pair[1].x - pair[0].x >= PITCH - 1e-9);
            assert!((pair[0].room - (pair[1].x - pair[0].x)).abs() < 1e-9);
        }
        let reasoning = placed
            .notes
            .iter()
            .find(|note| note.row == TraceRow::Reasoning)
            .expect("the reasoning note is drawn");
        assert!((reasoning.x - (1.0 - 290.0 / 600.0)).abs() < 1e-9);
    }

    #[test]
    fn lifecycle_activity_marks_the_trace_and_a_trailing_sign_out_ends_the_thread() {
        let placed = trace(
            &[
                at(AgentActivityKind::Turn, 540),
                at(AgentActivityKind::Tool, 480),
                at(AgentActivityKind::SignOut, 300),
            ],
            NOW,
            PITCH,
        );
        assert_eq!(placed.notes.len(), 1);
        assert_eq!(
            placed
                .markers
                .iter()
                .map(|marker| marker.kind)
                .collect::<Vec<_>>(),
            [TraceMarkerKind::Turn, TraceMarkerKind::SignOut]
        );
        assert!((placed.thread_end - 0.5).abs() < 1e-9);

        let resumed = trace(
            &[
                at(AgentActivityKind::SignOut, 300),
                at(AgentActivityKind::Session, 200),
                at(AgentActivityKind::Tool, 100),
            ],
            NOW,
            PITCH,
        );
        assert!((resumed.thread_end - 1.0).abs() < 1e-9);
    }
}
