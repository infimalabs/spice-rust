//! Startup replay of durable lane tapes into the one Canvas message feed.

use std::fs;

use super::*;

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
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let mut previews = CallPreviews::default();
    for row in &residency.lanes {
        let Some(author) = row
            .occupant
            .or_else(|| row.session.as_ref().map(|session| session.occupant))
            .or_else(|| row.lineage.last().copied())
        else {
            continue;
        };
        let Some(vendor) = super::super::ingest_vendor(&row.vendor) else {
            continue;
        };
        let tape = paths
            .tape
            .join(row.lane.to_string())
            .join(row.vendor.as_str());
        match fs::metadata(&tape) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                let _ = writeln!(
                    diagnostic,
                    "canvas backlog unavailable for lane {}: {} is not a tape directory",
                    row.lane,
                    tape.display(),
                );
                continue;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                let _ = writeln!(
                    diagnostic,
                    "canvas backlog unavailable for lane {}: {error}",
                    row.lane,
                );
                continue;
            }
        }
        let scan = match listen::scan(&tape, vendor.dialect(), None) {
            Ok(scan) => scan,
            Err(error) => {
                let _ = writeln!(
                    diagnostic,
                    "canvas backlog unavailable for lane {}: {error}",
                    row.lane,
                );
                continue;
            }
        };
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
                    row.lane,
                    author,
                    event,
                    &sourced.event,
                    &mut previews,
                ) {
                    match outcome {
                        Outcome::Fault(fault) => {
                            let _ = writeln!(
                                diagnostic,
                                "canvas backlog unavailable for lane {}: {}: {}",
                                row.lane, fault.reason, fault.detail,
                            );
                        }
                        other => return Err(other),
                    }
                }
            }
        }
    }
    Ok(())
}
