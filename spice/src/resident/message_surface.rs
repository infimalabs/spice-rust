//! Fallible Canvas message views over authoritative lane capture.

use std::io;

use bus::Bus;
use lane::occupant::Seats;
use wire::{CanonicalUuid, Outcome};

use super::{CommandResult, lane_surface, store_fault};

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
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    if !lane_surface::capture_record(bus, epoch, seats, record)? {
        return Ok(());
    }
    match publish_agent_messages(publisher, record) {
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
) -> CommandResult<()> {
    for (event, decoded) in record.decoded.events().iter().enumerate() {
        let listen::Event::Prose(prose) = decoded else {
            continue;
        };
        let event = u32::try_from(event)
            .map_err(|error| store_fault("canvas::message-locus-invalid", &error.to_string()))?;
        publisher
            .publish(wire::AgentMessage {
                lane: record.lane,
                author: record.occupant,
                locus: wire::MessageLocus {
                    member: prose.locus.member,
                    offset: prose.locus.offset,
                    length: prose.locus.length,
                    event,
                },
                text: prose.text.clone(),
            })
            .map_err(|error| store_fault("canvas::message-publish-failed", &error.to_string()))?;
    }
    Ok(())
}
