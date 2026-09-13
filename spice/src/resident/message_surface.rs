//! Durable taste recognition and fallible Canvas views over admitted lane capture.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use bus::Bus;
use lane::occupant::Seats;
use wire::{AgentActivityKind, CanonicalUuid, Outcome};

use super::{CommandResult, RuntimePaths, lane_surface, store_fault, taste_surface};

mod automatic_speech;
mod backlog;
mod projection;
mod session_compaction;
mod tool_preview;
#[cfg(test)]
pub(super) use projection::publish_agent_messages;
use projection::{publish_agent_event, publish_agent_messages_in, publish_agent_record};

const SESSION_MESSAGE_MEMBER_NAMESPACE: u64 = 1_u64 << 63;
/// Reserved wire identity for record context that is not a decoded event.
const TURN_BOUNDARY_EVENT: u32 = u32::MAX;

#[cfg(test)]
pub(super) use backlog::restore_agent_messages;
pub(super) use backlog::{BacklogReader, restore_agent_messages_indexed};
pub(super) use session_compaction::SessionCompactions;
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
    prose: BTreeMap<CanonicalUuid, ProseState>,
}

#[derive(Debug)]
struct ProseState {
    published: wire::AgentMessage,
    terminal: wire::AgentMessage,
}

#[derive(Clone, Debug)]
struct MessageObservation {
    lane: CanonicalUuid,
    source: CanonicalUuid,
    from: wire::body::TranscriptPosition,
    to: wire::body::TranscriptPosition,
    at: wire::Instant,
}

#[derive(Clone, Debug, Default)]
struct MessageObservations(Vec<MessageObservation>);

impl MessageObservations {
    #[cfg(test)]
    fn from_snapshot(snapshot: &journal::FactSnapshot) -> Self {
        Self::from_facts(snapshot.facts())
    }

    fn from_facts<'a>(facts: impl IntoIterator<Item = &'a wire::Envelope<wire::AnyFact>>) -> Self {
        let mut observations = Self::default();
        for envelope in facts {
            if let Some(batch) = envelope.fact.read::<wire::body::JournalIngested>() {
                observations.push(&batch, envelope.at);
            }
        }
        observations
    }

    fn append(&mut self, mut observations: Self) {
        self.0.append(&mut observations.0);
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

    fn revise_final(
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

    fn revise_final(
        &self,
        message: wire::AgentMessage,
    ) -> Result<bool, canvas::stream::MessagePublishError> {
        Self::revise_final(self, message)
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
    // Session authority enriches presentation; capture admission decided freshness.
    // Its absence keeps the non-disclosing count preview.
    let worktree = bus
        .residency_head()
        .ok()
        .flatten()
        .filter(|head| head.body().epoch == epoch)
        .and_then(|head| {
            head.body()
                .lanes
                .iter()
                .find(|lane| lane.lane == record.lane)
                .and_then(|lane| lane.session.as_ref())
                .filter(|session| session.occupant == record.occupant)
                .map(|session| PathBuf::from(&session.worktree))
        });
    match publish_agent_messages_in(
        surfaces.publisher,
        record,
        views.state,
        steering.as_ref(),
        worktree.as_deref(),
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

struct AgentRecord<'a> {
    lane: CanonicalUuid,
    author: CanonicalUuid,
    source: Option<CanonicalUuid>,
    worktree: Option<&'a Path>,
    locus: listen::Locus,
    context: &'a listen::RecordContext,
    events: &'a [(u32, &'a listen::Event)],
    observed: Option<wire::Instant>,
    unavailable_tool_terminals: &'a [String],
}

struct AgentEvent<'a> {
    lane: CanonicalUuid,
    author: CanonicalUuid,
    source: Option<CanonicalUuid>,
    worktree: Option<&'a Path>,
    ordinal: u32,
    decoded: &'a listen::Event,
    authored: Option<&'a str>,
    observed: Option<wire::Instant>,
    observations: &'a MessageObservations,
    unavailable_tool_terminals: &'a [String],
}

struct ProjectedMessage {
    published: wire::AgentMessage,
    terminal: Option<wire::AgentMessage>,
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
    let keys = steering_keys(record.decoded.events().iter().filter_map(|event| {
        let listen::Event::Prose(prose) = event else {
            return None;
        };
        (prose.role == listen::ProseRole::Assistant).then_some(prose.text.as_str())
    }));
    let presentations = load_steering_presentations(&bus.reader(), &keys)?;
    PresentationDirectives::from_presentations(&presentations)
        .map(Some)
        .map_err(|error| store_fault("canvas::message-steering-invalid", &error.to_string()))
}

pub(super) fn load_steering_presentations(
    reader: &bus::BusReader,
    keys: &BTreeSet<steer::Key>,
) -> CommandResult<Vec<bus::SteeringPresentation>> {
    let keys = keys.iter().map(ToString::to_string).collect::<Vec<_>>();
    let mut presentations = Vec::new();
    for batch in keys.chunks(bus::MAX_STEERING_PRESENTATION_KEYS) {
        presentations.extend(
            reader
                .steering_presentations(batch.to_vec())
                .map_err(|error| {
                    store_fault("canvas::message-steering-unavailable", &error.to_string())
                })?,
        );
    }
    Ok(presentations)
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

#[derive(Clone, Debug, Default)]
pub(super) struct PresentationDirectives(BTreeMap<steer::Key, PresentationDirective>);

#[derive(Clone, Debug)]
struct PresentationDirective {
    lane: CanonicalUuid,
    text: String,
    authored: bool,
    taste: bool,
    review: bool,
    disposition: wire::Disposition,
    retirement: Option<wire::body::TranscriptLocus>,
}

impl PresentationDirectives {
    #[cfg(test)]
    fn from_snapshot(snapshot: &journal::FactSnapshot) -> Result<Self, String> {
        let steering = steer::Ledger::from_snapshot(snapshot).map_err(|error| error.to_string())?;
        let mut retirements = BTreeMap::new();
        let mut tastes = BTreeSet::new();
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
            } else if let Some(body) = envelope.fact.read::<wire::body::TasteReminded>() {
                tastes.insert(
                    body.key
                        .parse::<steer::Key>()
                        .map_err(|error| error.to_string())?,
                );
            } else if let Some(body) = envelope.fact.read::<wire::body::TasteJudged>()
                && let wire::body::TasteDecision::Confirmed { key, .. } = body.decision
            {
                tastes.insert(
                    key.parse::<steer::Key>()
                        .map_err(|error| error.to_string())?,
                );
            }
        }
        Ok(Self(
            steering
                .directives()
                .into_iter()
                .map(|directive| {
                    let key = directive.key().clone();
                    let retirement = retirements.get(&key).copied();
                    let taste = tastes.contains(&key);
                    let review = is_review_feedback(directive.control());
                    (
                        key,
                        PresentationDirective {
                            lane: directive.lane(),
                            text: directive.text().to_owned(),
                            authored: presents_as_authored_steering(directive.control()),
                            taste,
                            review,
                            disposition: directive.disposition(),
                            retirement,
                        },
                    )
                })
                .collect(),
        ))
    }

    fn from_presentations(rows: &[bus::SteeringPresentation]) -> Result<Self, String> {
        let mut directives = BTreeMap::new();
        for row in rows {
            let head = row.head().body();
            let key = head
                .key
                .parse::<steer::Key>()
                .map_err(|error| error.to_string())?;
            let retirement = row
                .current()
                .fact
                .read::<wire::body::SteerAcked>()
                .map(|body| body.locus)
                .or_else(|| {
                    row.current()
                        .fact
                        .read::<wire::body::SteerDeclined>()
                        .map(|body| body.locus)
                });
            let taste = row.created().fact.kind() == wire::FactKind::TasteReminded
                || row
                    .created()
                    .fact
                    .read::<wire::body::TasteJudged>()
                    .is_some_and(|body| {
                        matches!(body.decision, wire::body::TasteDecision::Confirmed { .. })
                    });
            directives.insert(
                key,
                PresentationDirective {
                    lane: head.lane,
                    text: head.text.clone(),
                    authored: presents_as_authored_steering(head.control.as_deref()),
                    taste,
                    review: is_review_feedback(head.control.as_deref()),
                    disposition: head.disposition,
                    retirement,
                },
            );
        }
        Ok(Self(directives))
    }

    fn extend_presentations(&mut self, rows: &[bus::SteeringPresentation]) -> Result<(), String> {
        self.0.extend(Self::from_presentations(rows)?.0);
        Ok(())
    }
}

struct AnySteeringKey;

impl steer::Standing for AnySteeringKey {
    fn disposition(&self, _key: &steer::Key) -> Option<wire::Disposition> {
        Some(wire::Disposition::Live)
    }
}

fn steering_keys<'a>(texts: impl IntoIterator<Item = &'a str>) -> BTreeSet<steer::Key> {
    texts
        .into_iter()
        .flat_map(|text| steer::scan(text, &AnySteeringKey))
        .filter_map(|recognition| match recognition.outcome {
            steer::Outcome::Retired { key, .. } => Some(key),
            steer::Outcome::Missed { .. } => None,
        })
        .collect()
}

fn presents_as_authored_steering(control: Option<&str>) -> bool {
    !control.is_some_and(|control| {
        control.starts_with(spice::session_meter::TASK_CONTRACT_CONTROL_PREFIX)
    })
}

fn is_review_feedback(control: Option<&str>) -> bool {
    control.is_some_and(|control| {
        control.starts_with(super::lifecycle::REVIEW_FEEDBACK_CONTROL_PREFIX)
    })
}

fn message_badges(
    text: &str,
    directives: Option<&PresentationDirectives>,
    event: PresentationEvent,
) -> wire::AgentMessageBadges {
    let Some(directives) = directives else {
        return wire::AgentMessageBadges::default();
    };
    let standing = EventDirectives { directives, event };
    let mut acknowledged = BTreeSet::new();
    for packet in steer::ack::scan::scan_acknowledgements(text, &standing) {
        for recognition in packet.recognitions {
            let steer::ack::scan::Outcome::Retired { key, disposition } = recognition.outcome
            else {
                continue;
            };
            if disposition == wire::Disposition::Acked
                && standing.recognizes(&key, disposition, recognition.locus)
            {
                acknowledged.insert(key);
            }
        }
    }
    let tastes = acknowledged
        .iter()
        .filter(|key| {
            directives
                .0
                .get(*key)
                .is_some_and(|directive| directive.taste)
        })
        .count();
    let broadcasts = acknowledged
        .iter()
        .filter(|key| {
            directives
                .0
                .get(*key)
                .is_some_and(|directive| !directive.taste && has_broadcast_marker(&directive.text))
        })
        .count();
    let reviews = acknowledged
        .iter()
        .filter(|key| {
            directives
                .0
                .get(*key)
                .is_some_and(|directive| directive.review)
        })
        .count();
    wire::AgentMessageBadges {
        acknowledgements: u32::try_from(acknowledged.len().saturating_sub(reviews))
            .unwrap_or(u32::MAX),
        tastes: u32::try_from(tastes).unwrap_or(u32::MAX),
        broadcasts: u32::try_from(broadcasts).unwrap_or(u32::MAX),
        reviews: u32::try_from(reviews).unwrap_or(u32::MAX),
    }
}

fn has_broadcast_marker(text: &str) -> bool {
    text.strip_prefix(wire::stream::BROADCAST_MARKER)
        .is_some_and(|suffix| suffix.starts_with(char::is_whitespace))
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

    fn recognizes_authored(
        &self,
        key: &steer::Key,
        disposition: wire::Disposition,
        locus: steer::ack::scan::Locus,
    ) -> bool {
        self.recognizes(key, disposition, locus)
            && self
                .directives
                .0
                .get(key)
                .is_some_and(|directive| directive.authored)
    }

    fn historical_context(&self, token: &str, miss: steer::ack::scan::Miss) -> Option<steer::Key> {
        if miss != steer::ack::scan::Miss::Historical {
            return None;
        }
        let key = token.parse::<steer::Key>().ok()?;
        let directive = self.directives.0.get(&key)?;
        (directive.lane == self.event.lane && directive.authored).then_some(key)
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
                    if directives
                        .0
                        .get(&key)
                        .is_some_and(|directive| directive.authored)
                        && !contexts.iter().any(|known| known == &key)
                    {
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
        if !contexts.is_empty() {
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
        }
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
