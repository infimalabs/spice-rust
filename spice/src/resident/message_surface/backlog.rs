//! Startup replay of durable lane tapes into the one Canvas message feed.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use super::*;

#[cfg(test)]
mod tests;

/// First newest-record window; sparse tapes expand geometrically until enough cards exist.
const INITIAL_WINDOW_BYTES: u64 = 256 * 1024;

/// Source-authenticated per-lane continuation state for repeated backlog reads.
#[derive(Debug, Default)]
pub(in crate::resident) struct BacklogReader {
    cursors: BTreeMap<CanonicalUuid, listen::Checkpoint>,
    state: MessageState,
}

impl BacklogReader {
    pub(in crate::resident) fn from_snapshot(snapshot: &journal::FactSnapshot) -> Self {
        Self {
            cursors: BTreeMap::new(),
            state: MessageState {
                previews: CallPreviews::default(),
                observations: MessageObservations::from_snapshot(snapshot),
            },
        }
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
pub(in crate::resident) fn restore_agent_messages(
    paths: &RuntimePaths,
    residency: &wire::ResidencyHead,
    publisher: &impl AgentMessagePublisher,
    reader: &mut BacklogReader,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    for row in &residency.lanes {
        restore_lane(paths, row, publisher, reader, diagnostic)?;
    }
    Ok(())
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
                    ordinal: event,
                    decoded: &sourced.event,
                    authored: sourced.context.timestamp.as_deref(),
                    observations: &reader.state.observations,
                },
                &mut reader.state.previews,
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
