//! Startup replay of durable lane tapes into the one Canvas message feed.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use super::*;

#[cfg(test)]
mod tests;

/// First newest-record window; sparse tapes expand geometrically until enough cards exist.
const INITIAL_WINDOW_BYTES: u64 = 32 * 1024;

/// Source-authenticated per-lane continuation state for repeated backlog reads.
#[derive(Debug, Default)]
pub(in crate::resident) struct BacklogReader {
    cursors: BTreeMap<CanonicalUuid, listen::Checkpoint>,
    state: MessageState,
    steering: PresentationDirectives,
}

impl BacklogReader {
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
            },
            steering,
        })
    }

    pub(in crate::resident) fn into_state(self) -> MessageState {
        self.state
    }

    pub(in crate::resident) fn state_mut(&mut self) -> &mut MessageState {
        &mut self.state
    }
}

/// Seed each current lane's bounded Canvas backlog before discovery is published.
///
/// Live capture and restart replay both cross [`publish_agent_event`], then the
/// generation-owned publisher applies the same identity and per-lane capacity
/// law. A record captured while startup scans is therefore either present in
/// the replay or delivered from the queued live channel, and the second arrival
/// is an exact duplicate rather than another execution path.
pub(in crate::resident) fn restore_agent_messages(
    paths: &RuntimePaths,
    residency: &wire::ResidencyHead,
    publisher: &canvas::stream::MessagePublisher,
    reader: &mut BacklogReader,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    for row in &residency.lanes {
        restore_lane(paths, row, publisher, reader, diagnostic)?;
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
    observations: MessageObservations,
    steering: PresentationDirectives,
}

#[derive(Debug)]
struct DurableLane {
    tape: std::path::PathBuf,
    dialect: listen::Dialect,
    author: CanonicalUuid,
    source: Option<listen::SourceId>,
    session: Option<DurableSession>,
}

#[derive(Debug)]
struct DurableSession {
    path: std::path::PathBuf,
    dialect: listen::Dialect,
    thread: String,
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
                    .map(|(path, dialect)| DurableSession {
                        path,
                        dialect,
                        thread: session.identity.clone(),
                    })
                });
                Some((
                    row.lane,
                    DurableLane {
                        tape: paths
                            .tape
                            .join(row.lane.to_string())
                            .join(row.vendor.as_str()),
                        dialect,
                        author,
                        source: reader
                            .cursors
                            .get(&row.lane)
                            .map(|checkpoint| checkpoint.source.clone()),
                        session,
                    },
                ))
            })
            .collect();
        Self {
            lanes,
            observations: reader.state.observations.clone(),
            steering: reader.steering.clone(),
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
        let mut visible = if source.tape.is_dir() {
            self.tape_messages(lane, source)?
        } else if source.source.is_some() {
            return Err("the transcript source disappeared after Canvas bootstrap".to_owned());
        } else {
            Vec::new()
        };
        if let Some(session) = &source.session {
            visible.extend(
                session_compaction::retained_messages(
                    session.path.clone(),
                    session.dialect,
                    session.thread.clone(),
                    lane,
                    source.author,
                    &self.observations,
                )
                .map_err(|error| format!("{error:?}"))?,
            );
        }
        visible.sort_by_key(wire::AgentMessage::chronology_key);
        visible.dedup_by(|right, left| right.locus == left.locus);
        let activities = visible
            .iter()
            .map(|message| message.activity)
            .collect::<Vec<_>>();
        let floor =
            wire::AgentActivityKind::retention_start(&activities, canvas::stream::MESSAGE_CAPACITY);
        let end =
            visible.partition_point(|message| message.chronology_key() < before.chronology_key());
        let start = end.saturating_sub(limit).max(floor.min(end));
        Ok(Some(canvas::stream::MessageHistoryPage {
            messages: visible[start..end].to_vec(),
            more: start > floor,
        }))
    }
}

impl DurableHistory {
    fn tape_messages(
        &self,
        lane: CanonicalUuid,
        source: &DurableLane,
    ) -> Result<Vec<wire::AgentMessage>, String> {
        let scan =
            retained_tail(&source.tape, source.dialect).map_err(|error| error.to_string())?;
        if source
            .source
            .as_ref()
            .is_some_and(|expected| *expected != scan.checkpoint.source)
        {
            return Err("the transcript source changed after Canvas bootstrap".to_owned());
        }
        let messages = CollectedMessages::default();
        let mut reader = BacklogReader {
            cursors: BTreeMap::new(),
            state: MessageState {
                previews: CallPreviews::default(),
                observations: self.observations.clone(),
            },
            steering: self.steering.clone(),
        };
        let mut diagnostic = Vec::new();
        publish_scan(
            scan,
            lane,
            source.author,
            &messages,
            &mut reader,
            &mut diagnostic,
        )
        .map_err(|error| format!("{error:?}"))?;
        if !diagnostic.is_empty() {
            return Err(String::from_utf8_lossy(&diagnostic).trim().to_owned());
        }
        messages
            .0
            .into_inner()
            .map_err(|_| "durable history collection is unavailable".to_owned())
            .map(|messages| messages.into_iter().filter(visible_message).collect())
    }
}

#[derive(Debug, Default)]
struct CollectedMessages(Mutex<Vec<wire::AgentMessage>>);

impl AgentMessagePublisher for CollectedMessages {
    fn publish(
        &self,
        message: wire::AgentMessage,
    ) -> Result<bool, canvas::stream::MessagePublishError> {
        self.0
            .lock()
            .map_err(|_| canvas::stream::MessagePublishError::Unavailable)?
            .push(message);
        Ok(true)
    }
}

fn restore_lane(
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
    publish_scan(scan, row.lane, author, publisher, reader, diagnostic)
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
        for sourced in events {
            let event = u32::try_from(sourced.ordinal).map_err(|error| {
                store_fault("canvas::message-locus-invalid", &error.to_string())
            })?;
            if let Err(outcome) = publish_agent_event(
                publisher,
                AgentEvent {
                    lane,
                    author,
                    source: Some(source),
                    ordinal: event,
                    decoded: &sourced.event,
                    authored: sourced.context.timestamp.as_deref(),
                    observed: None,
                    observations: &reader.state.observations,
                    unavailable_tool_terminals: &[],
                },
                &mut reader.state.previews,
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
        let tail = listen::scan_tail(tape, dialect, bytes)?;
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

fn retained_tail(tape: &Path, dialect: listen::Dialect) -> Result<listen::Scan, listen::ScanError> {
    let mut bytes = INITIAL_WINDOW_BYTES;
    loop {
        let tail = listen::scan_tail(tape, dialect, bytes)?;
        if visible_cards(&tail.scan) >= canvas::stream::MESSAGE_CAPACITY
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
