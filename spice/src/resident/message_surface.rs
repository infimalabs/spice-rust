//! Durable taste recognition and fallible app views over admitted lane capture.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use bus::Bus;
use lane::occupant::Seats;
use pulldown_cmark::{Event as MarkdownEvent, Options, Parser, Tag, TagEnd};
use spice::ambient::Ambient;
use wire::{AgentActivityKind, CanonicalUuid, Outcome};

use super::{
    CommandResult, RuntimePaths, current_residency, ingest_vendor, lane_surface, narration,
    steering, store_fault, taste_surface,
};

mod acknowledgement;
mod automatic_speech;
mod backlog;
mod direction;
mod media_hydration;
mod pending;
mod projection;
mod prose_stream;
mod session_compaction;
mod sweep;
mod tool_preview;
mod turn_cause;
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
/// Historical source bytes one app history request may chain through per
/// source, and the scan time it may spend doing so.
///
/// The tape and preserved-session halves of a merged page each derive their
/// window count from this one span and their own fixed window, so the two
/// halves cover the same reach per request and neither outruns the other.
const HISTORY_REQUEST_BYTES: u64 = 32 * 1024 * 1024;
const HISTORY_DURATION: Duration = Duration::from_secs(5);

#[cfg(test)]
pub(super) use backlog::restore_agent_messages;
pub(super) use backlog::{BacklogReader, advance_profile_warming, restore_agent_messages_indexed};
pub(super) use session_compaction::SessionCompactions;
pub(super) use tool_preview::CallPreviews;

/// Captured-generation policy and disposable publishers used after one record
/// passes occupant admission.
pub(super) struct CaptureSurfaces<'a, P> {
    paths: &'a RuntimePaths,
    tastes: &'a taste::Bag,
    steering: &'a steer::Config,
    publisher: &'a P,
    /// The environment transcript lookups resolve vendor homes from.
    ambient: &'a Ambient,
    /// Tool calls an occupant may run without prose before each prod.
    narration: u32,
}

impl<'a, P> CaptureSurfaces<'a, P> {
    pub(super) fn new(
        paths: &'a RuntimePaths,
        tastes: &'a taste::Bag,
        steering: &'a steer::Config,
        publisher: &'a P,
        ambient: &'a Ambient,
    ) -> Self {
        Self {
            paths,
            tastes,
            steering,
            publisher,
            ambient,
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
pub(crate) struct MessageState {
    previews: CallPreviews,
    media_hydration: media_hydration::MediaHydration,
    observations: MessageObservations,
    prose: BTreeMap<CanonicalUuid, ProseState>,
    pending: pending::PendingPresentations,
    pub(crate) profiles: spice::rooms::profile::SessionProfiles,
    pub(in crate::resident) narration: super::narration::Streaks,
    presented: BTreeMap<(CanonicalUuid, CanonicalUuid, Option<CanonicalUuid>), listen::Position>,
    /// Current OMP message creation time, including silent message-start records.
    messages: BTreeMap<CanonicalUuid, OpenMessage>,
    /// Transcript spans read and presented but not yet accounted.
    ///
    /// Held here because the accounting bound spans sweeps: the state that
    /// outlives one record is the only place a later sweep can find what an
    /// earlier one held.
    accounting: lane::ingest::Coalescing,
}

pub(crate) fn lock_state(
    state: &Mutex<MessageState>,
) -> CommandResult<MutexGuard<'_, MessageState>> {
    state
        .lock()
        .map_err(|error| store_fault("serve::message-state-unavailable", &error.to_string()))
}

/// Name a card the app server refused instead of raising it: the app is a projection
/// of durable tapes, so its refusal is a diagnostic, never a resident fault.
pub(in crate::resident) fn app_diagnostic(
    presented: CommandResult<()>,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    match presented {
        Err(Outcome::Fault(fault)) => {
            let _ = writeln!(
                diagnostic,
                "message unavailable: {}: {}",
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

#[derive(Debug)]
struct OpenMessage {
    author: CanonicalUuid,
    source: Option<CanonicalUuid>,
    id: String,
    timestamp: u64,
    answers: BTreeMap<u32, BTreeSet<steer::Key>>,
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
    ) -> Result<bool, serve::stream::MessagePublishError>;

    fn revise_final(
        &self,
        message: wire::AgentMessage,
    ) -> Result<bool, serve::stream::MessagePublishError>;
}

pub(super) enum TasteRecognition<'a> {
    Direct,
    Immediate(&'a mut dyn taste_surface::Adjudicator),
    Deferred(&'a mut taste_surface::AdjudicationSubmitter),
}

impl AgentMessagePublisher for serve::stream::MessagePublisher {
    fn publish(
        &self,
        message: wire::AgentMessage,
    ) -> Result<bool, serve::stream::MessagePublishError> {
        Self::publish(self, message)
    }

    fn revise_final(
        &self,
        message: wire::AgentMessage,
    ) -> Result<bool, serve::stream::MessagePublishError> {
        Self::revise_final(self, message)
    }
}

impl AgentMessagePublisher for serve::stream::MessageBatch {
    fn publish(
        &self,
        message: wire::AgentMessage,
    ) -> Result<bool, serve::stream::MessagePublishError> {
        Self::publish(self, message)
    }

    fn revise_final(
        &self,
        message: wire::AgentMessage,
    ) -> Result<bool, serve::stream::MessagePublishError> {
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
    if record.declaration.is_some() {
        let residency = current_residency(bus, epoch)?;
        if let Some(session) = residency
            .body()
            .lanes
            .iter()
            .find(|row| row.lane == record.lane)
            .and_then(|row| row.session.as_ref())
            .filter(|session| session.occupant == record.occupant)
        {
            lock_state(views.state)?.profiles.bind(
                record.occupant,
                spice::rooms::profile::SessionKey::new(
                    session.lane,
                    session.vendor.clone(),
                    session.identity.clone(),
                ),
            );
        }
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
    // A noticed record the sweep did not cover was accounted, and its packets
    // read, by an earlier pass; only the sweep's own records are new to MSG.
    let noticed_new = observations.iter().any(|observation| {
        observation.batch.lane == record.lane
            && observation
                .records
                .iter()
                .any(|swept| swept.locus == record.locus)
    });
    let swept = sweep::swept_records(record, &mut observations);
    {
        let mut state = lock_state(views.state)?;
        state.accounting = held;
        state.observations.extend(observations);
    }
    sweep::present_swept(bus, epoch, surfaces, &mut recognition, &swept, &mut views)?;
    let authored = if noticed_new { &swept[..] } else { &swept[1..] };
    publish_swept(
        bus,
        epoch,
        surfaces,
        authored,
        steering::Refusals::Notify,
        &mut views,
    )?;
    hear_swept(
        bus,
        surfaces.paths,
        epoch,
        record.lane,
        &swept,
        views.diagnostic,
    )
}

/// Publish the MSG packets each swept record's author wrote.
///
/// Authorship is the sweep's capture binding: a live sweep's records belong
/// to the noticed occupant, a closing sweep's to the occupant that left.
fn publish_swept<P>(
    bus: &Bus,
    epoch: CanonicalUuid,
    surfaces: &CaptureSurfaces<'_, P>,
    swept: &[std::borrow::Cow<'_, lane::capture::CaptureRecord>],
    refusals: steering::Refusals,
    views: &mut CaptureViews<'_>,
) -> CommandResult<()> {
    let sources = {
        let state = lock_state(views.state)?;
        swept
            .iter()
            .map(|record| state.observations.source(record.lane, record.locus))
            .collect::<Vec<_>>()
    };
    for (record, source) in swept.iter().zip(sources) {
        let Some(source) = source else {
            continue;
        };
        steering::publish_authored(
            bus,
            surfaces.paths,
            epoch,
            surfaces.steering,
            &steering::AuthoredRecord {
                lane: record.lane,
                author: record.occupant,
                source,
                locus: record.locus,
                events: record.decoded.events(),
            },
            refusals,
            views.diagnostic,
        );
        let _ = super::room_catch_up::recognize(bus, surfaces.paths, record, source)?;
        retain_conversation_evidence(record, source, views)?;
    }
    Ok(())
}

fn retain_conversation_evidence(
    record: &lane::capture::CaptureRecord,
    source: CanonicalUuid,
    views: &mut CaptureViews<'_>,
) -> CommandResult<()> {
    let mut state = lock_state(views.state)?;
    for prose in record
        .decoded
        .events()
        .iter()
        .filter_map(|event| match event {
            listen::Event::Prose(prose) if prose.role == listen::ProseRole::Assistant => {
                Some(prose)
            }
            _ => None,
        })
    {
        let Some(conversation) = spice::rooms::evidence::Conversation::read(&prose.text) else {
            continue;
        };
        let key = spice::rooms::profile::EvidenceKey::Conversation {
            source,
            member: prose.locus.member,
            offset: prose.locus.offset,
            length: prose.locus.length,
        };
        state.profiles.insert(
            record.lane,
            record.occupant,
            spice::rooms::evidence::Evidence::conversation(key, &conversation),
        );
    }
    Ok(())
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

/// Account and present whatever a lane's tape gained before its seat closed,
/// then take the occupant out of the matchers' background.
///
/// An occupant writes its last records moments before it exits, and the exit
/// can be accounted before their notices are read; a notice read afterwards
/// no longer names the seated occupant and is refused. The closing sweep
/// runs once per vacated seat, before a successor starts, and projects any
/// remaining records in tape order under that occupant.
///
/// The seat is gone whether or not its records were presented, so its exact
/// binding leaves [`SessionProfiles`] either way: the next background fit
/// counts only seated occupants, while the continuing profile keeps every
/// card it retained for a successor that declares the same session. The
/// fitted generation itself is untouched; it moves at its own refit boundary.
///
/// [`SessionProfiles`]: spice::rooms::profile::SessionProfiles
pub(super) fn present_closed_transcript<P: AgentMessagePublisher>(
    bus: &Bus,
    epoch: CanonicalUuid,
    surfaces: &CaptureSurfaces<'_, P>,
    recognition: TasteRecognition<'_>,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    mut views: CaptureViews<'_>,
) -> CommandResult<()> {
    let presented = present_closing_records(
        bus,
        epoch,
        surfaces,
        recognition,
        lane,
        occupant,
        &mut views,
    );
    lock_state(views.state)?.profiles.vacate(lane, occupant);
    presented
}

fn present_closing_records<P: AgentMessagePublisher>(
    bus: &Bus,
    epoch: CanonicalUuid,
    surfaces: &CaptureSurfaces<'_, P>,
    mut recognition: TasteRecognition<'_>,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    views: &mut CaptureViews<'_>,
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
    sweep::present_swept(bus, epoch, surfaces, &mut recognition, &swept, views)?;
    publish_swept(
        bus,
        epoch,
        surfaces,
        &swept,
        steering::Refusals::Record,
        views,
    )?;
    lock_state(views.state)?.media_hydration.clear_lane(lane);
    Ok(())
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
    transcript: Option<&'a std::path::Path>,
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
    prose_origin: Option<wire::MessageProseOrigin>,
    suppress_answer: bool,
    superseded_answers: &'a [steer::Key],
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
    let authored = || {
        record
            .decoded
            .events()
            .iter()
            .filter_map(listen::Event::assistant_text)
    };
    let acknowledges = authored().any(steer::has_acknowledgement);
    let (packets, addresses) = packet_destinations(authored());
    let mut keys = direction::delivered(record.decoded.events());
    if !acknowledges && !packets && keys.is_empty() {
        return Ok(None);
    }
    if acknowledges {
        keys.extend(steering_keys(authored()));
    }
    let reader = bus.reader();
    let mut ledger = if keys.is_empty() {
        PresentationDirectives::default()
    } else {
        let presentations = load_steering_presentations(&reader, &keys)?;
        PresentationDirectives::from_presentations(
            &reader,
            &presentations,
            load_principal_roles(&reader)?,
        )
        .map_err(|error| store_fault("serve::message-steering-invalid", &error))?
    };
    ledger.prove_recipients(&reader, addresses)?;
    Ok(Some(ledger))
}

/// Whether `texts` hold a placed, well-formed packet and every occupant address
/// those packets name.
fn packet_destinations<'a>(
    texts: impl IntoIterator<Item = &'a str>,
) -> (bool, BTreeSet<wire::OccupantAddress>) {
    let mut packets = false;
    let mut addresses = BTreeSet::new();
    for publication in texts
        .into_iter()
        .filter(|text| text.contains(steer::ack::scan::message::MARKER))
        .flat_map(steer::ack::scan::scan_messages)
        .filter_map(|message| message.publication.ok())
    {
        packets = true;
        addresses.extend(publication.addresses());
    }
    (packets, addresses)
}

fn message_addresses<'a>(
    texts: impl IntoIterator<Item = &'a str>,
) -> BTreeSet<wire::OccupantAddress> {
    texts
        .into_iter()
        .filter(|text| text.contains(steer::ack::scan::message::MARKER))
        .flat_map(steer::ack::scan::scan_messages)
        .filter_map(|message| message.publication.ok())
        .flat_map(|publication| publication.addresses().collect::<Vec<_>>())
        .collect()
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
                    store_fault("serve::message-steering-unavailable", &error.to_string())
                })?,
        );
    }
    Ok(presentations)
}

/// The role heads deciding which loaded directives the operator authored.
pub(super) fn load_principal_roles(
    reader: &bus::BusReader,
) -> CommandResult<wire::principal::PrincipalRoles> {
    reader
        .principal_roles()
        .map_err(|error| store_fault("serve::message-roles-unavailable", &error.to_string()))
}

fn present_steering_acknowledgements(
    text: &str,
    steering: Option<&PresentationDirectives>,
    event: PresentationEvent,
) -> CommandResult<serve::stream::MessagePresentation> {
    let Some(steering) = steering else {
        return Ok(serve::stream::MessagePresentation::plain(text.to_owned()));
    };
    let presented = present_steering_acknowledgements_with_directives(text, steering, event)
        .map_err(|detail| store_fault("serve::message-steering-presentation-invalid", &detail))?;
    let broadcasts = answered_broadcasts(text, steering, event);
    let classifications = hydrate_message_classifications(&presented, broadcasts)
        .map_err(|detail| store_fault("serve::message-steering-presentation-invalid", &detail))?;
    let directive_contexts = hydrate_directive_contexts(&presented, steering)
        .map_err(|detail| store_fault("serve::message-steering-presentation-invalid", &detail))?;
    let sends = hydrate_sends(&presented, event)
        .map_err(|detail| store_fault("serve::message-steering-presentation-invalid", &detail))?;
    Ok(serve::stream::MessagePresentation {
        text: presented.text,
        acknowledgement: presented.acknowledgement,
        classifications,
        directive_contexts,
        sends,
    })
}

pub(crate) fn quote_agent_prose(
    reader: &bus::BusReader,
    lane: CanonicalUuid,
    source: CanonicalUuid,
    ordinal: u32,
    prose: &listen::Prose,
) -> Result<String, String> {
    let serve::stream::MessagePresentation {
        mut text,
        directive_contexts,
        ..
    } = present_acknowledged_prose(reader, lane, None, source, ordinal, prose)?;
    for context in directive_contexts.iter().rev() {
        let start = usize::try_from(context.start).expect("wire directive start always fits usize");
        let end = usize::try_from(context.end).expect("wire directive end always fits usize");
        text.replace_range(start..end, &context.quote_target.markdown());
    }
    Ok(text)
}

/// Present one lane's settled prose against the journal steering its
/// acknowledgement headers name, as its card shows it. `author` is the
/// occupant whose transcript wrote it, when the presenter knows it.
pub(super) fn present_acknowledged_prose(
    reader: &bus::BusReader,
    lane: CanonicalUuid,
    author: Option<CanonicalUuid>,
    source: CanonicalUuid,
    ordinal: u32,
    prose: &listen::Prose,
) -> Result<serve::stream::MessagePresentation, String> {
    let keys = steering_keys([prose.text.as_str()]);
    if keys.is_empty() {
        return Ok(serve::stream::MessagePresentation::plain(
            prose.text.clone(),
        ));
    }
    let rows = load_steering_presentations(reader, &keys).map_err(|error| format!("{error:?}"))?;
    let roles = load_principal_roles(reader).map_err(|error| format!("{error:?}"))?;
    let directives = PresentationDirectives::from_presentations(reader, &rows, roles)?;
    present_steering_acknowledgements(
        &prose.text,
        Some(&directives),
        PresentationEvent {
            lane,
            author,
            source: Some(source),
            locus: prose.locus,
            ordinal,
            superseded_answers: &[],
        },
    )
    .map_err(|error| format!("{error:?}"))
}

/// The journal context one card's presentation reads: the steering directives
/// its acknowledgement headers name, the occupant births its packet addresses
/// name, and the role heads deciding which directives the operator sent.
#[derive(Clone, Debug, Default)]
pub(super) struct PresentationDirectives {
    directives: BTreeMap<steer::Key, PresentationDirective>,
    /// An address names one birth forever, so a proved recipient never
    /// changes and departure never redirects it to the lane's next occupant.
    recipients: BTreeMap<wire::OccupantAddress, wire::MessageRecipient>,
    roles: wire::principal::PrincipalRoles,
}

#[derive(Clone, Debug)]
struct PresentationDirective {
    lane: CanonicalUuid,
    text: String,
    authored: bool,
    taste: bool,
    review: Option<wire::MessageReview>,
    disposition: wire::Disposition,
    quote_target: wire::QuoteTarget,
    /// Images retained by this directive, each with its exact quote target.
    media_quotes: Vec<wire::MediaQuote>,
    /// Actor on the creating fact, whose principal roles grant authority.
    actor: wire::Actor,
    /// Who said the directive and whom it reached, exactly as a quote of it
    /// names them. A packet's body already stands in `text`.
    attribution: wire::QuoteAttribution,
    /// Journal instant at which the directive's creating fact was committed.
    published: wire::Instant,
    /// Operator submit action this directive's send shares with its siblings.
    fanout: Option<CanonicalUuid>,
    retirement: Option<wire::body::TranscriptLocus>,
}

impl PresentationDirective {
    /// Badge of the resident-generated directive kind, from typed provenance.
    fn badge(&self) -> Option<wire::DirectiveBadge> {
        if self.review.is_some() {
            Some(wire::DirectiveBadge::Review)
        } else if self.taste {
            Some(wire::DirectiveBadge::Taste)
        } else {
            None
        }
    }
}

impl PresentationDirectives {
    fn new(directives: BTreeMap<steer::Key, PresentationDirective>) -> Self {
        Self {
            directives,
            recipients: BTreeMap::new(),
            roles: wire::principal::PrincipalRoles::default(),
        }
    }

    /// Whether `directive` was sent by an actor whose principal currently
    /// holds the operator role.
    fn operator_sent(&self, directive: &PresentationDirective) -> bool {
        self.roles
            .holds(&directive.actor, wire::principal::PrincipalRole::Operator)
    }

    fn from_presentations(
        reader: &bus::BusReader,
        rows: &[bus::SteeringPresentation],
        roles: wire::principal::PrincipalRoles,
    ) -> Result<Self, String> {
        let needs_review = rows.iter().any(|row| {
            row.head()
                .body()
                .control
                .as_deref()
                .and_then(wire::parse_review_feedback_control)
                .is_some()
        });
        let snapshot = needs_review
            .then(|| reader.task_snapshot().map_err(|error| error.to_string()))
            .transpose()?;
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
            let media_quotes = spice::quote::media_quotes(&quote_target, &head.attachments)
                .map_err(|error| format!("{error:?}"))?;
            let spice::quote::SteeringSource { attribution, body } =
                spice::quote::steering_source(row, &roles);
            let text = body.as_deref().unwrap_or(&head.text);
            directives.insert(
                key,
                PresentationDirective {
                    lane: head.lane,
                    text: wire::media::with_media_payloads(text, &head.attachments)
                        .map_err(|error| error.to_string())?
                        .into_owned(),
                    authored: true,
                    taste,
                    review: review_presentation(snapshot.as_ref(), head.control.as_deref())?,
                    disposition: head.disposition,
                    retirement,
                    quote_target,
                    media_quotes,
                    actor: row.created().actor.clone(),
                    attribution,
                    published: row.created().at,
                    fanout: head.fanout,
                },
            );
        }
        Ok(Self {
            roles,
            ..Self::new(directives)
        })
    }

    fn extend_presentations(
        &mut self,
        reader: &bus::BusReader,
        rows: &[bus::SteeringPresentation],
        roles: wire::principal::PrincipalRoles,
    ) -> Result<(), String> {
        let loaded = Self::from_presentations(reader, rows, roles)?;
        self.directives.extend(loaded.directives);
        self.roles = loaded.roles;
        Ok(())
    }

    /// Prove the occupant birth each not-yet-proved address names. An address
    /// no birth proves leaves its packet presented as written.
    fn prove_recipients(
        &mut self,
        reader: &bus::BusReader,
        addresses: BTreeSet<wire::OccupantAddress>,
    ) -> CommandResult<()> {
        for address in addresses {
            if self.recipients.contains_key(&address) {
                continue;
            }
            let proof = reader.occupant_birth(address).map_err(|error| {
                store_fault("serve::message-recipient-unreadable", &error.to_string())
            })?;
            if let Ok(birth) = proof {
                self.recipients.insert(
                    address,
                    wire::MessageRecipient {
                        address,
                        lane: birth.lane,
                        occupant: birth.occupant,
                    },
                );
            }
        }
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

pub(super) fn review_presentation(
    snapshot: Option<&bus::TaskSnapshot>,
    control: Option<&str>,
) -> Result<Option<wire::MessageReview>, String> {
    let Some((task, reviewed_at)) = control.and_then(wire::parse_review_feedback_control) else {
        return Ok(None);
    };
    let snapshot = snapshot.ok_or_else(|| "review feedback has no task snapshot".to_owned())?;
    let reviewed = snapshot
        .facts()
        .at(reviewed_at)
        .and_then(|envelope| envelope.fact.read::<wire::body::TaskReviewed>())
        .ok_or_else(|| format!("review feedback names missing review fact {reviewed_at}"))?;
    if reviewed.task != task {
        return Err(format!(
            "review feedback task {task} disagrees with review fact task {}",
            reviewed.task
        ));
    }
    let finding = reviewed
        .finding
        .filter(|finding| !finding.trim().is_empty())
        .ok_or_else(|| format!("review feedback for {task} has no finding"))?;
    let heads = snapshot
        .tasks()
        .binary_search_by(|heads| heads.statement().body().born.cmp(&task))
        .ok()
        .map(|index| &snapshot.tasks()[index])
        .ok_or_else(|| format!("review feedback task {task} has no current statement"))?;
    Ok(Some(wire::MessageReview {
        task: wire::TaskHandle::for_project(&heads.statement().body().project, &task),
        finding,
        guidance: wire::REVIEW_FEEDBACK_GUIDANCE.to_owned(),
    }))
}

/// How many broadcast markers the quoted contexts of directives this answer
/// actually answered may present.
fn answered_broadcasts(
    text: &str,
    directives: &PresentationDirectives,
    event: PresentationEvent,
) -> usize {
    let standing = EventDirectives { directives, event };
    recognized_answers(text, &standing)
        .map(|(key, _)| key)
        .collect::<BTreeSet<_>>()
        .iter()
        .filter_map(|key| directives.directives.get(key))
        .filter(|directive| !directive.taste && has_broadcast_marker(&directive.text))
        .count()
}

/// Classify the broadcast markers a presented message carries.
///
/// A broadcast marker opens first-level quoted directive text and is bounded
/// by the broadcasts this answer's recognized keys earned. Identical words
/// anywhere the agent wrote them stay literal, including inside a presented
/// MSG body. Acknowledgment disposition and badges are independent of source
/// ranges and never enter this collection.
fn hydrate_message_classifications(
    presented: &Presentation,
    mut broadcasts: usize,
) -> Result<Vec<wire::MessageClassification>, String> {
    let text = presented.text.as_str();
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_SMART_PUNCTUATION
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM;
    let kind = wire::MessageClassificationKind::Broadcast;
    let marker = kind.marker();
    let mut quote_depth = 0usize;
    let mut code_block = false;
    let mut image_depth = 0usize;
    let mut classifications = Vec::new();
    for (event, source) in Parser::new_ext(text, options).into_offset_iter() {
        if broadcasts == 0 {
            break;
        }
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
        if code_block
            || image_depth != 0
            || quote_depth != 1
            || presented
                .sends
                .iter()
                .any(|send| send.range.contains(&source.start))
        {
            continue;
        }
        let source_tail = text
            .get(source.start..)
            .ok_or_else(|| "Markdown source offset is not UTF-8 aligned".to_owned())?;
        if !source_tail
            .strip_prefix(marker)
            .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with(char::is_whitespace))
        {
            continue;
        }
        broadcasts -= 1;
        let end = source
            .start
            .checked_add(marker.len())
            .ok_or_else(|| "classification end overflowed".to_owned())?;
        classifications.push(wire::MessageClassification {
            start: u32::try_from(source.start)
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
/// Each context carries its directive's own attribution, exactly as a quote
/// of that directive names it; the card presenting it adds nothing.
fn hydrate_directive_contexts(
    presented: &Presentation,
    directives: &PresentationDirectives,
) -> Result<Vec<wire::MessageDirective>, String> {
    presented
        .contexts
        .iter()
        .map(|context| {
            let directive = directives.directives.get(&context.key).ok_or_else(|| {
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
                media_quotes: directive.media_quotes.clone(),
                attribution: directive.attribution.clone(),
                authored: directive.published,
                badge: directive.badge(),
                review: directive.review.clone(),
                delivery: None,
            })
        })
        .collect()
}

fn hydrate_sends(
    presented: &Presentation,
    event: PresentationEvent,
) -> Result<Vec<wire::MessageSend>, String> {
    if presented.sends.is_empty() {
        return Ok(Vec::new());
    }
    let sender = event
        .author
        .ok_or_else(|| "presented packet names no authoring occupant".to_owned())?;
    presented
        .sends
        .iter()
        .map(|send| {
            Ok(wire::MessageSend {
                start: u32::try_from(send.range.start)
                    .map_err(|_| "packet quote start exceeds the wire range".to_owned())?,
                end: u32::try_from(send.range.end)
                    .map_err(|_| "packet quote end exceeds the wire range".to_owned())?,
                from: event.lane,
                sender,
                recipients: send.recipients.clone(),
                operator: send.operator,
                contexts: send.contexts.clone(),
            })
        })
        .collect()
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

fn has_broadcast_marker(text: &str) -> bool {
    text.strip_prefix(wire::stream::BROADCAST_MARKER)
        .is_some_and(|suffix| suffix.starts_with(char::is_whitespace))
}

#[derive(Clone, Copy, Debug)]
struct PresentationEvent<'a> {
    lane: CanonicalUuid,
    /// Occupant whose transcript wrote the event, when the presenter knows it;
    /// only an event with an author presents its MSG packets as sends.
    author: Option<CanonicalUuid>,
    source: Option<CanonicalUuid>,
    locus: listen::Locus,
    ordinal: u32,
    /// Keys whose answer a later event in this captured record owns.
    superseded_answers: &'a [steer::Key],
}

struct EventDirectives<'a> {
    directives: &'a PresentationDirectives,
    event: PresentationEvent<'a>,
}

impl EventDirectives<'_> {
    fn recognizes(
        &self,
        key: &steer::Key,
        disposition: wire::Disposition,
        locus: steer::ack::scan::Locus,
    ) -> bool {
        if self.event.superseded_answers.contains(key) {
            return false;
        }
        let Some(directive) = self.directives.directives.get(key) else {
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
        if self.event.superseded_answers.contains(key) {
            return None;
        }
        self.directives
            .directives
            .get(key)
            .filter(|directive| {
                directive.lane == self.event.lane
                    && matches!(
                        directive.disposition,
                        wire::Disposition::Acked | wire::Disposition::Declined
                    )
                    && directive.review.is_none()
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
            .directives
            .get(key)
            .is_some_and(|directive| directive.authored)
    }

    /// Whether the journal still holds `key` pending, so an acknowledgement
    /// spoken here awaits the steer::acked fact before it reads as credit.
    fn pending(&self, key: &steer::Key) -> bool {
        self.directives
            .directives
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
}

impl steer::Standing for EventDirectives<'_> {
    fn disposition(&self, key: &steer::Key) -> Option<wire::Disposition> {
        if self.event.superseded_answers.contains(key) {
            return None;
        }
        let directive = self.directives.directives.get(key)?;
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
    event: PresentationEvent<'_>,
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
    event: PresentationEvent<'_>,
    locus: steer::ack::scan::Locus,
) -> bool {
    retirement_record_matches(retirement, event)
        && retirement.line as usize == locus.line
        && retirement.column as usize == locus.column
}
