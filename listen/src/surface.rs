//! Daemonless observation and replay over owned transcript tape.
//!
//! A selected root contains sessions at exactly `<root>/<lane>/<vendor>`; the
//! supplied vendor map binds that final component to the decoder contract and
//! the parent component is the lane spelling. A root that is itself a
//! `<lane>/<vendor>` tape directory selects that one session. A vendor-named
//! directory at any other depth is not a session. Discovery follows real
//! directories only and never guesses a dialect from transcript contents.
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
    Checkpoint, Decoded, Dialect, Event, EventKind, ProseRole, ReasoningContent, RecordPayload,
    ResidueKind, Scan, ScanError, Span, ToolMoment, ToolResult, decode, scan as scan_records,
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
        /// Admitted media carried beside the prose.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        media: Vec<wire::Media>,
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
        /// Completed vendor turns represented by this event.
        turns: u64,
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
            Event::Prose(event) | Event::ProseEvidence(event) => Self::Prose {
                text: event.text.clone(),
                phase: event.phase.clone(),
                media: event.media.clone(),
            },
            Event::Reasoning(event) => Self::Reasoning {
                content: (&event.content).into(),
            },
            Event::Tool(event) => Self::Tool {
                call_id: event.call_id.clone(),
                moment: (&event.moment).into(),
            },
            Event::Usage(event) => Self::Usage {
                turns: event.turns,
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
            Self::Prose { text, phase, media } => {
                write!(
                    formatter,
                    "prose text={text:?} phase={phase:?} media={}",
                    media.len(),
                )
            }
            Self::Reasoning { content } => write!(formatter, "reasoning {content}"),
            Self::Tool { call_id, moment } => {
                write!(formatter, "tool call_id={call_id:?} {moment}")
            }
            Self::Usage {
                turns,
                input_tokens,
                cached_input_tokens,
                cache_write_input_tokens,
                output_tokens,
                reasoning_output_tokens,
            } => write!(
                formatter,
                concat!(
                    "usage turns={} input={} cached={} cache_write={} ",
                    "output={} reasoning={:?}"
                ),
                turns,
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
}

impl From<&ReasoningContent> for SurfaceReasoning {
    fn from(content: &ReasoningContent) -> Self {
        match content {
            ReasoningContent::Plain(lines) => Self::Plain {
                lines: lines.clone(),
            },
        }
    }
}

impl fmt::Display for SurfaceReasoning {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plain { lines } => write!(formatter, "plain lines={lines:?}"),
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
        /// Exact vendor call arguments.
        arguments: String,
    },
    /// A running call published updated state.
    Update {
        /// Tool name.
        name: String,
        /// Exact vendor update arguments.
        arguments: String,
    },
    /// A result reports its standing.
    Result {
        /// Vendor knowledge of completion.
        standing: SurfaceToolResult,
        /// Rendered non-media result body, when recorded.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        output: Option<String>,
        /// Process exit code, when recorded.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        exit_code: Option<i32>,
        /// Admitted media carried by the result.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        media: Vec<wire::Media>,
    },
}

impl From<&ToolMoment> for SurfaceToolMoment {
    fn from(moment: &ToolMoment) -> Self {
        match moment {
            ToolMoment::Call { name, arguments } => Self::Call {
                name: name.clone(),
                arguments: arguments.clone(),
            },
            ToolMoment::Update { name, arguments } => Self::Update {
                name: name.clone(),
                arguments: arguments.clone(),
            },
            ToolMoment::Result(outcome) => Self::Result {
                standing: outcome.result.into(),
                output: outcome.output.clone(),
                exit_code: outcome.exit_code,
                media: outcome.media.clone(),
            },
        }
    }
}

impl fmt::Display for SurfaceToolMoment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Call { name, arguments } => {
                write!(formatter, "call name={name:?} arguments={arguments:?}")
            }
            Self::Update { name, arguments } => {
                write!(formatter, "update name={name:?} arguments={arguments:?}")
            }
            Self::Result {
                standing,
                output,
                exit_code,
                media,
            } => write!(
                formatter,
                "result standing={standing} output={output:?} exit_code={exit_code:?} media={}",
                media.len(),
            ),
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

/// Cursor-following observation over one selected root.
#[derive(Debug)]
pub struct Watch {
    root: PathBuf,
    dialects: Vec<(&'static str, Dialect)>,
    checkpoints: BTreeMap<PathBuf, Checkpoint>,
    seen: BTreeMap<PathBuf, BTreeSet<EventKind>>,
}

impl Watch {
    /// Begin a read-only cursor view. No filesystem access occurs until poll.
    #[must_use]
    pub fn new(
        root: impl Into<PathBuf>,
        dialects: impl IntoIterator<Item = (&'static str, Dialect)>,
    ) -> Self {
        Self {
            root: root.into(),
            dialects: dialects.into_iter().collect(),
            checkpoints: BTreeMap::new(),
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
        let sessions = discover(&self.root, &self.dialects)?;
        let mut observations = Vec::new();
        if sessions.is_empty() {
            observations.push(Observation::Absent {
                selector: self.root.display().to_string(),
            });
        }

        for session in sessions {
            let scanned = scan_records(
                &session.tape,
                session.decoder(),
                self.checkpoints.get(&session.tape),
            )
            .map_err(|source| SurfaceError::Scan {
                path: session.tape.clone(),
                source,
            })?;
            self.checkpoints
                .insert(session.tape.clone(), scanned.checkpoint.clone());
            let surfaced = surface_scan(&session, &scanned, false);
            if surfaced.observations.is_empty() && scanned.extents_empty {
                observations.push(Observation::Absent {
                    selector: session.tape.display().to_string(),
                });
            }

            if surfaced.accounted {
                let seen = self.seen.entry(session.tape.clone()).or_default();
                seen.extend(surfaced.event_kinds);
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
            observations.extend(surfaced.observations);
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
    dialects: &[(&'static str, Dialect)],
) -> Result<StreamOutcome, SurfaceError> {
    let root = root.into();
    let selector_text = selector_text(&selector);
    let mut sessions = match &selector {
        RetroSelector::Lane(lane) => discover(&root, dialects)?
            .into_iter()
            .filter(|session| &session.lane == lane)
            .collect::<Vec<_>>(),
        RetroSelector::Path(path) => discover(path, dialects)?,
        RetroSelector::Locus(_) => discover(&root, dialects)?,
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
            let scanned =
                scan_records(&session.tape, session.decoder(), None).map_err(|source| {
                    SurfaceError::Scan {
                        path: session.tape.clone(),
                        source,
                    }
                })?;
            let surfaced = surface_scan(&session, &scanned, true);
            if surfaced.observations.is_empty() && scanned.extents_empty {
                observations.push(Observation::Absent {
                    selector: session.tape.display().to_string(),
                });
            }
            observations.extend(surfaced.observations);
        }
    }

    Ok(StreamOutcome {
        mode: Mode::Retro,
        root,
        observations,
    })
}

struct SurfacedScan {
    observations: Vec<Observation>,
    event_kinds: Vec<EventKind>,
    accounted: bool,
}

fn surface_scan(session: &Session, scan: &Scan, finish_partial: bool) -> SurfacedScan {
    let mut observations = Vec::new();
    let mut event_kinds = Vec::new();
    for record in &scan.records {
        match &record.payload {
            RecordPayload::Events(events) => {
                for sourced in events {
                    event_kinds.push(sourced.event.kind());
                    observations.push(Observation::Event {
                        lane: session.lane.clone(),
                        dialect: session.dialect,
                        tape: session.tape.clone(),
                        spans: sourced.spans.clone(),
                        event: (&sourced.event).into(),
                    });
                }
            }
            RecordPayload::Residue(sourced) => observations.push(Observation::Residue {
                lane: session.lane.clone(),
                dialect: session.dialect,
                tape: session.tape.clone(),
                spans: sourced.spans.clone(),
                residue: (&sourced.residue.kind).into(),
            }),
        }
    }

    if !scan.pending.is_empty() {
        if finish_partial {
            observations.push(Observation::Residue {
                lane: session.lane.clone(),
                dialect: session.dialect,
                tape: session.tape.clone(),
                spans: scan.pending.clone(),
                residue: SurfaceResidue::Truncated {},
            });
        } else {
            observations.push(Observation::Pending {
                lane: session.lane.clone(),
                dialect: session.dialect,
                tape: session.tape.clone(),
                spans: scan.pending.clone(),
            });
        }
    }

    SurfacedScan {
        observations,
        event_kinds,
        accounted: !scan.records.is_empty() || (finish_partial && !scan.pending.is_empty()),
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
        Decoded::Events { events, .. } => events
            .iter()
            .filter(
                |event| !matches!(event, Event::Prose(prose) if prose.role == ProseRole::Operator),
            )
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

fn discover(
    root: &Path,
    dialects: &[(&'static str, Dialect)],
) -> Result<Vec<Session>, SurfaceError> {
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
    if let Some(session) = tape_session(root, dialects)? {
        sessions.push(session);
        return Ok(sessions);
    }
    for lane in real_directories(root)? {
        for tape in real_directories(&lane)? {
            sessions.extend(tape_session(&tape, dialects)?);
        }
    }
    sessions.sort();
    Ok(sessions)
}

/// The session a `<lane>/<vendor>` directory names, if its final component is
/// a known vendor.
fn tape_session(
    path: &Path,
    dialects: &[(&'static str, Dialect)],
) -> Result<Option<Session>, SurfaceError> {
    let Some(dialect) = dialect_name(path.file_name(), dialects) else {
        return Ok(None);
    };
    let Some(lane) = path
        .parent()
        .and_then(Path::file_name)
        .and_then(OsStr::to_str)
    else {
        return Err(SurfaceError::NonUtf8Lane(path.to_path_buf()));
    };
    Ok(Some(Session {
        lane: lane.to_owned(),
        dialect,
        tape: path.to_path_buf(),
    }))
}

/// The non-symlink subdirectories of `path`, in path order.
fn real_directories(path: &Path) -> Result<Vec<PathBuf>, SurfaceError> {
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
    let mut directories = Vec::new();
    for entry in entries {
        let file_type = entry.file_type().map_err(|source| SurfaceError::Io {
            operation: "inspect observation entry",
            path: entry.path(),
            source,
        })?;
        if file_type.is_dir() && !file_type.is_symlink() {
            directories.push(entry.path());
        }
    }
    Ok(directories)
}

fn dialect_name(name: Option<&OsStr>, dialects: &[(&'static str, Dialect)]) -> Option<Dialect> {
    let name = name.and_then(OsStr::to_str)?;
    dialects
        .iter()
        .find_map(|(candidate, dialect)| (*candidate == name).then_some(*dialect))
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
    /// Generation-authenticated record scanning failed.
    Scan {
        /// Tape involved.
        path: PathBuf,
        /// Typed resume or tape failure.
        source: ScanError,
    },
    /// A lane directory has no stable UTF-8 spelling.
    NonUtf8Lane(PathBuf),
    /// A locus-shaped selector was not the current grammar.
    Selector(String),
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
            Self::Scan { path, source } => write!(
                formatter,
                "failed to scan tape at {}: {source}",
                path.display()
            ),
            Self::NonUtf8Lane(path) => write!(
                formatter,
                "session lane above {} has no UTF-8 spelling",
                path.display()
            ),
            Self::Selector(selector) => write!(
                formatter,
                "retro selector {selector:?} is neither a lane, path, nor current locus"
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
            Self::Scan { source, .. } => Some(source),
            Self::NonUtf8Lane(_) | Self::Selector(_) | Self::DecodeContract { .. } => None,
        }
    }
}
