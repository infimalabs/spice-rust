//! Durable taste recognition and fallible Canvas views over admitted lane capture.

use std::io;

use bus::Bus;
use lane::occupant::Seats;
use wire::{AgentActivityKind, CanonicalUuid, Outcome};

use super::{CommandResult, lane_surface, store_fault};
use super::{RuntimePaths, taste_surface};

mod tool_preview;

pub(super) use tool_preview::CallPreviews;

/// Captured-generation policy and disposable publishers used after one record
/// passes occupant admission.
pub(super) struct CaptureSurfaces<'a, P> {
    paths: &'a RuntimePaths,
    tastes: &'a taste::Bag,
    steering: &'a steer::Config,
    publisher: &'a P,
}

impl<'a, P> CaptureSurfaces<'a, P> {
    pub(super) const fn new(
        paths: &'a RuntimePaths,
        tastes: &'a taste::Bag,
        steering: &'a steer::Config,
        publisher: &'a P,
    ) -> Self {
        Self {
            paths,
            tastes,
            steering,
            publisher,
        }
    }
}

/// Mutable per-record projection state at the admitted capture boundary.
pub(super) struct CaptureViews<'a> {
    previews: &'a mut CallPreviews,
    diagnostic: &'a mut dyn io::Write,
}

impl<'a> CaptureViews<'a> {
    pub(super) fn new(previews: &'a mut CallPreviews, diagnostic: &'a mut dyn io::Write) -> Self {
        Self {
            previews,
            diagnostic,
        }
    }
}

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

pub(super) fn accept_capture_record<P: AgentMessagePublisher>(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    surfaces: &CaptureSurfaces<'_, P>,
    judge: Option<&mut dyn taste_surface::Adjudicator>,
    record: &lane::capture::CaptureRecord,
    views: CaptureViews<'_>,
) -> CommandResult<()> {
    if !lane_surface::capture_record(bus, epoch, seats, record)? {
        return Ok(());
    }
    // The bytes behind this record are already durable, so accounting them here
    // keeps the transcript head level with the records the resident has taken
    // responsibility for, rather than leaving the gap for the next start to
    // find. A record the resident did not accept above does not account either.
    super::ingest_transcripts(bus, surfaces.paths, Some(record.lane), views.diagnostic)?;
    taste_surface::recognize(
        bus,
        surfaces.paths,
        epoch,
        surfaces.tastes,
        surfaces.steering,
        judge,
        record,
    )?;
    match publish_agent_messages(surfaces.publisher, record, views.previews) {
        Ok(()) => Ok(()),
        Err(Outcome::Fault(fault)) => {
            let _ = writeln!(
                views.diagnostic,
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
        let (activity, locus, text, media) = match decoded {
            listen::Event::Prose(prose) => (
                AgentActivityKind::Prose,
                prose.locus,
                prose.text.clone(),
                prose.media.clone(),
            ),
            listen::Event::Tool(tool) => (
                AgentActivityKind::Tool,
                tool.locus,
                tool_preview::activity(record.lane, tool, previews),
                match &tool.moment {
                    listen::ToolMoment::Result(outcome) => outcome.media.clone(),
                    listen::ToolMoment::Call { .. } | listen::ToolMoment::Update { .. } => {
                        Vec::new()
                    }
                },
            ),
            _ => continue,
        };
        wire::media::admit_media(&media)
            .map_err(|error| store_fault("canvas::message-media-invalid", &error.to_string()))?;
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
                media,
            })
            .map_err(|error| store_fault("canvas::message-publish-failed", &error.to_string()))?;
    }
    Ok(())
}
