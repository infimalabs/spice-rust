//! Resident-owned steering reads, transitions, and disposable renderings.
//! Transcript-backed disposition is the sole acknowledgment path.

use std::fs;
use std::sync::Mutex;

use bus::Bus;
use journal::{FactCause, GuardError, JournalGuard, JournalWrite};
use steer::Standing as _;
use steer::{Miss, Outcome as ScanOutcome};
use wire::body::TranscriptLocus;
use wire::intent::{
    CommandAnswer, DeliveryAnswer, DeliveryStanding, DirectiveAnswer, InboxAnswer, LaneInbox,
    PendingDirective,
};
use wire::{
    Actor, CanonicalUuid, Command, Disposition, Fault, FaultClass, Instant, Outcome, Producer,
    Refusal, Reply, ReplyPosition, ResidencyHead, Seq, SeqRange, Witness,
};

use super::{
    CommandResult, RuntimePaths, commit, current_residency, environment_fault, seq, store_fault,
    typed_outcome, typed_protocol_fault, typed_store_fault, write, write_at_origin,
};

mod preview;
#[cfg(test)]
mod tests;
mod transcript;
mod transitions;

use preview::directive_preview;
use transcript::transcript_recognition;
pub(super) use transitions::{cancel, dispose, recover, resend, send};
use transitions::{evidence_refusal, miss_recorded};

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
    let facts = bus
        .fact_snapshot()
        .map_err(|error| store_fault("steer::journal-unreadable", &error.to_string()))?;
    let ledger = steer::Ledger::from_snapshot(&facts)
        .map_err(|error| store_fault("steer::replay-invalid", &error.to_string()))?;
    let Some(current) = ledger.directive(expected.directive) else {
        return Ok(false);
    };
    if current.key() != &expected.key
        || current.lane() != expected.lane
        || current.expires() != expected.expires
        || current.disposition() != Disposition::Live
    {
        return Ok(false);
    }
    let transition_matches = match expected.transition {
        DeadlineTransition::Escalate { from, .. } => current.stage() == from,
        DeadlineTransition::Lapse => true,
    };
    if !transition_matches {
        return Ok(false);
    }
    let (origin, cause) = directive_cause(&facts, expected.directive);
    let mut proposal = JournalWrite::new(Actor::Spice, origin, at, Producer::BusTimer);
    match expected.transition {
        DeadlineTransition::Escalate { from, to } => {
            proposal.guard(stage_guard(expected.directive, from));
            proposal
                .fact(
                    wire::body::SteerEscalated {
                        directive: expected.directive,
                        key: expected.key.to_string(),
                        lane: expected.lane,
                        stage: to,
                    },
                    FactCause::Existing(cause),
                )
                .map_err(|error| store_fault("steer::fact-invalid", &error.to_string()))?;
        }
        DeadlineTransition::Lapse => {
            proposal.guard(lapse_guard(
                expected.directive,
                expected.key.clone(),
                expected.lane,
                expected.expires,
            ));
            proposal
                .fact(
                    wire::body::SteerLapsed {
                        key: expected.key.to_string(),
                        lane: expected.lane,
                        expires: expected.expires,
                    },
                    FactCause::Existing(cause),
                )
                .map_err(|error| store_fault("steer::fact-invalid", &error.to_string()))?;
        }
    }
    match commit(bus, proposal) {
        Ok(_) => Ok(true),
        Err(Outcome::Refusal(_)) => Ok(false),
        Err(outcome) => Err(outcome),
    }
}

const FOLD_RETRIES: usize = 4;
static FOLD_AUTHORITY: Mutex<()> = Mutex::new(());

pub(super) fn publish_lane_discoveries(
    paths: &RuntimePaths,
    operator: &canvas::Discovery,
    residency: &ResidencyHead,
) -> CommandResult<()> {
    fs::create_dir_all(&paths.lane_canvas).map_err(|error| {
        environment_fault("canvas::lane-directory-unavailable", &error.to_string())
    })?;
    for lane in &residency.lanes {
        let capability = lane_capability(lane)?;
        publish_lane_discovery(paths, operator, lane.lane, capability)?;
    }
    Ok(())
}

pub(super) fn publish_lane_surface(
    paths: &RuntimePaths,
    operator: &canvas::Discovery,
    lane: CanonicalUuid,
    capability: CanonicalUuid,
) -> CommandResult<()> {
    fs::create_dir_all(&paths.lane_canvas).map_err(|error| {
        environment_fault("canvas::lane-directory-unavailable", &error.to_string())
    })?;
    publish_lane_discovery(paths, operator, lane, capability)?;
    let seal = steer::Key::for_identity(capability);
    if let Err(error) = steer::inbox::publish(&paths.inbox, lane, &seal, &[]) {
        remove_lane_surface(paths, lane);
        return Err(environment_fault(
            "steer::inbox-unwritable",
            &error.to_string(),
        ));
    }
    Ok(())
}

pub(super) fn remove_lane_surface(paths: &RuntimePaths, lane: CanonicalUuid) {
    let _ = fs::remove_file(paths.lane_canvas.join(lane.to_string()));
    let _ = fs::remove_file(steer::inbox::path(&paths.inbox, lane));
}

fn publish_lane_discovery(
    paths: &RuntimePaths,
    operator: &canvas::Discovery,
    lane: CanonicalUuid,
    capability: CanonicalUuid,
) -> CommandResult<()> {
    canvas::Discovery {
        endpoint: operator.endpoint.clone(),
        capability,
        epoch: operator.epoch,
    }
    .publish(paths.lane_canvas.join(lane.to_string()))
    .map_err(|error| environment_fault("canvas::lane-publish-failed", &error.to_string()))
}

pub(super) fn publish_inboxes(
    bus: &Bus,
    paths: &RuntimePaths,
    residency: &ResidencyHead,
) -> CommandResult<()> {
    for lane in &residency.lanes {
        publish_inbox(bus, paths, residency, lane.lane)?;
    }
    Ok(())
}

pub(super) fn publish_inbox(
    bus: &Bus,
    paths: &RuntimePaths,
    residency: &ResidencyHead,
    selected: CanonicalUuid,
) -> CommandResult<()> {
    let lane = residency
        .lanes
        .iter()
        .find(|lane| lane.lane == selected)
        .ok_or_else(|| {
            store_fault(
                "steer::lane-missing-after-commit",
                &format!("lane {selected} disappeared while publishing its inbox"),
            )
        })?;
    fold_lane_inbox(bus, paths, lane).map(|_| ())
}

fn fold_lane_inbox(
    bus: &Bus,
    paths: &RuntimePaths,
    lane: &wire::LaneResidency,
) -> CommandResult<DeliveryStanding> {
    fold_lane_inbox_with(bus, paths, lane, |_| {})
}

pub(super) fn fold_lane_inbox_with(
    bus: &Bus,
    paths: &RuntimePaths,
    lane: &wire::LaneResidency,
    mut after_publication: impl FnMut(usize),
) -> CommandResult<DeliveryStanding> {
    // Publication and its durable progress record are one process-local
    // authority. A newer command may commit while this guard is held, but its
    // fold queues behind this one and therefore has the final word.
    let _authority = FOLD_AUTHORITY
        .lock()
        .map_err(|error| store_fault("steer::fold-authority-poisoned", &error.to_string()))?;
    for attempt in 0..FOLD_RETRIES {
        let snapshot = bus
            .fact_snapshot()
            .map_err(|error| store_fault("steer::journal-unreadable", &error.to_string()))?;
        let source = snapshot.horizon().ok_or_else(|| {
            store_fault(
                "steer::journal-empty",
                "a running resident has no durable startup fact",
            )
        })?;
        let ledger = steer::Ledger::from_snapshot(&snapshot)
            .map_err(|error| store_fault("steer::replay-invalid", &error.to_string()))?;
        let keys = pending_keys(&ledger, lane.lane);
        let publication = publish_lane_inbox(paths, lane, &ledger);
        after_publication(attempt);
        let current = bus
            .fact_snapshot()
            .map_err(|error| store_fault("steer::journal-unreadable", &error.to_string()))?;
        let current = steer::Ledger::from_snapshot(&current)
            .map_err(|error| store_fault("steer::replay-invalid", &error.to_string()))?;
        let moved = pending_keys(&current, lane.lane) != keys;
        if moved && attempt + 1 < FOLD_RETRIES {
            continue;
        }
        if moved {
            return Err(store_fault(
                "steer::fold-moving",
                "lane steering authority moved throughout the bounded fold refill",
            ));
        }
        let delivery = match publication {
            Ok(()) => DeliveryStanding::Published { source },
            Err(Outcome::Fault(fault)) => DeliveryStanding::Parked {
                source,
                keys,
                failure: format!("{}: {}", fault.reason, fault.detail),
                repair: format!("spice steer repair {}", lane.lane),
            },
            Err(outcome) => return Err(outcome),
        };
        let record = steer::fold::Record::new(lane.lane, delivery.clone());
        steer::fold::publish(&paths.data, &record).map_err(|error| {
            environment_fault("steer::fold-progress-unwritable", &error.to_string())
        })?;
        return Ok(delivery);
    }
    unreachable!("the final fold attempt returns instead of retrying")
}

fn pending_keys(ledger: &steer::Ledger, lane: CanonicalUuid) -> Vec<String> {
    ledger
        .pending(lane)
        .iter()
        .map(|directive| directive.key().to_string())
        .collect()
}

fn fold_delivery(paths: &RuntimePaths, lane: CanonicalUuid) -> CommandResult<DeliveryStanding> {
    steer::fold::read(&paths.data, lane)
        .map(|record| {
            record.map_or(DeliveryStanding::Unknown, |record| {
                record.delivery().clone()
            })
        })
        .map_err(|error| environment_fault("steer::fold-progress-unreadable", &error.to_string()))
}

fn publish_lane_inbox(
    paths: &RuntimePaths,
    lane: &wire::LaneResidency,
    ledger: &steer::Ledger,
) -> CommandResult<()> {
    let seal = steer::Key::for_identity(lane_capability(lane)?);
    steer::inbox::publish(&paths.inbox, lane.lane, &seal, &ledger.pending(lane.lane))
        .map_err(|error| environment_fault("steer::inbox-unwritable", &error.to_string()))
}

pub(super) fn lane_capability(lane: &wire::LaneResidency) -> CommandResult<CanonicalUuid> {
    match lane.capabilities.as_slice() {
        [capability] => Ok(*capability),
        capabilities => Err(store_fault(
            "canvas::lane-capability-invalid",
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
    let residency = match current_residency(bus, epoch) {
        Ok(residency) => residency,
        Err(outcome) => return typed_outcome(outcome),
    };
    let snapshot = match snapshot(bus) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let selected = match command.actor() {
        Actor::Operator => request.lane,
        Actor::Lane(lane) if request.lane.is_none_or(|requested| requested == *lane) => Some(*lane),
        Actor::Lane(lane) => {
            return refusal(
                "steer::lane-scope",
                Some(lane.to_string()),
                request.lane.map(|requested| requested.to_string()),
                "read only the inbox bound to the authenticated lane",
            );
        }
        Actor::Spice => {
            return typed_protocol_fault(
                "steer::principal-invalid",
                "resident principal cannot submit an interactive inbox read",
            );
        }
    };
    let mut lanes = Vec::new();
    for lane in &residency.body().lanes {
        if selected.is_some_and(|selected| selected != lane.lane) {
            continue;
        }
        lanes.push(LaneInbox {
            lane: lane.lane,
            occupant: lane.occupant,
            pending: snapshot
                .ledger
                .pending(lane.lane)
                .into_iter()
                .map(pending_directive)
                .collect(),
            delivery: match fold_delivery(paths, lane.lane) {
                Ok(delivery) => delivery,
                Err(outcome) => return typed_outcome(outcome),
            },
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

pub(super) fn repair(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    request: &wire::intent::SteerRepair,
) -> Outcome<CommandAnswer> {
    if !matches!(command.actor(), Actor::Operator) {
        return refusal(
            "steer::operator-required",
            Some("operator".to_owned()),
            Some(command.actor().to_string()),
            "submit fold repair through the current operator discovery",
        );
    }
    let residency = match current_residency(bus, epoch) {
        Ok(residency) => residency,
        Err(outcome) => return typed_outcome(outcome),
    };
    let Some(lane) = residency
        .body()
        .lanes
        .iter()
        .find(|lane| lane.lane == request.lane)
    else {
        return refusal(
            "steer::repair-lane-unknown",
            Some("one current seated lane".to_owned()),
            Some(request.lane.to_string()),
            "read lane state and repair a lane the resident currently owns",
        );
    };
    let delivery = match fold_lane_inbox(bus, paths, lane) {
        Ok(delivery) => delivery,
        Err(outcome) => return typed_outcome(outcome),
    };
    let source = match &delivery {
        DeliveryStanding::Published { source } | DeliveryStanding::Parked { source, .. } => *source,
        DeliveryStanding::Unknown => {
            return typed_store_fault(
                "steer::fold-standing-invalid",
                "repair completed without a published or parked standing",
            );
        }
    };
    Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(source),
        body: CommandAnswer::Delivery(DeliveryAnswer {
            lane: request.lane,
            delivery: Box::new(delivery),
        }),
    })
}

struct SteeringSnapshot {
    facts: journal::FactSnapshot,
    ledger: steer::Ledger,
    position: Seq,
}

fn snapshot(bus: &Bus) -> Result<SteeringSnapshot, Outcome<CommandAnswer>> {
    let facts = bus
        .fact_snapshot()
        .map_err(|error| typed_store_fault("steer::journal-unreadable", &error.to_string()))?;
    let position = facts.horizon().ok_or_else(|| {
        typed_store_fault(
            "steer::journal-empty",
            "a running resident has no durable startup fact",
        )
    })?;
    let ledger = steer::Ledger::from_snapshot(&facts)
        .map_err(|error| typed_store_fault("steer::replay-invalid", &error.to_string()))?;
    Ok(SteeringSnapshot {
        facts,
        ledger,
        position,
    })
}

fn pending_directive(directive: &steer::Directive) -> PendingDirective {
    PendingDirective {
        directive: directive.directive(),
        key: directive.key().to_string(),
        text: directive.text().to_owned(),
        preview: directive_preview(directive.text()),
        control: directive.control().map(str::to_owned),
        attachments: directive.attachments().to_vec(),
        authored: directive
            .published()
            .expect("a directive replayed from authority has a publication instant"),
        stage: directive.stage(),
        escalations: directive.escalations().to_vec(),
        expires: directive.expires(),
    }
}

fn live_directive(
    ledger: &steer::Ledger,
    id: CanonicalUuid,
) -> Result<&steer::Directive, Outcome<CommandAnswer>> {
    let Some(directive) = ledger.directive(id) else {
        return Err(refusal(
            "steer::directive-unknown",
            None,
            Some(id.to_string()),
            "read the current inbox and use a directive identity it names",
        ));
    };
    if directive.disposition() != Disposition::Live {
        return Err(refusal(
            "steer::directive-retired",
            Some("live".to_owned()),
            Some(format!("{:?}", directive.disposition()).to_lowercase()),
            "read the current inbox before retrying the disposition",
        ));
    }
    Ok(directive)
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
    remedy: &str,
) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: Some("steering-standing".to_owned()),
        expected,
        observed,
        remedy: remedy.to_owned(),
    })
}

fn current_instant() -> Result<Instant, Outcome<CommandAnswer>> {
    Instant::new(jiff::Timestamp::now())
        .map_err(|error| typed_environment_fault("resident::clock-invalid", &error.to_string()))
}

/// Mints the first unused key at or after the current millisecond.
///
/// The journal guard repeats the collision check at commit time. Advancing a
/// millisecond here matches the predecessor when a durable key already owns
/// the clock's current millisecond.
fn mint_key(ledger: &steer::Ledger, at: Instant) -> Result<steer::Key, Outcome<CommandAnswer>> {
    let mut millis = at.get().as_millisecond();
    loop {
        let key = steer::Key::for_millis(millis).map_err(|error| {
            typed_environment_fault("steer::key-clock-invalid", &error.to_string())
        })?;
        if ledger.disposition(&key).is_none() {
            return Ok(key);
        }
        millis = millis.checked_add(1).ok_or_else(|| {
            typed_environment_fault(
                "steer::key-clock-invalid",
                "epoch millisecond overflowed while advancing past a collision",
            )
        })?;
    }
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
    let origin = format!("resident:{epoch}")
        .parse::<wire::Origin>()
        .map_err(|error| store_fault("resident::origin-invalid", &error.to_string()))?;
    Ok(JournalWrite::new(actor, origin, at, producer))
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
                .any(|row| row.lane == lane && row.occupant == Some(occupant));
        if !matches {
            return Err(GuardError::refused("steer::residency-moved"));
        }
        let snapshot = read
            .fact_snapshot()
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        let ledger = steer::Ledger::from_snapshot(&snapshot)
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        if ledger.directive(directive).is_some() || ledger.disposition(&key).is_some() {
            return Err(GuardError::refused("steer::identity-taken"));
        }
        Ok(())
    })
}

fn live_guard(directive: CanonicalUuid) -> JournalGuard {
    JournalGuard::new(move |read| {
        let snapshot = read
            .fact_snapshot()
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        let ledger = steer::Ledger::from_snapshot(&snapshot)
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        match ledger.directive(directive) {
            Some(current) if current.disposition() == Disposition::Live => Ok(()),
            _ => Err(GuardError::refused("steer::directive-moved")),
        }
    })
}

fn stage_guard(directive: CanonicalUuid, stage: wire::body::SteeringStage) -> JournalGuard {
    JournalGuard::new(move |read| {
        let snapshot = read
            .fact_snapshot()
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        let ledger = steer::Ledger::from_snapshot(&snapshot)
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        match ledger.directive(directive) {
            Some(current)
                if current.disposition() == Disposition::Live && current.stage() == stage =>
            {
                Ok(())
            }
            _ => Err(GuardError::refused("steer::presentation-moved")),
        }
    })
}

fn lapse_guard(
    directive: CanonicalUuid,
    key: steer::Key,
    lane: CanonicalUuid,
    expires: Instant,
) -> JournalGuard {
    JournalGuard::new(move |read| {
        let snapshot = read
            .fact_snapshot()
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        let ledger = steer::Ledger::from_snapshot(&snapshot)
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        match ledger.directive(directive) {
            Some(current)
                if current.key() == &key
                    && current.lane() == lane
                    && current.expires() == expires
                    && current.disposition() == Disposition::Live =>
            {
                Ok(())
            }
            _ => Err(GuardError::refused("steer::expiry-moved")),
        }
    })
}

fn recovery_guard(
    directive: CanonicalUuid,
    key: steer::Key,
    lane: CanonicalUuid,
    lapsed_at: Instant,
) -> JournalGuard {
    JournalGuard::new(move |read| {
        let snapshot = read
            .fact_snapshot()
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        let ledger = steer::Ledger::from_snapshot(&snapshot)
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        match ledger.directive(directive) {
            Some(current)
                if current.key() == &key
                    && current.lane() == lane
                    && current.expires() == lapsed_at
                    && current.disposition() == Disposition::Lapsed =>
            {
                Ok(())
            }
            _ => Err(GuardError::refused("steer::recovery-moved")),
        }
    })
}

fn miss_guard(expected: wire::body::SteerMissed) -> JournalGuard {
    JournalGuard::new(move |read| {
        let snapshot = read
            .fact_snapshot()
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        if miss_recorded(&snapshot, &expected) {
            Err(GuardError::refused("steer::recognition-recorded"))
        } else {
            Ok(())
        }
    })
}

fn directive_cause(
    snapshot: &journal::FactSnapshot,
    directive: CanonicalUuid,
) -> (wire::Origin, Seq) {
    let envelope = directive_creation(snapshot, directive);
    (envelope.origin.clone(), envelope.seq)
}

fn directive_creation(
    snapshot: &journal::FactSnapshot,
    directive: CanonicalUuid,
) -> &wire::Envelope<wire::AnyFact> {
    snapshot
        .facts()
        .iter()
        .find_map(|envelope| match envelope.fact.kind() {
            wire::FactKind::SteerSent => steering_body::<wire::body::SteerSent>(envelope)
                .filter(|body| body.directive == directive)
                .map(|_| envelope),
            // A reminder publishes a directive exactly as a send does, so it
            // is a creating fact here for the same reason. Every kind the
            // ledger admits as a publication has to be findable, because the
            // `expect` below reads a missing one as impossible rather than as absent.
            wire::FactKind::TasteReminded => steering_body::<wire::body::TasteReminded>(envelope)
                .filter(|body| body.directive == directive)
                .map(|_| envelope),
            _ => None,
        })
        .expect("a reduced directive has one creating fact")
}

fn steering_body<T: serde::de::DeserializeOwned>(
    envelope: &wire::Envelope<wire::AnyFact>,
) -> Option<T> {
    wire::encode(envelope.fact.body())
        .ok()
        .and_then(|bytes| wire::decode(&bytes).ok())
}

fn finish(
    bus: &Bus,
    paths: &RuntimePaths,
    residency: &ResidencyHead,
    lane: CanonicalUuid,
    proposal: JournalWrite,
    mut answer: DirectiveAnswer,
) -> Outcome<CommandAnswer> {
    let committed = match commit(bus, proposal) {
        Ok(committed) => committed,
        Err(outcome) => return typed_outcome(outcome),
    };
    let selected = match residency.lanes.iter().find(|row| row.lane == lane) {
        Some(selected) => selected,
        None => {
            return typed_store_fault(
                "steer::lane-missing-after-commit",
                &format!("lane {lane} disappeared while publishing its inbox"),
            );
        }
    };
    answer.delivery = match fold_lane_inbox(bus, paths, selected) {
        Ok(delivery) => Box::new(delivery),
        Err(outcome) => return typed_outcome(outcome),
    };
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
