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
    state: &'a mut MessageState,
    diagnostic: &'a mut dyn io::Write,
}

impl<'a> CaptureViews<'a> {
    pub(super) fn new(state: &'a mut MessageState, diagnostic: &'a mut dyn io::Write) -> Self {
        Self { state, diagnostic }
    }
}

#[derive(Debug, Default)]
pub(in crate::resident) struct MessageState {
    previews: CallPreviews,
    observations: MessageObservations,
}

#[derive(Clone, Debug)]
struct MessageObservation {
    lane: CanonicalUuid,
    from: wire::body::TranscriptPosition,
    to: wire::body::TranscriptPosition,
    at: wire::Instant,
}

#[derive(Debug, Default)]
struct MessageObservations(Vec<MessageObservation>);

impl MessageObservations {
    fn from_snapshot(snapshot: &journal::FactSnapshot) -> Self {
        let mut observations = Self::default();
        for envelope in snapshot.facts() {
            if let Some(batch) = envelope.fact.read::<wire::body::JournalIngested>() {
                observations.push(&batch, envelope.at);
            }
        }
        observations
    }

    fn extend(&mut self, observations: Vec<super::TranscriptObservation>) {
        for observation in observations {
            self.push(&observation.batch, observation.at);
        }
    }

    fn push(&mut self, batch: &wire::body::JournalIngested, at: wire::Instant) {
        self.0.push(MessageObservation {
            lane: batch.lane,
            from: batch.from,
            to: batch.to,
            at,
        });
    }

    fn at(&self, lane: CanonicalUuid, locus: listen::Locus) -> Option<wire::Instant> {
        let position = (locus.member, locus.offset);
        self.0
            .iter()
            .rev()
            .find(|observation| {
                observation.lane == lane
                    && (observation.from.member, observation.from.offset) <= position
                    && position < (observation.to.member, observation.to.offset)
            })
            .map(|observation| observation.at)
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
    let observations =
        super::ingest_transcripts(bus, surfaces.paths, Some(record.lane), views.diagnostic)?;
    views.state.observations.extend(observations);
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
    let steering = steering_ledger(bus, record)?;
    match publish_agent_messages(surfaces.publisher, record, views.state, steering.as_ref()) {
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
    state: &mut MessageState,
    steering: Option<&steer::Ledger>,
) -> CommandResult<()> {
    let authored = record
        .decoded
        .context()
        .and_then(|context| context.timestamp.as_deref());
    for (event, decoded) in record.decoded.events().iter().enumerate() {
        let event = u32::try_from(event)
            .map_err(|error| store_fault("canvas::message-locus-invalid", &error.to_string()))?;
        publish_agent_event(
            publisher,
            AgentEvent {
                lane: record.lane,
                author: record.occupant,
                ordinal: event,
                decoded,
                authored,
                observations: &state.observations,
            },
            &mut state.previews,
            steering,
        )?;
    }
    Ok(())
}

struct AgentEvent<'a> {
    lane: CanonicalUuid,
    author: CanonicalUuid,
    ordinal: u32,
    decoded: &'a listen::Event,
    authored: Option<&'a str>,
    observations: &'a MessageObservations,
}

/// Project one typed tape event through the sole live/restored message policy.
fn publish_agent_event(
    publisher: &impl AgentMessagePublisher,
    event: AgentEvent<'_>,
    previews: &mut CallPreviews,
    steering: Option<&steer::Ledger>,
) -> CommandResult<()> {
    let AgentEvent {
        lane,
        author,
        ordinal,
        decoded,
        authored,
        observations,
    } = event;
    let (activity, locus, text, media) = match decoded {
        listen::Event::Prose(prose) if prose.role == listen::ProseRole::Assistant => (
            AgentActivityKind::Prose,
            prose.locus,
            present_steering_acknowledgements(&prose.text, steering)?,
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
    let (authored, authored_synthetic) = parse_agent_message_authored(authored, locus, ordinal)?;
    let observed = authored_synthetic
        .then(|| observations.at(lane, locus))
        .flatten();
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
                event: ordinal,
            },
            authored,
            authored_synthetic,
            observed,
            activity,
            text,
            media,
        })
        .map_err(|error| store_fault("canvas::message-publish-failed", &error.to_string()))?;
    Ok(())
}

fn parse_agent_message_authored(
    timestamp: Option<&str>,
    locus: listen::Locus,
    event: u32,
) -> CommandResult<(wire::Instant, bool)> {
    match timestamp {
        Some(timestamp) => timestamp
            .parse::<wire::Instant>()
            .map(|authored| (authored, false))
            .map_err(|error| store_fault("canvas::message-authored-invalid", &error.to_string())),
        None => synthetic_agent_message_authored(locus, event).map(|authored| (authored, true)),
    }
}

fn synthetic_agent_message_authored(
    locus: listen::Locus,
    event: u32,
) -> CommandResult<wire::Instant> {
    // Some current Codex records omit vendor time. The wire identity still
    // needs a stable ordering key, so derive one from durable tape coordinates;
    // presentation must use `observed` and never expose this surrogate as time.
    const BASE_MILLIS: i64 = 946_684_800_000;
    const MEMBER_MILLIS: i64 = 86_400_000;

    let member = i64::try_from(locus.member.saturating_sub(1)).map_err(|error| {
        store_fault(
            "canvas::message-authored-unrepresentable",
            &error.to_string(),
        )
    })?;
    let offset = i64::try_from(locus.offset).map_err(|error| {
        store_fault(
            "canvas::message-authored-unrepresentable",
            &error.to_string(),
        )
    })?;
    let delta = member
        .checked_mul(MEMBER_MILLIS)
        .and_then(|millis| millis.checked_add(offset))
        .and_then(|millis| millis.checked_add(i64::from(event)))
        .ok_or_else(|| {
            store_fault(
                "canvas::message-authored-unrepresentable",
                "transcript locus cannot fit a synthetic authored instant",
            )
        })?;
    let millis = BASE_MILLIS.checked_add(delta).ok_or_else(|| {
        store_fault(
            "canvas::message-authored-unrepresentable",
            "transcript locus cannot fit a synthetic authored instant",
        )
    })?;
    let authored = jiff::Timestamp::from_millisecond(millis).map_err(|error| {
        store_fault(
            "canvas::message-authored-unrepresentable",
            &error.to_string(),
        )
    })?;
    wire::Instant::new(authored).map_err(|error| {
        store_fault(
            "canvas::message-authored-unrepresentable",
            &error.to_string(),
        )
    })
}

pub(super) fn steering_ledger(
    bus: &Bus,
    record: &lane::capture::CaptureRecord,
) -> CommandResult<Option<steer::Ledger>> {
    let required = record.decoded.events().iter().any(|event| {
        matches!(
            event,
            listen::Event::Prose(prose)
                if prose.role == listen::ProseRole::Assistant
                    && steer::has_acknowledgement(&prose.text)
        )
    });
    if !required {
        return Ok(None);
    }
    let snapshot = bus
        .fact_snapshot()
        .map_err(|error| store_fault("canvas::message-steering-unavailable", &error.to_string()))?;
    steer::Ledger::from_snapshot(&snapshot)
        .map(Some)
        .map_err(|error| store_fault("canvas::message-steering-invalid", &error.to_string()))
}

fn present_steering_acknowledgements(
    text: &str,
    steering: Option<&steer::Ledger>,
) -> CommandResult<String> {
    let Some(steering) = steering else {
        return Ok(text.to_owned());
    };
    present_steering_acknowledgements_with_directives(
        text,
        &PresentationDirectives::from_ledger(steering),
    )
    .map_err(|detail| store_fault("canvas::message-steering-presentation-invalid", &detail))
}

#[derive(Debug, Default)]
struct PresentationDirectives(BTreeMap<steer::Key, String>);

impl PresentationDirectives {
    fn from_ledger(steering: &steer::Ledger) -> Self {
        Self(
            steering
                .directives()
                .into_iter()
                .map(|directive| (directive.key().clone(), directive.text().to_owned()))
                .collect(),
        )
    }
}

impl steer::Standing for PresentationDirectives {
    fn disposition(&self, key: &steer::Key) -> Option<wire::Disposition> {
        // Presentation replays acknowledgements after their directives retired.
        // "Live" here lets the canonical scanner recover marker polarity; it
        // does not mutate the durable steering ledger.
        self.0.contains_key(key).then_some(wire::Disposition::Live)
    }
}

#[derive(Debug, Default)]
struct LineAcknowledgements {
    recognized: Vec<(steer::Key, wire::Disposition)>,
    missed: Vec<String>,
}

fn present_steering_acknowledgements_with_directives(
    text: &str,
    directives: &PresentationDirectives,
) -> Result<String, String> {
    let mut lines = BTreeMap::<usize, LineAcknowledgements>::new();
    for recognition in steer::scan(text, directives) {
        let line = lines.entry(recognition.locus.line).or_default();
        match recognition.outcome {
            steer::ack::scan::Outcome::Retired { key, disposition } => {
                if !line
                    .recognized
                    .iter()
                    .any(|known| known == &(key.clone(), disposition))
                {
                    line.recognized.push((key, disposition));
                }
            }
            steer::ack::scan::Outcome::Missed { token, .. } => line.missed.push(token),
        }
    }
    if lines.is_empty() {
        return Ok(text.to_owned());
    }

    let mut rendered = Vec::new();
    for (index, source) in text.split('\n').enumerate() {
        match lines.get(&(index + 1)) {
            Some(acknowledgements) => rendered.push(render_acknowledgement_line(
                source,
                acknowledgements,
                directives,
            )?),
            None => rendered.push(source.to_owned()),
        }
    }
    Ok(rendered.join("\n"))
}

fn render_acknowledgement_line(
    source: &str,
    acknowledgements: &LineAcknowledgements,
    directives: &PresentationDirectives,
) -> Result<String, String> {
    if acknowledgements.recognized.is_empty() {
        return Ok(source.to_owned());
    }
    let colon = source.find(':').ok_or_else(|| {
        "canonical acknowledgment recognition named a line without a colon".to_owned()
    })?;
    let mut rendered = String::new();
    for (key, disposition) in &acknowledgements.recognized {
        let directive = directives
            .0
            .get(key)
            .ok_or_else(|| format!("canonical acknowledgment recognition lost directive {key}"))?;
        if !rendered.is_empty() {
            rendered.push_str("\n\n");
        }
        append_acknowledgement_quote(&mut rendered, key, *disposition, directive)?;
    }
    let prose = source
        .get(colon + 1..)
        .ok_or_else(|| "acknowledgment body crossed a non-character boundary".to_owned())?
        .trim_start();
    if acknowledgements.missed.is_empty() {
        if !prose.is_empty() {
            rendered.push_str("\n\n");
            rendered.push_str(prose);
        }
    } else {
        rendered.push_str("\n\n");
        rendered.push_str(acknowledgement_marker(acknowledgements.recognized[0].1)?);
        for token in &acknowledgements.missed {
            rendered.push(' ');
            rendered.push_str(token);
        }
        rendered.push(':');
        if !prose.is_empty() {
            rendered.push(' ');
            rendered.push_str(prose);
        }
    }
    Ok(rendered)
}

fn acknowledgement_marker(disposition: wire::Disposition) -> Result<&'static str, String> {
    match disposition {
        wire::Disposition::Acked => Ok("ACK"),
        wire::Disposition::Declined => Ok("NACK"),
        wire::Disposition::Live
        | wire::Disposition::Lapsed
        | wire::Disposition::DeadLettered
        | wire::Disposition::Canceled => Err(format!(
            "acknowledgment scanner returned nonterminal disposition {disposition:?}"
        )),
    }
}

fn append_acknowledgement_quote(
    rendered: &mut String,
    key: &steer::Key,
    disposition: wire::Disposition,
    directive: &str,
) -> Result<(), String> {
    let marker = acknowledgement_marker(disposition)?;
    rendered.push_str("> ");
    rendered.push_str(marker);
    rendered.push(' ');
    rendered.push_str(key.as_str());
    rendered.push_str(": ");
    for (index, line) in directive.lines().enumerate() {
        if index > 0 {
            rendered.push_str("\n> ");
        }
        rendered.push_str(line);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directives(rows: &[(&str, &str)]) -> PresentationDirectives {
        PresentationDirectives(
            rows.iter()
                .map(|(key, text)| {
                    (
                        key.parse::<steer::Key>().expect("fixture key parses"),
                        (*text).to_owned(),
                    )
                })
                .collect(),
        )
    }

    #[test]
    fn steering_acknowledgement_renders_key_context_and_agent_prose_once() {
        let directives = directives(&[("1kQnrzNZ", "Keep one tested, visible current path.")]);
        assert_eq!(
            present_steering_acknowledgements_with_directives(
                "ACK 1kQnrzNZ: Landed in the stream presentation path.",
                &directives,
            )
            .expect("acknowledgment presents"),
            "> ACK 1kQnrzNZ: Keep one tested, visible current path.\n\nLanded in the stream presentation path.",
        );
    }

    #[test]
    fn distributed_acknowledgements_render_every_context_without_repeating_the_message() {
        let text = concat!(
            "ACK 1kQwPZtK: The current completion path is singular and visible.\n\n",
            "ACK 1kQwPZtL: Historical records remain evidence only."
        );
        let directives = directives(&[
            ("1kQwPZtK", "Keep one current completion path."),
            ("1kQwPZtL", "Do not execute historical records."),
        ]);

        let rendered = present_steering_acknowledgements_with_directives(text, &directives)
            .expect("distributed acknowledgments present");
        assert_eq!(rendered.matches("The current completion path").count(), 1);
        assert_eq!(rendered.matches("Historical records").count(), 1);
        assert_eq!(rendered.matches("> ACK ").count(), 2);
        assert!(rendered.contains("> ACK 1kQwPZtK: Keep one current completion path."));
        assert!(rendered.contains("> ACK 1kQwPZtL: Do not execute historical records."));
    }

    #[test]
    fn shared_acknowledgement_prose_renders_once_after_every_context() {
        let directives = directives(&[
            ("1kQwPZtK", "Keep one current completion path."),
            ("1kQwPZtL", "Do not execute historical records."),
        ]);

        assert_eq!(
            present_steering_acknowledgements_with_directives(
                "ACK 1kQwPZtK 1kQwPZtL: One response covers both directions.",
                &directives,
            )
            .expect("shared acknowledgment presents"),
            concat!(
                "> ACK 1kQwPZtK: Keep one current completion path.\n\n",
                "> ACK 1kQwPZtL: Do not execute historical records.\n\n",
                "One response covers both directions."
            ),
        );
    }

    #[test]
    fn recognized_acknowledgement_disposition_renders_beside_each_missed_sibling() {
        let directives = directives(&[("1kQwPZtK", "Keep the recognized direction visible.")]);
        for marker in ["ACK", "NACK"] {
            let text = format!(
                "{marker} 1kQwPZtK nope 1kQwPZtM: Shared prose remains attached to the attempt."
            );
            let expected = format!(
                "> {marker} 1kQwPZtK: Keep the recognized direction visible.\n\n\
                 {marker} nope 1kQwPZtM: Shared prose remains attached to the attempt."
            );

            let rendered = present_steering_acknowledgements_with_directives(&text, &directives)
                .expect("mixed acknowledgment presents");
            assert_eq!(rendered, expected);
            assert_eq!(rendered.matches("1kQwPZtK").count(), 1);
            assert_eq!(rendered.matches("nope").count(), 1);
            assert_eq!(rendered.matches("1kQwPZtM").count(), 1);
            assert_eq!(rendered.matches("Shared prose").count(), 1);
        }
    }

    #[test]
    fn mixed_acknowledgement_dispositions_keep_their_own_keys_and_contexts() {
        let directives = directives(&[
            ("1kQwPZtK", "Apply the current path."),
            ("1kQwPZtL", "Remove the safety check."),
        ]);
        assert_eq!(
            present_steering_acknowledgements_with_directives(
                "ACK 1kQwPZtK: Applied.\nNACK 1kQwPZtL: Unsafe.",
                &directives,
            )
            .expect("mixed acknowledgments present"),
            "> ACK 1kQwPZtK: Apply the current path.\n\nApplied.\n> NACK 1kQwPZtL: Remove the safety check.\n\nUnsafe.",
        );
    }

    #[test]
    fn unknown_key_preserves_the_original_message_across_replay() {
        let text = "ACK 1kQwPZtK: No directive owns this key.";
        let directives = PresentationDirectives::default();
        let first = present_steering_acknowledgements_with_directives(text, &directives)
            .expect("unknown key remains visible");
        let replayed = present_steering_acknowledgements_with_directives(text, &directives)
            .expect("unknown key replay remains visible");

        assert_eq!(first, text);
        assert_eq!(replayed, first);
    }
}
