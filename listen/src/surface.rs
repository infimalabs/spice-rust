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

use serde::{Deserialize, Serialize};
use tape::{Locus, Tape};

use crate::{Decoded, Dialect, EventKind, Residue, ResidueKind, decode};

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

/// One visible, accounted listening result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Observation {
    /// A current decoder emitted a typed event.
    Event {
        /// Lane owning the session.
        lane: String,
        /// Decoder spelling.
        dialect: String,
        /// Preserved tape.
        tape: PathBuf,
        /// Exact record bytes.
        locus: Span,
        /// Closed event kind.
        kind: String,
        /// Current typed payload, rendered without losing its variant.
        detail: String,
    },
    /// Complete or terminal bytes remained untyped.
    Residue {
        /// Lane owning the session.
        lane: String,
        /// Decoder spelling.
        dialect: String,
        /// Preserved tape.
        tape: PathBuf,
        /// Exact accounted bytes.
        locus: Span,
        /// Closed residue reason.
        reason: String,
    },
    /// An active member ends in a record that may still grow.
    Pending {
        /// Lane owning the session.
        lane: String,
        /// Decoder spelling.
        dialect: String,
        /// Preserved tape.
        tape: PathBuf,
        /// Bytes waiting for their record delimiter.
        locus: Span,
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
        dialect: String,
        /// Preserved tape.
        tape: PathBuf,
        /// Missing event kind.
        expected: String,
    },
}

/// One typed stream result rendered by either CLI leaf.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
                    locus,
                    kind,
                    detail,
                } => writeln!(
                    formatter,
                    "event lane={lane} dialect={dialect} tape={} locus={locus} kind={kind} detail={detail}",
                    tape.display(),
                )?,
                Observation::Residue {
                    lane,
                    dialect,
                    tape,
                    locus,
                    reason,
                } => writeln!(
                    formatter,
                    "residue lane={lane} dialect={dialect} tape={} locus={locus} reason={reason}",
                    tape.display()
                )?,
                Observation::Pending {
                    lane,
                    dialect,
                    tape,
                    locus,
                } => writeln!(
                    formatter,
                    "pending lane={lane} dialect={dialect} tape={} locus={locus}",
                    tape.display()
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
                    "canary lane={lane} dialect={dialect} tape={} expected={expected}",
                    tape.display()
                )?,
            }
        }
        Ok(())
    }
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
                            dialect: session.dialect.as_str().to_owned(),
                            tape: session.tape.clone(),
                            expected: kind.as_str().to_owned(),
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

    for extent in extents {
        if extent.member < cursor.member {
            continue;
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
            continue;
        }

        let locus = Locus::new(extent.member, start, extent.length - start);
        let bytes = tape.read(locus).map_err(|source| SurfaceError::Tape {
            path: session.tape.clone(),
            source,
        })?;
        let mut record_start = 0_usize;
        for (position, byte) in bytes.iter().enumerate() {
            if *byte != b'\n' {
                continue;
            }
            let end = position + 1;
            let record = &bytes[record_start..end];
            let offset = start + record_start as u64;
            let record_locus = Locus::new(extent.member, offset, record.len() as u64);
            let observation = decode_record(session, record, record_locus)?;
            decoded.push(observation.0);
            observations.extend(observation.1);
            cursor.member = extent.member;
            cursor.offset = start + end as u64;
            record_start = end;
        }

        if record_start < bytes.len() {
            let trailing = &bytes[record_start..];
            let offset = start + record_start as u64;
            let trailing_locus = Locus::new(extent.member, offset, trailing.len() as u64);
            if finish_partial || extent.sealed {
                let residue = Decoded::Residue(Residue::new(
                    session.decoder(),
                    trailing_locus,
                    ResidueKind::Truncated,
                ));
                observations.extend(observe(session, &residue));
                decoded.push(residue);
                cursor.member = extent.member;
                cursor.offset = extent.length;
            } else {
                observations.push(Observation::Pending {
                    lane: session.lane.clone(),
                    dialect: session.dialect.as_str().to_owned(),
                    tape: session.tape.clone(),
                    locus: trailing_locus.into(),
                });
            }
        }

        if extent.sealed && cursor.offset == extent.length {
            *cursor = Cursor {
                member: extent.member + 1,
                offset: 0,
            };
        }
    }

    Ok(Scan {
        observations,
        decoded,
        extents_empty,
    })
}

fn decode_record(
    session: &Session,
    bytes: &[u8],
    locus: Locus,
) -> Result<(Decoded, Vec<Observation>), SurfaceError> {
    let decoded =
        decode(session.decoder(), bytes, locus).map_err(|error| SurfaceError::DecodeContract {
            tape: session.tape.clone(),
            detail: error.to_string(),
        })?;
    let observations = observe(session, &decoded);
    Ok((decoded, observations))
}

fn observe(session: &Session, decoded: &Decoded) -> Vec<Observation> {
    match decoded {
        Decoded::Events(events) => events
            .iter()
            .map(|event| Observation::Event {
                lane: session.lane.clone(),
                dialect: session.dialect.as_str().to_owned(),
                tape: session.tape.clone(),
                locus: event.locus().into(),
                kind: event.kind().as_str().to_owned(),
                detail: format!("{event:?}"),
            })
            .collect(),
        Decoded::Residue(residue) => vec![Observation::Residue {
            lane: session.lane.clone(),
            dialect: session.dialect.as_str().to_owned(),
            tape: session.tape.clone(),
            locus: residue.locus.into(),
            reason: residue_reason(&residue.kind),
        }],
    }
}

fn residue_reason(kind: &ResidueKind) -> String {
    match kind {
        ResidueKind::Absent { field } => format!("absent:{field}"),
        ResidueKind::Unknown { discriminator } => format!("unknown:{discriminator}"),
        ResidueKind::Malformed { field, expected } => {
            format!("malformed:{field}:{expected}")
        }
        ResidueKind::Truncated => "truncated".to_owned(),
        ResidueKind::VersionDrift { discriminator } => {
            format!("version-drift:{discriminator}")
        }
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
    let (_, observations) = decode_record(session, &bytes, locus)?;
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
