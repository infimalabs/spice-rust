//! Durable taste recognition and fallible Canvas views over admitted lane capture.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use bus::Bus;
use lane::occupant::Seats;
use pulldown_cmark::{Event as MarkdownEvent, Options, Parser, Tag, TagEnd};
use wire::{AgentActivityKind, CanonicalUuid, Outcome};

use super::{
    CommandResult, RuntimePaths, current_residency, ingest_vendor, lane_surface, narration,
    steering, store_fault, taste_surface,
};

mod acknowledgement;
mod automatic_speech;
mod backlog;
mod pending;
mod projection;
mod session_compaction;
mod sweep;
mod tool_preview;
use acknowledgement::{
    ContextState, Presentation, present_steering_acknowledgements_with_directives,
    scan_presentations,
};
pub(in crate::resident) use pending::settle_pending_presentations;
#[cfg(test)]
pub(super) use projection::publish_agent_messages;
use projection::{publish_agent_event, publish_agent_messages_in, publish_agent_record};

const SESSION_MESSAGE_MEMBER_NAMESPACE: u64 = 1_u64 << 63;
/// Reserved wire identity for record context that is not a decoded event.
const TURN_BOUNDARY_EVENT: u32 = u32::MAX;
/// Historical source bytes one Canvas history request may chain through per
/// source, and the scan time it may spend doing so.
///
/// The tape and preserved-session halves of a merged page each derive their
/// window count from this one span and their own fixed window, so the two
/// halves cover the same reach per request and neither outruns the other.
const HISTORY_REQUEST_BYTES: u64 = 32 * 1024 * 1024;
const HISTORY_DURATION: Duration = Duration::from_secs(5);

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
    /// Tool calls an occupant may run without prose before each prod.
    narration: u32,
}

impl<'a, P> CaptureSurfaces<'a, P> {
    pub(super) fn new(
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
            narration: lane::Config::default().narration(),
        }
    }

    /// Replace the compiled narration bound with the configured one.
    pub(super) const fn narration(mut self, bound: u32) -> Self {
        self.narration = bound;
        self
    }

    /// Fold no narration streak: the seat these records belong to has closed,
    /// so a prod could neither be recorded under it nor read back by it.
    pub(super) const fn without_narration(mut self) -> Self {
        self.narration = 0;
        self
    }
}

/// Mutable per-record fold state at the admitted capture boundary.
pub(super) struct CaptureViews<'a> {
    state: &'a Mutex<MessageState>,
    diagnostic: &'a mut dyn io::Write,
}

impl<'a> CaptureViews<'a> {
    pub(super) fn new(state: &'a Mutex<MessageState>, diagnostic: &'a mut dyn io::Write) -> Self {
        Self { state, diagnostic }
    }
}

#[derive(Debug, Default)]
pub(in crate::resident) struct MessageState {
    previews: CallPreviews,
    observations: MessageObservations,
    prose: BTreeMap<CanonicalUuid, ProseState>,
    pending: pending::PendingPresentations,
    pub(in crate::resident) narration: super::narration::Streaks,
    presented: BTreeMap<(CanonicalUuid, CanonicalUuid, Option<CanonicalUuid>), listen::Position>,
    /// Transcript spans read and presented but not yet accounted.
    ///
    /// Held here because the accounting bound spans sweeps: the state that
    /// outlives one record is the only place a later sweep can find what an
    /// earlier one held.
    accounting: lane::ingest::Coalescing,
}

pub(in crate::resident) fn lock_state(
    state: &Mutex<MessageState>,
) -> CommandResult<MutexGuard<'_, MessageState>> {
    state
        .lock()
        .map_err(|error| store_fault("canvas::message-state-unavailable", &error.to_string()))
}

/// Name a card Canvas refused instead of raising it: Canvas is a projection
/// of durable tapes, so its refusal is a diagnostic, never a resident fault.
pub(in crate::resident) fn canvas_diagnostic(
    presented: CommandResult<()>,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    match presented {
        Err(Outcome::Fault(fault)) => {
            let _ = writeln!(
                diagnostic,
                "canvas message unavailable: {}: {}",
                fault.reason, fault.detail
            );
            Ok(())
        }
        outcome => outcome,
    }
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

    /// The batches one lane's reads can match, without every other lane's.
    fn for_lane(&self, lane: CanonicalUuid) -> Self {
        Self(
            self.0
                .iter()
                .filter(|observation| observation.lane == lane)
                .cloned()
                .collect(),
        )
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

    /// Whether a known batch already spans one tape position on this lane.
    fn covers(&self, lane: CanonicalUuid, position: (u64, u64)) -> bool {
        self.0
            .iter()
            .any(|observation| observation.spans(lane, position))
    }

    fn find(&self, lane: CanonicalUuid, locus: listen::Locus) -> Option<&MessageObservation> {
        let position = (locus.member, locus.offset);
        self.0
            .iter()
            .rev()
            .find(|observation| observation.spans(lane, position))
    }
}

impl MessageObservation {
    /// Whether this batch read that tape position on that lane.
    fn spans(&self, lane: CanonicalUuid, position: (u64, u64)) -> bool {
        self.lane == lane
            && (self.from.member, self.from.offset) <= position
            && position < (self.to.member, self.to.offset)
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

pub(super) enum TasteRecognition<'a> {
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

impl AgentMessagePublisher for canvas::stream::MessageBatch {
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
    mut recognition: TasteRecognition<'_>,
    record: &lane::capture::CaptureRecord,
    mut views: CaptureViews<'_>,
) -> CommandResult<()> {
    if !lane_surface::capture_record(bus, epoch, seats, record)? {
        return Ok(());
    }
    // The bytes behind this record are already durable, so accounting them here
    // keeps the transcript head level with the records the resident has taken
    // responsibility for, rather than leaving the gap for the next start to
    // find. A record the resident did not accept above does not account either.
    //
    // Accounting and live presentation share this sweep, so nothing publishes
    // before capture admission. Scanning through the live head also catches
    // records whose finite-queue notice never arrived.
    let held = std::mem::take(&mut lock_state(views.state)?.accounting);
    let (mut observations, held) = super::ingest_transcripts(
        bus,
        surfaces.paths,
        Some(record.lane),
        lane::ingest::Bound::LIVE,
        held,
        views.diagnostic,
    )?;
    let swept = sweep::swept_records(record, &mut observations);
    {
        let mut state = lock_state(views.state)?;
        state.accounting = held;
        state.observations.extend(observations);
    }
    sweep::present_swept(bus, epoch, surfaces, &mut recognition, &swept, &mut views)?;
    hear_swept(
        bus,
        surfaces.paths,
        epoch,
        record.lane,
        &swept,
        views.diagnostic,
    )
}

/// Retire the directives the swept prose answered.
///
/// What a lane says is its answer, and the accounting sweep hears it even if
/// live projection displayed it first.
fn hear_swept(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    swept: &[std::borrow::Cow<'_, lane::capture::CaptureRecord>],
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let keys = steering_keys(swept.iter().flat_map(|record| {
        record
            .decoded
            .events()
            .iter()
            .filter_map(listen::Event::assistant_text)
    }));
    if keys.is_empty() {
        return Ok(());
    }
    let residency = current_residency(bus, epoch)?;
    let Some(row) = residency.body().lanes.iter().find(|row| row.lane == lane) else {
        return Ok(());
    };
    if ingest_vendor(&row.vendor).is_some() {
        steering::dispose_heard(bus, paths, epoch, lane, &row.vendor, &keys, diagnostic);
    }
    Ok(())
}

/// Account and present whatever a lane's tape gained before its seat closed.
///
/// An occupant writes its last records moments before it exits, and the exit
/// can be accounted before their notices are read; a notice read afterwards
/// no longer names the seated occupant and is refused. The closing sweep
/// runs once per vacated seat, before a successor starts, and projects any
/// remaining records in tape order under that occupant.
pub(super) fn present_closed_transcript<P: AgentMessagePublisher>(
    bus: &Bus,
    epoch: CanonicalUuid,
    surfaces: &CaptureSurfaces<'_, P>,
    mut recognition: TasteRecognition<'_>,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    mut views: CaptureViews<'_>,
) -> CommandResult<()> {
    // A seat that closed writes nothing more, so nothing may stay held: the
    // closing sweep accounts every span this lane still owes before its
    // successor opens a generation against the same head.
    let held = std::mem::take(&mut lock_state(views.state)?.accounting);
    let (mut observations, held) = super::ingest_transcripts(
        bus,
        surfaces.paths,
        Some(lane),
        lane::ingest::Bound::IMMEDIATE,
        held,
        views.diagnostic,
    )?;
    let swept = sweep::closing_records(lane, occupant, &mut observations);
    {
        let mut state = lock_state(views.state)?;
        state.accounting = held;
        state.observations.extend(observations);
    }
    sweep::present_swept(bus, epoch, surfaces, &mut recognition, &swept, &mut views)
}

struct AgentRecord<'a> {
    lane: CanonicalUuid,
    author: CanonicalUuid,
    source: Option<CanonicalUuid>,
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
    /// The keys this prose acknowledged before the journal retired them.
    pending: Option<pending::PendingPresentation>,
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
) -> CommandResult<PresentedMessageText> {
    let Some(steering) = steering else {
        return Ok(PresentedMessageText {
            text: text.to_owned(),
            classifications: Vec::new(),
            directive_contexts: Vec::new(),
        });
    };
    let presented = present_steering_acknowledgements_with_directives(text, steering, event)
        .map_err(|detail| store_fault("canvas::message-steering-presentation-invalid", &detail))?;
    let classifications =
        hydrate_message_classifications(&presented, message_badges(text, Some(steering), event))
            .map_err(|detail| {
                store_fault("canvas::message-steering-presentation-invalid", &detail)
            })?;
    let directive_contexts = hydrate_directive_contexts(&presented, steering)
        .map_err(|detail| store_fault("canvas::message-steering-presentation-invalid", &detail))?;
    Ok(PresentedMessageText {
        text: presented.text,
        classifications,
        directive_contexts,
    })
}

struct PresentedMessageText {
    text: String,
    classifications: Vec<wire::MessageClassification>,
    directive_contexts: Vec<wire::MessageDirective>,
}
pub(crate) fn quote_agent_prose(
    reader: &bus::BusReader,
    lane: CanonicalUuid,
    source: CanonicalUuid,
    ordinal: u32,
    prose: &listen::Prose,
) -> Result<String, String> {
    let keys = steering_keys([prose.text.as_str()]);
    if keys.is_empty() {
        return Ok(prose.text.clone());
    }
    let rows = load_steering_presentations(reader, &keys).map_err(|error| format!("{error:?}"))?;
    let directives = PresentationDirectives::from_presentations(&rows)?;
    let PresentedMessageText {
        mut text,
        directive_contexts,
        ..
    } = present_steering_acknowledgements(
        &prose.text,
        Some(&directives),
        PresentationEvent {
            lane,
            source: Some(source),
            locus: prose.locus,
            ordinal,
        },
    )
    .map_err(|error| format!("{error:?}"))?;
    for context in directive_contexts.iter().rev() {
        let start = usize::try_from(context.start).expect("wire directive start always fits usize");
        let end = usize::try_from(context.end).expect("wire directive end always fits usize");
        text.replace_range(start..end, &context.quote_target.markdown());
    }
    Ok(text)
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
    quote_target: wire::QuoteTarget,
    /// Actor on the creating fact, which a quote of this directive names.
    actor: wire::Actor,
    /// Journal instant at which the directive's creating fact was committed.
    published: wire::Instant,
    retirement: Option<wire::body::TranscriptLocus>,
}

impl PresentationDirectives {
    #[cfg(test)]
    fn from_snapshot(snapshot: &journal::FactSnapshot) -> Result<Self, String> {
        let steering = steer::Ledger::from_snapshot(snapshot).map_err(|error| error.to_string())?;
        let creators = snapshot
            .facts()
            .iter()
            .map(|envelope| (envelope.seq, (envelope.actor.clone(), envelope.at)))
            .collect::<BTreeMap<_, _>>();
        let origins = steer::Ledger::authority_index(snapshot)
            .map_err(|error| error.to_string())?
            .heads()
            .iter()
            .map(|head| {
                let origin = creators.get(&head.body().created).ok_or_else(|| {
                    format!("directive {} lost its creating fact", head.body().key)
                })?;
                Ok((head.body().directive, origin.clone()))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?;
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
                .filter(|directive| {
                    !super::steering::is_legacy_task_generation_control(directive.control())
                })
                .map(|directive| {
                    let key = directive.key().clone();
                    let retirement = retirements.get(&key).copied();
                    let taste = tastes.contains(&key);
                    let review = is_review_feedback(directive.control());
                    let quote_target = wire::QuoteTarget::Steering {
                        key: key.to_string(),
                        digest: wire::QuoteTarget::text_digest(directive.text()),
                    };
                    let (actor, published) = origins
                        .get(&directive.directive())
                        .cloned()
                        .ok_or_else(|| format!("directive {key} has no creating fact"))?;
                    Ok((
                        key,
                        PresentationDirective {
                            lane: directive.lane(),
                            text: wire::media::with_media_payloads(
                                directive.text(),
                                directive.attachments(),
                            )
                            .map_err(|error| error.to_string())?
                            .into_owned(),
                            authored: true,
                            taste,
                            review,
                            disposition: directive.disposition(),
                            retirement,
                            quote_target,
                            actor,
                            published,
                        },
                    ))
                })
                .collect::<Result<_, String>>()?,
        ))
    }

    fn from_presentations(rows: &[bus::SteeringPresentation]) -> Result<Self, String> {
        let mut directives = BTreeMap::new();
        for row in rows {
            let head = row.head().body();
            if super::steering::is_legacy_task_generation_control(head.control.as_deref()) {
                continue;
            }
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
            let quote_target = wire::QuoteTarget::Steering {
                key: key.to_string(),
                digest: wire::QuoteTarget::text_digest(&head.text),
            };
            directives.insert(
                key,
                PresentationDirective {
                    lane: head.lane,
                    text: wire::media::with_media_payloads(&head.text, &head.attachments)
                        .map_err(|error| error.to_string())?
                        .into_owned(),
                    authored: true,
                    taste,
                    review: is_review_feedback(head.control.as_deref()),
                    disposition: head.disposition,
                    retirement,
                    quote_target,
                    actor: row.created().actor.clone(),
                    published: row.created().at,
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

fn is_review_feedback(control: Option<&str>) -> bool {
    control
        .and_then(wire::parse_review_feedback_control)
        .is_some()
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ClassificationCounts {
    acknowledgements: u32,
    tastes: u32,
    broadcasts: u32,
    reviews: u32,
    pending: u32,
    declines: u32,
}
impl ClassificationCounts {
    #[cfg(test)]
    fn is_empty(self) -> bool {
        self == Self::default()
    }
}

fn message_badges(
    text: &str,
    directives: Option<&PresentationDirectives>,
    event: PresentationEvent,
) -> ClassificationCounts {
    let Some(directives) = directives else {
        return ClassificationCounts::default();
    };
    let standing = EventDirectives { directives, event };
    let mut acknowledged = BTreeSet::new();
    let mut declined = BTreeSet::new();
    for (key, disposition) in recognized_answers(text, &standing) {
        match disposition {
            wire::Disposition::Acked => {
                acknowledged.insert(key);
            }
            wire::Disposition::Declined if directives.0.get(&key).is_some_and(is_ordinary) => {
                declined.insert(key);
            }
            _ => {}
        }
    }
    let count = |select: &dyn Fn(&PresentationDirective) -> bool| {
        let matched = acknowledged
            .iter()
            .filter(|key| directives.0.get(*key).is_some_and(select))
            .count();
        u32::try_from(matched).unwrap_or(u32::MAX)
    };
    let tastes = count(&|directive| directive.taste);
    let broadcasts = count(&|directive| !directive.taste && has_broadcast_marker(&directive.text));
    let reviews = count(&|directive| directive.review);
    // A spoken acknowledgement the journal still holds pending is not credit
    // yet; it counts apart until steer::acked retires the key.
    let pending = count(&|directive| {
        is_ordinary(directive) && directive.disposition == wire::Disposition::Live
    });
    // A decline is an answer, not credit: it authorizes the refusal mark on
    // the quoted directive and never adds to the acknowledged total.
    let declines = u32::try_from(declined.len()).unwrap_or(u32::MAX);
    ClassificationCounts {
        acknowledgements: u32::try_from(acknowledged.len())
            .unwrap_or(u32::MAX)
            .saturating_sub(reviews)
            .saturating_sub(pending),
        tastes,
        broadcasts,
        reviews,
        pending,
        declines,
    }
}

#[derive(Debug)]
struct RemainingClassifications {
    acknowledgements: usize,
    tastes: usize,
    broadcasts: usize,
    reviews: usize,
    pending: usize,
    declines: usize,
}

impl From<ClassificationCounts> for RemainingClassifications {
    fn from(counts: ClassificationCounts) -> Self {
        let count = |value| usize::try_from(value).unwrap_or(usize::MAX);
        Self {
            acknowledgements: count(
                counts
                    .acknowledgements
                    .saturating_sub(counts.tastes)
                    .saturating_sub(counts.broadcasts),
            ),
            tastes: count(counts.tastes),
            broadcasts: count(counts.broadcasts),
            reviews: count(counts.reviews),
            pending: count(counts.pending),
            declines: count(counts.declines),
        }
    }
}

/// Classify the markers a presented message carries, one quote level at a time.
///
/// The first quote level is the live answer: its markers are bounded by the
/// counts this answer's recognized keys earned. Each level below it is the
/// history a quoted directive carried, and within a directive context the
/// resident inserted every level repeats the same rule on its own lines: an
/// acknowledgement, pending, or decline marker opening a line is the pill an
/// earlier presentation showed. Those historical pills never draw on the live
/// counts, and the same words anywhere the agent wrote them stay literal.
fn hydrate_message_classifications(
    presented: &Presentation,
    counts: ClassificationCounts,
) -> Result<Vec<wire::MessageClassification>, String> {
    let text = presented.text.as_str();
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_SMART_PUNCTUATION
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM;
    let mut remaining = RemainingClassifications::from(counts);
    let mut quote_depth = 0usize;
    let mut code_block = false;
    let mut image_depth = 0usize;
    let mut opens_line = false;
    let mut classifications = Vec::new();
    for (event, source) in Parser::new_ext(text, options).into_offset_iter() {
        let line_start = std::mem::replace(
            &mut opens_line,
            matches!(
                event,
                MarkdownEvent::Start(Tag::Paragraph)
                    | MarkdownEvent::SoftBreak
                    | MarkdownEvent::HardBreak
            ),
        );
        match &event {
            MarkdownEvent::Start(Tag::CodeBlock(_)) => code_block = true,
            MarkdownEvent::End(TagEnd::CodeBlock) => code_block = false,
            MarkdownEvent::Start(Tag::BlockQuote(_)) => quote_depth += 1,
            MarkdownEvent::End(TagEnd::BlockQuote(_)) => {
                quote_depth = quote_depth.saturating_sub(1);
            }
            MarkdownEvent::Start(Tag::Image { .. }) => image_depth += 1,
            MarkdownEvent::End(TagEnd::Image) => {
                image_depth = image_depth.saturating_sub(1);
            }
            _ => {}
        }
        let MarkdownEvent::Text(_) = event else {
            continue;
        };
        if code_block || image_depth != 0 || quote_depth == 0 {
            continue;
        }
        let source_tail = text
            .get(source.start..)
            .ok_or_else(|| "Markdown source offset is not UTF-8 aligned".to_owned())?;
        let historical = quote_depth > 1
            && line_start
            && presented
                .contexts
                .iter()
                .any(|context| context.range.contains(&source.start));
        let kind = if historical {
            historical_classification(source_tail)
        } else {
            None
        };
        let Some(kind) =
            kind.or_else(|| live_classification(source_tail, quote_depth, &mut remaining))
        else {
            continue;
        };
        let start = source.start;
        let end = start
            .checked_add(kind.marker().len())
            .ok_or_else(|| "classification end overflowed".to_owned())?;
        classifications.push(wire::MessageClassification {
            start: u32::try_from(start)
                .map_err(|_| "classification start exceeds the wire range".to_owned())?,
            end: u32::try_from(end)
                .map_err(|_| "classification end exceeds the wire range".to_owned())?,
            kind,
        });
    }
    if !wire::valid_message_classifications(text, &classifications) {
        return Err("hydrated message classifications are not exact and ordered".to_owned());
    }
    Ok(classifications)
}
fn hydrate_directive_contexts(
    presented: &Presentation,
    directives: &PresentationDirectives,
) -> Result<Vec<wire::MessageDirective>, String> {
    presented
        .contexts
        .iter()
        .map(|context| {
            let directive = directives.0.get(&context.key).ok_or_else(|| {
                format!(
                    "canonical acknowledgment presentation lost directive {}",
                    context.key
                )
            })?;
            Ok(wire::MessageDirective {
                start: u32::try_from(context.range.start)
                    .map_err(|_| "directive context start exceeds the wire range".to_owned())?,
                end: u32::try_from(context.range.end)
                    .map_err(|_| "directive context end exceeds the wire range".to_owned())?,
                quote_target: directive.quote_target.clone(),
                actor: directive.actor.clone(),
                authored: directive.published,
            })
        })
        .collect()
}

/// A marker on the live answer's level, drawn from what the answer earned.
/// Taste is recognized at any depth; every other kind only on the first level.
fn live_classification(
    source_tail: &str,
    quote_depth: usize,
    remaining: &mut RemainingClassifications,
) -> Option<wire::MessageClassificationKind> {
    use wire::MessageClassificationKind as Kind;
    let live = [
        (Kind::Review, &mut remaining.reviews),
        (Kind::Broadcast, &mut remaining.broadcasts),
        (Kind::AcknowledgementPending, &mut remaining.pending),
        (Kind::Decline, &mut remaining.declines),
        (Kind::Acknowledgement, &mut remaining.acknowledgements),
    ];
    if quote_depth == 1 {
        for (kind, remaining) in live {
            if take_classification(source_tail, kind, remaining) {
                return Some(kind);
            }
        }
    }
    take_classification(source_tail, Kind::Taste, &mut remaining.tastes).then_some(Kind::Taste)
}

/// A marker an earlier presentation settled, carried in quoted history.
fn historical_classification(source_tail: &str) -> Option<wire::MessageClassificationKind> {
    use wire::MessageClassificationKind as Kind;
    [
        Kind::AcknowledgementPending,
        Kind::Decline,
        Kind::Acknowledgement,
    ]
    .into_iter()
    .find(|kind| take_classification(source_tail, *kind, &mut 1))
}

fn take_classification(
    text: &str,
    kind: wire::MessageClassificationKind,
    remaining: &mut usize,
) -> bool {
    if *remaining == 0 {
        return false;
    }
    let marker = kind.marker();
    if !text
        .strip_prefix(marker)
        .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with(char::is_whitespace))
    {
        return false;
    }
    *remaining -= 1;
    true
}

/// Recognized answers from one presentation scan. Callers select dispositions
/// before collecting distinct keys, so pending cards need no refusal set.
fn recognized_answers<'a>(
    text: &str,
    standing: &'a EventDirectives<'_>,
) -> impl Iterator<Item = (steer::Key, wire::Disposition)> + 'a {
    scan_presentations(text, standing)
        .into_iter()
        .flat_map(|packet| packet.recognitions)
        .filter_map(move |recognition| {
            let steer::ack::scan::Outcome::Retired { key, disposition } = recognition.outcome
            else {
                return None;
            };
            standing
                .presents(&key, disposition, recognition.locus)
                .then_some((key, disposition))
        })
}

/// Whether a directive carries no flavor marker of its own, so its
/// acknowledgement state is the only inline mark Canvas can show for it.
fn is_ordinary(directive: &PresentationDirective) -> bool {
    !directive.taste && !directive.review && !has_broadcast_marker(&directive.text)
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

    /// The answer the journal retired `key` with for this lane, which display
    /// may repeat without repeating retirement.
    ///
    /// The lane owns that conversation continuity across transcript replacement
    /// and across the records of one message: a key retires at the streamed
    /// text while the card projects from the settled message. The original
    /// retirement locus remains the authority for retirement and automatic
    /// speech; later records only reuse its already-authenticated context for
    /// presentation.
    fn answered_for_lane(&self, key: &steer::Key) -> Option<wire::Disposition> {
        self.directives
            .0
            .get(key)
            .filter(|directive| {
                directive.lane == self.event.lane
                    && matches!(
                        directive.disposition,
                        wire::Disposition::Acked | wire::Disposition::Declined
                    )
                    && !directive.review
                    && directive.retirement.is_some()
            })
            .map(|directive| directive.disposition)
    }

    /// The stable seal authenticating steering delivery to this lane.
    ///
    /// It is a presentation target rather than a directive: acknowledging it
    /// never mutates steering standing or gains automatic-speech authority.
    fn is_lane_seal(&self, key: &steer::Key) -> bool {
        key == &steer::Key::for_identity(self.event.lane)
    }

    fn presents(
        &self,
        key: &steer::Key,
        disposition: wire::Disposition,
        locus: steer::ack::scan::Locus,
    ) -> bool {
        self.recognizes(key, disposition, locus)
            || self.answered_for_lane(key) == Some(disposition)
            || (disposition == wire::Disposition::Acked && self.is_lane_seal(key))
    }

    fn recognizes_authored(
        &self,
        key: &steer::Key,
        disposition: wire::Disposition,
        locus: steer::ack::scan::Locus,
    ) -> bool {
        self.recognizes(key, disposition, locus) && self.authored(key)
    }

    fn authored(&self, key: &steer::Key) -> bool {
        self.directives
            .0
            .get(key)
            .is_some_and(|directive| directive.authored)
    }

    /// Whether the journal still holds `key` pending, so an acknowledgement
    /// spoken here awaits the steer::acked fact before it reads as credit.
    fn pending(&self, key: &steer::Key) -> bool {
        self.directives
            .0
            .get(key)
            .is_some_and(|directive| directive.disposition == wire::Disposition::Live)
    }

    fn context_state(&self, key: &steer::Key, disposition: wire::Disposition) -> ContextState {
        match disposition {
            wire::Disposition::Acked if self.pending(key) => ContextState::Pending,
            wire::Disposition::Acked => ContextState::Acknowledged,
            wire::Disposition::Declined => ContextState::Declined,
            _ => ContextState::Unmarked,
        }
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
