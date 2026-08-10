//! Steering command transitions and transcript-backed disposition.
//!
//! Every transition in this module addresses the original directive. Delivery
//! repair remains a projection concern and never mints a replacement identity.

use super::*;

pub(in crate::resident) fn send(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    config: &steer::Config,
    command: &wire::AdmittedCommand,
    send: &wire::intent::SteerSend,
) -> Outcome<CommandAnswer> {
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
    if snapshot.ledger.directive(command.id()).is_some() {
        return refusal(
            "steer::identity-taken",
            Some("unused directive identity".to_owned()),
            Some(command.id().to_string()),
            "submit a new command identity",
        );
    }
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let key = match mint_key(&snapshot.ledger, at) {
        Ok(key) => key,
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
        attachments: send.attachments.clone(),
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
        delivery: Box::new(DeliveryStanding::Unknown),
    };
    if let Err(error) =
        proposal.command_result(typed_command(command, send.clone()), epoch, answer.clone())
    {
        return typed_store_fault("steer::receipt-invalid", &error.to_string());
    }
    finish(bus, paths, residency.body(), send.lane, proposal, answer)
}

pub(in crate::resident) fn cancel(
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
        delivery: Box::new(DeliveryStanding::Unknown),
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

pub(in crate::resident) fn resend(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    config: &steer::Config,
    command: &wire::AdmittedCommand,
    resend: &wire::intent::DirectiveRef,
) -> Outcome<CommandAnswer> {
    if !matches!(command.actor(), Actor::Operator) {
        return refusal(
            "steer::operator-required",
            Some("operator".to_owned()),
            Some(command.actor().to_string()),
            "submit resend through the current operator discovery",
        );
    }
    let residency = match current_residency(bus, epoch) {
        Ok(residency) => residency,
        Err(outcome) => return typed_outcome(outcome),
    };
    let snapshot = match snapshot(bus) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let directive = match live_directive(&snapshot.ledger, resend.directive) {
        Ok(directive) => directive,
        Err(outcome) => return outcome,
    };
    let witness = Witness::Directive {
        steer: resend.directive,
        disposition: Disposition::Live,
    };
    if command.expected() != [witness.clone()] {
        return witness_refusal(command.expected(), &witness);
    }
    let Some(published) = directive.published() else {
        return typed_store_fault(
            "steer::publication-missing",
            "a journal-replayed directive has no original publication instant",
        );
    };
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let due = match due_stage(published, at, config) {
        Ok(stage) => stage,
        Err(outcome) => return outcome,
    };
    let stages = missing_stages(directive.stage(), due);
    let mut answer = DirectiveAnswer {
        directive: directive.directive(),
        key: directive.key().to_string(),
        disposition: Disposition::Live,
        delivery: Box::new(DeliveryStanding::Unknown),
    };
    if stages.is_empty() {
        let selected = match residency
            .body()
            .lanes
            .iter()
            .find(|row| row.lane == directive.lane())
        {
            Some(selected) => selected,
            None => {
                return typed_store_fault(
                    "steer::lane-missing-after-commit",
                    &format!(
                        "lane {} disappeared while publishing its inbox",
                        directive.lane()
                    ),
                );
            }
        };
        answer.delivery = match fold_lane_inbox(bus, paths, selected) {
            Ok(delivery) => Box::new(delivery),
            Err(outcome) => return typed_outcome(outcome),
        };
        return Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(snapshot.position),
            body: CommandAnswer::Directive(answer),
        });
    }
    let (origin, cause) = directive_cause(&snapshot.facts, resend.directive);
    let mut proposal = JournalWrite::new(command.actor().clone(), origin, at, Producer::BusTimer);
    proposal.guard(stage_guard(resend.directive, directive.stage()));
    for stage in stages {
        if let Err(error) = proposal.fact(
            wire::body::SteerEscalated {
                directive: directive.directive(),
                key: directive.key().to_string(),
                lane: directive.lane(),
                stage,
            },
            FactCause::Existing(cause),
        ) {
            return typed_store_fault("steer::fact-invalid", &error.to_string());
        }
    }
    if let Err(error) = proposal.command_result(
        typed_command(command, resend.clone()),
        epoch,
        answer.clone(),
    ) {
        return typed_store_fault("steer::receipt-invalid", &error.to_string());
    }
    finish(
        bus,
        paths,
        residency.body(),
        directive.lane(),
        proposal,
        answer,
    )
}

pub(in crate::resident) fn recover(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    config: &steer::Config,
    command: &wire::AdmittedCommand,
    recover: &wire::intent::DirectiveRef,
) -> Outcome<CommandAnswer> {
    if !matches!(command.actor(), Actor::Operator) {
        return refusal(
            "steer::operator-required",
            Some("operator".to_owned()),
            Some(command.actor().to_string()),
            "submit recovery through the current operator discovery",
        );
    }
    let residency = match current_residency(bus, epoch) {
        Ok(residency) => residency,
        Err(outcome) => return typed_outcome(outcome),
    };
    let snapshot = match snapshot(bus) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(directive) = snapshot.ledger.directive(recover.directive) else {
        return refusal(
            "steer::recovery-missing",
            Some("one durable lapsed directive".to_owned()),
            Some(recover.directive.to_string()),
            "inspect durable steering history and name its exact directive identity",
        );
    };
    if directive.disposition() != Disposition::Lapsed {
        let reason = if directive.disposition() == Disposition::Live {
            "steer::recovery-live"
        } else {
            "steer::recovery-retired"
        };
        return refusal(
            reason,
            Some("lapsed".to_owned()),
            Some(format!("{:?}", directive.disposition()).to_lowercase()),
            "recover only a durable directive that has lapsed",
        );
    }
    let expected = [Witness::Directive {
        steer: recover.directive,
        disposition: Disposition::Lapsed,
    }];
    if command.expected() != expected {
        return refusal(
            "steer::witness-moved",
            Some(format!("{expected:?}")),
            Some(format!("{:?}", command.expected())),
            "read the failed directive again before recovering it",
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
    let (origin, cause) = directive_cause(&snapshot.facts, recover.directive);
    let key = directive.key().clone();
    let lane = directive.lane();
    let lapsed_at = directive.expires();
    let mut proposal = match write_at_origin(command.actor().clone(), origin, Producer::CommandDoor)
    {
        Ok(proposal) => proposal,
        Err(outcome) => return typed_outcome(outcome),
    };
    proposal.guard(recovery_guard(
        recover.directive,
        key.clone(),
        lane,
        lapsed_at,
    ));
    if let Err(error) = proposal.fact(
        wire::body::SteerRecovered {
            directive: recover.directive,
            key: key.to_string(),
            lane,
            lapsed_at,
            expires,
        },
        FactCause::Existing(cause),
    ) {
        return typed_store_fault("steer::recovery-fact-invalid", &error.to_string());
    }
    let answer = DirectiveAnswer {
        directive: recover.directive,
        key: key.to_string(),
        disposition: Disposition::Live,
        delivery: Box::new(DeliveryStanding::Unknown),
    };
    if let Err(error) = proposal.command_result(
        typed_command(command, recover.clone()),
        epoch,
        answer.clone(),
    ) {
        return typed_store_fault("steer::recovery-receipt-invalid", &error.to_string());
    }
    finish(bus, paths, residency.body(), lane, proposal, answer)
}

pub(in crate::resident) fn dispose(
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
        delivery: Box::new(DeliveryStanding::Unknown),
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

pub(super) fn evidence_refusal(token: &str, observed: &str) -> Outcome<CommandAnswer> {
    refusal(
        "steer::transcript-evidence-absent",
        Some(format!("assistant prose recognizing {token}")),
        Some(observed.to_owned()),
        "emit an ACK or NACK header in assistant prose before invoking ack",
    )
}

pub(super) fn miss_recorded(
    snapshot: &journal::FactSnapshot,
    expected: &wire::body::SteerMissed,
) -> bool {
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
