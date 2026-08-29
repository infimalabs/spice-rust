//! Durable taste recognition and fallible Canvas views over admitted lane capture.

use std::collections::BTreeMap;
use std::io;

use bus::Bus;
use lane::occupant::Seats;
use wire::{AgentActivityKind, CanonicalUuid, Outcome};

use super::{CommandResult, lane_surface, store_fault};
use super::{RuntimePaths, taste_surface};

mod backlog;
mod tool_preview;

pub(super) use backlog::{BacklogReader, restore_agent_messages};
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

/// Mutable per-record fold state at the admitted capture boundary.
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

enum TasteRecognition<'a> {
    Direct,
    Immediate(&'a mut dyn taste_surface::Adjudicator),
    Deferred(&'a mut taste_surface::AdjudicationSubmitter),
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
    let recognition = judge.map_or(TasteRecognition::Direct, TasteRecognition::Immediate);
    accept_capture_record_with(bus, epoch, seats, surfaces, recognition, record, views)
}

pub(super) fn accept_capture_record_deferred<P: AgentMessagePublisher>(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    surfaces: &CaptureSurfaces<'_, P>,
    judge: &mut taste_surface::AdjudicationSubmitter,
    record: &lane::capture::CaptureRecord,
    views: CaptureViews<'_>,
) -> CommandResult<()> {
    accept_capture_record_with(
        bus,
        epoch,
        seats,
        surfaces,
        TasteRecognition::Deferred(judge),
        record,
        views,
    )
}

fn accept_capture_record_with<P: AgentMessagePublisher>(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    surfaces: &CaptureSurfaces<'_, P>,
    recognition: TasteRecognition<'_>,
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
    match recognition {
        TasteRecognition::Deferred(judge) => {
            judge.recognize(bus, epoch, surfaces.tastes, record)?;
        }
        TasteRecognition::Immediate(judge) => taste_surface::recognize(
            bus,
            surfaces.paths,
            epoch,
            surfaces.tastes,
            surfaces.steering,
            Some(judge),
            record,
        )?,
        TasteRecognition::Direct => {
            // Disabled adjudication is deliberately direct: it starts no process
            // or worker and publishes the deterministic reminder immediately.
            taste_surface::recognize(
                bus,
                surfaces.paths,
                epoch,
                surfaces.tastes,
                surfaces.steering,
                None,
                record,
            )?;
        }
    }
    let steering = steering_ledger(bus, record);
    match publish_agent_messages(
        surfaces.publisher,
        record,
        views.previews,
        steering.as_ref(),
    ) {
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
    steering: Option<&steer::Ledger>,
) -> CommandResult<()> {
    for (event, decoded) in record.decoded.events().iter().enumerate() {
        let event = u32::try_from(event)
            .map_err(|error| store_fault("canvas::message-locus-invalid", &error.to_string()))?;
        publish_agent_event(
            publisher,
            record.lane,
            record.occupant,
            event,
            decoded,
            previews,
            steering,
        )?;
    }
    Ok(())
}

/// Project one typed tape event through the sole live/restored message policy.
pub(super) fn publish_agent_event(
    publisher: &impl AgentMessagePublisher,
    lane: CanonicalUuid,
    author: CanonicalUuid,
    event: u32,
    decoded: &listen::Event,
    previews: &mut CallPreviews,
    steering: Option<&steer::Ledger>,
) -> CommandResult<()> {
    let (activity, locus, text, media) = match decoded {
        listen::Event::Prose(prose) if prose.role == listen::ProseRole::Assistant => (
            AgentActivityKind::Prose,
            prose.locus,
            present_steering_acknowledgement(&prose.text, steering),
            prose.media.clone(),
        ),
        listen::Event::Tool(tool) => (
            AgentActivityKind::Tool,
            tool.locus,
            tool_preview::activity(lane, tool, previews),
            match &tool.moment {
                listen::ToolMoment::Result(outcome) => outcome.media.clone(),
                listen::ToolMoment::Call { .. } | listen::ToolMoment::Update { .. } => Vec::new(),
            },
        ),
        listen::Event::Compaction(compaction) => (
            AgentActivityKind::Compaction,
            compaction.locus,
            compaction
                .summary
                .clone()
                .filter(|summary| !summary.trim().is_empty())
                .unwrap_or_else(|| "Context compacted".to_owned()),
            Vec::new(),
        ),
        _ => return Ok(()),
    };
    wire::media::admit_media(&media)
        .map_err(|error| store_fault("canvas::message-media-invalid", &error.to_string()))?;
    publisher
        .publish(wire::AgentMessage {
            lane,
            author,
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
    Ok(())
}

fn steering_ledger(bus: &Bus, record: &lane::capture::CaptureRecord) -> Option<steer::Ledger> {
    record
        .decoded
        .events()
        .iter()
        .any(|event| {
            matches!(
                event,
                listen::Event::Prose(prose)
                    if prose.role == listen::ProseRole::Assistant
                        && steering_acknowledgement(&prose.text).is_some()
            )
        })
        .then(|| bus.fact_snapshot().ok())
        .flatten()
        .and_then(|snapshot| steer::Ledger::from_snapshot(&snapshot).ok())
}

fn present_steering_acknowledgement(text: &str, steering: Option<&steer::Ledger>) -> String {
    let Some(steering) = steering else {
        return text.to_owned();
    };
    let directives = directive_text_by_key(steering);
    present_steering_acknowledgement_with_directives(text, &directives)
        .unwrap_or_else(|| text.to_owned())
}

fn present_steering_acknowledgement_with_directives(
    text: &str,
    directives: &BTreeMap<String, String>,
) -> Option<String> {
    let acknowledgement = steering_acknowledgement(text)?;
    if !acknowledgement
        .keys
        .iter()
        .all(|key| directives.contains_key(*key))
    {
        return None;
    }

    let mut rendered = String::new();
    for key in &acknowledgement.keys {
        if !rendered.is_empty() {
            rendered.push_str("\n\n");
        }
        append_quote(&mut rendered, &directives[*key]);
    }
    let prose = acknowledgement.prose.trim_start();
    if !prose.is_empty() {
        if !rendered.is_empty() {
            rendered.push_str("\n\n");
        }
        rendered.push_str(prose);
    }
    Some(rendered)
}

fn directive_text_by_key(steering: &steer::Ledger) -> BTreeMap<String, String> {
    steering
        .directives()
        .into_iter()
        .map(|directive| (directive.key().to_string(), directive.text().to_owned()))
        .collect()
}

fn append_quote(rendered: &mut String, directive: &str) {
    for (index, line) in directive.lines().enumerate() {
        if index > 0 {
            rendered.push('\n');
        }
        rendered.push_str("> ");
        rendered.push_str(line);
    }
}

#[derive(Debug, Clone)]
struct SteeringAcknowledgement<'a> {
    keys: Vec<&'a str>,
    prose: &'a str,
}

fn steering_acknowledgement(text: &str) -> Option<SteeringAcknowledgement<'_>> {
    let trimmed = text.trim_start();
    let leading = text.len() - trimmed.len();
    let line = trimmed.lines().next()?;
    let (marker_len, rest) = line
        .strip_prefix("ACK ")
        .map(|rest| ("ACK".len(), rest))
        .or_else(|| line.strip_prefix("NACK ").map(|rest| ("NACK".len(), rest)))?;
    let colon = rest.find(':')?;
    let keys = rest[..colon].split_whitespace().collect::<Vec<_>>();
    if keys.is_empty() || !keys.iter().all(|key| key.parse::<steer::Key>().is_ok()) {
        return None;
    }
    let prefix_len = leading + marker_len + 1 + colon + 1;
    Some(SteeringAcknowledgement {
        keys,
        prose: text.get(prefix_len..)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steering_acknowledgement_quotes_directive_and_strips_transport_clause() {
        let directives = BTreeMap::from([(
            "1kQnrzNZ".to_owned(),
            "Keep one tested, visible current path.".to_owned(),
        )]);

        assert_eq!(
            present_steering_acknowledgement_with_directives(
                "ACK 1kQnrzNZ: Landed in the stream presentation path.",
                &directives,
            )
            .as_deref(),
            Some(
                "> Keep one tested, visible current path.\n\nLanded in the stream presentation path."
            ),
        );
    }

    #[test]
    fn steering_acknowledgement_expands_keys_in_presentation_order() {
        let directives = BTreeMap::from([
            ("1kQnrzNZ".to_owned(), "First directive.".to_owned()),
            (
                "1kQnqLHc".to_owned(),
                "Second directive.\nPreserve remaining prose.".to_owned(),
            ),
        ]);

        assert_eq!(
            present_steering_acknowledgement_with_directives(
                "ACK 1kQnrzNZ 1kQnqLHc: Done.",
                &directives,
            )
            .as_deref(),
            Some("> First directive.\n\n> Second directive.\n> Preserve remaining prose.\n\nDone."),
        );
    }

    #[test]
    fn steering_acknowledgement_without_directive_context_is_not_rewritten() {
        let directives = BTreeMap::new();

        assert_eq!(
            present_steering_acknowledgement_with_directives(
                "ACK 1kQnrzNZ: The original key stays visible when context is unavailable.",
                &directives,
            ),
            None,
        );
    }
}
