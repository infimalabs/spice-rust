//! Startup replay of durable lane tapes into the one Canvas message feed.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;

#[cfg(test)]
mod tests;

/// First newest-record window; sparse tapes expand geometrically until enough cards exist.
const INITIAL_WINDOW_BYTES: u64 = 32 * 1024;
const TAIL_BOUNDARY_BYTES: u64 = 256 * 1024;
const TAIL_RECORD_LIMIT: usize = 32 * 1024;
const TAIL_PASS_LIMIT: usize = 64;
const TAIL_DURATION: Duration = Duration::from_secs(1);

fn tail_budget(window_bytes: u64) -> listen::TailBudget {
    listen::TailBudget::new(
        window_bytes,
        window_bytes.saturating_add(TAIL_BOUNDARY_BYTES),
        TAIL_RECORD_LIMIT,
        TAIL_PASS_LIMIT,
        TAIL_DURATION,
    )
    .expect("Canvas tail allowances are positive")
}

#[derive(Clone, Debug, Default)]
struct SessionWorktrees(BTreeMap<(CanonicalUuid, CanonicalUuid), Vec<SessionWorktree>>);

#[derive(Clone, Debug)]
struct SessionWorktree {
    member: u64,
    offset: u64,
    worktree: std::path::PathBuf,
}

impl SessionWorktrees {
    #[cfg(test)]
    fn from_snapshot(snapshot: &journal::FactSnapshot) -> Self {
        let ingestions = snapshot
            .facts()
            .iter()
            .filter_map(|envelope| {
                envelope
                    .fact
                    .read::<wire::body::JournalIngested>()
                    .map(|batch| (envelope.seq, batch))
            })
            .collect::<Vec<_>>();
        let mut worktrees = BTreeMap::<(CanonicalUuid, CanonicalUuid), Vec<SessionWorktree>>::new();
        for envelope in snapshot.facts() {
            let Some(session) = envelope.fact.read::<wire::body::LaneSessionDeclared>() else {
                continue;
            };
            let position = (session.member, session.offset);
            let Some(source) = ingestions.iter().find_map(|(seq, batch)| {
                (*seq > envelope.seq
                    && batch.lane == session.lane
                    && batch.vendor == session.vendor
                    && batch.stream == session.stream
                    && (batch.from.member, batch.from.offset) <= position
                    && position < (batch.to.member, batch.to.offset))
                    .then_some(batch.source)
            }) else {
                continue;
            };
            worktrees
                .entry((session.lane, source))
                .or_default()
                .push(SessionWorktree {
                    member: session.member,
                    offset: session.offset,
                    worktree: session.worktree.into(),
                });
        }
        for sessions in worktrees.values_mut() {
            sessions.sort_by_key(|session| (session.member, session.offset));
        }
        Self(worktrees)
    }

    fn at(&self, lane: CanonicalUuid, source: CanonicalUuid, locus: tape::Locus) -> Option<&Path> {
        self.0
            .get(&(lane, source))?
            .iter()
            .rev()
            .find(|session| (session.member, session.offset) <= (locus.member, locus.offset))
            .map(|session| session.worktree.as_path())
    }
}

/// Source-authenticated per-lane continuation state for repeated backlog reads.
#[derive(Debug, Default)]
pub(in crate::resident) struct BacklogReader {
    cursors: BTreeMap<CanonicalUuid, listen::Checkpoint>,
    state: MessageState,
    steering: PresentationDirectives,
    worktrees: SessionWorktrees,
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
            state: MessageState {
                previews: CallPreviews::default(),
                observations: MessageObservations::from_snapshot(snapshot),
                prose: BTreeMap::new(),
            },
            steering,
            worktrees: SessionWorktrees::from_snapshot(snapshot),
        })
    }

    pub(in crate::resident) fn into_state(self) -> MessageState {
        self.state
    }
}

/// Seed each current lane's bounded Canvas backlog before discovery is published.
///
/// Live capture and restart replay both cross [`publish_agent_event`], then the
/// generation-owned publisher applies the same identity and per-lane capacity
/// law. A record captured while startup scans is therefore either present in
/// the replay or delivered from the queued live channel, and the second arrival
/// is an exact duplicate rather than another execution path.
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
    for row in &residency.lanes {
        restore_lane(bus, paths, row, publisher, reader, diagnostic)?;
    }
    publisher
        .set_history_provider(Arc::new(DurableHistory::new(paths, residency, reader)))
        .map_err(|error| store_fault("canvas::message-history-unavailable", &error.to_string()))?;
    publisher
        .bound_initial_history()
        .map_err(|error| store_fault("canvas::message-history-unavailable", &error.to_string()))?;
    Ok(())
}

#[derive(Debug)]
struct DurableHistory {
    lanes: BTreeMap<CanonicalUuid, DurableLane>,
    continuations: Mutex<HashMap<(CanonicalUuid, wire::MessageLocus), HistoryContinuation>>,
    observations: MessageObservations,
    steering: PresentationDirectives,
    worktrees: SessionWorktrees,
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
    fn new(paths: &RuntimePaths, residency: &wire::ResidencyHead, reader: &BacklogReader) -> Self {
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
            observations: reader.state.observations.clone(),
            steering: reader.steering.clone(),
            worktrees: reader.worktrees.clone(),
        }
    }
}

impl canvas::stream::MessageHistoryProvider for DurableHistory {
    fn history(
        &self,
        lane: CanonicalUuid,
        before: &wire::AgentMessage,
        limit: usize,
    ) -> Result<Option<canvas::stream::MessageHistoryPage>, String> {
        let Some(source) = self.lanes.get(&lane) else {
            return Ok(None);
        };
        let continuation = self.continuation(lane, source, before)?;
        let tape = if source.tape.is_dir() {
            continuation
                .tape
                .as_ref()
                .map(|end| self.tape_messages(lane, source, before, end, limit))
                .transpose()?
        } else if source.source.is_some() {
            return Err("the transcript source disappeared after Canvas bootstrap".to_owned());
        } else {
            None
        };
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
                    observations: &self.observations,
                    before,
                    end,
                    limit,
                })
                .map_err(|error| format!("{error:?}"))
            })
            .transpose()?;
        let mut visible = tape
            .as_ref()
            .map_or_else(Vec::new, |tape| tape.messages.clone());
        let mut more = tape.as_ref().is_some_and(|tape| tape.more);
        if let Some(session) = &session {
            visible.extend(session.messages.clone());
            more |= session.more;
        }
        visible.sort_by_key(wire::AgentMessage::chronology_key);
        visible.dedup_by(|right, left| right.locus == left.locus);
        let end =
            visible.partition_point(|message| message.chronology_key() < before.chronology_key());
        let start = end.saturating_sub(limit);
        let messages = visible[start..end].to_vec();
        if let Some(oldest) = messages.first() {
            let next = HistoryContinuation {
                tape: tape
                    .as_ref()
                    .and_then(|tape| tape_continuation(tape, continuation.tape.as_ref(), oldest)),
                session: session.as_ref().and_then(|session| {
                    session_continuation(session, continuation.session.as_ref(), oldest)
                }),
            };
            self.continuations
                .lock()
                .map_err(|_| "durable history continuation is unavailable".to_owned())?
                .insert((lane, oldest.locus), next);
        }
        Ok(Some(canvas::stream::MessageHistoryPage {
            messages,
            more: more || start > 0,
        }))
    }
}

impl DurableHistory {
    fn continuation(
        &self,
        lane: CanonicalUuid,
        source: &DurableLane,
        before: &wire::AgentMessage,
    ) -> Result<HistoryContinuation, String> {
        if let Some(continuation) = self
            .continuations
            .lock()
            .map_err(|_| "durable history continuation is unavailable".to_owned())?
            .get(&(lane, before.locus))
            .cloned()
        {
            return Ok(continuation);
        }
        let session_member = source
            .session
            .as_ref()
            .map(|session| session_compaction::session_member(&session.thread));
        let tape = source.source.as_ref().map(|source_end| {
            if session_member == Some(before.locus.member) {
                source_end.clone()
            } else {
                listen::Checkpoint {
                    source: source_end.source.clone(),
                    next: listen::Position {
                        member: before.locus.member,
                        offset: before.locus.offset,
                    },
                }
            }
        });
        let session = source.session.as_ref().map(|session| {
            if session_member == Some(before.locus.member) {
                session.source.retreat(before.locus.offset)
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
        before: &wire::AgentMessage,
        end: &listen::Checkpoint,
        limit: usize,
    ) -> Result<TapeHistoryScan, String> {
        let tail = history_tail(&source.tape, source.dialect, end, limit).map_err(|error| {
            if matches!(error, listen::ScanError::SourceChanged { .. }) {
                "the transcript source changed after Canvas bootstrap".to_owned()
            } else {
                error.to_string()
            }
        })?;
        let messages = CollectedMessages::new(limit);
        let mut reader = BacklogReader {
            cursors: BTreeMap::new(),
            state: MessageState {
                previews: CallPreviews::default(),
                observations: self.observations.clone(),
                prose: BTreeMap::new(),
            },
            steering: self.steering.clone(),
            worktrees: self.worktrees.clone(),
        };
        let mut diagnostic = Vec::new();
        publish_scan(
            tail.scan,
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
        messages.retain(|message| message.chronology_key() < before.chronology_key());
        let start = messages.len().saturating_sub(limit);
        let selected = messages.split_off(start);
        Ok(TapeHistoryScan {
            messages: selected,
            more: collected.dropped || start > 0 || tail.start.next != listen::Position::start(),
            start: tail.start,
        })
    }
}

struct TapeHistoryScan {
    messages: Vec<wire::AgentMessage>,
    more: bool,
    start: listen::Checkpoint,
}

fn tape_continuation(
    scan: &TapeHistoryScan,
    end: Option<&listen::Checkpoint>,
    before: &wire::AgentMessage,
) -> Option<listen::Checkpoint> {
    let end = end?.clone();
    if scan.messages.is_empty() {
        return Some(scan.start.clone());
    }
    let boundary = scan
        .messages
        .iter()
        .filter(|message| message.chronology_key() >= before.chronology_key())
        .min_by_key(|message| message.chronology_key());
    let Some(message) = boundary else {
        return Some(end);
    };
    Some(listen::Checkpoint {
        source: end.source,
        next: listen::Position {
            member: message.locus.member,
            offset: message.locus.offset,
        },
    })
}

fn session_continuation(
    scan: &session_compaction::HistoryScan,
    end: Option<&session_compaction::HistoryCheckpoint>,
    before: &wire::AgentMessage,
) -> Option<session_compaction::HistoryCheckpoint> {
    debug_assert!(scan.work.within_limits());
    let end = end?;
    if scan.messages.is_empty() {
        return Some(end.retreat(scan.start));
    }
    let next = scan
        .messages
        .iter()
        .filter(|message| message.chronology_key() >= before.chronology_key())
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
) -> CommandResult<()> {
    let Some(author) = row
        .occupant
        .or_else(|| row.session.as_ref().map(|session| session.occupant))
        .or_else(|| row.lineage.last().copied())
    else {
        return Ok(());
    };
    let Some(vendor) = super::super::ingest_vendor(&row.vendor) else {
        return Ok(());
    };
    let tape = paths
        .tape
        .join(row.lane.to_string())
        .join(row.vendor.as_str());
    if !tape_available(&tape, row.lane, diagnostic) {
        return Ok(());
    }
    let scan = match read_backlog(&tape, row.lane, vendor.dialect(), reader) {
        Ok(scan) => scan,
        Err(error) => {
            forget_changed_source(error, row.lane, reader, diagnostic);
            return Ok(());
        }
    };
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
    )
}

fn hydrate_observations(
    bus: &bus::BusReader,
    lane: CanonicalUuid,
    scan: &listen::Scan,
    reader: &mut BacklogReader,
) -> CommandResult<()> {
    let Some(oldest) = scan
        .records
        .iter()
        .flat_map(|record| match &record.payload {
            listen::RecordPayload::Events(events) => events
                .iter()
                .map(|event| (event.event.locus().member, event.event.locus().offset))
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .min()
    else {
        return Ok(());
    };
    let source = scan
        .checkpoint
        .source
        .as_str()
        .parse::<CanonicalUuid>()
        .map_err(|error| store_fault("canvas::message-source-invalid", &error.to_string()))?;
    let origin = format!("lane:{lane}")
        .parse::<wire::Origin>()
        .map_err(|error| store_fault("canvas::message-origin-invalid", &error.to_string()))?;
    let mut remaining = scan.records.len().saturating_mul(4).clamp(32, 4_096);
    let mut before = None;
    let mut facts = Vec::new();
    while remaining > 0 {
        let limit = remaining.min(128);
        let page = bus
            .recent_origin_facts(
                origin.clone(),
                wire::FactKind::JournalIngested,
                before,
                limit,
            )
            .map_err(|error| store_fault("canvas::message-time-unavailable", &error.to_string()))?;
        if page.is_empty() {
            break;
        }
        before = page.last().map(|fact| fact.seq);
        let reached = page.iter().any(|envelope| {
            envelope
                .fact
                .read::<wire::body::JournalIngested>()
                .is_some_and(|batch| {
                    batch.lane == lane
                        && batch.source == source
                        && (batch.from.member, batch.from.offset) <= oldest
                })
        });
        remaining = remaining.saturating_sub(page.len());
        facts.extend(page);
        if reached {
            break;
        }
    }
    facts.sort_by_key(|fact| fact.seq);
    reader
        .state
        .observations
        .append(MessageObservations::from_facts(&facts));
    Ok(())
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
        let listen::RecordPayload::Events(events) = record.payload else {
            continue;
        };
        let Some(context) = events.first().map(|sourced| &sourced.context) else {
            continue;
        };
        let worktree = events
            .first()
            .and_then(|sourced| reader.worktrees.at(lane, source, sourced.event.locus()))
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
    reader: &mut BacklogReader,
) -> Result<listen::Scan, listen::ScanError> {
    if let Some(cursor) = reader.cursors.get(&lane) {
        listen::scan(tape, dialect, Some(cursor))
    } else {
        tail_backlog(tape, dialect)
    }
}

fn tail_backlog(tape: &Path, dialect: listen::Dialect) -> Result<listen::Scan, listen::ScanError> {
    let mut bytes = INITIAL_WINDOW_BYTES;
    loop {
        let tail = listen::scan_tail(tape, dialect, tail_budget(bytes))?;
        if visible_cards(&tail.scan) > canvas::stream::INITIAL_MESSAGE_LIMIT
            || tail.start.next == listen::Position::start()
        {
            return Ok(tail.scan);
        }
        let expanded = bytes.saturating_mul(2);
        if expanded == bytes {
            return Ok(tail.scan);
        }
        bytes = expanded;
    }
}

fn history_tail(
    tape: &Path,
    dialect: listen::Dialect,
    before: &listen::Checkpoint,
    visible_limit: usize,
) -> Result<listen::TailScan, listen::ScanError> {
    let mut bytes = INITIAL_WINDOW_BYTES;
    loop {
        let tail = listen::scan_before(tape, dialect, before, tail_budget(bytes))?;
        if visible_cards(&tail.scan) >= visible_limit
            || tail.start.next == listen::Position::start()
        {
            return Ok(tail);
        }
        let expanded = bytes.saturating_mul(2);
        if expanded == bytes {
            return Ok(tail);
        }
        bytes = expanded;
    }
}

fn visible_cards(scan: &listen::Scan) -> usize {
    scan.records
        .iter()
        .filter_map(|record| match &record.payload {
            listen::RecordPayload::Events(events) => Some(events),
            listen::RecordPayload::Residue(_) => None,
        })
        .flatten()
        .filter(|event| match &event.event {
            listen::Event::Prose(prose) => prose.role == listen::ProseRole::Assistant,
            listen::Event::Compaction(_) => true,
            listen::Event::Tool(tool) => match &tool.moment {
                listen::ToolMoment::Result(outcome) => !outcome.media.is_empty(),
                listen::ToolMoment::Call { .. } | listen::ToolMoment::Update { .. } => false,
            },
            _ => false,
        })
        .count()
}

fn visible_message(message: &wire::AgentMessage) -> bool {
    matches!(
        message.activity,
        wire::AgentActivityKind::Prose | wire::AgentActivityKind::Compaction
    ) || !message.media.is_empty()
}
