//! Startup replay of durable lane tapes into the one Canvas message feed.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use spice::quote::TranscriptSessions;

mod history;
#[cfg(test)]
mod tests;
use history::{HistorySources, history_newest, history_tail, history_tail_error};

/// Fixed newest-record window for one startup tape read.
///
/// Startup seeds every current lane before discovery is published, so each
/// lane reads one window and the pageable history below fills the rest.
const STARTUP_WINDOW_BYTES: u64 = 1024 * 1024;
/// Newest-record window for one historical tape read.
///
/// The predecessor read 8 MiB reverse windows and kept reading older ones
/// until the page held its visible cards; a tool-heavy lane can run for
/// megabytes without one, so a smaller window served pages of nothing.
const HISTORY_WINDOW_BYTES: u64 = 8 * 1024 * 1024;
/// Tape index bytes one scan may read; a member row is tens of bytes.
const TAIL_INDEX_BYTES: u64 = 64 * 1024;
/// Dialect whose admitted record is the longest, and so the one every ceiling
/// shared across lanes and windows is sized from.
const WIDEST_DIALECT: listen::Dialect = listen::Dialect::Omp;
const TAIL_RECORD_LIMIT: usize = 256 * 1024;
/// Bounded reads one scan may spend outside its boundary retreat: the tape
/// index, the extents it enumerates, and each extent the window itself reads.
const TAIL_EXTENT_PASSES: usize = 256;
const TAIL_DURATION: Duration = Duration::from_secs(5);
/// Historical windows chained toward one requested page, sized from the
/// shared [`HISTORY_REQUEST_BYTES`] span.
///
/// A page served short keeps its continuation, so the client asks again from
/// where this request stopped rather than rescanning.
const HISTORY_READ_LIMIT: usize = (HISTORY_REQUEST_BYTES / HISTORY_WINDOW_BYTES) as usize;
const STARTUP_LANE_READ_LIMIT: usize = 8;
const STARTUP_SOURCE_BYTE_LIMIT: u64 =
    startup_source_bytes(WIDEST_DIALECT) * STARTUP_LANE_READ_LIMIT as u64;

/// Longest record one dialect can land on a tape, with the delimiter the tape
/// carries after it.
///
/// OMP capture tees a reassembled logical record: physical RPC frames stay
/// bounded at [`vendor::omp::frame::MAX_FRAME_BYTES`] while the record those
/// chunks carry reaches [`vendor::omp::frame::MAX_REASSEMBLED_BYTES`]. Every
/// other dialect is teed as captured, so its record ends at the physical
/// capture limit. Sizing the allowance per dialect is what keeps one vendor's
/// transport from widening the budget every other lane reads under.
const fn record_bytes(dialect: listen::Dialect) -> u64 {
    let logical = match dialect {
        listen::Dialect::Omp => vendor::omp::frame::MAX_REASSEMBLED_BYTES,
        listen::Dialect::Claude | listen::Dialect::Codex => lane_surface::CAPTURE_LIMIT,
    };
    logical as u64 + 1
}

/// Bytes one scan may spend beyond its window: the tape index it reads to
/// find the newest members, then one longest record, when the window it was
/// asked for begins inside that record.
const fn tail_boundary_bytes(dialect: listen::Dialect) -> u64 {
    TAIL_INDEX_BYTES + record_bytes(dialect)
}

const fn startup_source_bytes(dialect: listen::Dialect) -> u64 {
    STARTUP_WINDOW_BYTES + tail_boundary_bytes(dialect)
}

const fn history_source_bytes(dialect: listen::Dialect) -> u64 {
    HISTORY_WINDOW_BYTES + tail_boundary_bytes(dialect)
}

/// Bounded reads one scan may perform: one for every boundary chunk its
/// source allowance can pay for, since a retreat over one longest record
/// spends exactly that many, plus [`TAIL_EXTENT_PASSES`].
const fn tail_passes(source_bytes: u64) -> usize {
    (source_bytes / listen::BOUNDARY_CHUNK_BYTES + 1) as usize + TAIL_EXTENT_PASSES
}

fn startup_budget(dialect: listen::Dialect) -> listen::TailBudget {
    tail_budget(STARTUP_WINDOW_BYTES, startup_source_bytes(dialect))
}

fn history_budget(dialect: listen::Dialect) -> listen::TailBudget {
    tail_budget(HISTORY_WINDOW_BYTES, history_source_bytes(dialect))
}

fn tail_budget(window_bytes: u64, source_bytes: u64) -> listen::TailBudget {
    listen::TailBudget::new(
        window_bytes,
        source_bytes,
        TAIL_RECORD_LIMIT,
        tail_passes(source_bytes),
        TAIL_DURATION,
    )
    .expect("Canvas tail allowances are positive")
}

/// Source-authenticated per-lane continuation state for repeated backlog reads.
#[derive(Debug, Default)]
pub(in crate::resident) struct BacklogReader {
    cursors: BTreeMap<CanonicalUuid, listen::Checkpoint>,
    startup_work: StartupWork,
    state: MessageState,
    steering: PresentationDirectives,
    sessions: TranscriptSessions,
}

impl BacklogReader {
    pub(in crate::resident) fn indexed() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(in crate::resident) fn from_snapshot(
        snapshot: &journal::FactSnapshot,
    ) -> CommandResult<Self> {
        let steering = PresentationDirectives::from_snapshot(snapshot)
            .map_err(|error| store_fault("canvas::message-steering-invalid", &error))?;
        Ok(Self {
            cursors: BTreeMap::new(),
            startup_work: StartupWork::default(),
            state: MessageState {
                observations: MessageObservations::from_snapshot(snapshot),
                ..MessageState::default()
            },
            steering,
            sessions: TranscriptSessions::from_snapshot(snapshot)
                .map_err(|error| store_fault("canvas::message-sessions-invalid", &error))?,
        })
    }

    pub(in crate::resident) fn into_state(self) -> MessageState {
        self.state
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct StartupWork {
    lanes: usize,
    source_bytes: u64,
    records: usize,
    passes: usize,
}

impl StartupWork {
    /// Whether one more lane reading under `dialect` fits the startup ceiling.
    ///
    /// A lane is admitted against its own allowance, so a dialect whose
    /// records are short keeps its place in the ceiling the widest one sets.
    fn admits_lane(self, dialect: listen::Dialect) -> bool {
        self.lanes < STARTUP_LANE_READ_LIMIT
            && self
                .source_bytes
                .saturating_add(startup_source_bytes(dialect))
                <= STARTUP_SOURCE_BYTE_LIMIT
    }

    fn begin_lane(&mut self) {
        self.lanes = self.lanes.saturating_add(1);
    }

    fn add(&mut self, work: listen::TailWork) {
        self.source_bytes = self.source_bytes.saturating_add(work.source_bytes);
        self.records = self.records.saturating_add(work.records);
        self.passes = self.passes.saturating_add(work.passes);
        debug_assert!(self.lanes <= STARTUP_LANE_READ_LIMIT);
        debug_assert!(self.source_bytes <= STARTUP_SOURCE_BYTE_LIMIT);
    }
}

/// Seed each current lane's bounded Canvas backlog before discovery is published.
///
/// Live capture and restart replay both cross [`publish_agent_event`]. A record
/// captured during startup is replayed or queued live, then deduplicated.
/// Startup work remains independently bounded from pageable history.
#[cfg(test)]
pub(in crate::resident) fn restore_agent_messages(
    paths: &RuntimePaths,
    residency: &wire::ResidencyHead,
    publisher: &canvas::stream::MessagePublisher,
    reader: &mut BacklogReader,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    restore_agent_messages_with(None, paths, residency, publisher, reader, diagnostic)
}

pub(in crate::resident) fn restore_agent_messages_indexed(
    bus: &bus::BusReader,
    paths: &RuntimePaths,
    residency: &wire::ResidencyHead,
    publisher: &canvas::stream::MessagePublisher,
    reader: &mut BacklogReader,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    restore_agent_messages_with(Some(bus), paths, residency, publisher, reader, diagnostic)
}

fn restore_agent_messages_with(
    bus: Option<&bus::BusReader>,
    paths: &RuntimePaths,
    residency: &wire::ResidencyHead,
    publisher: &canvas::stream::MessagePublisher,
    reader: &mut BacklogReader,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    reader.startup_work = StartupWork::default();
    let generation = publisher
        .batch()
        .map_err(|error| store_fault("canvas::message-publish-failed", &error.to_string()))?;
    let mut available = Vec::new();
    for row in &residency.lanes {
        // A row naming no installed vendor reads nothing below, but it is
        // admitted against the widest allowance rather than a dialect nobody
        // resolved.
        let dialect = super::super::ingest_vendor(&row.vendor)
            .map_or(WIDEST_DIALECT, |vendor| vendor.dialect());
        if !reader.startup_work.admits_lane(dialect) {
            break;
        }
        reader.startup_work.begin_lane();
        available.push(row.lane);
        if let Some(work) = restore_lane(bus, paths, row, &generation, reader, diagnostic)? {
            reader.startup_work.add(work);
        }
    }
    let current = residency
        .lanes
        .iter()
        .map(|row| row.lane)
        .collect::<Vec<_>>();
    publisher
        .set_lane_availability(current, available.iter().copied())
        .map_err(|error| store_fault("canvas::message-history-unavailable", &error.to_string()))?;
    let history = Arc::new(DurableHistory::new(bus, paths, residency, reader));
    let history_open = history
        .lanes
        .keys()
        .filter(|lane| available.contains(lane))
        .copied()
        .collect::<Vec<_>>();
    publisher
        .set_history_provider(history)
        .map_err(|error| store_fault("canvas::message-history-unavailable", &error.to_string()))?;
    publisher
        .set_history_open(history_open)
        .map_err(|error| store_fault("canvas::message-history-unavailable", &error.to_string()))?;
    generation
        .commit()
        .map_err(|error| store_fault("canvas::message-publish-failed", &error.to_string()))?;
    publisher
        .bound_initial_history()
        .map_err(|error| store_fault("canvas::message-history-unavailable", &error.to_string()))?;
    Ok(())
}

#[derive(Debug)]
struct DurableHistory {
    lanes: BTreeMap<CanonicalUuid, DurableLane>,
    continuations: Mutex<HashMap<(CanonicalUuid, Option<wire::MessageLocus>), HistoryContinuation>>,
    hydration: Mutex<Hydration>,
    bus: Option<bus::BusReader>,
    steering: PresentationDirectives,
    sessions: TranscriptSessions,
}

/// Observation times known to this provider, and how far each lane's walk got.
///
/// Startup hydrates only the window it scanned, so an older page names loci no
/// startup fact covers. The provider therefore keeps walking the same lane's
/// ingested facts backwards across pages: each walk resumes where the last one
/// stopped and what it reads stays, so no page pays for a position an earlier
/// page already crossed.
#[derive(Debug, Default)]
struct Hydration {
    observations: MessageObservations,
    walks: HashMap<CanonicalUuid, LaneWalk>,
}

/// Where one lane's backward observation walk stands.
#[derive(Clone, Copy, Debug)]
enum LaneWalk {
    /// The walk resumes strictly before this fact.
    At(wire::Seq),
    /// The lane has no older ingested fact to read.
    Exhausted,
}

#[derive(Debug)]
struct DurableLane {
    tape: std::path::PathBuf,
    worktree: std::path::PathBuf,
    dialect: listen::Dialect,
    author: CanonicalUuid,
    source: Option<listen::Checkpoint>,
    session: Option<DurableSession>,
}

#[derive(Debug)]
struct DurableSession {
    path: std::path::PathBuf,
    worktree: std::path::PathBuf,
    dialect: listen::Dialect,
    thread: String,
    source: session_compaction::HistoryCheckpoint,
}

#[derive(Clone, Debug)]
struct HistoryContinuation {
    tape: Option<listen::Checkpoint>,
    session: Option<session_compaction::HistoryCheckpoint>,
}

impl DurableHistory {
    fn new(
        bus: Option<&bus::BusReader>,
        paths: &RuntimePaths,
        residency: &wire::ResidencyHead,
        reader: &BacklogReader,
    ) -> Self {
        let environment = crate::session_environment();
        let lanes = residency
            .lanes
            .iter()
            .filter_map(|row| {
                let author = row
                    .occupant
                    .or_else(|| row.session.as_ref().map(|session| session.occupant))
                    .or_else(|| row.lineage.last().copied())?;
                let dialect = super::super::ingest_vendor(&row.vendor)?.dialect();
                let session = row.session.as_ref().and_then(|session| {
                    crate::session_command::vendor_thread_transcript(
                        &session.identity,
                        &session.vendor,
                        &environment,
                    )
                    .ok()
                    .and_then(|(path, dialect)| {
                        let source = session_compaction::history_checkpoint(&path).ok()?;
                        Some(DurableSession {
                            path,
                            worktree: session.worktree.clone().into(),
                            dialect,
                            thread: session.identity.clone(),
                            source,
                        })
                    })
                });
                Some((
                    row.lane,
                    DurableLane {
                        tape: paths
                            .tape
                            .join(row.lane.to_string())
                            .join(row.vendor.as_str()),
                        worktree: row.worktree.clone().into(),
                        dialect,
                        author,
                        source: reader.cursors.get(&row.lane).cloned(),
                        session,
                    },
                ))
            })
            .collect();
        Self {
            lanes,
            continuations: Mutex::new(HashMap::new()),
            hydration: Mutex::new(Hydration {
                observations: reader.state.observations.clone(),
                walks: HashMap::new(),
            }),
            bus: bus.cloned(),
            steering: reader.steering.clone(),
            sessions: reader.sessions.clone(),
        }
    }
}

impl canvas::stream::MessageHistoryProvider for DurableHistory {
    fn history(
        &self,
        lane: CanonicalUuid,
        before: Option<&wire::AgentMessage>,
        limit: usize,
    ) -> Result<Option<canvas::stream::MessageHistoryPage>, String> {
        let Some(source) = self.lanes.get(&lane) else {
            return Ok(None);
        };
        let continuation = self.continuation(lane, source, before)?;
        let sources = self.history_sources(lane, source, before, limit, &continuation)?;
        let (messages, more) = sources.page(before, limit);
        if more {
            self.retain_continuation(lane, before, &messages, &sources, &continuation)?;
        }
        Ok(Some(canvas::stream::MessageHistoryPage { messages, more }))
    }
}

impl DurableHistory {
    fn history_sources(
        &self,
        lane: CanonicalUuid,
        source: &DurableLane,
        before: Option<&wire::AgentMessage>,
        limit: usize,
        continuation: &HistoryContinuation,
    ) -> Result<HistorySources, String> {
        let known = self.known_observations()?;
        let session = source
            .session
            .as_ref()
            .zip(continuation.session.as_ref())
            .map(|(session, end)| {
                session_compaction::retained_messages(session_compaction::HistoryRequest {
                    path: session.path.clone(),
                    worktree: session.worktree.clone(),
                    dialect: session.dialect,
                    thread: session.thread.clone(),
                    lane,
                    author: source.author,
                    observations: &known,
                    before,
                    end,
                    limit,
                })
                .map_err(|error| format!("{error:?}"))
            })
            .transpose()?;
        let tape = if source.tape.is_dir() {
            let newest = match continuation.tape.as_ref() {
                Some(end) => self.tape_messages(lane, source, before, end, limit)?,
                None => self.initial_tape_messages(lane, source, before, limit)?,
            };
            Some(self.fill_tape_messages(lane, source, before, limit, newest)?)
        } else if source.source.is_some() {
            return Err("the transcript source disappeared after Canvas bootstrap".to_owned());
        } else {
            None
        };
        Ok(HistorySources { tape, session })
    }

    fn retain_continuation(
        &self,
        lane: CanonicalUuid,
        before: Option<&wire::AgentMessage>,
        messages: &[wire::AgentMessage],
        sources: &HistorySources,
        continuation: &HistoryContinuation,
    ) -> Result<(), String> {
        let anchor = messages.first().or(before);
        let next = HistoryContinuation {
            tape: sources
                .tape
                .as_ref()
                .map(|tape| tape_continuation(tape, anchor)),
            session: sources.session.as_ref().and_then(|session| {
                session_continuation(session, continuation.session.as_ref(), anchor)
            }),
        };
        let mut retained = self
            .continuations
            .lock()
            .map_err(|_| "durable history continuation is unavailable".to_owned())?;
        retained.retain(|(known_lane, _), _| *known_lane != lane);
        retained.insert((lane, anchor.map(|message| message.locus)), next);
        Ok(())
    }
    fn continuation(
        &self,
        lane: CanonicalUuid,
        source: &DurableLane,
        before: Option<&wire::AgentMessage>,
    ) -> Result<HistoryContinuation, String> {
        if let Some(continuation) = self
            .continuations
            .lock()
            .map_err(|_| "durable history continuation is unavailable".to_owned())?
            .remove(&(lane, before.map(|message| message.locus)))
        {
            return Ok(continuation);
        }
        let session_member = source
            .session
            .as_ref()
            .map(|session| session_compaction::session_member(&session.thread));
        let tape = source.source.as_ref().map(|source_end| {
            if before.is_some_and(|before| session_member == Some(before.locus.member)) {
                source_end.clone()
            } else {
                before.map_or_else(
                    || source_end.clone(),
                    |before| listen::Checkpoint {
                        source: source_end.source.clone(),
                        next: listen::Position {
                            member: before.locus.member,
                            offset: before.locus.offset,
                        },
                    },
                )
            }
        });
        let session = source.session.as_ref().map(|session| {
            if before.is_some_and(|before| session_member == Some(before.locus.member)) {
                session
                    .source
                    .retreat(before.expect("checked session member").locus.offset)
            } else {
                session.source.clone()
            }
        });
        Ok(HistoryContinuation { tape, session })
    }

    fn tape_messages(
        &self,
        lane: CanonicalUuid,
        source: &DurableLane,
        before: Option<&wire::AgentMessage>,
        end: &listen::Checkpoint,
        limit: usize,
    ) -> Result<TapeHistoryScan, String> {
        let tail = history_tail(&source.tape, source.dialect, end).map_err(history_tail_error)?;
        self.collect_tape_messages(lane, source, before, limit, tail)
    }

    fn initial_tape_messages(
        &self,
        lane: CanonicalUuid,
        source: &DurableLane,
        before: Option<&wire::AgentMessage>,
        limit: usize,
    ) -> Result<TapeHistoryScan, String> {
        let tail =
            history_newest(&source.tape, source.dialect).map_err(|error| error.to_string())?;
        self.collect_tape_messages(lane, source, before, limit, tail)
    }

    fn collect_tape_messages(
        &self,
        lane: CanonicalUuid,
        source: &DurableLane,
        before: Option<&wire::AgentMessage>,
        limit: usize,
        tail: listen::TailScan,
    ) -> Result<TapeHistoryScan, String> {
        let source_start = tail.start;
        let end = tail.scan.checkpoint.clone();
        let work = tail.work;
        let messages = CollectedMessages::new(limit);
        let scan = tail.scan;
        let mut reader = BacklogReader {
            cursors: BTreeMap::new(),
            startup_work: StartupWork::default(),
            state: MessageState {
                observations: self.window_observations(lane, &scan)?,
                ..MessageState::default()
            },
            steering: self.steering.clone(),
            sessions: self.sessions.clone(),
        };
        let mut diagnostic = Vec::new();
        publish_scan(
            scan,
            lane,
            source.author,
            &source.worktree,
            &messages,
            &mut reader,
            &mut diagnostic,
        )
        .map_err(|error| format!("{error:?}"))?;
        if !diagnostic.is_empty() {
            return Err(String::from_utf8_lossy(&diagnostic).trim().to_owned());
        }
        let collected = messages
            .inner
            .into_inner()
            .map_err(|_| "durable history collection is unavailable".to_owned())?;
        let mut messages = collected.messages;
        if let Some(before) = before {
            messages.retain(|message| message.chronology_key() < before.chronology_key());
        }
        let selection_start = messages.len().saturating_sub(limit);
        let selected = messages.split_off(selection_start);
        Ok(TapeHistoryScan {
            messages: selected,
            more: collected.dropped
                || selection_start > 0
                || source_start.next != listen::Position::start(),
            start: source_start,
            end,
            work,
        })
    }

    /// Observation times known right now, without reading the journal.
    fn known_observations(&self) -> Result<MessageObservations, String> {
        Ok(self
            .hydration
            .lock()
            .map_err(|_| "durable history observations are unavailable".to_owned())?
            .observations
            .clone())
    }

    /// Observation times covering one window's records, reading only what the
    /// already-walked facts do not answer.
    ///
    /// Batches tile a source generation without gaps, so a fact covering the
    /// oldest locus of this window means every newer locus in it is covered
    /// too. That makes one containment test the whole decision, and leaves the
    /// walk to run exactly when a page reaches past what earlier pages read.
    ///
    /// The walk runs while the lock is held so two concurrent pages cannot
    /// read the same facts twice and retain both copies. That costs no real
    /// concurrency: every fact read crosses the one writer queue anyway.
    fn window_observations(
        &self,
        lane: CanonicalUuid,
        scan: &listen::Scan,
    ) -> Result<MessageObservations, String> {
        let mut hydration = self
            .hydration
            .lock()
            .map_err(|_| "durable history observations are unavailable".to_owned())?;
        let (Some(bus), Some(oldest)) = (self.bus.as_ref(), oldest_scan_locus(scan)) else {
            return Ok(hydration.observations.clone());
        };
        if hydration.observations.covers(lane, oldest)
            || matches!(hydration.walks.get(&lane), Some(LaneWalk::Exhausted))
        {
            return Ok(hydration.observations.clone());
        }
        let source = scan_source(scan).map_err(|error| format!("{error:?}"))?;
        let resume = match hydration.walks.get(&lane) {
            Some(LaneWalk::At(seq)) => Some(*seq),
            Some(LaneWalk::Exhausted) | None => None,
        };
        let walk = walk_observations(bus, lane, source, oldest, scan.records.len(), resume)
            .map_err(|error| format!("{error:?}"))?;
        hydration
            .walks
            .insert(lane, walk.resume.map_or(LaneWalk::Exhausted, LaneWalk::At));
        hydration.observations.append(walk.observations);
        Ok(hydration.observations.clone())
    }
}

/// Visible cards one history request collected from the tape.
///
/// `start` is the oldest boundary any chained window reached and `end` the
/// newest, while `work` sums every window read for the request.
struct TapeHistoryScan {
    messages: Vec<wire::AgentMessage>,
    more: bool,
    start: listen::Checkpoint,
    end: listen::Checkpoint,
    work: listen::TailWork,
}

fn tape_continuation(
    scan: &TapeHistoryScan,
    before: Option<&wire::AgentMessage>,
) -> listen::Checkpoint {
    const WIDEST_SOURCE_BYTES: u64 = history_source_bytes(WIDEST_DIALECT);
    debug_assert!(scan.work.source_bytes <= WIDEST_SOURCE_BYTES * HISTORY_READ_LIMIT as u64);
    debug_assert!(scan.work.records <= TAIL_RECORD_LIMIT * HISTORY_READ_LIMIT);
    debug_assert!(scan.work.passes <= tail_passes(WIDEST_SOURCE_BYTES) * HISTORY_READ_LIMIT);
    let end = scan.end.clone();
    if scan.messages.is_empty() {
        return scan.start.clone();
    }
    let boundary = scan
        .messages
        .iter()
        .filter(|message| {
            before.is_some_and(|before| message.chronology_key() >= before.chronology_key())
        })
        .min_by_key(|message| message.chronology_key());
    let Some(message) = boundary else {
        return end;
    };
    listen::Checkpoint {
        source: end.source,
        next: listen::Position {
            member: message.locus.member,
            offset: message.locus.offset,
        },
    }
}

fn session_continuation(
    scan: &session_compaction::HistoryScan,
    end: Option<&session_compaction::HistoryCheckpoint>,
    before: Option<&wire::AgentMessage>,
) -> Option<session_compaction::HistoryCheckpoint> {
    debug_assert!(scan.work.within_request_limits());
    let end = end?;
    if scan.messages.is_empty() {
        return Some(end.retreat(scan.start));
    }
    let next = scan
        .messages
        .iter()
        .filter(|message| {
            before.is_some_and(|before| message.chronology_key() >= before.chronology_key())
        })
        .min_by_key(|message| message.chronology_key())
        .map_or(end.next(), |message| message.locus.offset);
    Some(end.retreat(next))
}

#[derive(Debug)]
struct CollectedMessages {
    limit: usize,
    inner: Mutex<CollectedHistory>,
}

#[derive(Debug, Default)]
struct CollectedHistory {
    messages: Vec<wire::AgentMessage>,
    dropped: bool,
}

impl CollectedMessages {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            inner: Mutex::new(CollectedHistory::default()),
        }
    }
}

impl AgentMessagePublisher for CollectedMessages {
    fn publish(
        &self,
        message: wire::AgentMessage,
    ) -> Result<bool, canvas::stream::MessagePublishError> {
        if !visible_message(&message) {
            return Ok(true);
        }
        let mut collected = self
            .inner
            .lock()
            .map_err(|_| canvas::stream::MessagePublishError::Unavailable)?;
        collected.messages.push(message);
        if collected.messages.len() > self.limit {
            collected.messages.remove(0);
            collected.dropped = true;
        }
        Ok(true)
    }

    fn revise_final(
        &self,
        message: wire::AgentMessage,
    ) -> Result<bool, canvas::stream::MessagePublishError> {
        let mut collected = self
            .inner
            .lock()
            .map_err(|_| canvas::stream::MessagePublishError::Unavailable)?;
        let Some(existing) = collected
            .messages
            .iter_mut()
            .find(|existing| existing.lane == message.lane && existing.locus == message.locus)
        else {
            return Err(canvas::stream::MessagePublishError::IdentityConflict);
        };
        *existing = message;
        Ok(true)
    }
}

fn restore_lane(
    bus: Option<&bus::BusReader>,
    paths: &RuntimePaths,
    row: &wire::LaneResidency,
    publisher: &impl AgentMessagePublisher,
    reader: &mut BacklogReader,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<Option<listen::TailWork>> {
    let Some(author) = row
        .occupant
        .or_else(|| row.session.as_ref().map(|session| session.occupant))
        .or_else(|| row.lineage.last().copied())
    else {
        return Ok(None);
    };
    let Some(vendor) = super::super::ingest_vendor(&row.vendor) else {
        return Ok(None);
    };
    let tape = paths
        .tape
        .join(row.lane.to_string())
        .join(row.vendor.as_str());
    if !tape_available(&tape, row.lane, diagnostic) {
        return Ok(None);
    }
    let tail = match read_backlog(&tape, row.lane, vendor.dialect(), reader) {
        Ok(tail) => tail,
        Err(error) => {
            let work = error.tail_work().copied();
            forget_changed_source(error, row.lane, reader, diagnostic);
            return Ok(work);
        }
    };
    let work = tail.work;
    let scan = tail.scan;
    if let Some(bus) = bus {
        hydrate_observations(bus, row.lane, &scan, reader)?;
        let keys = scan_steering_keys(&scan);
        let rows = load_steering_presentations(bus, &keys)?;
        reader
            .steering
            .extend_presentations(&rows)
            .map_err(|error| store_fault("canvas::message-steering-invalid", &error))?;
    }
    publish_scan(
        scan,
        row.lane,
        author,
        Path::new(&row.worktree),
        publisher,
        reader,
        diagnostic,
    )?;
    Ok(Some(work))
}

fn hydrate_observations(
    bus: &bus::BusReader,
    lane: CanonicalUuid,
    scan: &listen::Scan,
    reader: &mut BacklogReader,
) -> CommandResult<()> {
    let Some(oldest) = oldest_scan_locus(scan) else {
        return Ok(());
    };
    let source = scan_source(scan)?;
    let walk = walk_observations(bus, lane, source, oldest, scan.records.len(), None)?;
    reader.state.observations.append(walk.observations);
    Ok(())
}

/// Oldest tape position any decoded event in one scan names.
fn oldest_scan_locus(scan: &listen::Scan) -> Option<(u64, u64)> {
    scan.records
        .iter()
        .flat_map(|record| match &record.payload {
            listen::RecordPayload::Events(events) => events
                .iter()
                .map(|event| (event.event.locus().member, event.event.locus().offset))
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .min()
}

/// Source generation the scan read, as the accounting facts name it.
fn scan_source(scan: &listen::Scan) -> CommandResult<CanonicalUuid> {
    scan.checkpoint
        .source
        .as_str()
        .parse::<CanonicalUuid>()
        .map_err(|error| store_fault("canvas::message-source-invalid", &error.to_string()))
}

/// One bounded backward walk of a lane's accounting facts.
struct ObservationWalk {
    observations: MessageObservations,
    /// Fact the next walk resumes strictly before, or `None` once the lane's
    /// oldest accounting fact has been read.
    resume: Option<wire::Seq>,
}

/// Read this lane's accounting facts backwards until one covers `oldest`.
///
/// The walk is capped rather than exhaustive: a window deep in a long tape may
/// stop before it reaches its own batch, and then reports where it stopped so
/// the next request continues from there instead of paying for the same facts
/// again. Only batches naming this lane and this source generation can end the
/// walk, so a replaced generation never lends its times to the current one.
fn walk_observations(
    bus: &bus::BusReader,
    lane: CanonicalUuid,
    source: CanonicalUuid,
    oldest: (u64, u64),
    records: usize,
    resume: Option<wire::Seq>,
) -> CommandResult<ObservationWalk> {
    let origin = format!("lane:{lane}")
        .parse::<wire::Origin>()
        .map_err(|error| store_fault("canvas::message-origin-invalid", &error.to_string()))?;
    let mut remaining = records.saturating_mul(4).clamp(32, 4_096);
    let mut before = resume;
    let mut facts = Vec::new();
    while remaining > 0 {
        let limit = remaining.min(128);
        let budget = journal::FactReadBudget::new(limit, journal::MAX_RETAINED_FACT_PAGE_BYTES)
            .expect("positive capped backlog work is a repository budget");
        let page = bus
            .recent_origin_facts(
                origin.clone(),
                wire::FactKind::JournalIngested,
                before,
                budget,
            )
            .map_err(|error| store_fault("canvas::message-time-unavailable", &error.to_string()))?;
        if page.rows.is_empty() {
            before = None;
            break;
        }
        before = page.rows.last().map(|fact| fact.seq);
        let reached = page.rows.iter().any(|envelope| {
            envelope
                .fact
                .read::<wire::body::JournalIngested>()
                .is_some_and(|batch| {
                    batch.lane == lane
                        && batch.source == source
                        && (batch.from.member, batch.from.offset) <= oldest
                })
        });
        remaining = remaining.saturating_sub(page.rows.len());
        facts.extend(page.rows);
        if reached {
            break;
        }
    }
    facts.sort_by_key(|fact| fact.seq);
    Ok(ObservationWalk {
        observations: MessageObservations::from_facts(&facts),
        resume: before,
    })
}

fn scan_steering_keys(scan: &listen::Scan) -> BTreeSet<steer::Key> {
    steering_keys(scan.records.iter().flat_map(|record| {
        match &record.payload {
            listen::RecordPayload::Events(events) => events
                .iter()
                .filter_map(|event| {
                    let listen::Event::Prose(prose) = &event.event else {
                        return None;
                    };
                    (prose.role == listen::ProseRole::Assistant).then_some(prose.text.as_str())
                })
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        }
    }))
}

fn tape_available(tape: &Path, lane: CanonicalUuid, diagnostic: &mut dyn io::Write) -> bool {
    match fs::metadata(tape) {
        Ok(metadata) if metadata.is_dir() => true,
        Ok(_) => {
            let _ = writeln!(
                diagnostic,
                "canvas backlog unavailable for lane {lane}: {} is not a tape directory",
                tape.display(),
            );
            false
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            let _ = writeln!(
                diagnostic,
                "canvas backlog unavailable for lane {lane}: {error}",
            );
            false
        }
    }
}

fn forget_changed_source(
    error: listen::ScanError,
    lane: CanonicalUuid,
    reader: &mut BacklogReader,
    diagnostic: &mut dyn io::Write,
) {
    if matches!(&error, listen::ScanError::SourceChanged { .. }) {
        reader.cursors.remove(&lane);
        reader.state.previews.clear_lane(lane);
        reader.state.prose.remove(&lane);
    }
    let _ = writeln!(
        diagnostic,
        "canvas backlog unavailable for lane {lane}: {error}",
    );
}

fn publish_scan(
    scan: listen::Scan,
    lane: CanonicalUuid,
    author: CanonicalUuid,
    fallback_worktree: &Path,
    publisher: &impl AgentMessagePublisher,
    reader: &mut BacklogReader,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let checkpoint = scan.checkpoint.clone();
    let source = scan
        .checkpoint
        .source
        .as_str()
        .parse::<CanonicalUuid>()
        .map_err(|error| store_fault("canvas::message-source-invalid", &error.to_string()))?;
    let mut published = true;
    for record in scan.records {
        let Some(context) = record.context.as_ref() else {
            continue;
        };
        let listen::RecordPayload::Events(events) = &record.payload else {
            continue;
        };
        let worktree = reader
            .sessions
            .worktree_at(lane, source, record.locus)
            .unwrap_or(fallback_worktree);
        let decoded = events
            .iter()
            .map(|sourced| {
                let event = u32::try_from(sourced.ordinal).map_err(|error| {
                    store_fault("canvas::message-locus-invalid", &error.to_string())
                })?;
                Ok((event, &sourced.event))
            })
            .collect::<CommandResult<Vec<_>>>()?;
        if let Err(outcome) = publish_agent_record(
            publisher,
            AgentRecord {
                lane,
                author,
                source: Some(source),
                worktree: Some(worktree),
                locus: record.locus,
                context,
                events: &decoded,
                observed: None,
                unavailable_tool_terminals: &[],
            },
            &mut reader.state,
            Some(&reader.steering),
        ) {
            match outcome {
                Outcome::Fault(fault) => {
                    published = false;
                    let _ = writeln!(
                        diagnostic,
                        "canvas backlog unavailable for lane {lane}: {}: {}",
                        fault.reason, fault.detail,
                    );
                }
                other => return Err(other),
            }
        }
    }
    if published {
        reader.cursors.insert(lane, checkpoint);
    }
    Ok(())
}

fn read_backlog(
    tape: &Path,
    lane: CanonicalUuid,
    dialect: listen::Dialect,
    reader: &BacklogReader,
) -> Result<listen::TailScan, listen::ScanError> {
    let tail = tail_backlog(tape, dialect)?;
    if let Some(cursor) = reader.cursors.get(&lane)
        && cursor.source != tail.scan.checkpoint.source
    {
        return Err(listen::ScanError::SourceChanged {
            expected: cursor.source.clone(),
            observed: tail.scan.checkpoint.source.clone(),
            next: cursor.next,
        });
    }
    Ok(tail)
}

fn tail_backlog(
    tape: &Path,
    dialect: listen::Dialect,
) -> Result<listen::TailScan, listen::ScanError> {
    listen::scan_tail(tape, dialect, startup_budget(dialect))
}

fn visible_message(message: &wire::AgentMessage) -> bool {
    message.activity.is_visible() || !message.media.is_empty()
}
