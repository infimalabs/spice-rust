//! Resident-owned steering reads, transitions, and disposable renderings.
//! Transcript-backed disposition is the sole acknowledgment path.

use std::fs;

use bus::Bus;
use journal::{FactCause, GuardError, JournalGuard, JournalWrite};
use steer::Standing as _;
use steer::{Miss, Outcome as ScanOutcome};
use wire::body::TranscriptLocus;
use wire::intent::{CommandAnswer, DirectiveAnswer, InboxAnswer, LaneInbox, PendingDirective};
use wire::{
    Actor, CanonicalUuid, Command, Disposition, Fault, FaultClass, Instant, Outcome, Producer,
    Refusal, Reply, ReplyPosition, ResidencyHead, Seq, SeqRange, Witness,
};

use super::{
    CommandResult, RuntimePaths, commit, current_residency, environment_fault, seq, store_fault,
    typed_outcome, typed_protocol_fault, typed_store_fault, write, write_at_origin,
};

mod transcript;

use transcript::transcript_recognition;

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
    let seal = steer::Key::for_directive(capability);
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
    let snapshot = bus
        .fact_snapshot()
        .map_err(|error| store_fault("steer::journal-unreadable", &error.to_string()))?;
    let ledger = steer::Ledger::from_snapshot(&snapshot)
        .map_err(|error| store_fault("steer::replay-invalid", &error.to_string()))?;
    for lane in &residency.lanes {
        publish_lane_inbox(paths, lane, &ledger)?;
    }
    Ok(())
}

fn publish_inbox(
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
    let snapshot = bus
        .fact_snapshot()
        .map_err(|error| store_fault("steer::journal-unreadable", &error.to_string()))?;
    let ledger = steer::Ledger::from_snapshot(&snapshot)
        .map_err(|error| store_fault("steer::replay-invalid", &error.to_string()))?;
    publish_lane_inbox(paths, lane, &ledger)
}

fn publish_lane_inbox(
    paths: &RuntimePaths,
    lane: &wire::LaneResidency,
    ledger: &steer::Ledger,
) -> CommandResult<()> {
    let seal = steer::Key::for_directive(lane_capability(lane)?);
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

pub(super) fn send(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    config: &steer::Config,
    command: &wire::AdmittedCommand,
    send: &wire::intent::SteerSend,
) -> Outcome<CommandAnswer> {
    if !matches!(command.actor(), Actor::Operator) {
        return refusal(
            "steer::operator-required",
            Some("operator".to_owned()),
            Some(command.actor().to_string()),
            "submit steering through the current operator discovery",
        );
    }
    if send.message.trim().is_empty() {
        return refusal(
            "steer::message-empty",
            Some("non-empty direction".to_owned()),
            Some(send.message.clone()),
            "supply direction text that the lane can act on",
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
        .find(|lane| lane.lane == send.lane)
    else {
        return refusal(
            "steer::lane-unknown",
            None,
            Some(send.lane.to_string()),
            "seat the lane before sending it direction",
        );
    };
    let Some(occupant) = lane.occupant else {
        return refusal(
            "steer::lane-vacant",
            Some("occupied".to_owned()),
            Some("vacant".to_owned()),
            "start an occupant in the lane before sending it direction",
        );
    };
    let witness = Witness::Residency {
        lane: send.lane,
        occupant,
        epoch,
    };
    if command.expected() != [witness.clone()] {
        return witness_refusal(command.expected(), &witness);
    }
    let snapshot = match snapshot(bus) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let key = steer::Key::for_directive(command.id());
    if snapshot.ledger.directive(command.id()).is_some()
        || snapshot.ledger.disposition(&key).is_some()
    {
        return refusal(
            "steer::identity-taken",
            Some("unused directive identity and key".to_owned()),
            Some(format!("{}:{key}", command.id())),
            "submit a new command identity",
        );
    }
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let expires = match expiry(at, config.expiry().get()) {
        Ok(expires) => expires,
        Err(outcome) => return outcome,
    };
    let sent = wire::body::SteerSent {
        directive: command.id(),
        key: key.to_string(),
        lane: send.lane,
        text: send.message.clone(),
        control: None,
        expires,
    };
    let mut proposal = match write_at(command.actor().clone(), epoch, at, Producer::CommandDoor) {
        Ok(proposal) => proposal,
        Err(outcome) => return typed_outcome(outcome),
    };
    proposal.guard(send_guard(
        residency.position(),
        witness,
        command.id(),
        key.clone(),
    ));
    if let Err(error) = proposal.fact(sent, FactCause::Existing(residency.position())) {
        return typed_store_fault("steer::fact-invalid", &error.to_string());
    }
    let answer = DirectiveAnswer {
        directive: command.id(),
        key: key.to_string(),
        disposition: Disposition::Live,
    };
    if let Err(error) =
        proposal.command_result(typed_command(command, send.clone()), epoch, answer.clone())
    {
        return typed_store_fault("steer::receipt-invalid", &error.to_string());
    }
    finish(bus, paths, residency.body(), send.lane, proposal, answer)
}

pub(super) fn cancel(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    cancel: &wire::intent::DirectiveRef,
) -> Outcome<CommandAnswer> {
    let snapshot = match snapshot(bus) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let directive = match live_directive(&snapshot.ledger, cancel.directive) {
        Ok(directive) => directive,
        Err(outcome) => return outcome,
    };
    let witness = Witness::Directive {
        steer: cancel.directive,
        disposition: Disposition::Live,
    };
    if command.expected() != [witness.clone()] {
        return witness_refusal(command.expected(), &witness);
    }
    let (origin, cause) = directive_cause(&snapshot.facts, cancel.directive);
    let mut proposal = match write_at_origin(command.actor().clone(), origin, Producer::CommandDoor)
    {
        Ok(proposal) => proposal,
        Err(outcome) => return typed_outcome(outcome),
    };
    proposal.guard(live_guard(cancel.directive));
    if let Err(error) = proposal.fact(
        wire::body::SteerCanceled {
            key: directive.key().to_string(),
            lane: directive.lane(),
        },
        FactCause::Existing(cause),
    ) {
        return typed_store_fault("steer::fact-invalid", &error.to_string());
    }
    let answer = DirectiveAnswer {
        directive: cancel.directive,
        key: directive.key().to_string(),
        disposition: Disposition::Canceled,
    };
    if let Err(error) = proposal.command_result(
        typed_command(command, cancel.clone()),
        epoch,
        answer.clone(),
    ) {
        return typed_store_fault("steer::receipt-invalid", &error.to_string());
    }
    let residency = match current_residency(bus, epoch) {
        Ok(residency) => residency,
        Err(outcome) => return typed_outcome(outcome),
    };
    finish(
        bus,
        paths,
        residency.body(),
        directive.lane(),
        proposal,
        answer,
    )
}

pub(super) fn dispose(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    dispose: &wire::intent::SteerDispose,
) -> Outcome<CommandAnswer> {
    if !matches!(
        dispose.disposition,
        Disposition::Acked | Disposition::Declined
    ) {
        return refusal(
            "steer::disposition-invalid",
            Some("acked or declined".to_owned()),
            Some(format!("{:?}", dispose.disposition).to_lowercase()),
            "use ack with or without --decline",
        );
    }
    let Actor::Lane(actor_lane) = command.actor() else {
        return refusal(
            "steer::lane-required",
            Some("authenticated lane".to_owned()),
            Some(command.actor().to_string()),
            "submit acknowledgment from an activated lane shell",
        );
    };
    let residency = match current_residency(bus, epoch) {
        Ok(residency) => residency,
        Err(outcome) => return typed_outcome(outcome),
    };
    let snapshot = match snapshot(bus) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(vendor) = residency
        .body()
        .lanes
        .iter()
        .find(|lane| lane.lane == *actor_lane)
        .map(|lane| &lane.vendor)
    else {
        return refusal(
            "steer::lane-missing",
            Some("current durable lane".to_owned()),
            Some(actor_lane.to_string()),
            "use a capability belonging to one current lane",
        );
    };
    let recognition = match transcript_recognition(
        paths,
        *actor_lane,
        vendor,
        &dispose.token,
        dispose.disposition,
        &snapshot.ledger,
    ) {
        Ok(recognition) => recognition,
        Err(outcome) => return outcome,
    };
    match recognition.outcome {
        ScanOutcome::Retired { key, disposition } => record_disposition(
            bus,
            paths,
            epoch,
            command,
            dispose,
            residency.body(),
            &snapshot,
            *actor_lane,
            key,
            disposition,
            recognition.locus,
        ),
        ScanOutcome::Missed { token, miss } => record_miss(
            bus,
            epoch,
            command,
            residency.position(),
            &snapshot.facts,
            token,
            miss,
            recognition.locus,
            *actor_lane,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn record_disposition(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    dispose: &wire::intent::SteerDispose,
    residency: &ResidencyHead,
    snapshot: &SteeringSnapshot,
    actor_lane: CanonicalUuid,
    key: steer::Key,
    disposition: Disposition,
    locus: TranscriptLocus,
) -> Outcome<CommandAnswer> {
    let directive = snapshot
        .ledger
        .directives()
        .into_iter()
        .find(|directive| directive.key() == &key && directive.disposition() == Disposition::Live)
        .expect("the scanner retires only a live directive key");
    if directive.lane() != actor_lane {
        return refusal(
            "steer::lane-scope",
            Some(directive.lane().to_string()),
            Some(actor_lane.to_string()),
            "acknowledge only direction addressed to the authenticated lane",
        );
    }
    let witness = Witness::Directive {
        steer: directive.directive(),
        disposition: Disposition::Live,
    };
    if command.expected() != [witness.clone()] {
        return witness_refusal(command.expected(), &witness);
    }
    let (origin, cause) = directive_cause(&snapshot.facts, directive.directive());
    let mut proposal = match write_at_origin(command.actor().clone(), origin, Producer::AckScanner)
    {
        Ok(proposal) => proposal,
        Err(outcome) => return typed_outcome(outcome),
    };
    proposal.guard(live_guard(directive.directive()));
    if let Err(error) = append_disposition_fact(
        &mut proposal,
        directive,
        &key,
        epoch,
        locus,
        disposition,
        FactCause::Existing(cause),
    ) {
        return typed_store_fault("steer::fact-invalid", &error.to_string());
    }
    let answer = DirectiveAnswer {
        directive: directive.directive(),
        key: key.to_string(),
        disposition,
    };
    if let Err(error) = proposal.command_result(
        typed_command(command, dispose.clone()),
        epoch,
        answer.clone(),
    ) {
        return typed_store_fault("steer::receipt-invalid", &error.to_string());
    }
    finish(bus, paths, residency, directive.lane(), proposal, answer)
}

#[allow(clippy::too_many_arguments)]
fn append_disposition_fact(
    proposal: &mut JournalWrite,
    directive: &steer::Directive,
    key: &steer::Key,
    epoch: CanonicalUuid,
    locus: TranscriptLocus,
    disposition: Disposition,
    cause: FactCause,
) -> Result<(), journal::BuildError> {
    match disposition {
        Disposition::Acked => proposal.fact(
            wire::body::SteerAcked {
                directive: directive.directive(),
                key: key.to_string(),
                lane: directive.lane(),
                epoch,
                locus,
            },
            cause,
        ),
        Disposition::Declined => proposal.fact(
            wire::body::SteerDeclined {
                directive: directive.directive(),
                key: key.to_string(),
                lane: directive.lane(),
                epoch,
                locus,
                reason: "declined at the cited assistant transcript locus".to_owned(),
            },
            cause,
        ),
        _ => unreachable!("the transcript scanner emits only terminal lane dispositions"),
    }
    .map(|_| ())
}

#[allow(clippy::too_many_arguments)]
fn record_miss(
    bus: &Bus,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    residency: Seq,
    facts: &journal::FactSnapshot,
    token: String,
    miss: Miss,
    locus: TranscriptLocus,
    lane: CanonicalUuid,
) -> Outcome<CommandAnswer> {
    if !command.expected().is_empty() {
        return refusal(
            "steer::witness-moved",
            Some("no live directive witness".to_owned()),
            Some(format!("{:?}", command.expected())),
            "read the current steering snapshot before recording a missed recognition",
        );
    }
    let body = wire::body::SteerMissed {
        key: token.clone(),
        lane,
        epoch,
        locus,
        miss: acknowledgment_miss(miss),
    };
    if !miss_recorded(facts, &body) {
        let mut proposal = match write(command.actor().clone(), epoch, Producer::AckScanner) {
            Ok(proposal) => proposal,
            Err(outcome) => return typed_outcome(outcome),
        };
        proposal.guard(miss_guard(body.clone()));
        if let Err(error) = proposal.fact(body, FactCause::Existing(residency)) {
            return typed_store_fault("steer::fact-invalid", &error.to_string());
        }
        if let Err(outcome) = commit(bus, proposal) {
            return typed_outcome(outcome);
        }
    }
    refusal(
        "steer::ack-missed",
        Some("one live directive recognition".to_owned()),
        Some(format!("{token}:{}", miss.as_str())),
        "read the current inbox and acknowledge a live key in new assistant prose",
    )
}

fn acknowledgment_miss(miss: Miss) -> wire::body::AcknowledgmentMiss {
    match miss {
        Miss::Buried => wire::body::AcknowledgmentMiss::Buried,
        Miss::Quoted => wire::body::AcknowledgmentMiss::Quoted,
        Miss::Malformed => wire::body::AcknowledgmentMiss::Malformed,
        Miss::Unknown => wire::body::AcknowledgmentMiss::Unknown,
        Miss::Historical => wire::body::AcknowledgmentMiss::Historical,
    }
}

fn evidence_refusal(token: &str, observed: &str) -> Outcome<CommandAnswer> {
    refusal(
        "steer::transcript-evidence-absent",
        Some(format!("assistant prose recognizing {token}")),
        Some(observed.to_owned()),
        "emit an ACK or NACK header in assistant prose before invoking ack",
    )
}

fn miss_recorded(snapshot: &journal::FactSnapshot, expected: &wire::body::SteerMissed) -> bool {
    snapshot.facts().iter().any(|envelope| {
        if envelope.fact.kind() != wire::FactKind::SteerMissed {
            return false;
        }
        let Some(observed) = steering_body::<wire::body::SteerMissed>(envelope) else {
            return false;
        };
        observed.key == expected.key
            && observed.lane == expected.lane
            && observed.locus == expected.locus
            && observed.miss == expected.miss
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
        control: directive.control().map(str::to_owned),
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
    snapshot
        .facts()
        .iter()
        .find_map(|envelope| match envelope.fact.kind() {
            wire::FactKind::SteerSent => steering_body::<wire::body::SteerSent>(envelope)
                .filter(|body| body.directive == directive)
                .map(|_| (envelope.origin.clone(), envelope.seq)),
            wire::FactKind::SteerResent => steering_body::<wire::body::SteerResent>(envelope)
                .filter(|body| body.directive == directive)
                .map(|_| (envelope.origin.clone(), envelope.seq)),
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
    answer: DirectiveAnswer,
) -> Outcome<CommandAnswer> {
    let committed = match commit(bus, proposal) {
        Ok(committed) => committed,
        Err(outcome) => return typed_outcome(outcome),
    };
    // Rendering is disposable; a publication failure cannot rewrite a
    // command result that the journal and receipt already committed.
    let _rendering = publish_inbox(bus, paths, residency, lane);
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
