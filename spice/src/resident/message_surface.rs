//! Fallible Canvas activity views over authoritative lane capture.

use std::io;

use bus::Bus;
use lane::occupant::Seats;
use wire::{AgentActivityKind, CanonicalUuid, Outcome};

use super::{CommandResult, lane_surface, store_fault};

mod tool_preview;

pub(super) use tool_preview::CallPreviews;

pub(super) trait AgentMessagePublisher {
    fn publish(
        &self,
        message: wire::AgentMessage,
    ) -> Result<bool, canvas::stream::MessagePublishError>;
}

impl AgentMessagePublisher for canvas::stream::MessagePublisher {
    fn publish(
        &self,
        message: wire::AgentMessage,
    ) -> Result<bool, canvas::stream::MessagePublishError> {
        Self::publish(self, message)
    }
}

pub(super) fn accept_capture_record(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    publisher: &impl AgentMessagePublisher,
    record: &lane::capture::CaptureRecord,
    previews: &mut CallPreviews,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    if !lane_surface::capture_record(bus, epoch, seats, record)? {
        return Ok(());
    }
    match publish_agent_messages(publisher, record, previews) {
        Ok(()) => Ok(()),
        Err(Outcome::Fault(fault)) => {
            let _ = writeln!(
                diagnostic,
                "canvas message unavailable: {}: {}",
                fault.reason, fault.detail
            );
            Ok(())
        }
        Err(outcome) => Err(outcome),
    }
}

pub(super) fn publish_agent_messages(
    publisher: &impl AgentMessagePublisher,
    record: &lane::capture::CaptureRecord,
    previews: &mut CallPreviews,
) -> CommandResult<()> {
    for (event, decoded) in record.decoded.events().iter().enumerate() {
        let (activity, locus, text) = match decoded {
            listen::Event::Prose(prose) => {
                (AgentActivityKind::Prose, prose.locus, prose.text.clone())
            }
            listen::Event::Tool(tool) => (
                AgentActivityKind::Tool,
                tool.locus,
                tool_preview::activity(record.lane, tool, previews),
            ),
            _ => continue,
        };
        let event = u32::try_from(event)
            .map_err(|error| store_fault("canvas::message-locus-invalid", &error.to_string()))?;
        publisher
            .publish(wire::AgentMessage {
                lane: record.lane,
                author: record.occupant,
                locus: wire::MessageLocus {
                    member: locus.member,
                    offset: locus.offset,
                    length: locus.length,
                    event,
                },
                activity,
                text,
            })
            .map_err(|error| store_fault("canvas::message-publish-failed", &error.to_string()))?;
    }
    Ok(())
}
