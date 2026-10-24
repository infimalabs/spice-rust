//! Resident-owned steering reads and transitions.
//! Transcript-backed disposition is the sole acknowledgment path.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use bus::Bus;
use journal::{FactCause, GuardError, JournalGuard, JournalWrite, PendingFact};
use steer::Outcome as ScanOutcome;
use wire::body::TranscriptLocus;
use wire::intent::{
    CommandAnswer, DirectiveAnswer, InboxAnswer, LaneInbox, PendingDirective,
    RETIRED_DIRECTIVE_WINDOW, RetiredDirective,
};
use wire::{
    Actor, CanonicalUuid, Command, Disposition, Fault, FaultClass, Instant, Outcome, Producer,
    Refusal, Reply, ReplyPosition, ResidencyHead, Seq, SeqRange, Witness,
};

use super::{
    CommandEffects, CommandResult, RuntimePaths, commit, current_residency, environment_fault, seq,
    store_fault, typed_outcome, typed_protocol_fault, typed_store_fault,
};

mod exit;
mod history;
mod message;
mod preview;
mod reply;
#[cfg(test)]
mod tests;
mod transcript;
mod transitions;

pub(super) use exit::{dispose_at_exit, dispose_at_startup, dispose_heard};
pub(super) use history::history;
pub(super) use message::{
    AuthoredRecord, LANE_REMOVED, MessageRecordedNotice, MessageRefusalNotice, Refusals,
    carries_packet, dead_letter_removed_lanes, publish_at_startup, publish_authored,
};
#[cfg(test)]
pub(super) use message::{Packet, Published, Unpublished, publish_packet};
use preview::directive_preview;
use transitions::evidence_refusal;
pub(crate) use transitions::{
    OperatorDirective, admit_send_envelope as steering_envelope_admits, append_operator_directive,
};
pub(super) use transitions::{cancel, recover, resend, send};

const LEGACY_TASK_GENERATION_CONTROL_PREFIX: &str = "task-generation:";

/// Whether retained steering evidence belongs to the predecessor task notifier.
pub(super) fn is_legacy_task_generation_control(control: Option<&str>) -> bool {
    control.is_some_and(|control| control.starts_with(LEGACY_TASK_GENERATION_CONTROL_PREFIX))
}

/// The directive a send opened when its authority is allowed to wake a lane.
///
/// Sends by an actor whose principal holds the operator role retain universal
/// wake authority. Resident sends have none except the exact source-locus
/// control that tells a packet author their packet was wholly withheld. The
/// lifecycle resolver and the inbox's `wakes` flag both answer from this one
/// predicate.
pub(super) fn operator_sent(
    roles: &wire::principal::PrincipalRoles,
    envelope: &wire::Envelope<wire::AnyFact>,
) -> Option<CanonicalUuid> {
    let sent = envelope.fact.read::<wire::body::SteerSent>()?;
    sent_wakes(roles, &envelope.actor, sent.control.as_deref()).then_some(sent.directive)
}

fn sent_wakes(
    roles: &wire::principal::PrincipalRoles,
    actor: &Actor,
    control: Option<&str>,
) -> bool {
    roles.holds(actor, wire::principal::PrincipalRole::Operator)
        || (*actor == Actor::Spice
            && control
                .and_then(wire::parse_packet_withheld_control)
                .is_some())
}

/// Stable identity for one resident-owned control directive.
pub(super) fn internal_directive_identity(control: &str) -> CanonicalUuid {
    let hash = blake3::hash(format!("spice:{control}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&hash.as_bytes()[..16]);
    CanonicalUuid::new(uuid::Uuid::from_bytes(bytes))
}

pub(super) struct InternalDirective {
    pub(super) directive: CanonicalUuid,
    pub(super) lane: CanonicalUuid,
    pub(super) text: String,
    pub(super) control: String,
    pub(super) lifetime: std::time::Duration,
    pub(super) cause: FactCause,
    pub(super) published: Instant,
}

pub(super) fn internal_write(epoch: CanonicalUuid) -> CommandResult<(JournalWrite, Instant)> {
    let at = current_instant().map_err(unit_outcome)?;
    Ok((
        JournalWrite::new(
            Actor::Spice,
            resident_origin(epoch)?,
            at,
            Producer::CommandDoor,
        ),
        at,
    ))
}

/// Append one resident-authored control directive to an existing transaction.
pub(super) fn append_internal_directive(
    bus: &Bus,
    proposal: &mut JournalWrite,
    epoch: CanonicalUuid,
    internal: InternalDirective,
) -> CommandResult<PendingFact> {
    let directive = internal.directive;
    let (existing, minted_micros) = keyspace(bus, directive).map_err(unit_outcome)?;
    if existing.is_some() {
        return Err(store_fault(
            "steer::identity-taken",
            "a newly minted internal directive identity already exists",
        ));
    }
    let at = internal.published;
    let key = mint_key(minted_micros).map_err(unit_outcome)?;
    let expires = expiry(at, internal.lifetime).map_err(unit_outcome)?;
    let sent = wire::body::SteerSent {
        from: None,
        room: None,
        directive,
        fanout: None,
        key: key.to_string(),
        lane: internal.lane,
        text: internal.text,
        control: Some(internal.control),
        attachments: Vec::new(),
        expires,
    };
    let origin = resident_origin(epoch)?;
    proposal.guard(internal_send_guard(directive, key));
    let pending = proposal
        .fact(sent.clone(), internal.cause)
        .map_err(|error| store_fault("steer::fact-invalid", &error.to_string()))?;
    proposal.steering_head(pending, move |seq| {
        steer::Ledger::sent_head(&sent, origin, at, seq)
            .expect("the internal steering body is admitted before publication")
    });
    Ok(pending)
}

/// Current indexed standing for one deterministic resident directive.
pub(super) fn control_standing(
    bus: &Bus,
    directive: CanonicalUuid,
    lane: CanonicalUuid,
    control: &str,
) -> CommandResult<Option<Disposition>> {
    let Some(head) = bus
        .read(move |read| read.steering_head(directive))
        .map_err(|error| store_fault("steer::index-unreadable", &error.to_string()))?
    else {
        return Ok(None);
    };
    if head.body().lane != lane || head.body().control.as_deref() != Some(control) {
        return Err(store_fault(
            "steer::control-mismatched",
            "the deterministic resident directive identifies another lane or control",
        ));
    }
    Ok(Some(head.body().disposition))
}

/// Cancel the one live resident control directive, if it has not retired.
pub(super) fn append_control_cancellation(
    bus: &Bus,
    proposal: &mut JournalWrite,
    directive: CanonicalUuid,
    lane: CanonicalUuid,
    control: &str,
    published: Instant,
) -> CommandResult<Option<PendingFact>> {
    let Some(head) = bus
        .read(move |read| read.steering_head(directive))
        .map_err(|error| store_fault("steer::index-unreadable", &error.to_string()))?
    else {
        return Ok(None);
    };
    if head.body().lane != lane || head.body().control.as_deref() != Some(control) {
        return Err(store_fault(
            "steer::control-mismatched",
            "the deterministic resident directive identifies another lane or control",
        ));
    }
    if head.body().disposition != Disposition::Live {
        return Ok(None);
    }
    proposal.guard(live_guard(head.body().directive, head.position()));
    let canceled = wire::body::SteerCanceled {
        key: head.body().key.clone(),
        lane,
    };
    let pending = proposal
        .fact(canceled.clone(), FactCause::Existing(head.body().created))
        .map_err(|error| store_fault("steer::fact-invalid", &error.to_string()))?;
    let next = steer::Ledger::transition_head(
        head.body(),
        steer::Steering::Canceled(&canceled),
        published,
    )
    .map_err(|error| store_fault("steer::index-invalid", &error.to_string()))?;
    proposal.steering_head(pending, move |_| next);
    Ok(Some(pending))
}

/// Admit or rebuild the bounded steering authority before the bus accepts work.
pub(super) fn admit_index(journal: &mut journal::Journal) -> CommandResult<()> {
    if journal
        .steering_index_current()
        .map_err(|error| store_fault("steer::index-unreadable", &error.to_string()))?
    {
        return Ok(());
    }
    let snapshot = journal
        .steering_snapshot()
        .map_err(|error| store_fault("steer::journal-unreadable", &error.to_string()))?;
    let index = steer::Ledger::authority_index_from_facts(snapshot.facts())
        .map_err(|error| store_fault("steer::replay-invalid", &error.to_string()))?;
    journal
        .rebuild_steering_index(snapshot.horizon(), index.heads(), index.misses())
        .map_err(|error| store_fault("steer::index-unwritable", &error.to_string()))
}

/// Migrate steering-backed private notices into current authority once at startup.
///
/// Claim generations and renewal marks already retain their semantic state. A
/// cancellation preserves the retired directive as evidence while preventing
/// the old delivery mechanism from becoming an executable runtime arm.
pub(super) fn migrate_private_notices(bus: &Bus, epoch: CanonicalUuid) -> CommandResult<()> {
    let pending = bus
        .read(|read| crate::steering_read::pending(read, None))
        .map_err(|error| store_fault("steer::index-unreadable", &error.to_string()))?;
    let legacy = pending
        .iter()
        .filter_map(|head| {
            (is_legacy_task_generation_control(head.body().control.as_deref())
                || head
                    .body()
                    .control
                    .as_deref()
                    .is_some_and(|control| control.starts_with("lane-renewal-handoff:")))
            .then_some(head.body())
        })
        .collect::<Vec<_>>();
    if legacy.is_empty() {
        return Ok(());
    }
    let (mut proposal, published) = internal_write(epoch)?;
    for body in legacy {
        let control = body
            .control
            .as_deref()
            .expect("a classified legacy control is present");
        append_control_cancellation(
            bus,
            &mut proposal,
            body.directive,
            body.lane,
            control,
            published,
        )?;
    }
    commit(bus, proposal)?;
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct DeadLetter {
    pub(super) directive: CanonicalUuid,
    pub(super) key: String,
    pub(super) requeue: String,
}

/// Steering one failed start parked, with the failure that parked it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct DeadLettered {
    pub(super) parked: Vec<DeadLetter>,
    reason: String,
    detail: Option<String>,
}

impl DeadLettered {
    /// One line per parked directive, each naming the failure exactly as
    /// recorded, so the log alone explains the loss.
    pub(super) fn log(
        &self,
        diagnostic: &mut dyn std::io::Write,
        trigger: &str,
        lane: CanonicalUuid,
    ) {
        for directive in &self.parked {
            let _ = write!(
                diagnostic,
                "lifecycle dead-lettered trigger={trigger} lane={lane} key={} directive={} requeue={} reason={}",
                directive.key, directive.directive, directive.requeue, self.reason,
            );
            let _ = match &self.detail {
                Some(detail) => writeln!(diagnostic, " detail={detail}"),
                None => writeln!(diagnostic),
            };
        }
    }
}

/// Pending steering one inbox wake was admitted to deliver.
pub(super) struct WokenInbox(Vec<journal::JournalHead<wire::SteeringHead>>);

/// Read the pending steering an inbox wake on `lane` is admitted to deliver.
pub(super) fn woken_inbox(bus: &Bus, lane: CanonicalUuid) -> CommandResult<WokenInbox> {
    bus.read(move |read| crate::steering_read::pending(read, Some(lane)))
        .map(WokenInbox)
        .map_err(|error| store_fault("steer::dead-letter-index-unreadable", &error.to_string()))
}

/// Park the steering one inbox wake was admitted to deliver after its start
/// failed terminally.
///
/// Only directives whose standing is unchanged since admission park. Steering
/// that arrived or moved while the occupant proved its session stays pending
/// for the next wake, and unrelated commits never refuse the parking.
pub(super) fn dead_letter_pending(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    woken: &WokenInbox,
    failure: &Outcome<()>,
) -> CommandResult<DeadLettered> {
    let current = bus
        .read(move |read| crate::steering_read::pending(read, Some(lane)))
        .map_err(|error| store_fault("steer::dead-letter-index-unreadable", &error.to_string()))?;
    let heads = woken
        .0
        .iter()
        .filter(|head| current.contains(head))
        .cloned()
        .collect::<Vec<_>>();
    if heads.is_empty() {
        return Err(Outcome::Refusal(Refusal::bare(
            wire::RefusalDiagnostic::new(
                "steer::dead-letter-standing-moved".to_owned(),
                Some("pending-inbox".to_owned()),
                Some(lane.to_string()),
                Some("empty".to_owned()),
            )
            .with_detail("reconcile the current pending-inbox arm"),
        )));
    }
    let (reason, detail) = lifecycle_failure(failure);
    let mut parked = Vec::with_capacity(heads.len());
    let at = current_instant().map_err(|outcome| match outcome {
        Outcome::Fault(fault) => Outcome::Fault(fault),
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Reply(_) => unreachable!("clock reads are not command replies"),
    })?;
    let mut proposal = write_at(Actor::Spice, epoch, at, Producer::ResidentRecognition)?;
    let standing = heads.clone();
    proposal.guard(JournalGuard::new(move |read| {
        for head in standing {
            if read.steering_head(head.body().directive)? != Some(head) {
                return Err(GuardError::refused("steer::dead-letter-standing-moved"));
            }
        }
        Ok(())
    }));
    for directive in &heads {
        let requeue = format!("spice steer recover {}", directive.body().directive);
        let body = wire::steering::SteerDeadLettered {
            directive: directive.body().directive,
            key: directive.body().key.clone(),
            lane,
            reason: reason.clone(),
            requeue: requeue.clone(),
        };
        let pending = proposal
            .fact(body.clone(), FactCause::Existing(directive.body().created))
            .map_err(|error| store_fault("steer::dead-letter-fact-invalid", &error.to_string()))?;
        let next = steer::Ledger::transition_head(
            directive.body(),
            steer::Steering::DeadLettered(&body),
            at,
        )
        .map_err(|error| store_fault("steer::index-invalid", &error.to_string()))?;
        proposal.steering_head(pending, move |_| next);
        parked.push(DeadLetter {
            directive: directive.body().directive,
            key: directive.body().key.clone(),
            requeue,
        });
    }
    commit(bus, proposal)?;
    notify_inbox(paths, lane);
    Ok(DeadLettered {
        parked,
        reason,
        detail,
    })
}

/// The reason and recorded detail of the failure that ended a start. A
/// supervisor's start failure carries its `state.json` failure text as detail.
fn lifecycle_failure(failure: &Outcome<()>) -> (String, Option<String>) {
    match failure {
        Outcome::Refusal(refusal) => (
            refusal.reason().to_owned(),
            refusal
                .diagnostic()
                .and_then(|diagnostic| diagnostic.detail())
                .map(ToOwned::to_owned),
        ),
        Outcome::Fault(fault) => (fault.reason.clone(), Some(fault.detail.clone())),
        Outcome::Reply(_) => unreachable!("a successful lifecycle start was not dead-lettered"),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SteeringDeadline {
    pub(super) directive: CanonicalUuid,
    pub(super) key: steer::Key,
    pub(super) lane: CanonicalUuid,
    pub(super) expires: Instant,
    pub(super) transition: DeadlineTransition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DeadlineTransition {
    Escalate {
        from: wire::body::SteeringStage,
        to: wire::body::SteeringStage,
    },
    Lapse,
}

pub(super) fn apply_deadline(
    bus: &Bus,
    expected: &SteeringDeadline,
    at: Instant,
) -> CommandResult<bool> {
    let (_, current) =
        directive_head(bus, expected.directive).map_err(|outcome| match outcome {
            Outcome::Fault(fault) => Outcome::Fault(fault),
            Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
            Outcome::Reply(_) => unreachable!("an indexed directive read is not a command reply"),
        })?;
    let Some(current) = current else {
        return Ok(false);
    };
    if current.body().key != expected.key.as_str()
        || current.body().lane != expected.lane
        || current.body().expires != expected.expires
        || current.body().disposition != Disposition::Live
    {
        return Ok(false);
    }
    let transition_matches = match expected.transition {
        DeadlineTransition::Escalate { from, .. } => current.body().stage == from,
        DeadlineTransition::Lapse => true,
    };
    if !transition_matches {
        return Ok(false);
    }
    let mut proposal = JournalWrite::new(
        Actor::Spice,
        current.body().origin.clone(),
        at,
        Producer::BusTimer,
    );
    let cause = FactCause::Existing(current.body().created);
    let (pending, next) = match expected.transition {
        DeadlineTransition::Escalate { from, to } => {
            proposal.guard(stage_guard(expected.directive, current.position(), from));
            let body = wire::body::SteerEscalated {
                directive: expected.directive,
                key: expected.key.to_string(),
                lane: expected.lane,
                stage: to,
            };
            let pending = proposal
                .fact(body.clone(), cause)
                .map_err(|error| store_fault("steer::fact-invalid", &error.to_string()))?;
            let next = steer::Ledger::transition_head(
                current.body(),
                steer::Steering::Escalated(&body),
                at,
            )
            .map_err(|error| store_fault("steer::index-invalid", &error.to_string()))?;
            (pending, next)
        }
        DeadlineTransition::Lapse => {
            proposal.guard(lapse_guard(
                expected.directive,
                current.position(),
                expected.key.clone(),
                expected.lane,
                expected.expires,
            ));
            let body = wire::body::SteerLapsed {
                key: expected.key.to_string(),
                lane: expected.lane,
                expires: expected.expires,
            };
            let pending = proposal
                .fact(body.clone(), cause)
                .map_err(|error| store_fault("steer::fact-invalid", &error.to_string()))?;
            let next =
                steer::Ledger::transition_head(current.body(), steer::Steering::Lapsed(&body), at)
                    .map_err(|error| store_fault("steer::index-invalid", &error.to_string()))?;
            (pending, next)
        }
    };
    proposal.steering_head(pending, move |_| next);
    match commit(bus, proposal) {
        Ok(_) => Ok(true),
        Err(Outcome::Refusal(_)) => Ok(false),
        Err(outcome) => Err(outcome),
    }
}

pub(super) fn publish_lane_discoveries(
    paths: &RuntimePaths,
    operator: &serve::Discovery,
    residency: &ResidencyHead,
) -> CommandResult<()> {
    fs::create_dir_all(&paths.lane_serve).map_err(|error| {
        environment_fault("serve::lane-directory-unavailable", &error.to_string())
    })?;
    for lane in &residency.lanes {
        let capability = lane_capability(lane)?;
        publish_lane_discovery(paths, operator, lane.lane, capability)?;
    }
    Ok(())
}

pub(super) fn publish_lane_surface(
    paths: &RuntimePaths,
    operator: &serve::Discovery,
    lane: CanonicalUuid,
    capability: CanonicalUuid,
) -> CommandResult<()> {
    fs::create_dir_all(&paths.lane_serve).map_err(|error| {
        environment_fault("serve::lane-directory-unavailable", &error.to_string())
    })?;
    publish_lane_discovery(paths, operator, lane, capability)
}

pub(super) fn remove_lane_surface(paths: &RuntimePaths, lane: CanonicalUuid) {
    let _ = fs::remove_file(paths.lane_serve.join(lane.to_string()));
}

fn publish_lane_discovery(
    paths: &RuntimePaths,
    operator: &serve::Discovery,
    lane: CanonicalUuid,
    capability: CanonicalUuid,
) -> CommandResult<()> {
    serve::Discovery {
        endpoint: operator.endpoint.clone(),
        capability,
        epoch: operator.epoch,
    }
    .publish(paths.lane_serve.join(lane.to_string()))
    .map_err(|error| environment_fault("serve::lane-publish-failed", &error.to_string()))
}

pub(super) fn notify_inboxes(paths: &RuntimePaths, residency: &ResidencyHead) {
    for lane in &residency.lanes {
        notify_inbox(paths, lane.lane);
    }
}

pub(super) fn notify_inbox(paths: &RuntimePaths, lane: CanonicalUuid) {
    super::supervisor::command::kick(paths, lane);
}

pub(super) fn lane_capability(lane: &wire::LaneResidency) -> CommandResult<CanonicalUuid> {
    match lane.capabilities.as_slice() {
        [capability] => Ok(*capability),
        capabilities => Err(store_fault(
            "serve::lane-capability-invalid",
            &format!(
                "lane {} carries {} capabilities instead of exactly one",
                lane.lane,
                capabilities.len()
            ),
        )),
    }
}

pub(super) fn inbox(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    request: &wire::intent::SteerInbox,
) -> Outcome<CommandAnswer> {
    let selected = match command.actor() {
        _ if command.holds(wire::principal::PrincipalRole::Operator) => request.lane,
        Actor::Lane(lane) if request.lane.is_none_or(|requested| requested == *lane) => Some(*lane),
        Actor::Lane(lane) => {
            return refusal(
                "steer::lane-scope",
                Some(lane.to_string()),
                request.lane.map(|requested| requested.to_string()),
                "read only the inbox bound to the authenticated lane",
            );
        }
        actor => {
            return refusal(
                "steer::operator-required",
                Some("operator".to_owned()),
                Some(actor.to_string()),
                "read another lane's inbox while holding the operator role",
            );
        }
    };
    let retired = if request.retired {
        RETIRED_DIRECTIVE_WINDOW
    } else {
        0
    };
    let snapshot = match pending_snapshot(bus, selected, epoch, retired) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let reader = bus.reader();
    let resolver = spice::quote::Resolver::from_tape_with_agent_presenter(
        &paths.tape,
        &reader,
        super::quote_agent_prose,
    );
    let mut lanes = Vec::new();
    for lane in &snapshot.residency.body().lanes {
        if selected.is_some_and(|selected| selected != lane.lane) {
            continue;
        }
        let compaction = match lane.occupant {
            Some(occupant) => {
                match super::taste_surface::context_anchor(
                    snapshot.authority.facts(),
                    lane.lane,
                    occupant,
                ) {
                    Ok(anchor) => Some(anchor.get()),
                    Err(error) => {
                        return typed_store_fault("steer::compaction-anchor-missing", &error);
                    }
                }
            }
            None => None,
        };
        let mut pending = match pending_presentations(
            &snapshot.ledger,
            &snapshot.waking,
            &snapshot.authority,
            lane.lane,
        ) {
            Ok(pending) => pending,
            Err(outcome) => return outcome,
        };
        for directive in &mut pending {
            if let Some(origin) = snapshot.peers.get(&directive.directive) {
                directive.sender = Some(origin.sender);
                directive.addressed = origin.addressed;
            }
            directive.text = resolver.expand(&directive.text);
            directive.preview = directive_preview(&directive.text);
        }
        let recent = snapshot
            .retired
            .iter()
            .filter(|head| head.lane == lane.lane)
            .map(|head| retired_directive(&resolver, head))
            .collect();
        lanes.push(LaneInbox {
            lane: lane.lane,
            occupant: lane.occupant,
            compaction,
            pending,
            retired: recent,
        });
    }
    if let Some(selected) = selected
        && !lanes.iter().any(|lane| lane.lane == selected)
    {
        return refusal(
            "steer::lane-unknown",
            None,
            Some(selected.to_string()),
            "seat the lane before reading its inbox",
        );
    }
    Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(snapshot.position),
        body: CommandAnswer::Inbox(InboxAnswer { epoch, lanes }),
    })
}

/// Resolve one quote target exactly as delivery and the app do.
///
/// Resolution reads tape and journal authority and commits nothing, so a
/// reader may ask as often as it reads.
pub(super) fn quote(
    bus: &Bus,
    paths: &RuntimePaths,
    request: &wire::intent::QuoteRead,
) -> Outcome<CommandAnswer> {
    let cursor = match bus.read(|read| read.cursor()) {
        Ok(Some(cursor)) => cursor,
        Ok(None) => {
            return typed_store_fault(
                "quote::journal-empty",
                "a running resident has no startup authority fact",
            );
        }
        Err(error) => return typed_store_fault("quote::journal-unreadable", &error.to_string()),
    };
    let reader = bus.reader();
    let resolution = spice::quote::Resolver::from_tape_with_agent_presenter(
        &paths.tape,
        &reader,
        super::quote_agent_prose,
    )
    .resolve(&request.target, 0);
    Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(cursor),
        body: CommandAnswer::Quote(Box::new(resolution)),
    })
}

struct SteeringSnapshot {
    ledger: steer::Ledger,
    /// Pending directives an operator send opened, which alone wake a lane.
    waking: BTreeSet<CanonicalUuid>,
    /// Pending peer directives whose sending occupant the journal proves.
    peers: BTreeMap<CanonicalUuid, PeerOrigin>,
    retired: Vec<journal::RetiredSteering>,
    position: Seq,
    authority: bus::TaskSnapshot,
    residency: journal::JournalHead<ResidencyHead>,
}

/// Who sent one pending peer directive and, for a message that named an
/// earlier occupant of its lane, whom it named.
#[derive(Clone, Copy)]
struct PeerOrigin {
    sender: wire::intent::PeerSender,
    addressed: Option<wire::intent::PeerAddressed>,
}

/// The exact occupant that sent the pending peer directive `head`'s creating
/// fact opened, when the journal proves one, with the earlier occupant a
/// redirected message named.
///
/// An MSG names its authoring occupant. A reply is spoken by whichever
/// occupant of the answered lane retired the MSG: the reply's cause is that
/// retirement, whose transcript locus names the session that wrote it. A
/// locus no source-qualified session declares proves no one. Other direction
/// records no sending occupant.
fn peer_origin(
    authority: &bus::TaskSnapshot,
    reader: &bus::BusReader,
    head: &wire::SteeringHead,
) -> Result<Option<PeerOrigin>, String> {
    let facts = authority.facts();
    let Some(created) = facts.at(head.created) else {
        return Ok(None);
    };
    if let Some(message) = created.fact.read::<wire::body::SteerMessaged>() {
        if message.directive != head.directive {
            return Ok(None);
        }
        let addressed = message
            .addressed
            .map(|address| peer_addressed(authority, reader, &message, address))
            .transpose()?;
        return Ok(Some(PeerOrigin {
            sender: peer_sender(authority, message.from, message.sender),
            addressed,
        }));
    }
    if let Some(message) = created.fact.read::<wire::body::SteerQueued>() {
        return Ok((message.directive == head.directive).then(|| PeerOrigin {
            sender: peer_sender(authority, message.from, message.sender),
            addressed: None,
        }));
    }
    if created.fact.read::<wire::body::SteerReplied>().is_none() {
        return Ok(None);
    }
    let Some(retirement) = created.cause.and_then(|cause| facts.at(cause)) else {
        return Ok(None);
    };
    let answer = retirement
        .fact
        .read::<wire::body::SteerAcked>()
        .map(|body| (body.lane, body.locus))
        .or_else(|| {
            retirement
                .fact
                .read::<wire::body::SteerDeclined>()
                .map(|body| (body.lane, body.locus))
        });
    let Some((lane, locus)) = answer else {
        return Ok(None);
    };
    let source = locus
        .source
        .expect("source-qualified steering retirement carries a tape generation");
    let session = reader
        .transcript_session(lane, source, locus.member, locus.offset)
        .map_err(|error| format!("{error:?}"))?;
    Ok(session.map(|session| PeerOrigin {
        sender: peer_sender(authority, lane, session.occupant),
        addressed: None,
    }))
}

/// The earlier occupant a redirected message named, with its birth and the
/// birth of the occupant that heard the message, both proved from the
/// journal's `lane::started` facts.
fn peer_addressed(
    authority: &bus::TaskSnapshot,
    reader: &bus::BusReader,
    message: &wire::body::SteerMessaged,
    address: wire::OccupantAddress,
) -> Result<wire::intent::PeerAddressed, String> {
    let birth = |address| {
        reader
            .occupant_birth(address)
            .map_err(|error| format!("{error:?}"))?
            .map_err(|refusal| refusal.to_string())
    };
    let successor = authority
        .facts()
        .occupant_address(message.lane, message.occupant)
        .map_err(|refusal| refusal.to_string())?;
    Ok(wire::intent::PeerAddressed {
        address,
        born: birth(address)?.at,
        successor_born: birth(successor)?.at,
    })
}

/// One exact sending occupant, with the address its birth on `lane` proves.
fn peer_sender(
    authority: &bus::TaskSnapshot,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> wire::intent::PeerSender {
    wire::intent::PeerSender {
        occupant,
        address: authority.facts().occupant_address(lane, occupant).ok(),
    }
}

fn pending_snapshot(
    bus: &Bus,
    lane: Option<CanonicalUuid>,
    epoch: CanonicalUuid,
    retired: usize,
) -> Result<SteeringSnapshot, Outcome<CommandAnswer>> {
    const PAGE_RETRIES: usize = 3;
    for _ in 0..PAGE_RETRIES {
        let first = bus
            .steering_snapshot(lane, journal::FactPageStart::After(None), retired)
            .map_err(|error| typed_store_fault("steer::index-unreadable", &error.to_string()))?;
        let authority = first.authority().clone();
        let position = authority.cursor().ok_or_else(|| {
            typed_store_fault(
                "steer::journal-empty",
                "a running resident has no durable startup fact",
            )
        })?;
        let mut heads = first.pending().rows().to_vec();
        let mut continuation = first
            .pending()
            .overflow()
            .map(|overflow| overflow.continuation);
        let mut moved = first.pending().horizon() != Some(position);
        while !moved {
            let Some(next) = continuation else {
                break;
            };
            let page = bus
                .steering_snapshot(lane, journal::FactPageStart::At(next), 0)
                .map_err(|error| {
                    typed_store_fault("steer::index-unreadable", &error.to_string())
                })?;
            moved = page.authority().cursor() != Some(position)
                || page.pending().horizon() != Some(position);
            if moved {
                break;
            }
            heads.extend_from_slice(page.pending().rows());
            continuation = page
                .pending()
                .overflow()
                .map(|overflow| overflow.continuation);
        }
        if moved {
            continue;
        }
        let residency = authority.residency().cloned().ok_or_else(|| {
            typed_store_fault("resident::head-missing", "residency head is absent")
        })?;
        if residency.body().epoch != epoch {
            return Err(typed_store_fault(
                "resident::epoch-moved",
                &format!("expected {epoch}, observed {}", residency.body().epoch),
            ));
        }
        let ledger = steer::Ledger::from_authority_heads(heads.iter().map(|head| head.body()))
            .map_err(|error| typed_store_fault("steer::index-invalid", &error.to_string()))?;
        let roles = bus
            .reader()
            .principal_roles()
            .map_err(|error| typed_store_fault("steer::roles-unreadable", &error.to_string()))?;
        let waking = heads
            .iter()
            .map(journal::JournalHead::body)
            .filter(|head| {
                authority
                    .facts()
                    .at(head.created)
                    .and_then(|envelope| operator_sent(&roles, envelope))
                    == Some(head.directive)
            })
            .map(|head| head.directive)
            .collect();
        let reader = bus.reader();
        let mut peers = BTreeMap::new();
        for head in heads.iter().map(journal::JournalHead::body) {
            let sender = peer_origin(&authority, &reader, head)
                .map_err(|error| typed_store_fault("steer::answer-author-unreadable", &error))?;
            if let Some(sender) = sender {
                peers.insert(head.directive, sender);
            }
        }
        return Ok(SteeringSnapshot {
            ledger,
            waking,
            peers,
            retired: first.into_retired(),
            position,
            authority,
            residency,
        });
    }
    Err(typed_store_fault(
        "steer::index-moving",
        "steering authority moved throughout bounded page recovery",
    ))
}

fn directive_head(
    bus: &Bus,
    directive: CanonicalUuid,
) -> Result<(Seq, Option<journal::JournalHead<wire::SteeringHead>>), Outcome<CommandAnswer>> {
    bus.read(move |read| {
        let cursor = read
            .cursor()?
            .ok_or_else(|| GuardError::Invalid("running resident journal is empty".to_owned()))?;
        Ok((cursor, read.steering_head(directive)?))
    })
    .map_err(|error| typed_store_fault("steer::index-unreadable", &error.to_string()))
}

struct IndexedStanding {
    key: Option<steer::Key>,
    disposition: Option<Disposition>,
}

impl IndexedStanding {
    fn new(
        key: Option<steer::Key>,
        head: Option<&journal::JournalHead<wire::SteeringHead>>,
    ) -> Self {
        Self {
            key,
            disposition: head.map(|head| head.body().disposition),
        }
    }
}

impl steer::Standing for IndexedStanding {
    fn disposition(&self, key: &steer::Key) -> Option<Disposition> {
        (self.key.as_ref() == Some(key))
            .then_some(self.disposition)
            .flatten()
    }
}

type SteeringKeyspace = (
    Option<journal::JournalHead<wire::SteeringHead>>,
    Option<i64>,
);

fn keyspace(
    bus: &Bus,
    directive: CanonicalUuid,
) -> Result<SteeringKeyspace, Outcome<CommandAnswer>> {
    bus.read(move |read| {
        Ok((
            read.steering_head(directive)?,
            read.steering_minted_micros()?,
        ))
    })
    .map_err(|error| typed_store_fault("steer::index-unreadable", &error.to_string()))
}

fn pending_directive(
    directive: &steer::Directive,
    wakes: bool,
    authority: &bus::TaskSnapshot,
) -> Result<PendingDirective, Outcome<CommandAnswer>> {
    let room = directive
        .room()
        .map(|room| {
            authority
                .task_heads(room)
                .map(|heads| wire::TaskHandle::for_project(&heads.statement().body().project, room))
                .ok_or_else(|| {
                    typed_store_fault(
                        "steer::room-head-missing",
                        &format!("frozen room anchor {room} has no task head"),
                    )
                })
        })
        .transpose()?;
    Ok(PendingDirective {
        directive: directive.directive(),
        key: directive.key().to_string(),
        fanout: directive.fanout(),
        text: directive.text().to_owned(),
        from: directive.from(),
        sender: None,
        room,
        addressed: None,
        wakes,
        preview: directive_preview(directive.text()),
        control: directive.control().map(str::to_owned),
        attachments: directive.attachments().to_vec(),
        authored: directive
            .published()
            .expect("a directive replayed from authority has a publication instant"),
        stage: directive.stage(),
        escalations: directive.escalations().to_vec(),
        expires: directive.expires(),
    })
}

fn retired_directive(
    resolver: &spice::quote::Resolver<'_>,
    head: &journal::RetiredSteering,
) -> RetiredDirective {
    let text = resolver.expand(&head.text);
    let preview = directive_preview(&text);
    RetiredDirective {
        directive: head.directive,
        key: head.key.clone(),
        fanout: head.fanout,
        text,
        from: head.from,
        preview,
        attachments: head.attachments.clone(),
        authored: head.published,
        disposition: head.disposition,
        position: head.retired,
        retired: head.retired_at,
    }
}

/// Project every pending directive onto the ordinary inline steering path.
pub(super) fn pending_presentations(
    ledger: &steer::Ledger,
    waking: &BTreeSet<CanonicalUuid>,
    authority: &bus::TaskSnapshot,
    lane: CanonicalUuid,
) -> Result<Vec<PendingDirective>, Outcome<CommandAnswer>> {
    ledger
        .pending(lane)
        .into_iter()
        .map(|directive| {
            pending_directive(
                directive,
                waking.contains(&directive.directive()),
                authority,
            )
        })
        .collect()
}

fn live_head(
    head: Option<journal::JournalHead<wire::SteeringHead>>,
    id: CanonicalUuid,
) -> Result<journal::JournalHead<wire::SteeringHead>, Outcome<CommandAnswer>> {
    let Some(head) = head else {
        return Err(refusal(
            "steer::directive-unknown",
            None,
            Some(id.to_string()),
            "read the current inbox and use a directive identity it names",
        ));
    };
    if head.body().disposition != Disposition::Live {
        return Err(refusal(
            "steer::directive-retired",
            Some("live".to_owned()),
            Some(format!("{:?}", head.body().disposition).to_lowercase()),
            "read the current inbox before retrying the disposition",
        ));
    }
    Ok(head)
}

fn witness_refusal(observed: &[Witness], expected: &Witness) -> Outcome<CommandAnswer> {
    refusal(
        "steer::witness-moved",
        Some(format!("{expected:?}")),
        Some(format!("{observed:?}")),
        "read the current steering snapshot and retry its exact witness",
    )
}

fn refusal(
    reason: &str,
    expected: Option<String>,
    observed: Option<String>,
    detail: &str,
) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(
            reason.to_owned(),
            Some("steering-standing".to_owned()),
            expected,
            observed,
        )
        .with_detail(detail),
    ))
}

#[cfg(test)]
thread_local! {
    static PINNED_AUTHORED: std::cell::Cell<Option<jiff::Timestamp>> =
        const { std::cell::Cell::new(None) };
}

/// Run `work` with every directive this thread authors stamped at `at`.
#[cfg(test)]
pub(super) fn authored_at<T>(at: jiff::Timestamp, work: impl FnOnce() -> T) -> T {
    let previous = PINNED_AUTHORED.replace(Some(at));
    let value = work();
    PINNED_AUTHORED.set(previous);
    value
}

fn current_instant() -> Result<Instant, Outcome<CommandAnswer>> {
    #[cfg(test)]
    let now = PINNED_AUTHORED.get().unwrap_or_else(jiff::Timestamp::now);
    #[cfg(not(test))]
    let now = jiff::Timestamp::now();
    Instant::new(now)
        .map_err(|error| typed_environment_fault("resident::clock-invalid", &error.to_string()))
}

/// Mints the first unused key at or after the current microsecond.
///
/// The journal guard repeats the collision check at commit time. Advancing a
/// microsecond here preserves the monotone high-water when a durable key
/// already owns the clock's current microsecond.
fn mint_key(previous: Option<i64>) -> Result<steer::Key, Outcome<CommandAnswer>> {
    mint_key_at(previous, jiff::Timestamp::now().as_microsecond())
}

fn mint_key_at(previous: Option<i64>, now: i64) -> Result<steer::Key, Outcome<CommandAnswer>> {
    let micros = previous.map_or(Ok(now), |previous| {
        previous
            .checked_add(1)
            .map(|next| next.max(now))
            .ok_or_else(|| {
                typed_environment_fault(
                    "steer::key-clock-invalid",
                    "epoch microsecond overflowed while advancing the indexed key clock",
                )
            })
    })?;
    steer::Key::for_micros(micros)
        .map_err(|error| typed_environment_fault("steer::key-clock-invalid", &error.to_string()))
}

fn expiry(at: Instant, duration: std::time::Duration) -> Result<Instant, Outcome<CommandAnswer>> {
    let duration = jiff::SignedDuration::try_from(duration)
        .map_err(|error| typed_environment_fault("steer::expiry-invalid", &error.to_string()))?;
    let expires = at
        .get()
        .checked_add(duration)
        .map_err(|error| typed_environment_fault("steer::expiry-invalid", &error.to_string()))?;
    Instant::new(expires)
        .map_err(|error| typed_environment_fault("steer::expiry-invalid", &error.to_string()))
}

fn due_stage(
    published: Instant,
    at: Instant,
    config: &steer::Config,
) -> Result<wire::body::SteeringStage, Outcome<CommandAnswer>> {
    let elapsed = at.get().duration_since(published.get());
    let [reminded, escalated, overdue] = config.stage_horizons();
    let signed = |duration: std::time::Duration| {
        jiff::SignedDuration::try_from(duration)
            .map_err(|error| typed_environment_fault("steer::horizon-invalid", &error.to_string()))
    };
    if elapsed >= signed(overdue)? {
        Ok(wire::body::SteeringStage::Overdue)
    } else if elapsed >= signed(escalated)? {
        Ok(wire::body::SteeringStage::Escalated)
    } else if elapsed >= signed(reminded)? {
        Ok(wire::body::SteeringStage::Reminded)
    } else {
        Ok(wire::body::SteeringStage::Published)
    }
}

fn missing_stages(
    current: wire::body::SteeringStage,
    due: wire::body::SteeringStage,
) -> Vec<wire::body::SteeringStage> {
    let mut stages = Vec::new();
    let mut cursor = current;
    while cursor < due {
        let Some(next) = cursor.next() else {
            break;
        };
        stages.push(next);
        cursor = next;
    }
    stages
}

fn write_at(
    actor: Actor,
    epoch: CanonicalUuid,
    at: Instant,
    producer: Producer,
) -> CommandResult<JournalWrite> {
    Ok(JournalWrite::new(
        actor,
        resident_origin(epoch)?,
        at,
        producer,
    ))
}

fn resident_origin(epoch: CanonicalUuid) -> CommandResult<wire::Origin> {
    format!("resident:{epoch}")
        .parse::<wire::Origin>()
        .map_err(|error| store_fault("resident::origin-invalid", &error.to_string()))
}

fn typed_command<B: serde::Serialize>(command: &wire::AdmittedCommand, body: B) -> Command<B> {
    Command {
        v: wire::WireVersion::CURRENT,
        id: command.id(),
        epoch: command.epoch(),
        kind: command.kind(),
        expected: command.expected().to_vec(),
        body,
    }
}

fn send_guard(
    head: Seq,
    witness: Witness,
    directive: CanonicalUuid,
    key: steer::Key,
) -> JournalGuard {
    JournalGuard::new(move |read| {
        let current = read
            .residency_head()?
            .ok_or_else(|| GuardError::refused("steer::residency-missing"))?;
        if current.position() != head {
            return Err(GuardError::refused("steer::residency-moved"));
        }
        let Witness::Residency {
            lane,
            occupant,
            epoch,
        } = witness
        else {
            return Err(GuardError::Invalid(
                "steering send guard received another witness".to_owned(),
            ));
        };
        let matches = current.body().epoch == epoch
            && current
                .body()
                .lanes
                .iter()
                .any(|row| row.lane == lane && row.occupant == occupant);
        if !matches {
            return Err(GuardError::refused("steer::residency-moved"));
        }
        if read.steering_head(directive)?.is_some() || read.steering_key(key.as_str())?.is_some() {
            return Err(GuardError::refused("steer::identity-taken"));
        }
        Ok(())
    })
}

fn internal_send_guard(directive: CanonicalUuid, key: steer::Key) -> JournalGuard {
    JournalGuard::new(move |read| {
        if read.steering_head(directive)?.is_some() || read.steering_key(key.as_str())?.is_some() {
            Err(GuardError::refused("steer::identity-taken"))
        } else {
            Ok(())
        }
    })
}

fn unit_outcome(outcome: Outcome<CommandAnswer>) -> Outcome<()> {
    match outcome {
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
        Outcome::Reply(_) => unreachable!("a steering helper failure returned a reply"),
    }
}

fn live_guard(directive: CanonicalUuid, position: Seq) -> JournalGuard {
    JournalGuard::new(move |read| match read.steering_head(directive)? {
        Some(current)
            if current.position() == position
                && current.body().disposition == Disposition::Live =>
        {
            Ok(())
        }
        _ => Err(GuardError::refused("steer::directive-moved")),
    })
}

fn stage_guard(
    directive: CanonicalUuid,
    position: Seq,
    stage: wire::body::SteeringStage,
) -> JournalGuard {
    JournalGuard::new(move |read| match read.steering_head(directive)? {
        Some(current)
            if current.position() == position
                && current.body().disposition == Disposition::Live
                && current.body().stage == stage =>
        {
            Ok(())
        }
        _ => Err(GuardError::refused("steer::presentation-moved")),
    })
}

fn lapse_guard(
    directive: CanonicalUuid,
    position: Seq,
    key: steer::Key,
    lane: CanonicalUuid,
    expires: Instant,
) -> JournalGuard {
    JournalGuard::new(move |read| match read.steering_head(directive)? {
        Some(current)
            if current.position() == position
                && current.body().key == key.as_str()
                && current.body().lane == lane
                && current.body().expires == expires
                && current.body().disposition == Disposition::Live =>
        {
            Ok(())
        }
        _ => Err(GuardError::refused("steer::expiry-moved")),
    })
}

fn recovery_guard(
    directive: CanonicalUuid,
    position: Seq,
    key: steer::Key,
    lane: CanonicalUuid,
    lapsed_at: Instant,
    disposition: Disposition,
) -> JournalGuard {
    JournalGuard::new(move |read| match read.steering_head(directive)? {
        Some(current)
            if current.position() == position
                && current.body().key == key.as_str()
                && current.body().lane == lane
                && current.body().expires == lapsed_at
                && current.body().disposition == disposition =>
        {
            Ok(())
        }
        _ => Err(GuardError::refused("steer::recovery-moved")),
    })
}

fn finish(
    bus: &Bus,
    lane: CanonicalUuid,
    proposal: JournalWrite,
    answer: DirectiveAnswer,
    effects: &mut CommandEffects,
) -> Outcome<CommandAnswer> {
    let committed = match commit(bus, proposal) {
        Ok(committed) => committed,
        Err(outcome) => return typed_outcome(outcome),
    };
    effects.notify_inbox(lane);
    Outcome::Reply(Reply {
        position: ReplyPosition::Committed(SeqRange {
            first: seq(committed.first()),
            last: seq(committed.last()),
        }),
        body: CommandAnswer::Directive(answer),
    })
}

fn typed_environment_fault(reason: &str, detail: &str) -> Outcome<CommandAnswer> {
    Outcome::Fault(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}
