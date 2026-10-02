use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use tape::SourceId;
use wire::output::{Admission, Bounded, Budget, Resume};

use super::{
    Observation, SurfaceError, SurfaceEvent, SurfaceReasoning, SurfaceResidue, SurfaceToolMoment,
};
use crate::{
    Checkpoint, Dialect, EventKind, Position, Record, RecordPayload, Span, TailBudget, TailWork,
    scan_before, scan_tail,
};

/// Stable authenticated boundary used to continue toward older transcript records.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptCursor {
    /// Durable generation of the selected tape.
    pub source: SourceId,
    /// Exclusive end boundary for the next older page.
    pub before: Position,
}

impl fmt::Display for TranscriptCursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}:{}",
            self.source, self.before.member, self.before.offset
        )
    }
}

impl FromStr for TranscriptCursor {
    type Err = TranscriptCursorError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (source, position) = text.split_once(':').ok_or(TranscriptCursorError)?;
        let (member, offset) = position.split_once(':').ok_or(TranscriptCursorError)?;
        if offset.contains(':') {
            return Err(TranscriptCursorError);
        }
        Ok(Self {
            source: source.parse().map_err(|_| TranscriptCursorError)?,
            before: Position {
                member: member.parse().map_err(|_| TranscriptCursorError)?,
                offset: offset.parse().map_err(|_| TranscriptCursorError)?,
            },
        })
    }
}

/// A transcript cursor was not in its one canonical spelling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TranscriptCursorError;

impl fmt::Display for TranscriptCursorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("transcript cursor must be SOURCE:MEMBER:OFFSET")
    }
}

impl std::error::Error for TranscriptCursorError {}

/// Event selection for one transcript page. An empty set admits every event kind.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TranscriptFilter {
    kinds: BTreeSet<EventKind>,
}

impl TranscriptFilter {
    /// Select only the supplied event kinds.
    #[must_use]
    pub fn only(kinds: impl IntoIterator<Item = EventKind>) -> Self {
        Self {
            kinds: kinds.into_iter().collect(),
        }
    }

    fn admits(&self, observation: &Observation) -> bool {
        match observation {
            Observation::Event { event, .. } => {
                self.kinds.is_empty() || self.kinds.contains(&event.kind())
            }
            _ => self.kinds.is_empty(),
        }
    }
}

/// Caller-owned work and presentation limits for one transcript page.
#[derive(Clone, Debug)]
pub struct TranscriptPageRequest {
    /// Hard source-read, decode, pass, and wall-time allowance.
    pub scan: TailBudget,
    /// Aggregate row/encoded-byte and per-text-field allowance.
    pub output: Budget,
    /// Authenticated exclusive boundary for an older page.
    pub before: Option<TranscriptCursor>,
    /// Event selection applied before presentation admission.
    pub filter: TranscriptFilter,
}

/// One complete vendor record and every visible observation it decoded to.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptRecord {
    /// Boundary that re-includes this record on the next older page.
    pub end: TranscriptCursor,
    /// Observations emitted from the record, kept atomically for pagination.
    pub observations: Vec<TranscriptObservation>,
}

/// One observation after per-field text clipping.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptObservation {
    /// Existing typed transcript evidence.
    pub observation: Observation,
    /// Exact UTF-8 bytes withheld across this observation's text fields.
    pub withheld_text_bytes: usize,
}

/// How a presentation omission can be recovered without flooding stdout.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TranscriptContinuation {
    /// Continue from an authenticated complete-record boundary.
    Cursor { cursor: TranscriptCursor },
    /// The complete record exceeds the terminal maximum and needs exact export.
    Export { spans: Vec<Span> },
}

impl fmt::Display for TranscriptContinuation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cursor { cursor } => write!(formatter, "cursor {cursor}"),
            Self::Export { spans } => write!(
                formatter,
                "exact export for spans {}",
                spans
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    }
}

/// Older source bytes outside the newest bounded scan window.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptHistory {
    /// Exact plaintext bytes preceding this page's scan window.
    pub withheld_source_bytes: u64,
    /// Authenticated exclusive boundary for the next older scan.
    pub continuation: TranscriptCursor,
}

/// Exact bounded work consumed by one page.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptWork {
    pub source_bytes: u64,
    pub records: usize,
    pub passes: usize,
    pub elapsed_millis: u64,
}

impl From<TailWork> for TranscriptWork {
    fn from(work: TailWork) -> Self {
        Self {
            source_bytes: work.source_bytes,
            records: work.records,
            passes: work.passes,
            elapsed_millis: u64::try_from(work.duration.as_millis()).unwrap_or(u64::MAX),
        }
    }
}

/// One bounded, newest-first page over an exact transcript tape.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptPage {
    /// Selected tape.
    pub tape: PathBuf,
    /// Complete records admitted in newest-first order.
    pub records: Bounded<TranscriptRecord, TranscriptContinuation>,
    /// Older bytes outside the bounded source window, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<TranscriptHistory>,
    /// Unterminated bytes at the current end of the tape.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<Span>,
    /// Event kinds absent from the decoded current window.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub canaries: Vec<Observation>,
    /// Work consumed before presentation admission.
    pub work: TranscriptWork,
}

/// Read one exact transcript tape under hard source and presentation bounds.
///
/// Records are presented newest first. One vendor record is the atomic paging
/// row, so a record that emits several events cannot be split across pages or
/// replay a previously admitted event. A continuation authenticates the tape
/// generation and is refused by the tail scanner after replacement.
///
/// # Errors
///
/// Returns a typed surface error when the tape cannot be read, the source work
/// allowance is exhausted, or a supplied cursor does not belong to the tape.
pub fn transcript_page(
    tape: impl AsRef<Path>,
    lane: impl Into<String>,
    dialect: Dialect,
    request: TranscriptPageRequest,
) -> Result<TranscriptPage, SurfaceError> {
    let tape = tape.as_ref();
    let lane = lane.into();
    let tail = match request.before {
        Some(cursor) => scan_before(
            tape,
            dialect,
            &Checkpoint {
                source: cursor.source,
                next: cursor.before,
            },
            request.scan,
        ),
        None => scan_tail(tape, dialect, request.scan),
    }
    .map_err(|source| SurfaceError::Scan {
        path: tape.to_path_buf(),
        source,
    })?;

    let mut seen = BTreeSet::new();
    let mut admission = Admission::new(request.output);
    for record in tail.scan.records.iter().rev() {
        let mut observations = record_observations(&lane, dialect, tape, record);
        for observation in &observations {
            if let Observation::Event { event, .. } = observation {
                seen.insert(event.kind());
            }
        }
        observations.retain(|observation| request.filter.admits(observation));
        if observations.is_empty() {
            continue;
        }
        let observations = observations
            .into_iter()
            .map(|observation| clipped(observation, request.output))
            .collect();
        let row = TranscriptRecord {
            end: TranscriptCursor {
                source: record.next.source.clone(),
                before: record.next.next,
            },
            observations,
        };
        admission
            .offer(row)
            .map_err(|error| SurfaceError::Encode(error.to_string()))?;
    }
    let presentation_closed = admission.is_closed();
    let records = admission.finish(|resume| match resume {
        Resume::At(record) | Resume::Widened { record, .. } => TranscriptContinuation::Cursor {
            cursor: record.end.clone(),
        },
        Resume::Export(record) => TranscriptContinuation::Export {
            spans: record
                .observations
                .iter()
                .flat_map(|observation| observation_spans(&observation.observation))
                .collect(),
        },
    });
    let history =
        (!presentation_closed && tail.withheld_source_bytes > 0).then(|| TranscriptHistory {
            withheld_source_bytes: tail.withheld_source_bytes,
            continuation: TranscriptCursor {
                source: tail.start.source.clone(),
                before: tail.start.next,
            },
        });
    let canaries = EventKind::ALL
        .into_iter()
        .filter(|kind| !seen.contains(kind))
        .map(|expected| Observation::Canary {
            lane: lane.clone(),
            dialect,
            tape: tape.to_path_buf(),
            expected,
        })
        .collect();

    Ok(TranscriptPage {
        tape: tape.to_path_buf(),
        records,
        history,
        pending: tail.scan.pending,
        canaries,
        work: tail.work.into(),
    })
}

fn record_observations(
    lane: &str,
    dialect: Dialect,
    tape: &Path,
    record: &Record,
) -> Vec<Observation> {
    match &record.payload {
        RecordPayload::Events(events) => events
            .iter()
            .map(|sourced| Observation::Event {
                lane: lane.to_owned(),
                dialect,
                tape: tape.to_path_buf(),
                spans: sourced.spans.clone(),
                event: (&sourced.event).into(),
            })
            .collect(),
        RecordPayload::Residue(sourced) => vec![Observation::Residue {
            lane: lane.to_owned(),
            dialect,
            tape: tape.to_path_buf(),
            spans: sourced.spans.clone(),
            residue: (&sourced.residue.kind).into(),
        }],
    }
}

fn clipped(mut observation: Observation, budget: Budget) -> TranscriptObservation {
    let mut withheld = 0;
    match &mut observation {
        Observation::Event { event, .. } => clip_event(event, budget, &mut withheld),
        Observation::Residue { residue, .. } => clip_residue(residue, budget, &mut withheld),
        Observation::Missing { selector, detail } => {
            clip_field(selector, budget, &mut withheld);
            clip_field(detail, budget, &mut withheld);
        }
        Observation::Absent { selector } => clip_field(selector, budget, &mut withheld),
        Observation::Ambiguous { selector, .. } => clip_field(selector, budget, &mut withheld),
        Observation::Pending { .. } | Observation::Canary { .. } => {}
    }
    TranscriptObservation {
        observation,
        withheld_text_bytes: withheld,
    }
}

fn clip_event(event: &mut SurfaceEvent, budget: Budget, withheld: &mut usize) {
    match event {
        SurfaceEvent::Session { identity } => clip_field(identity, budget, withheld),
        SurfaceEvent::Prose { text, phase, .. } => {
            clip_field(text, budget, withheld);
            if let Some(phase) = phase {
                clip_field(phase, budget, withheld);
            }
        }
        SurfaceEvent::Reasoning {
            content: SurfaceReasoning::Plain { lines },
        } => {
            for line in lines {
                clip_field(line, budget, withheld);
            }
        }
        SurfaceEvent::Tool { call_id, moment } => {
            clip_field(call_id, budget, withheld);
            match moment {
                SurfaceToolMoment::Call { name, arguments }
                | SurfaceToolMoment::Update { name, arguments } => {
                    clip_field(name, budget, withheld);
                    clip_field(arguments, budget, withheld);
                }
                SurfaceToolMoment::Result { output, .. } => {
                    if let Some(output) = output {
                        clip_field(output, budget, withheld);
                    }
                }
            }
        }
        SurfaceEvent::Failure { message } => clip_field(message, budget, withheld),
        SurfaceEvent::SignOut { message, login } => {
            clip_field(message, budget, withheld);
            clip_field(login, budget, withheld);
        }
        SurfaceEvent::Usage { .. } | SurfaceEvent::Compaction {} => {}
    }
}

fn clip_residue(residue: &mut SurfaceResidue, budget: Budget, withheld: &mut usize) {
    match residue {
        SurfaceResidue::Absent { field } => clip_field(field, budget, withheld),
        SurfaceResidue::Unknown { discriminator }
        | SurfaceResidue::VersionDrift { discriminator } => {
            clip_field(discriminator, budget, withheld);
        }
        SurfaceResidue::Malformed { field, expected } => {
            clip_field(field, budget, withheld);
            clip_field(expected, budget, withheld);
        }
        SurfaceResidue::Truncated {} => {}
    }
}

fn clip_field(field: &mut String, budget: Budget, withheld: &mut usize) {
    let clipped = budget.clip(field);
    *withheld = withheld.saturating_add(clipped.withheld_bytes.map_or(0, |count| count.get()));
    *field = clipped.text;
}

fn observation_spans(observation: &Observation) -> Vec<Span> {
    match observation {
        Observation::Event { spans, .. }
        | Observation::Residue { spans, .. }
        | Observation::Pending { spans, .. } => spans.clone(),
        Observation::Absent { .. }
        | Observation::Missing { .. }
        | Observation::Ambiguous { .. }
        | Observation::Canary { .. } => Vec::new(),
    }
}
