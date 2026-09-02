//! Durable taste recognition and fallible Canvas views over admitted lane capture.

use std::collections::BTreeMap;
use std::io;

use bus::Bus;
use lane::occupant::Seats;
use wire::{AgentActivityKind, CanonicalUuid, Outcome};

use super::{CommandResult, lane_surface, store_fault};
use super::{RuntimePaths, taste_surface};

mod backlog;
mod session_compaction;
mod tool_preview;

const SESSION_MESSAGE_MEMBER_NAMESPACE: u64 = 1_u64 << 63;

pub(super) use backlog::{BacklogReader, restore_agent_messages};
pub(super) use session_compaction::{
    POLL_INTERVAL as SESSION_COMPACTION_POLL_INTERVAL, SessionCompactions,
};
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
    source: CanonicalUuid,
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
            source: batch.source,
            from: batch.from,
            to: batch.to,
            at,
        });
    }

    fn at(&self, lane: CanonicalUuid, locus: listen::Locus) -> Option<wire::Instant> {
        self.find(lane, locus).map(|observation| observation.at)
    }

    fn source(&self, lane: CanonicalUuid, locus: listen::Locus) -> Option<CanonicalUuid> {
        self.find(lane, locus).map(|observation| observation.source)
    }

    fn find(&self, lane: CanonicalUuid, locus: listen::Locus) -> Option<&MessageObservation> {
        let position = (locus.member, locus.offset);
        self.0.iter().rev().find(|observation| {
            observation.lane == lane
                && (observation.from.member, observation.from.offset) <= position
                && position < (observation.to.member, observation.to.offset)
        })
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
    steering: Option<&PresentationDirectives>,
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
                source: None,
                ordinal: event,
                decoded,
                authored,
                observed: None,
                observations: &state.observations,
                unavailable_tool_terminals: &[],
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
    source: Option<CanonicalUuid>,
    ordinal: u32,
    decoded: &'a listen::Event,
    authored: Option<&'a str>,
    observed: Option<wire::Instant>,
    observations: &'a MessageObservations,
    unavailable_tool_terminals: &'a [String],
}

/// Project one typed tape event through the sole live/restored message policy.
fn publish_agent_event(
    publisher: &impl AgentMessagePublisher,
    event: AgentEvent<'_>,
    previews: &mut CallPreviews,
    steering: Option<&PresentationDirectives>,
) -> CommandResult<()> {
    let AgentEvent {
        lane,
        author,
        source,
        ordinal,
        decoded,
        authored,
        observed,
        observations,
        unavailable_tool_terminals,
    } = event;
    let (activity, locus, text, media) = match decoded {
        listen::Event::Prose(prose) if prose.role == listen::ProseRole::Assistant => (
            AgentActivityKind::Prose,
            prose.locus,
            present_steering_acknowledgements(
                &prose.text,
                steering,
                PresentationEvent {
                    lane,
                    source: source.or_else(|| observations.source(lane, prose.locus)),
                    locus: prose.locus,
                    ordinal,
                },
            )?,
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
        .then(|| observed.or_else(|| observations.at(lane, locus)))
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
            unavailable_tool_terminals: unavailable_tool_terminals.to_vec(),
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

    // A preserved session is one file, so its namespaced member identifies the
    // source rather than advancing tape chronology. Its byte offset still gives
    // every timestamp-less event a stable in-file order, while the unchanged
    // locus carries the full session identity through publication and dedupe.
    let member = if locus.member & SESSION_MESSAGE_MEMBER_NAMESPACE == 0 {
        locus.member.saturating_sub(1)
    } else {
        0
    };
    let member = i64::try_from(member).map_err(|error| {
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
) -> CommandResult<Option<PresentationDirectives>> {
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
    PresentationDirectives::from_snapshot(&snapshot)
        .map(Some)
        .map_err(|error| store_fault("canvas::message-steering-invalid", &error.to_string()))
}

fn present_steering_acknowledgements(
    text: &str,
    steering: Option<&PresentationDirectives>,
    event: PresentationEvent,
) -> CommandResult<String> {
    let Some(steering) = steering else {
        return Ok(text.to_owned());
    };
    present_steering_acknowledgements_with_directives(text, steering, event)
        .map_err(|detail| store_fault("canvas::message-steering-presentation-invalid", &detail))
}

#[derive(Debug, Default)]
pub(super) struct PresentationDirectives(BTreeMap<steer::Key, PresentationDirective>);

#[derive(Debug)]
struct PresentationDirective {
    lane: CanonicalUuid,
    text: String,
    disposition: wire::Disposition,
    retirement: Option<wire::body::TranscriptLocus>,
}

impl PresentationDirectives {
    fn from_snapshot(snapshot: &journal::FactSnapshot) -> Result<Self, String> {
        let steering = steer::Ledger::from_snapshot(snapshot).map_err(|error| error.to_string())?;
        let mut retirements = BTreeMap::new();
        for envelope in snapshot.facts() {
            if let Some(body) = envelope.fact.read::<wire::body::SteerAcked>() {
                let key = body
                    .key
                    .parse::<steer::Key>()
                    .map_err(|error| error.to_string())?;
                retirements.insert(key, body.locus);
            } else if let Some(body) = envelope.fact.read::<wire::body::SteerDeclined>() {
                let key = body
                    .key
                    .parse::<steer::Key>()
                    .map_err(|error| error.to_string())?;
                retirements.insert(key, body.locus);
            }
        }
        Ok(Self(
            steering
                .directives()
                .into_iter()
                .map(|directive| {
                    let key = directive.key().clone();
                    let retirement = retirements.get(&key).copied();
                    (
                        key,
                        PresentationDirective {
                            lane: directive.lane(),
                            text: directive.text().to_owned(),
                            disposition: directive.disposition(),
                            retirement,
                        },
                    )
                })
                .collect(),
        ))
    }
}

#[derive(Clone, Copy, Debug)]
struct PresentationEvent {
    lane: CanonicalUuid,
    source: Option<CanonicalUuid>,
    locus: listen::Locus,
    ordinal: u32,
}

struct EventDirectives<'a> {
    directives: &'a PresentationDirectives,
    event: PresentationEvent,
}

impl EventDirectives<'_> {
    fn recognizes(
        &self,
        key: &steer::Key,
        disposition: wire::Disposition,
        locus: steer::ack::scan::Locus,
    ) -> bool {
        let Some(directive) = self.directives.0.get(key) else {
            return false;
        };
        directive.lane == self.event.lane
            && (directive.disposition == wire::Disposition::Live
                || (directive.disposition == disposition
                    && directive.retirement.is_some_and(|retirement| {
                        retirement_matches(retirement, self.event, locus)
                    })))
    }

    fn historical_context(&self, token: &str, miss: steer::ack::scan::Miss) -> Option<steer::Key> {
        if miss != steer::ack::scan::Miss::Historical {
            return None;
        }
        let key = token.parse::<steer::Key>().ok()?;
        let directive = self.directives.0.get(&key)?;
        (directive.lane == self.event.lane).then_some(key)
    }
}

impl steer::Standing for EventDirectives<'_> {
    fn disposition(&self, key: &steer::Key) -> Option<wire::Disposition> {
        let directive = self.directives.0.get(key)?;
        if directive.disposition == wire::Disposition::Live
            || (directive.lane == self.event.lane
                && directive
                    .retirement
                    .is_some_and(|retirement| retirement_record_matches(retirement, self.event)))
        {
            Some(wire::Disposition::Live)
        } else {
            Some(directive.disposition)
        }
    }
}

fn retirement_record_matches(
    retirement: wire::body::TranscriptLocus,
    event: PresentationEvent,
) -> bool {
    retirement
        .source
        .is_none_or(|expected| event.source == Some(expected))
        && retirement.member == event.locus.member
        && retirement.offset == event.locus.offset
        && retirement.length == event.locus.length
        && retirement.event == event.ordinal
}

fn retirement_matches(
    retirement: wire::body::TranscriptLocus,
    event: PresentationEvent,
    locus: steer::ack::scan::Locus,
) -> bool {
    retirement_record_matches(retirement, event)
        && retirement.line as usize == locus.line
        && retirement.column as usize == locus.column
}

fn present_steering_acknowledgements_with_directives(
    text: &str,
    directives: &PresentationDirectives,
    event: PresentationEvent,
) -> Result<String, String> {
    let standing = EventDirectives { directives, event };
    let mut rendered = String::new();
    let mut cursor = 0;
    let mut changed = false;
    let mut capitalize_body = false;
    for acknowledgement in steer::ack::scan::scan_acknowledgements(text, &standing) {
        let separator = acknowledgement.separator;
        let mut recognized = Vec::new();
        let mut contexts = Vec::new();
        let mut missed = Vec::new();
        for recognition in acknowledgement.recognitions {
            match recognition.outcome {
                steer::ack::scan::Outcome::Retired { key, disposition }
                    if standing.recognizes(&key, disposition, recognition.locus) =>
                {
                    if !recognized
                        .iter()
                        .any(|known| known == &(key.clone(), disposition))
                    {
                        recognized.push((key.clone(), disposition));
                    }
                    if !contexts.iter().any(|known| known == &key) {
                        contexts.push(key);
                    }
                }
                steer::ack::scan::Outcome::Retired { key, .. } => {
                    missed.push(key.to_string());
                }
                steer::ack::scan::Outcome::Missed { token, miss } => {
                    if let Some(key) = standing.historical_context(&token, miss) {
                        if !contexts.iter().any(|known| known == &key) {
                            contexts.push(key);
                        }
                    } else {
                        missed.push(token);
                    }
                }
            }
        }
        // Historical keys add display context only when this packet also
        // carries a retirement authenticated to the current event.
        if recognized.is_empty() {
            continue;
        }

        let source = text
            .get(cursor..acknowledgement.start)
            .ok_or_else(|| "acknowledgment header crossed a non-character boundary".to_owned())?;
        append_presented_source(&mut rendered, source, capitalize_body);
        capitalize_body = false;
        start_quote_block(&mut rendered);
        for (index, key) in contexts.iter().enumerate() {
            let directive = directives.0.get(key).ok_or_else(|| {
                format!("canonical acknowledgment recognition lost directive {key}")
            })?;
            if index > 0 {
                rendered.push_str("\n\n");
            }
            append_acknowledgement_quote(&mut rendered, &directive.text);
        }
        rendered.push_str("\n\n");
        cursor = acknowledgement.end;
        if missed.is_empty() {
            cursor = skip_horizontal_space(text, cursor);
            capitalize_body = true;
        } else {
            rendered.push_str(acknowledgement_marker(recognized[0].1)?);
            for token in missed {
                rendered.push(' ');
                rendered.push_str(&token);
            }
            if let Some(separator) = separator {
                rendered.push(separator);
            }
        }
        changed = true;
    }

    if !changed {
        return Ok(text.to_owned());
    }
    let source = text
        .get(cursor..)
        .ok_or_else(|| "acknowledgment body crossed a non-character boundary".to_owned())?;
    append_presented_source(&mut rendered, source, capitalize_body);
    Ok(rendered)
}

fn append_presented_source(rendered: &mut String, source: &str, capitalize: bool) {
    if !capitalize {
        rendered.push_str(source);
        return;
    }
    let mut characters = source.chars();
    let Some(first) = characters.next() else {
        return;
    };
    rendered.extend(first.to_uppercase());
    rendered.push_str(characters.as_str());
}

fn start_quote_block(rendered: &mut String) {
    while rendered.ends_with([' ', '\t']) {
        rendered.pop();
    }
    if !rendered.is_empty() && !rendered.ends_with('\n') {
        rendered.push('\n');
    }
}

fn skip_horizontal_space(text: &str, mut cursor: usize) -> usize {
    while text
        .as_bytes()
        .get(cursor)
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        cursor += 1;
    }
    cursor
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

fn append_acknowledgement_quote(rendered: &mut String, directive: &str) {
    for (index, line) in directive.split('\n').enumerate() {
        if index > 0 {
            rendered.push('\n');
        }
        rendered.push('>');
        if !line.is_empty() {
            rendered.push(' ');
        }
        rendered.push_str(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane() -> CanonicalUuid {
        "00000000-0000-4000-8000-000000000001"
            .parse()
            .expect("fixture lane")
    }

    fn event() -> PresentationEvent {
        PresentationEvent {
            lane: lane(),
            source: None,
            locus: listen::Locus::new(1, 0, 1),
            ordinal: 0,
        }
    }

    fn directives(rows: &[(&str, &str)]) -> PresentationDirectives {
        PresentationDirectives(
            rows.iter()
                .map(|(key, text)| {
                    (
                        key.parse::<steer::Key>().expect("fixture key parses"),
                        PresentationDirective {
                            lane: lane(),
                            text: (*text).to_owned(),
                            disposition: wire::Disposition::Live,
                            retirement: None,
                        },
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
                event(),
            )
            .expect("acknowledgment presents"),
            "> Keep one tested, visible current path.\n\nLanded in the stream presentation path.",
        );
    }

    #[test]
    fn current_header_forms_hide_framing_and_capitalize_response_prose() {
        let directives = directives(&[("1kQnrzNZ", "Keep the current path.")]);
        for text in [
            "ACK 1kQnrzNZ: applied.",
            "ACK 1kQnrzNZ. applied.",
            "ACK 1kQnrzNZ applied.",
            "**ACK 1kQnrzNZ:** applied.",
            "**ACK** **1kQnrzNZ**:** applied.",
        ] {
            assert_eq!(
                present_steering_acknowledgements_with_directives(text, &directives, event())
                    .expect("acknowledgment presents"),
                "> Keep the current path.\n\nApplied.",
                "wrong presentation for {text:?}",
            );
        }
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

        let rendered =
            present_steering_acknowledgements_with_directives(text, &directives, event())
                .expect("distributed acknowledgments present");
        assert_eq!(rendered.matches("The current completion path").count(), 1);
        assert_eq!(rendered.matches("Historical records").count(), 1);
        assert_eq!(rendered.matches("ACK ").count(), 0);
        assert!(rendered.contains("> Keep one current completion path."));
        assert!(rendered.contains("> Do not execute historical records."));
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
                event(),
            )
            .expect("shared acknowledgment presents"),
            concat!(
                "> Keep one current completion path.\n\n",
                "> Do not execute historical records.\n\n",
                "One response covers both directions."
            ),
        );
    }

    #[test]
    fn non_key_text_after_a_recognized_header_remains_response_prose() {
        let directives = directives(&[("1kQwPZtK", "Keep the recognized direction visible.")]);
        for marker in ["ACK", "NACK"] {
            let text = format!(
                "{marker} 1kQwPZtK nope 1kQwPZtM: Shared prose remains attached to the attempt."
            );
            let expected = "> Keep the recognized direction visible.\n\n\
                            Nope 1kQwPZtM: Shared prose remains attached to the attempt.";

            let rendered =
                present_steering_acknowledgements_with_directives(&text, &directives, event())
                    .expect("mixed acknowledgment presents");
            assert_eq!(rendered, expected);
            assert_eq!(rendered.matches("1kQwPZtK").count(), 0);
            assert!(!rendered.contains(marker));
            assert_eq!(rendered.matches("Nope").count(), 1);
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
                event(),
            )
            .expect("mixed acknowledgments present"),
            "> Apply the current path.\n\nApplied.\n> Remove the safety check.\n\nUnsafe.",
        );
    }

    #[test]
    fn inline_packets_render_each_context_and_body_in_source_order() {
        let directives = directives(&[
            ("1kQwPZtK", "Keep the first path."),
            ("1kQwPZtL", "Keep the second path."),
        ]);

        assert_eq!(
            present_steering_acknowledgements_with_directives(
                "ACK 1kQwPZtK: First applied. ACK 1kQwPZtL: Second applied.",
                &directives,
                event(),
            )
            .expect("inline acknowledgments present"),
            concat!(
                "> Keep the first path.\n\n",
                "First applied.\n",
                "> Keep the second path.\n\n",
                "Second applied."
            ),
        );
    }

    #[test]
    fn operator_reply_quotes_remain_nested_inside_acknowledgement_context() {
        let directives = directives(&[(
            "1kQwPZtK",
            "> The original assertion remains visible.\n\napproved",
        )]);

        assert_eq!(
            present_steering_acknowledgements_with_directives(
                "ACK 1kQwPZtK: Applied.",
                &directives,
                event(),
            )
            .expect("nested acknowledgment presents"),
            "> > The original assertion remains visible.\n>\n> approved\n\nApplied.",
        );
    }

    #[test]
    fn unknown_key_preserves_the_original_message_across_replay() {
        let text = "ACK 1kQwPZtK: No directive owns this key.";
        let directives = PresentationDirectives::default();
        let first = present_steering_acknowledgements_with_directives(text, &directives, event())
            .expect("unknown key remains visible");
        let replayed =
            present_steering_acknowledgements_with_directives(text, &directives, event())
                .expect("unknown key replay remains visible");

        assert_eq!(first, text);
        assert_eq!(replayed, first);
    }

    #[test]
    fn live_key_for_another_lane_preserves_the_original_message() {
        let text = "ACK 1kQwPZtK: This lane does not own the directive.";
        let mut directives = directives(&[("1kQwPZtK", "Do not quote this direction here.")]);
        directives
            .0
            .get_mut(&"1kQwPZtK".parse().expect("fixture key"))
            .expect("fixture directive")
            .lane = "00000000-0000-4000-8000-000000000002"
            .parse()
            .expect("other lane");

        assert_eq!(
            present_steering_acknowledgements_with_directives(text, &directives, event())
                .expect("cross-lane key remains visible"),
            text,
        );
    }

    #[test]
    fn historical_key_is_presented_only_at_its_exact_retirement_locus() {
        let key = "1kQwPZtK"
            .parse::<steer::Key>()
            .expect("fixture key parses");
        let retirement = wire::body::TranscriptLocus {
            source: None,
            member: 1,
            offset: 0,
            length: 1,
            event: 0,
            line: 1,
            column: 5,
        };
        let directives = PresentationDirectives(BTreeMap::from([(
            key,
            PresentationDirective {
                lane: lane(),
                text: "Keep the original response context.".to_owned(),
                disposition: wire::Disposition::Acked,
                retirement: Some(retirement),
            },
        )]));
        let text = "ACK 1kQwPZtK: Applied.";

        assert_eq!(
            present_steering_acknowledgements_with_directives(text, &directives, event())
                .expect("retirement locus presents"),
            "> Keep the original response context.\n\nApplied.",
        );
        let mut later = event();
        later.locus = listen::Locus::new(1, 8, 1);
        assert_eq!(
            present_steering_acknowledgements_with_directives(text, &directives, later)
                .expect("historical duplicate remains visible"),
            text,
        );
    }

    #[test]
    fn mixed_current_and_historical_keys_present_every_known_context() {
        let historical = "1kR5x8yR"
            .parse::<steer::Key>()
            .expect("historical fixture key parses");
        let current = "1kR5xJDD"
            .parse::<steer::Key>()
            .expect("current fixture key parses");
        let retirement = |offset, column| wire::body::TranscriptLocus {
            source: None,
            member: 1,
            offset,
            length: 1,
            event: 0,
            line: 1,
            column,
        };
        let directives = PresentationDirectives(BTreeMap::from([
            (
                historical,
                PresentationDirective {
                    lane: lane(),
                    text: "Continue".to_owned(),
                    disposition: wire::Disposition::Acked,
                    retirement: Some(retirement(8, 5)),
                },
            ),
            (
                current,
                PresentationDirective {
                    lane: lane(),
                    text: "[TASTE] fallbacks: Keep one deterministic path.".to_owned(),
                    disposition: wire::Disposition::Acked,
                    retirement: Some(retirement(0, 14)),
                },
            ),
        ]));
        let text = concat!(
            "ACK 1kR5x8yR 1kR5xJDD: Continuing with the explicit two-context contract ",
            "only."
        );

        assert_eq!(
            present_steering_acknowledgements_with_directives(text, &directives, event())
                .expect("mixed historical and current keys present"),
            concat!(
                "> Continue\n\n",
                "> [TASTE] fallbacks: Keep one deterministic path.\n\n",
                "Continuing with the explicit two-context contract only."
            ),
        );
    }

    #[test]
    fn source_aware_retirement_requires_the_original_tape_generation() {
        let expected_source = "00000000-0000-4000-8000-000000000002"
            .parse()
            .expect("fixture source");
        let key = "1kQwPZtK"
            .parse::<steer::Key>()
            .expect("fixture key parses");
        let directives = PresentationDirectives(BTreeMap::from([(
            key,
            PresentationDirective {
                lane: lane(),
                text: "Keep the original response context.".to_owned(),
                disposition: wire::Disposition::Acked,
                retirement: Some(wire::body::TranscriptLocus {
                    source: Some(expected_source),
                    member: 1,
                    offset: 0,
                    length: 1,
                    event: 0,
                    line: 1,
                    column: 5,
                }),
            },
        )]));
        let text = "ACK 1kQwPZtK: Applied.";

        let mut original = event();
        original.source = Some(expected_source);
        assert_eq!(
            present_steering_acknowledgements_with_directives(text, &directives, original)
                .expect("original source presents"),
            "> Keep the original response context.\n\nApplied.",
        );

        let mut replacement = event();
        replacement.source = Some(
            "00000000-0000-4000-8000-000000000003"
                .parse()
                .expect("replacement source"),
        );
        assert_eq!(
            present_steering_acknowledgements_with_directives(text, &directives, replacement)
                .expect("replacement source remains historical"),
            text,
        );
        assert_eq!(
            present_steering_acknowledgements_with_directives(text, &directives, event())
                .expect("unknown source remains historical"),
            text,
        );
    }
}
