//! Durable taste recognition and fallible Canvas views over admitted lane capture.

use std::collections::{BTreeMap, BTreeSet};
use std::io;

use bus::Bus;
use lane::occupant::Seats;
use wire::{AgentActivityKind, CanonicalUuid, Outcome};

use super::{CommandResult, lane_surface, store_fault};
use super::{RuntimePaths, taste_surface};

mod automatic_speech;
mod backlog;
mod session_compaction;
mod tool_preview;

const SESSION_MESSAGE_MEMBER_NAMESPACE: u64 = 1_u64 << 63;

pub(super) use backlog::{BacklogReader, restore_agent_messages};
pub(super) use session_compaction::{
    POLL_INTERVAL as SESSION_COMPACTION_POLL_INTERVAL, SessionCompactions,
};
pub(super) use tool_preview::CallPreviews;
#[cfg(test)]

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
    let Some(context) = record.decoded.context() else {
        return Ok(());
    };
    let mut events = Vec::new();
    for (event, decoded) in record.decoded.events().iter().enumerate() {
        let event = u32::try_from(event)
            .map_err(|error| store_fault("canvas::message-locus-invalid", &error.to_string()))?;
        events.push((event, decoded));
    }
    publish_agent_record(
        publisher,
        AgentRecord {
            lane: record.lane,
            author: record.occupant,
            source: None,
            context,
            events: &events,
            observed: None,
            unavailable_tool_terminals: &[],
        },
        state,
        steering,
    )
}

struct AgentRecord<'a> {
    lane: CanonicalUuid,
    author: CanonicalUuid,
    source: Option<CanonicalUuid>,
    context: &'a listen::RecordContext,
    events: &'a [(u32, &'a listen::Event)],
    observed: Option<wire::Instant>,
    unavailable_tool_terminals: &'a [String],
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

struct ProjectedMessage {
    published: wire::AgentMessage,
    terminal: Option<wire::AgentMessage>,
}

fn publish_agent_record(
    publisher: &impl AgentMessagePublisher,
    record: AgentRecord<'_>,
    state: &mut MessageState,
    steering: Option<&PresentationDirectives>,
) -> CommandResult<()> {
    if record.context.turn == Some(listen::TurnMoment::Started)
        || record
            .events
            .iter()
            .any(|(_, event)| matches!(event, listen::Event::Session(_)))
    {
        state.prose.remove(&record.lane);
    }
    let successful_terminal = record.context.turn == Some(listen::TurnMoment::Completed)
        && record
            .events
            .iter()
            .any(|(_, event)| matches!(event, listen::Event::Usage(_)));
    let mut explicit_final = false;
    for &(ordinal, decoded) in record.events {
        let Some(projected) = project_agent_event(
            AgentEvent {
                lane: record.lane,
                author: record.author,
                source: record.source,
                ordinal,
                decoded,
                authored: record.context.timestamp.as_deref(),
                observed: record.observed,
                observations: &state.observations,
                unavailable_tool_terminals: record.unavailable_tool_terminals,
            },
            &mut state.previews,
            steering,
        )?
        else {
            continue;
        };
        if projected.published.activity != AgentActivityKind::Prose {
            publish_message(publisher, projected.published)?;
            continue;
        }
        if projected.published.final_answer {
            explicit_final = true;
            if let Some(previous) = state.prose.get(&record.lane)
                && same_prose(&previous.published, &projected.published)
            {
                if !previous.published.final_answer {
                    let terminal = previous.terminal.clone();
                    revise_final_message(publisher, terminal.clone())?;
                    state.prose.insert(
                        record.lane,
                        ProseState {
                            published: terminal.clone(),
                            terminal,
                        },
                    );
                }
                continue;
            }
        }
        publish_message(publisher, projected.published.clone())?;
        let terminal = projected
            .terminal
            .unwrap_or_else(|| projected.published.clone());
        state.prose.insert(
            record.lane,
            ProseState {
                published: projected.published,
                terminal,
            },
        );
    }
    if successful_terminal
        && !explicit_final
        && let Some(previous) = state.prose.get(&record.lane)
        && previous.published.author == record.author
        && !previous.published.final_answer
    {
        revise_final_message(publisher, previous.terminal.clone())?;
    }
    if record.context.turn == Some(listen::TurnMoment::Completed) {
        state.prose.remove(&record.lane);
    }
    Ok(())
}

fn same_prose(left: &wire::AgentMessage, right: &wire::AgentMessage) -> bool {
    left.author == right.author && left.text == right.text && left.media == right.media
}

fn publish_message(
    publisher: &impl AgentMessagePublisher,
    message: wire::AgentMessage,
) -> CommandResult<()> {
    publisher
        .publish(message)
        .map_err(|error| store_fault("canvas::message-publish-failed", &error.to_string()))?;
    Ok(())
}

fn revise_final_message(
    publisher: &impl AgentMessagePublisher,
    message: wire::AgentMessage,
) -> CommandResult<()> {
    publisher
        .revise_final(message)
        .map_err(|error| store_fault("canvas::message-publish-failed", &error.to_string()))?;
    Ok(())
}

/// Project one typed tape event through the sole live/restored message policy.
fn publish_agent_event(
    publisher: &impl AgentMessagePublisher,
    event: AgentEvent<'_>,
    previews: &mut CallPreviews,
    steering: Option<&PresentationDirectives>,
) -> CommandResult<()> {
    if let Some(projected) = project_agent_event(event, previews, steering)? {
        publish_message(publisher, projected.published)?;
    }
    Ok(())
}

fn project_agent_event(
    event: AgentEvent<'_>,
    previews: &mut CallPreviews,
    steering: Option<&PresentationDirectives>,
) -> CommandResult<Option<ProjectedMessage>> {
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
    let presentation_event = |locus| PresentationEvent {
        lane,
        source: source.or_else(|| observations.source(lane, locus)),
        locus,
        ordinal,
    };
    let (activity, final_answer, locus, text, badges, automatic_speech, media) = match decoded {
        listen::Event::Prose(prose) if prose.role == listen::ProseRole::Assistant => {
            let event = presentation_event(prose.locus);
            (
                AgentActivityKind::Prose,
                prose.phase.as_deref() == Some("final_answer"),
                prose.locus,
                present_steering_acknowledgements(&prose.text, steering, event)?,
                message_badges(&prose.text, steering, event),
                automatic_speech::classify(prose, steering, event),
                prose.media.clone(),
            )
        }
        listen::Event::Tool(tool) => {
            let activity = tool_preview::activity(lane, tool, previews);
            let media = match &tool.moment {
                listen::ToolMoment::Result(outcome) => outcome.media.clone(),
                listen::ToolMoment::Call { .. } | listen::ToolMoment::Update { .. } => Vec::new(),
            };
            let text = tool_preview::message_text(tool, activity, &media);
            (
                AgentActivityKind::Tool,
                false,
                tool.locus,
                text,
                wire::AgentMessageBadges::default(),
                None,
                media,
            )
        }
        listen::Event::Compaction(compaction) => (
            AgentActivityKind::Compaction,
            false,
            compaction.locus,
            compaction
                .summary
                .clone()
                .filter(|summary| !summary.trim().is_empty())
                .unwrap_or_else(|| "Context compacted".to_owned()),
            wire::AgentMessageBadges::default(),
            None,
            Vec::new(),
        ),
        _ => return Ok(None),
    };
    let (authored, authored_synthetic) = parse_agent_message_authored(authored, locus, ordinal)?;
    let observed = authored_synthetic
        .then(|| observed.or_else(|| observations.at(lane, locus)))
        .flatten();
    wire::media::admit_media(&media)
        .map_err(|error| store_fault("canvas::message-media-invalid", &error.to_string()))?;
    let published = wire::AgentMessage {
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
        final_answer,
        text,
        badges,
        automatic_speech,
        unavailable_tool_terminals: unavailable_tool_terminals.to_vec(),
        media,
    };
    let terminal = if published.activity == AgentActivityKind::Prose && !published.final_answer {
        let listen::Event::Prose(prose) = decoded else {
            unreachable!("only assistant prose projects a prose card")
        };
        let mut terminal_prose = prose.clone();
        terminal_prose.phase = Some("final_answer".to_owned());
        let mut terminal = published.clone();
        terminal.final_answer = true;
        terminal.automatic_speech =
            automatic_speech::classify(&terminal_prose, steering, presentation_event(prose.locus));
        Some(terminal)
    } else {
        None
    };
    Ok(Some(ProjectedMessage {
        published,
        terminal,
    }))
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

#[derive(Clone, Debug, Default)]
pub(super) struct PresentationDirectives(BTreeMap<steer::Key, PresentationDirective>);

#[derive(Clone, Debug)]
struct PresentationDirective {
    lane: CanonicalUuid,
    text: String,
    authored: bool,
    taste: bool,
    disposition: wire::Disposition,
    retirement: Option<wire::body::TranscriptLocus>,
}

impl PresentationDirectives {
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
                    (
                        key,
                        PresentationDirective {
                            lane: directive.lane(),
                            text: directive.text().to_owned(),
                            authored: presents_as_authored_steering(directive.control()),
                            taste,
                            disposition: directive.disposition(),
                            retirement,
                        },
                    )
                })
                .collect(),
        ))
    }
}

fn presents_as_authored_steering(control: Option<&str>) -> bool {
    !control.is_some_and(|control| {
        control.starts_with(spice::session_meter::TASK_CONTRACT_CONTROL_PREFIX)
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
    wire::AgentMessageBadges {
        acknowledgements: u32::try_from(acknowledged.len().saturating_sub(tastes))
            .unwrap_or(u32::MAX),
        tastes: u32::try_from(tastes).unwrap_or(u32::MAX),
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
