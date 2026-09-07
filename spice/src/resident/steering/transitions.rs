//! Steering command transitions and transcript-backed disposition.
//!
//! Every transition in this module addresses the original directive. Delivery
//! repair remains a fold concern and never mints a replacement identity.

use super::*;

pub(in crate::resident) fn send(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    config: &steer::Config,
    command: &wire::AdmittedCommand,
    send: &wire::intent::SteerSend,
) -> Outcome<CommandAnswer> {
    if let Err(outcome) = admit_send_content(send) {
        return outcome;
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
    let (existing, minted_micros) = match keyspace(bus, command.id()) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    if existing.is_some() {
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
    let key = match mint_key(minted_micros) {
        Ok(key) => key,
        Err(outcome) => return outcome,
    };
    let expires = match expiry(at, config.expiry().get()) {
        Ok(expires) => expires,
        Err(outcome) => return outcome,
    };
    let sent = wire::body::SteerSent {
        directive: command.id(),
        fanout: Some(send.fanout),
        key: key.to_string(),
        lane: send.lane,
        text: send.message.clone(),
        control: None,
        attachments: send.attachments.clone(),
        expires,
    };
    let origin = match resident_origin(epoch) {
        Ok(origin) => origin,
        Err(outcome) => return typed_outcome(outcome),
    };
    let mut proposal = JournalWrite::new(
        command.actor().clone(),
        origin.clone(),
        at,
        Producer::CommandDoor,
    );
    proposal.guard(send_guard(
        residency.position(),
        witness,
        command.id(),
        key.clone(),
    ));
    let pending = match proposal.fact(sent.clone(), FactCause::Existing(residency.position())) {
        Ok(pending) => pending,
        Err(error) => return typed_store_fault("steer::fact-invalid", &error.to_string()),
    };
    proposal.steering_head(pending, move |seq| {
        steer::Ledger::sent_head(&sent, origin, at, seq)
            .expect("the admitted send body builds its authority head")
    });
    let answer = DirectiveAnswer {
        directive: command.id(),
        key: key.to_string(),
        authored: at,
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

fn admit_send_content(send: &wire::intent::SteerSend) -> Result<(), Outcome<CommandAnswer>> {
    if let Err(error) = wire::media::admit_media(&send.attachments) {
        return Err(refusal(
            "steer::media-invalid",
            Some(format!(
                "at most {} admitted images of at most {} decoded bytes each",
                wire::media::MAX_MEDIA_ITEMS,
                wire::media::MAX_MEDIA_BYTES,
            )),
            Some(error.to_string()),
            "remove or replace the invalid attachment and retry the same authored message",
        ));
    }
    if send.message.trim().is_empty() {
        return Err(refusal(
            "steer::message-empty",
            Some("non-empty direction".to_owned()),
            Some(send.message.clone()),
            "supply direction text that the lane can act on",
        ));
    }
    Ok(())
}

pub(in crate::resident) fn cancel(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    cancel: &wire::intent::DirectiveRef,
) -> Outcome<CommandAnswer> {
    let (_, indexed) = match directive_head(bus, cancel.directive) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let directive = match live_head(indexed, cancel.directive) {
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
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let (origin, cause) = (directive.body().origin.clone(), directive.body().created);
    let mut proposal =
        JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    proposal.guard(live_guard(cancel.directive, directive.position()));
    let canceled = wire::body::SteerCanceled {
        key: directive.body().key.clone(),
        lane: directive.body().lane,
    };
    let pending = match proposal.fact(canceled.clone(), FactCause::Existing(cause)) {
        Ok(pending) => pending,
        Err(error) => return typed_store_fault("steer::fact-invalid", &error.to_string()),
    };
    let next = match steer::Ledger::transition_head(
        directive.body(),
        steer::Steering::Canceled(&canceled),
        at,
    ) {
        Ok(next) => next,
        Err(error) => return typed_store_fault("steer::index-invalid", &error.to_string()),
    };
    proposal.steering_head(pending, move |_| next);
    let answer = DirectiveAnswer {
        directive: cancel.directive,
        key: directive.body().key.clone(),
        authored: directive.body().published,
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
        directive.body().lane,
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
    let (position, indexed) = match directive_head(bus, resend.directive) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let directive = match live_head(indexed, resend.directive) {
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
    let published = directive.body().published;
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let due = match due_stage(published, at, config) {
        Ok(stage) => stage,
        Err(outcome) => return outcome,
    };
    let stages = missing_stages(directive.body().stage, due);
    let answer = DirectiveAnswer {
        directive: directive.body().directive,
        key: directive.body().key.clone(),
        authored: published,
        disposition: Disposition::Live,
        delivery: Box::new(DeliveryStanding::Unknown),
    };
    if stages.is_empty() {
        return resend_snapshot(bus, paths, residency.body(), &directive, position, answer);
    }
    commit_resend(
        bus,
        paths,
        epoch,
        command,
        resend,
        residency.body(),
        &directive,
        at,
        stages,
        answer,
    )
}

fn resend_snapshot(
    bus: &Bus,
    paths: &RuntimePaths,
    residency: &ResidencyHead,
    directive: &journal::JournalHead<wire::SteeringHead>,
    position: Seq,
    mut answer: DirectiveAnswer,
) -> Outcome<CommandAnswer> {
    let selected = match residency
        .lanes
        .iter()
        .find(|row| row.lane == directive.body().lane)
    {
        Some(selected) => selected,
        None => {
            return typed_store_fault(
                "steer::lane-missing-after-commit",
                &format!(
                    "lane {} disappeared while publishing its inbox",
                    directive.body().lane
                ),
            );
        }
    };
    answer.delivery = match fold_lane_inbox(bus, paths, selected) {
        Ok(delivery) => Box::new(delivery),
        Err(outcome) => return typed_outcome(outcome),
    };
    Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(position),
        body: CommandAnswer::Directive(answer),
    })
}

#[allow(clippy::too_many_arguments)]
fn commit_resend(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    resend: &wire::intent::DirectiveRef,
    residency: &ResidencyHead,
    directive: &journal::JournalHead<wire::SteeringHead>,
    at: Instant,
    stages: Vec<wire::body::SteeringStage>,
    answer: DirectiveAnswer,
) -> Outcome<CommandAnswer> {
    let origin = directive.body().origin.clone();
    let cause = directive.body().created;
    let mut proposal = JournalWrite::new(command.actor().clone(), origin, at, Producer::BusTimer);
    proposal.guard(stage_guard(
        resend.directive,
        directive.position(),
        directive.body().stage,
    ));
    let mut next = directive.body().clone();
    let mut last = None;
    for stage in stages {
        let escalated = wire::body::SteerEscalated {
            directive: directive.body().directive,
            key: directive.body().key.clone(),
            lane: directive.body().lane,
            stage,
        };
        let pending = match proposal.fact(escalated.clone(), FactCause::Existing(cause)) {
            Ok(pending) => pending,
            Err(error) => return typed_store_fault("steer::fact-invalid", &error.to_string()),
        };
        next =
            match steer::Ledger::transition_head(&next, steer::Steering::Escalated(&escalated), at)
            {
                Ok(next) => next,
                Err(error) => return typed_store_fault("steer::index-invalid", &error.to_string()),
            };
        last = Some(pending);
    }
    proposal.steering_head(last.expect("nonempty stages append a fact"), move |_| next);
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
        residency,
        directive.body().lane,
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
    let (_, indexed) = match directive_head(bus, recover.directive) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(directive) = indexed else {
        return refusal(
            "steer::recovery-missing",
            Some("one durable lapsed or deadlettered directive".to_owned()),
            Some(recover.directive.to_string()),
            "inspect durable steering history and name its exact directive identity",
        );
    };
    if !matches!(
        directive.body().disposition,
        Disposition::Lapsed | Disposition::DeadLettered
    ) {
        let reason = if directive.body().disposition == Disposition::Live {
            "steer::recovery-live"
        } else {
            "steer::recovery-retired"
        };
        return refusal(
            reason,
            Some("lapsed or deadlettered".to_owned()),
            Some(format!("{:?}", directive.body().disposition).to_lowercase()),
            "recover only a durable directive that lapsed or was deadlettered",
        );
    }
    // The caller binds identity; the journal guard below binds the failed disposition.
    let witness = Witness::Recoverable {
        steer: recover.directive,
    };
    if command.expected() != [witness.clone()] {
        return witness_refusal(command.expected(), &witness);
    }
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let expires = match expiry(at, config.expiry().get()) {
        Ok(expires) => expires,
        Err(outcome) => return outcome,
    };
    let origin = directive.body().origin.clone();
    let cause = directive.body().created;
    let key: steer::Key = directive
        .body()
        .key
        .parse()
        .expect("indexed steering key is admitted");
    let lane = directive.body().lane;
    let lapsed_at = directive.body().expires;
    let mut proposal =
        JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    proposal.guard(recovery_guard(
        recover.directive,
        directive.position(),
        key.clone(),
        lane,
        lapsed_at,
        directive.body().disposition,
    ));
    let recovered = wire::body::SteerRecovered {
        directive: recover.directive,
        key: key.to_string(),
        lane,
        lapsed_at,
        expires,
    };
    let pending = match proposal.fact(recovered.clone(), FactCause::Existing(cause)) {
        Ok(pending) => pending,
        Err(error) => {
            return typed_store_fault("steer::recovery-fact-invalid", &error.to_string());
        }
    };
    let next = match steer::Ledger::transition_head(
        directive.body(),
        steer::Steering::Recovered(&recovered),
        at,
    ) {
        Ok(next) => next,
        Err(error) => return typed_store_fault("steer::index-invalid", &error.to_string()),
    };
    proposal.steering_head(pending, move |_| next);
    let answer = DirectiveAnswer {
        directive: recover.directive,
        key: key.to_string(),
        authored: directive.body().published,
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
    let parsed = dispose.token.parse::<steer::Key>().ok();
    let indexed = match parsed.as_ref() {
        Some(key) => match key_head(bus, key) {
            Ok(indexed) => indexed,
            Err(outcome) => return outcome,
        },
        None => None,
    };
    let standing = IndexedStanding::new(parsed, indexed.as_ref());
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
        &standing,
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
            indexed.expect("the scanner retires only an indexed live key"),
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
    directive: journal::JournalHead<wire::SteeringHead>,
    actor_lane: CanonicalUuid,
    key: steer::Key,
    disposition: Disposition,
    locus: TranscriptLocus,
) -> Outcome<CommandAnswer> {
    if directive.body().lane != actor_lane {
        return refusal(
            "steer::lane-scope",
            Some(directive.body().lane.to_string()),
            Some(actor_lane.to_string()),
            "acknowledge only direction addressed to the authenticated lane",
        );
    }
    let witness = Witness::Directive {
        steer: directive.body().directive,
        disposition: Disposition::Live,
    };
    if command.expected() != [witness.clone()] {
        return witness_refusal(command.expected(), &witness);
    }
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let mut proposal = JournalWrite::new(
        command.actor().clone(),
        directive.body().origin.clone(),
        at,
        Producer::AckScanner,
    );
    proposal.guard(live_guard(directive.body().directive, directive.position()));
    let cause = FactCause::Existing(directive.body().created);
    let (pending, next) = match disposition {
        Disposition::Acked => {
            let body = wire::body::SteerAcked {
                directive: directive.body().directive,
                key: key.to_string(),
                lane: directive.body().lane,
                epoch,
                locus,
            };
            let pending = match proposal.fact(body.clone(), cause) {
                Ok(pending) => pending,
                Err(error) => return typed_store_fault("steer::fact-invalid", &error.to_string()),
            };
            let next =
                steer::Ledger::transition_head(directive.body(), steer::Steering::Acked(&body), at);
            (pending, next)
        }
        Disposition::Declined => {
            let body = wire::body::SteerDeclined {
                directive: directive.body().directive,
                key: key.to_string(),
                lane: directive.body().lane,
                epoch,
                locus,
                reason: "declined at the cited assistant transcript locus".to_owned(),
            };
            let pending = match proposal.fact(body.clone(), cause) {
                Ok(pending) => pending,
                Err(error) => return typed_store_fault("steer::fact-invalid", &error.to_string()),
            };
            let next = steer::Ledger::transition_head(
                directive.body(),
                steer::Steering::Declined(&body),
                at,
            );
            (pending, next)
        }
        _ => unreachable!("the transcript scanner emits only terminal lane dispositions"),
    };
    let next = match next {
        Ok(next) => next,
        Err(error) => return typed_store_fault("steer::index-invalid", &error.to_string()),
    };
    proposal.steering_head(pending, move |_| next);
    let answer = DirectiveAnswer {
        directive: directive.body().directive,
        key: key.to_string(),
        authored: directive.body().published,
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
    finish(
        bus,
        paths,
        residency,
        directive.body().lane,
        proposal,
        answer,
    )
}

#[allow(clippy::too_many_arguments)]
fn record_miss(
    bus: &Bus,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    residency: Seq,
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
    let recorded = match bus.read({
        let body = body.clone();
        move |read| read.steering_miss_recorded(&body)
    }) {
        Ok(recorded) => recorded,
        Err(error) => return typed_store_fault("steer::index-unreadable", &error.to_string()),
    };
    if !recorded {
        let mut proposal = match write(command.actor().clone(), epoch, Producer::AckScanner) {
            Ok(proposal) => proposal,
            Err(outcome) => return typed_outcome(outcome),
        };
        proposal.guard(miss_guard(body.clone()));
        let pending = match proposal.fact(body.clone(), FactCause::Existing(residency)) {
            Ok(pending) => pending,
            Err(error) => return typed_store_fault("steer::fact-invalid", &error.to_string()),
        };
        proposal.steering_miss(pending, body);
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
