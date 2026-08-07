//! Daemonless observation and replay over owned transcript tape.
//!
//! A selected root contains sessions at `<root>/<lane>/codex` or
//! `<root>/<lane>/claude`; the final component is the decoder contract and the
//! parent component is the lane spelling. Discovery follows real directories
//! only and never guesses a dialect from transcript contents.
//!
//! [`Watch`] retains one cursor per tape. A filesystem watcher may coalesce or
//! miss notifications without losing a record because every wakeup compares
//! that cursor to the durable member extents again. [`retro`] is finite: it
//! resolves one syntactic selector, reads only the resulting tape spans, and
//! turns every absence or untyped byte range into an observation.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};
use tape::{Locus, Tape};

use crate::{
    Decoded, Dialect, Event, EventKind, ReasoningContent, Residue, ResidueKind, ToolMoment,
    ToolResult, decode,
};

/// Which daemonless surface produced a stream outcome.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Cursor-following observation.
    Watch,
    /// Finite replay.
    Retro,
}

/// Serializable spelling of one durable tape span.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Span {
    /// Tape member.
    pub member: u64,
    /// First byte.
    pub offset: u64,
    /// Number of bytes.
    pub length: u64,
}

impl From<Locus> for Span {
    fn from(locus: Locus) -> Self {
        Self {
            member: locus.member,
            offset: locus.offset,
            length: locus.length,
        }
    }
}

impl From<Span> for Locus {
    fn from(span: Span) -> Self {
        Self::new(span.member, span.offset, span.length)
    }
}

impl fmt::Display for Span {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}:{}", self.member, self.offset, self.length)
    }
}

/// Stable machine payload of one decoded event.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SurfaceEvent {
    /// Exact resumable vendor session declaration.
    Session {
        /// Vendor spelling of the session identity.
        identity: String,
    },
    /// Assistant prose.
    Prose {
        /// Text reported by the vendor.
        text: String,
        /// Vendor phase when the current dialect reports one.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        phase: Option<String>,
    },
    /// Model reasoning.
    Reasoning {
        /// Available reasoning representation.
        content: SurfaceReasoning,
    },
    /// Tool activity.
    Tool {
        /// Vendor call identifier.
        call_id: String,
        /// Position in the tool lifecycle.
        moment: SurfaceToolMoment,
    },
    /// Token usage.
    Usage {
        /// Uncached input tokens.
        input_tokens: u64,
        /// Cached input tokens.
        cached_input_tokens: u64,
        /// Input tokens written to a cache.
        cache_write_input_tokens: u64,
        /// Output tokens.
        output_tokens: u64,
        /// Reasoning output tokens, when exposed.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        reasoning_output_tokens: Option<u64>,
    },
    /// Context compaction.
    Compaction {},
    /// Vendor failure.
    Failure {
        /// Vendor failure message.
        message: String,
    },
}

fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

impl SurfaceEvent {
    /// Closed kind of this surface event.
    #[must_use]
    pub const fn kind(&self) -> EventKind {
        match self {
            Self::Session { .. } => EventKind::Session,
            Self::Prose { .. } => EventKind::Prose,
            Self::Reasoning { .. } => EventKind::Reasoning,
            Self::Tool { .. } => EventKind::Tool,
            Self::Usage { .. } => EventKind::Usage,
            Self::Compaction {} => EventKind::Compaction,
            Self::Failure { .. } => EventKind::Failure,
        }
    }
}

impl From<&Event> for SurfaceEvent {
    fn from(event: &Event) -> Self {
        match event {
            Event::Session(event) => Self::Session {
                identity: event.identity.clone(),
            },
            Event::Prose(event) => Self::Prose {
                text: event.text.clone(),
                phase: event.phase.clone(),
            },
            Event::Reasoning(event) => Self::Reasoning {
                content: (&event.content).into(),
            },
            Event::Tool(event) => Self::Tool {
                call_id: event.call_id.clone(),
                moment: (&event.moment).into(),
            },
            Event::Usage(event) => Self::Usage {
                input_tokens: event.input_tokens,
                cached_input_tokens: event.cached_input_tokens,
                cache_write_input_tokens: event.cache_write_input_tokens,
                output_tokens: event.output_tokens,
                reasoning_output_tokens: event.reasoning_output_tokens,
            },
            Event::Compaction(_) => Self::Compaction {},
            Event::Failure(event) => Self::Failure {
                message: event.message.clone(),
            },
        }
    }
}

impl fmt::Display for SurfaceEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Session { identity } => write!(formatter, "session identity={identity:?}"),
            Self::Prose { text, phase } => {
                write!(formatter, "prose text={text:?} phase={phase:?}")
            }
            Self::Reasoning { content } => write!(formatter, "reasoning {content}"),
            Self::Tool { call_id, moment } => {
                write!(formatter, "tool call_id={call_id:?} {moment}")
            }
            Self::Usage {
                input_tokens,
                cached_input_tokens,
                cache_write_input_tokens,
                output_tokens,
                reasoning_output_tokens,
            } => write!(
                formatter,
                concat!(
                    "usage input={} cached={} cache_write={} ",
                    "output={} reasoning={:?}"
                ),
                input_tokens,
                cached_input_tokens,
                cache_write_input_tokens,
                output_tokens,
                reasoning_output_tokens,
            ),
            Self::Compaction {} => formatter.write_str("compaction"),
            Self::Failure { message } => write!(formatter, "failure message={message:?}"),
        }
    }
}

/// Stable reasoning representation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "form", rename_all = "snake_case", deny_unknown_fields)]
pub enum SurfaceReasoning {
    /// Plain reasoning lines.
    Plain {
        /// Lines reported by the vendor.
        lines: Vec<String>,
    },
    /// Reasoning exists but its text is unavailable.
    Opaque {},
}

impl From<&ReasoningContent> for SurfaceReasoning {
    fn from(content: &ReasoningContent) -> Self {
        match content {
            ReasoningContent::Plain(lines) => Self::Plain {
                lines: lines.clone(),
            },
            ReasoningContent::Opaque => Self::Opaque {},
        }
    }
}

impl fmt::Display for SurfaceReasoning {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plain { lines } => write!(formatter, "plain lines={lines:?}"),
            Self::Opaque {} => formatter.write_str("opaque"),
        }
    }
}

/// Stable tool lifecycle representation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "moment", rename_all = "snake_case", deny_unknown_fields)]
pub enum SurfaceToolMoment {
    /// A call names its tool.
    Call {
        /// Tool name.
        name: String,
    },
    /// A running call published updated state.
    Update {
        /// Tool name.
        name: String,
    },
    /// A result reports its standing.
    Result {
        /// Vendor knowledge of completion.
        standing: SurfaceToolResult,
    },
}

impl From<&ToolMoment> for SurfaceToolMoment {
    fn from(moment: &ToolMoment) -> Self {
        match moment {
            ToolMoment::Call { name, .. } => Self::Call { name: name.clone() },
            ToolMoment::Update { name, .. } => Self::Update { name: name.clone() },
            ToolMoment::Result(result) => Self::Result {
                standing: (*result).into(),
            },
        }
    }
}

impl fmt::Display for SurfaceToolMoment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Call { name } => write!(formatter, "call name={name:?}"),
            Self::Update { name } => write!(formatter, "update name={name:?}"),
            Self::Result { standing } => write!(formatter, "result standing={standing}"),
        }
    }
}

/// Vendor knowledge of a tool result.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceToolResult {
    /// The vendor reported success.
    Succeeded,
    /// The vendor reported failure.
    Failed,
    /// The vendor supplied no status.
    Unreported,
}

impl From<ToolResult> for SurfaceToolResult {
    fn from(result: ToolResult) -> Self {
        match result {
            ToolResult::Succeeded => Self::Succeeded,
            ToolResult::Failed => Self::Failed,
            ToolResult::Unreported => Self::Unreported,
        }
    }
}

impl fmt::Display for SurfaceToolResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Unreported => "unreported",
        })
    }
}

/// Stable reason a complete record remained untyped.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case", deny_unknown_fields)]
pub enum SurfaceResidue {
    /// A required field was absent.
    Absent {
        /// Missing field.
        field: String,
    },
    /// The decoder did not own the discriminator.
    Unknown {
        /// Observed discriminator.
        discriminator: String,
    },
    /// A field had the wrong representation.
    Malformed {
        /// Malformed field.
        field: String,
        /// Expected representation.
        expected: String,
    },
    /// JSON ended before one complete value was available.
    Truncated {},
    /// A current envelope carried another dialect generation.
    VersionDrift {
        /// Observed discriminator.
        discriminator: String,
    },
}

impl From<&ResidueKind> for SurfaceResidue {
    fn from(kind: &ResidueKind) -> Self {
        match kind {
            ResidueKind::Absent { field } => Self::Absent {
                field: (*field).to_owned(),
            },
            ResidueKind::Unknown { discriminator } => Self::Unknown {
                discriminator: discriminator.clone(),
            },
            ResidueKind::Malformed { field, expected } => Self::Malformed {
                field: (*field).to_owned(),
                expected: (*expected).to_owned(),
            },
            ResidueKind::Truncated => Self::Truncated {},
            ResidueKind::VersionDrift { discriminator } => Self::VersionDrift {
                discriminator: discriminator.clone(),
            },
        }
    }
}

impl fmt::Display for SurfaceResidue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absent { field } => write!(formatter, "absent field={field}"),
            Self::Unknown { discriminator } => {
                write!(formatter, "unknown discriminator={discriminator:?}")
            }
            Self::Malformed { field, expected } => {
                write!(formatter, "malformed field={field} expected={expected}")
            }
            Self::Truncated {} => formatter.write_str("truncated"),
            Self::VersionDrift { discriminator } => {
                write!(formatter, "version_drift discriminator={discriminator:?}")
            }
        }
    }
}

/// One visible, accounted listening result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Observation {
    /// A current decoder emitted a typed event.
    Event {
        /// Lane owning the session.
        lane: String,
        /// Decoder spelling.
        dialect: Dialect,
        /// Preserved tape.
        tape: PathBuf,
        /// Exact ordered record spans.
        spans: Vec<Span>,
        /// Current typed payload.
        event: SurfaceEvent,
    },
    /// Complete or terminal bytes remained untyped.
    Residue {
        /// Lane owning the session.
        lane: String,
        /// Decoder spelling.
        dialect: Dialect,
        /// Preserved tape.
        tape: PathBuf,
        /// Exact ordered accounted spans.
        spans: Vec<Span>,
        /// Closed residue reason.
        residue: SurfaceResidue,
    },
    /// An active member ends in a record that may still grow.
    Pending {
        /// Lane owning the session.
        lane: String,
        /// Decoder spelling.
        dialect: Dialect,
        /// Preserved tape.
        tape: PathBuf,
        /// Ordered bytes waiting for their record delimiter.
        spans: Vec<Span>,
    },
    /// A selected root or session currently contains no durable record.
    Absent {
        /// What was inspected.
        selector: String,
    },
    /// A named lane, locus, or path did not resolve.
    Missing {
        /// Exact selector spelling.
        selector: String,
        /// Observable reason.
        detail: String,
    },
    /// A locus selector named more than one possible tape.
    Ambiguous {
        /// Exact selector spelling.
        selector: String,
        /// Candidate tape paths in deterministic order.
        candidates: Vec<PathBuf>,
    },
    /// Session activity still lacks one expected current event kind.
    Canary {
        /// Lane owning the session.
        lane: String,
        /// Decoder spelling.
        dialect: Dialect,
        /// Preserved tape.
        tape: PathBuf,
        /// Missing event kind.
        expected: EventKind,
    },
}

/// One typed stream result rendered by either CLI leaf.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StreamOutcome {
    /// Producing surface.
    pub mode: Mode,
    /// Selected observation root.
    pub root: PathBuf,
    /// Ordered accounted results.
    pub observations: Vec<Observation>,
}

impl StreamOutcome {
    /// Whether this wakeup discovered anything to render.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.observations.is_empty()
    }
}

impl fmt::Display for StreamOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for observation in &self.observations {
            match observation {
                Observation::Event {
                    lane,
                    dialect,
                    tape,
                    spans,
                    event,
                } => writeln!(
                    formatter,
                    "event lane={lane} dialect={} tape={} spans={} kind={} detail={event}",
                    dialect.as_str(),
                    tape.display(),
                    display_spans(spans),
                    event.kind().as_str(),
                )?,
                Observation::Residue {
                    lane,
                    dialect,
                    tape,
                    spans,
                    residue,
                } => writeln!(
                    formatter,
                    "residue lane={lane} dialect={} tape={} spans={} detail={residue}",
                    dialect.as_str(),
                    tape.display(),
                    display_spans(spans),
                )?,
                Observation::Pending {
                    lane,
                    dialect,
                    tape,
                    spans,
                } => writeln!(
                    formatter,
                    "pending lane={lane} dialect={} tape={} spans={}",
                    dialect.as_str(),
                    tape.display(),
                    display_spans(spans),
                )?,
                Observation::Absent { selector } => {
                    writeln!(formatter, "absent selector={selector}")?;
                }
                Observation::Missing { selector, detail } => {
                    writeln!(formatter, "missing selector={selector} detail={detail}")?;
                }
                Observation::Ambiguous {
                    selector,
                    candidates,
                } => writeln!(
                    formatter,
                    "ambiguous selector={selector} candidates={}",
                    candidates
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                )?,
                Observation::Canary {
                    lane,
                    dialect,
                    tape,
                    expected,
                } => writeln!(
                    formatter,
                    "canary lane={lane} dialect={} tape={} expected={}",
                    dialect.as_str(),
                    tape.display(),
                    expected.as_str(),
                )?,
            }
        }
        Ok(())
    }
}

fn display_spans(spans: &[Span]) -> String {
    spans
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// One finite retro selector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RetroSelector {
    /// Every preserved session for one lane.
    Lane(String),
    /// One exact span, admitted only when the selected root names one session.
    Locus(Span),
    /// One session tape or a root containing sessions.
    Path(PathBuf),
}

impl RetroSelector {
    /// Classify one CLI operand without trying several meanings in sequence.
    ///
    /// `M:O:L` and `member M offset O length L` are loci. A value with a path
    /// component is a path whether or not it exists. Every other value is a
    /// lane. This grammar makes a missing path stay a missing path rather than
    /// silently becoming a lane search.
    ///
    /// # Errors
    ///
    /// Returns [`SurfaceError::Selector`] when a locus-shaped value contains
    /// a non-number or the long form is incomplete.
    pub fn parse(value: &str) -> Result<Self, SurfaceError> {
        if value.contains(':') {
            if value.matches(':').count() != 2 {
                return Err(SurfaceError::Selector(value.to_owned()));
            }
            let numbers = value
                .split(':')
                .map(str::parse::<u64>)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| SurfaceError::Selector(value.to_owned()))?;
            return Ok(Self::Locus(Span {
                member: numbers[0],
                offset: numbers[1],
                length: numbers[2],
            }));
        }

        let words = value.split_whitespace().collect::<Vec<_>>();
        if words.first() == Some(&"member") {
            let ["member", member, "offset", offset, "length", length] = words.as_slice() else {
                return Err(SurfaceError::Selector(value.to_owned()));
            };
            return Ok(Self::Locus(Span {
                member: member
                    .parse()
                    .map_err(|_| SurfaceError::Selector(value.to_owned()))?,
                offset: offset
                    .parse()
                    .map_err(|_| SurfaceError::Selector(value.to_owned()))?,
                length: length
                    .parse()
                    .map_err(|_| SurfaceError::Selector(value.to_owned()))?,
            }));
        }

        let path = Path::new(value);
        if path.components().count() > 1
            || matches!(
                path.components().next(),
                Some(Component::RootDir | Component::CurDir | Component::ParentDir)
            )
        {
            return Ok(Self::Path(path.to_path_buf()));
        }
        Ok(Self::Lane(value.to_owned()))
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Session {
    lane: String,
    dialect: Dialect,
    tape: PathBuf,
}

impl Session {
    const fn decoder(&self) -> Dialect {
        self.dialect
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Cursor {
    member: u64,
    offset: u64,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            member: 1,
            offset: 0,
        }
    }
}

/// Cursor-following observation over one selected root.
#[derive(Debug)]
pub struct Watch {
    root: PathBuf,
    cursors: BTreeMap<PathBuf, Cursor>,
    seen: BTreeMap<PathBuf, BTreeSet<EventKind>>,
}

impl Watch {
    /// Begin a read-only cursor view. No filesystem access occurs until poll.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            cursors: BTreeMap::new(),
            seen: BTreeMap::new(),
        }
    }

    /// Reconcile every current session with its durable cursor once.
    ///
    /// Calling this after any filesystem notification is sufficient; the
    /// event's path and multiplicity are deliberately ignored because the tape
    /// bounds, not the notification queue, decide what has arrived.
    ///
    /// # Errors
    ///
    /// Returns a typed surface failure when discovery or tape reads fail, or
    /// when append-only member bounds retreat behind an emitted cursor.
    pub fn poll(&mut self) -> Result<StreamOutcome, SurfaceError> {
        let sessions = discover(&self.root)?;
        let mut observations = Vec::new();
        if sessions.is_empty() {
            observations.push(Observation::Absent {
                selector: self.root.display().to_string(),
            });
        }

        for session in sessions {
            let cursor = self.cursors.entry(session.tape.clone()).or_default();
            let scan = scan(&session, cursor, false)?;
            if scan.observations.is_empty() && scan.extents_empty {
                observations.push(Observation::Absent {
                    selector: session.tape.display().to_string(),
                });
            }

            if !scan.decoded.is_empty() {
                let seen = self.seen.entry(session.tape.clone()).or_default();
                seen.extend(
                    scan.decoded
                        .iter()
                        .flat_map(Decoded::events)
                        .map(|event| event.kind()),
                );
                observations.extend(
                    EventKind::ALL
                        .into_iter()
                        .filter(|kind| !seen.contains(kind))
                        .map(|kind| Observation::Canary {
                            lane: session.lane.clone(),
                            dialect: session.dialect,
                            tape: session.tape.clone(),
                            expected: kind,
                        }),
                );
            }
            observations.extend(scan.observations);
        }

        Ok(StreamOutcome {
            mode: Mode::Watch,
            root: self.root.clone(),
            observations,
        })
    }
}

/// Replay exactly the sessions or span selected beneath `root`.
///
/// # Errors
///
/// Returns a typed surface failure when discovery or tape reads fail. A named
/// lane, path, or locus that is absent is an [`Observation::Missing`] inside a
/// successful stream outcome, because absence is evidence rather than an I/O
/// defect.
pub fn retro(
    root: impl Into<PathBuf>,
    selector: RetroSelector,
) -> Result<StreamOutcome, SurfaceError> {
    let root = root.into();
    let selector_text = selector_text(&selector);
    let mut sessions = match &selector {
        RetroSelector::Lane(lane) => discover(&root)?
            .into_iter()
            .filter(|session| &session.lane == lane)
            .collect::<Vec<_>>(),
        RetroSelector::Path(path) => discover(path)?,
        RetroSelector::Locus(_) => discover(&root)?,
    };
    sessions.sort();

    let mut observations = Vec::new();
    if sessions.is_empty() {
        observations.push(Observation::Missing {
            selector: selector_text,
            detail: "no preserved session tape resolved".to_owned(),
        });
        return Ok(StreamOutcome {
            mode: Mode::Retro,
            root,
            observations,
        });
    }

    if let RetroSelector::Locus(span) = selector {
        if sessions.len() != 1 {
            observations.push(Observation::Ambiguous {
                selector: span.to_string(),
                candidates: sessions.into_iter().map(|session| session.tape).collect(),
            });
        } else {
            observations.extend(replay_locus(&sessions[0], span)?);
        }
    } else {
        for session in sessions {
            let mut cursor = Cursor::default();
            let scan = scan(&session, &mut cursor, true)?;
            if scan.observations.is_empty() && scan.extents_empty {
                observations.push(Observation::Absent {
                    selector: session.tape.display().to_string(),
                });
            }
            observations.extend(scan.observations);
        }
    }

    Ok(StreamOutcome {
        mode: Mode::Retro,
        root,
        observations,
    })
}

struct Scan {
    observations: Vec<Observation>,
    decoded: Vec<Decoded>,
    extents_empty: bool,
}

fn scan(
    session: &Session,
    cursor: &mut Cursor,
    finish_partial: bool,
) -> Result<Scan, SurfaceError> {
    let tape = Tape::open_existing(&session.tape).map_err(|source| SurfaceError::Tape {
        path: session.tape.clone(),
        source,
    })?;
    let extents = tape.extents().map_err(|source| SurfaceError::Tape {
        path: session.tape.clone(),
        source,
    })?;
    let extents_empty = extents.is_empty();
    let mut observations = Vec::new();
    let mut decoded = Vec::new();
    let mut record = Vec::new();
    let mut record_spans = Vec::new();

    for extent in &extents {
        scan_extent(
            session,
            &tape,
            cursor,
            extent,
            &mut record,
            &mut record_spans,
            &mut observations,
            &mut decoded,
        )?;
    }

    finish_partial_record(
        session,
        cursor,
        &extents,
        finish_partial,
        &record,
        record_spans,
        &mut observations,
        &mut decoded,
    );

    Ok(Scan {
        observations,
        decoded,
        extents_empty,
    })
}

#[allow(clippy::too_many_arguments)]
fn scan_extent(
    session: &Session,
    tape: &Tape,
    cursor: &mut Cursor,
    extent: &tape::Extent,
    record: &mut Vec<u8>,
    record_spans: &mut Vec<Span>,
    observations: &mut Vec<Observation>,
    decoded: &mut Vec<Decoded>,
) -> Result<(), SurfaceError> {
    if extent.member < cursor.member {
        return Ok(());
    }
    let start = if extent.member == cursor.member {
        cursor.offset
    } else {
        0
    };
    if start > extent.length {
        return Err(SurfaceError::CursorRetreated {
            tape: session.tape.clone(),
            member: extent.member,
            cursor: start,
            observed: extent.length,
        });
    }
    if start == extent.length {
        if extent.sealed {
            *cursor = Cursor {
                member: extent.member + 1,
                offset: 0,
            };
        }
        return Ok(());
    }

    let locus = Locus::new(extent.member, start, extent.length - start);
    let bytes = tape.read(locus).map_err(|source| SurfaceError::Tape {
        path: session.tape.clone(),
        source,
    })?;
    let mut fragment_start = 0_usize;
    for (position, byte) in bytes.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        let end = position + 1;
        append_record_fragment(
            record,
            record_spans,
            extent.member,
            start + fragment_start as u64,
            &bytes[fragment_start..end],
        );
        let observation = decode_record(
            session,
            record,
            synthetic_locus(record_spans, record.len()),
            record_spans,
        )?;
        decoded.push(observation.0);
        observations.extend(observation.1);
        cursor.member = extent.member;
        cursor.offset = start + end as u64;
        record.clear();
        record_spans.clear();
        fragment_start = end;
    }

    if fragment_start < bytes.len() {
        append_record_fragment(
            record,
            record_spans,
            extent.member,
            start + fragment_start as u64,
            &bytes[fragment_start..],
        );
    }

    if record.is_empty()
        && extent.sealed
        && cursor.member == extent.member
        && cursor.offset == extent.length
    {
        *cursor = Cursor {
            member: extent.member + 1,
            offset: 0,
        };
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn finish_partial_record(
    session: &Session,
    cursor: &mut Cursor,
    extents: &[tape::Extent],
    finish_partial: bool,
    record: &[u8],
    record_spans: Vec<Span>,
    observations: &mut Vec<Observation>,
    decoded: &mut Vec<Decoded>,
) {
    if record.is_empty() {
        return;
    }
    if !finish_partial {
        observations.push(Observation::Pending {
            lane: session.lane.clone(),
            dialect: session.dialect,
            tape: session.tape.clone(),
            spans: record_spans,
        });
        return;
    }

    let residue = Decoded::Residue(Residue::new(
        session.decoder(),
        synthetic_locus(&record_spans, record.len()),
        ResidueKind::Truncated,
    ));
    observations.extend(observe(session, &residue, &record_spans));
    decoded.push(residue);
    if let Some(last) = record_spans.last() {
        cursor.member = last.member;
        cursor.offset = last.offset + last.length;
        if extents
            .iter()
            .find(|extent| extent.member == last.member)
            .is_some_and(|extent| extent.sealed && cursor.offset == extent.length)
        {
            *cursor = Cursor {
                member: last.member + 1,
                offset: 0,
            };
        }
    }
}

fn decode_record(
    session: &Session,
    bytes: &[u8],
    locus: Locus,
    spans: &[Span],
) -> Result<(Decoded, Vec<Observation>), SurfaceError> {
    let decoded =
        decode(session.decoder(), bytes, locus).map_err(|error| SurfaceError::DecodeContract {
            tape: session.tape.clone(),
            detail: error.to_string(),
        })?;
    let observations = observe(session, &decoded, spans);
    Ok((decoded, observations))
}

fn observe(session: &Session, decoded: &Decoded, spans: &[Span]) -> Vec<Observation> {
    match decoded {
        Decoded::Events(events) => events
            .iter()
            .map(|event| Observation::Event {
                lane: session.lane.clone(),
                dialect: session.dialect,
                tape: session.tape.clone(),
                spans: spans.to_vec(),
                event: event.into(),
            })
            .collect(),
        Decoded::Residue(residue) => vec![Observation::Residue {
            lane: session.lane.clone(),
            dialect: session.dialect,
            tape: session.tape.clone(),
            spans: spans.to_vec(),
            residue: (&residue.kind).into(),
        }],
    }
}

fn append_record_fragment(
    record: &mut Vec<u8>,
    spans: &mut Vec<Span>,
    member: u64,
    offset: u64,
    fragment: &[u8],
) {
    if fragment.is_empty() {
        return;
    }
    record.extend_from_slice(fragment);
    let length = fragment.len() as u64;
    if let Some(last) = spans.last_mut()
        && last.member == member
        && last.offset.checked_add(last.length) == Some(offset)
    {
        last.length += length;
    } else {
        spans.push(Span {
            member,
            offset,
            length,
        });
    }
}

fn synthetic_locus(spans: &[Span], length: usize) -> Locus {
    let first = spans
        .first()
        .expect("a record with bytes has at least one source span");
    // Decoders require one locus whose length matches their input. A record
    // crossing members has no such physical locus, so this value is only the
    // decoder's length witness; public observations retain the exact spans.
    Locus::new(first.member, first.offset, length as u64)
}

fn replay_locus(session: &Session, span: Span) -> Result<Vec<Observation>, SurfaceError> {
    let tape = Tape::open_existing(&session.tape).map_err(|source| SurfaceError::Tape {
        path: session.tape.clone(),
        source,
    })?;
    let locus: Locus = span.into();
    let bytes = match tape.read(locus) {
        Ok(bytes) => bytes,
        Err(error) if error.refusal().is_some() => {
            return Ok(vec![Observation::Missing {
                selector: span.to_string(),
                detail: error.to_string(),
            }]);
        }
        Err(source) => {
            return Err(SurfaceError::Tape {
                path: session.tape.clone(),
                source,
            });
        }
    };
    let (_, observations) = decode_record(session, &bytes, locus, &[span])?;
    Ok(observations)
}

fn discover(root: &Path) -> Result<Vec<Session>, SurfaceError> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(SurfaceError::Io {
                operation: "inspect observation root",
                path: root.to_path_buf(),
                source,
            });
        }
    };
    if !metadata.is_dir() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    discover_beneath(root, &mut sessions)?;
    sessions.sort();
    Ok(sessions)
}

fn discover_beneath(path: &Path, sessions: &mut Vec<Session>) -> Result<(), SurfaceError> {
    if let Some(dialect) = dialect_name(path.file_name()) {
        let Some(lane) = path
            .parent()
            .and_then(Path::file_name)
            .and_then(OsStr::to_str)
        else {
            return Err(SurfaceError::NonUtf8Lane(path.to_path_buf()));
        };
        sessions.push(Session {
            lane: lane.to_owned(),
            dialect,
            tape: path.to_path_buf(),
        });
        return Ok(());
    }

    let mut entries = fs::read_dir(path)
        .map_err(|source| SurfaceError::Io {
            operation: "read observation root",
            path: path.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| SurfaceError::Io {
            operation: "read observation entry",
            path: path.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(std::fs::DirEntry::path);
    for entry in entries {
        let file_type = entry.file_type().map_err(|source| SurfaceError::Io {
            operation: "inspect observation entry",
            path: entry.path(),
            source,
        })?;
        if file_type.is_dir() && !file_type.is_symlink() {
            discover_beneath(&entry.path(), sessions)?;
        }
    }
    Ok(())
}

fn dialect_name(name: Option<&OsStr>) -> Option<Dialect> {
    match name.and_then(OsStr::to_str) {
        Some("claude") => Some(Dialect::Claude),
        Some("codex") => Some(Dialect::Codex),
        _ => None,
    }
}

fn selector_text(selector: &RetroSelector) -> String {
    match selector {
        RetroSelector::Lane(lane) => lane.clone(),
        RetroSelector::Locus(locus) => locus.to_string(),
        RetroSelector::Path(path) => path.display().to_string(),
    }
}

/// A daemonless surface could not read its selected preserved state.
#[derive(Debug)]
pub enum SurfaceError {
    /// Filesystem observation failed.
    Io {
        /// Attempted operation.
        operation: &'static str,
        /// Path involved.
        path: PathBuf,
        /// Underlying failure.
        source: io::Error,
    },
    /// Tape inspection or resolution failed.
    Tape {
        /// Tape involved.
        path: PathBuf,
        /// Typed tape failure.
        source: tape::Error,
    },
    /// A lane directory has no stable UTF-8 spelling.
    NonUtf8Lane(PathBuf),
    /// A locus-shaped selector was not the current grammar.
    Selector(String),
    /// Durable append-only bounds moved behind an emitted cursor.
    CursorRetreated {
        /// Tape involved.
        tape: PathBuf,
        /// Member involved.
        member: u64,
        /// Already emitted boundary.
        cursor: u64,
        /// Newly observed boundary.
        observed: u64,
    },
    /// The decoder was called with a locus that did not cover its bytes.
    DecodeContract {
        /// Tape involved.
        tape: PathBuf,
        /// Decoder report.
        detail: String,
    },
}

impl fmt::Display for SurfaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "failed to {operation} at {}: {source}",
                path.display()
            ),
            Self::Tape { path, source } => {
                write!(
                    formatter,
                    "failed to read tape at {}: {source}",
                    path.display()
                )
            }
            Self::NonUtf8Lane(path) => write!(
                formatter,
                "session lane above {} has no UTF-8 spelling",
                path.display()
            ),
            Self::Selector(selector) => write!(
                formatter,
                "retro selector {selector:?} is neither a lane, path, nor current locus"
            ),
            Self::CursorRetreated {
                tape,
                member,
                cursor,
                observed,
            } => write!(
                formatter,
                "append-only tape {} member {member} retreated from cursor {cursor} to {observed}",
                tape.display()
            ),
            Self::DecodeContract { tape, detail } => write!(
                formatter,
                "decoder contract failed for tape {}: {detail}",
                tape.display()
            ),
        }
    }
}

impl Error for SurfaceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Tape { source, .. } => Some(source),
            Self::NonUtf8Lane(_)
            | Self::Selector(_)
            | Self::CursorRetreated { .. }
            | Self::DecodeContract { .. } => None,
        }
    }
}
