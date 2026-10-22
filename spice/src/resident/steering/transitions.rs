//! Steering command transitions and transcript-backed disposition.

use super::*;

/// One ordinary operator directive staged as part of a larger guarded command.
pub(crate) struct OperatorDirective<'a> {
    pub(crate) directive: CanonicalUuid,
    pub(crate) fanout: Option<CanonicalUuid>,
    pub(crate) lane: CanonicalUuid,
    pub(crate) text: &'a str,
    /// Machine control marker, when the direction carries one.
    pub(crate) control: Option<&'a str>,
    /// The room this direction speaks in, when it has one.
    pub(crate) room: Option<wire::rooms::RoomAnchor>,
}

/// Append ordinary steering authority without committing it separately.
///
/// Compound native commands use this to keep their room marker and steering
/// request in one journal write. The caller owns the residency guard and final
/// envelope-size admission.
pub(crate) fn append_operator_directive(
    bus: &Bus,
    origin: wire::Origin,
    config: &steer::Config,
    proposal: &mut JournalWrite,
    publication: OperatorDirective<'_>,
    at: Instant,
    cause: FactCause,
) -> Result<(String, PendingFact), Outcome<CommandAnswer>> {
    let (existing, minted_micros) = keyspace(bus, publication.directive)?;
    if existing.is_some() {
        return Err(refusal(
            "steer::identity-taken",
            Some("unused directive identity".to_owned()),
            Some(publication.directive.to_string()),
            "submit a new command identity",
        ));
    }
    let key = mint_key(minted_micros)?;
    let expires = expiry(at, config.expiry().get())?;
    let sent = wire::body::SteerSent {
        directive: publication.directive,
        fanout: publication.fanout,
        key: key.to_string(),
        lane: publication.lane,
        text: publication.text.to_owned(),
        from: None,
        room: publication.room,
        control: publication.control.map(str::to_owned),
        attachments: Vec::new(),
        expires,
    };
    let pending = proposal
        .fact(sent.clone(), cause)
        .map_err(|error| typed_store_fault("steer::fact-invalid", &error.to_string()))?;
    proposal.steering_head(pending, move |seq| {
        steer::Ledger::sent_head(&sent, origin, at, seq)
            .expect("the admitted compound send body builds its authority head")
    });
    Ok((key.to_string(), pending))
}

pub(in crate::resident) fn send(
    bus: &Bus,
    epoch: CanonicalUuid,
    config: &steer::Config,
    command: &wire::AdmittedCommand,
    send: &wire::intent::SteerSend,
    effects: &mut CommandEffects,
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
    let occupant = lane.occupant;
    publish(
        bus,
        epoch,
        config,
        command,
        &residency,
        Publication {
            lane: send.lane,
            occupant,
            fanout: Some(send.fanout),
            text: &send.message,
            attachments: &send.attachments,
        },
        typed_command(command, send.clone()),
        effects,
    )
}

/// What one new directive says and whom it addresses.
struct Publication<'a> {
    lane: CanonicalUuid,
    occupant: Option<CanonicalUuid>,
    fanout: Option<CanonicalUuid>,
    text: &'a str,
    attachments: &'a [wire::body::DirectiveAttachment],
}

/// Journal one new directive against the residency it witnessed.
#[expect(
    clippy::too_many_arguments,
    reason = "the publication door takes each authority it guards separately"
)]
fn publish<B: serde::Serialize>(
    bus: &Bus,
    epoch: CanonicalUuid,
    config: &steer::Config,
    command: &wire::AdmittedCommand,
    residency: &journal::JournalHead<wire::ResidencyHead>,
    publication: Publication<'_>,
    receipt: wire::Command<B>,
    effects: &mut CommandEffects,
) -> Outcome<CommandAnswer> {
    let witness = Witness::Residency {
        lane: publication.lane,
        occupant: publication.occupant,
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
        fanout: publication.fanout,
        key: key.to_string(),
        lane: publication.lane,
        text: publication.text.to_owned(),
        from: None,
        room: None,
        control: None,
        attachments: publication.attachments.to_vec(),
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
    if let Err(outcome) = admit_send_envelope(&proposal) {
        return outcome;
    }
    proposal.steering_head(pending, move |seq| {
        steer::Ledger::sent_head(&sent, origin, at, seq)
            .expect("the admitted send body builds its authority head")
    });
    let answer = DirectiveAnswer {
        directive: command.id(),
        key: key.to_string(),
        authored: at,
        disposition: Disposition::Live,
    };
    if let Err(error) = proposal.command_result(receipt, epoch, answer.clone()) {
        return typed_store_fault("steer::receipt-invalid", &error.to_string());
    }
    finish(bus, publication.lane, proposal, answer, effects)
}

fn admit_send_content(send: &wire::intent::SteerSend) -> Result<(), Outcome<CommandAnswer>> {
    if let Err(error) = wire::media::admit_media(&send.attachments) {
        return Err(refusal(
            "steer::media-invalid",
            Some(format!(
                "at most {} admitted images with at most {} combined data-URL bytes",
                wire::media::MAX_MEDIA_ITEMS,
                wire::media::MAX_MEDIA_DATA_URL_BYTES,
            )),
            Some(error.to_string()),
            "remove or replace the invalid attachment and retry the same authored message",
        ));
    }
    if send
        .attachments
        .iter()
        .any(|media| send.message.contains(&media.data_url))
    {
        return Err(refusal(
            "steer::media-duplicated",
            Some("attachment bytes retained only in attachments".to_owned()),
            None,
            "use bounded attachment Markdown references instead of inline data URLs",
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

pub(crate) fn admit_send_envelope(proposal: &JournalWrite) -> Result<(), Outcome<CommandAnswer>> {
    let bytes = proposal
        .envelope_byte_bound()
        .map_err(|error| typed_store_fault("steer::fact-invalid", &error.to_string()))?;
    let maximum = journal::MAX_DURABLE_FACT_ENVELOPE_BYTES;
    if bytes > maximum {
        return Err(refusal(
            "steer::envelope-too-large",
            Some(format!("at most {maximum} durable envelope bytes")),
            Some(format!(
                "{bytes} bytes including reserved journal cursor width"
            )),
            "shorten the direction or reduce its attachments and retry the same authored message",
        ));
    }
    Ok(())
}

pub(in crate::resident) fn cancel(
    bus: &Bus,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    cancel: &wire::intent::DirectiveRef,
    effects: &mut CommandEffects,
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
    };
    if let Err(error) = proposal.command_result(
        typed_command(command, cancel.clone()),
        epoch,
        answer.clone(),
    ) {
        return typed_store_fault("steer::receipt-invalid", &error.to_string());
    }
    finish(bus, directive.body().lane, proposal, answer, effects)
}

pub(in crate::resident) fn resend(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    config: &steer::Config,
    command: &wire::AdmittedCommand,
    resend: &wire::intent::DirectiveRef,
    effects: &mut CommandEffects,
) -> Outcome<CommandAnswer> {
    if !command.holds(wire::principal::PrincipalRole::Operator) {
        return refusal(
            "steer::operator-required",
            Some("operator".to_owned()),
            Some(command.actor().to_string()),
            "submit resend through the current operator discovery",
        );
    }
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
    };
    if stages.is_empty() {
        return resend_snapshot(paths, &directive, position, answer);
    }
    commit_resend(
        bus, epoch, command, resend, &directive, at, stages, answer, effects,
    )
}

fn resend_snapshot(
    paths: &RuntimePaths,
    directive: &journal::JournalHead<wire::SteeringHead>,
    position: Seq,
    answer: DirectiveAnswer,
) -> Outcome<CommandAnswer> {
    notify_inbox(paths, directive.body().lane);
    Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(position),
        body: CommandAnswer::Directive(answer),
    })
}

#[allow(clippy::too_many_arguments)]
fn commit_resend(
    bus: &Bus,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    resend: &wire::intent::DirectiveRef,
    directive: &journal::JournalHead<wire::SteeringHead>,
    at: Instant,
    stages: Vec<wire::body::SteeringStage>,
    answer: DirectiveAnswer,
    effects: &mut CommandEffects,
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
    finish(bus, directive.body().lane, proposal, answer, effects)
}

pub(in crate::resident) fn recover(
    bus: &Bus,
    epoch: CanonicalUuid,
    config: &steer::Config,
    command: &wire::AdmittedCommand,
    recover: &wire::intent::DirectiveRef,
    effects: &mut CommandEffects,
) -> Outcome<CommandAnswer> {
    if !command.holds(wire::principal::PrincipalRole::Operator) {
        return refusal(
            "steer::operator-required",
            Some("operator".to_owned()),
            Some(command.actor().to_string()),
            "submit recovery through the current operator discovery",
        );
    }
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
    if let Some(refused) = removed_destination(bus, directive.body()) {
        return refused;
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
    };
    if let Err(error) = proposal.command_result(
        typed_command(command, recover.clone()),
        epoch,
        answer.clone(),
    ) {
        return typed_store_fault("steer::recovery-receipt-invalid", &error.to_string());
    }
    finish(bus, lane, proposal, answer, effects)
}

/// Refuse recovering a peer directive whose lane left the residency: a peer
/// duty is bound to its logical lane, and a lane that replaces it is another
/// destination, so recovery would hand it to nobody.
fn removed_destination(
    bus: &Bus,
    directive: &wire::SteeringHead,
) -> Option<Outcome<CommandAnswer>> {
    directive.from?;
    let view = match bus.task_snapshot() {
        Ok(view) => view,
        Err(error) => {
            return Some(typed_store_fault(
                "steer::residency-unreadable",
                &error.to_string(),
            ));
        }
    };
    let resident = view.residency().is_some_and(|residency| {
        residency
            .body()
            .lanes
            .iter()
            .any(|row| row.lane == directive.lane)
    });
    (!resident).then(|| {
        refusal(
            super::LANE_REMOVED,
            Some("a lane in the residency".to_owned()),
            Some(directive.lane.to_string()),
            "a peer duty belongs to its lane alone; author a new MSG to the lane that replaced it",
        )
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn append_disposition(
    proposal: &mut JournalWrite,
    directive: &journal::JournalHead<wire::SteeringHead>,
    key: &steer::Key,
    disposition: Disposition,
    locus: TranscriptLocus,
    epoch: CanonicalUuid,
    at: Instant,
) -> Result<(PendingFact, wire::SteeringHead), Outcome<CommandAnswer>> {
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
                Err(error) => {
                    return Err(typed_store_fault("steer::fact-invalid", &error.to_string()));
                }
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
                Err(error) => {
                    return Err(typed_store_fault("steer::fact-invalid", &error.to_string()));
                }
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
    next.map(|next| (pending, next))
        .map_err(|error| typed_store_fault("steer::index-invalid", &error.to_string()))
}

pub(super) fn evidence_refusal(
    token: &str,
    observed: &str,
    repair: &str,
) -> Outcome<CommandAnswer> {
    refusal(
        "steer::transcript-evidence-absent",
        Some(format!("assistant prose recognizing {token}")),
        Some(observed.to_owned()),
        repair,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_refuses_duplicate_attachment_payloads() {
        let media = wire::Media::from_base64("capture.png", "image/png", "iVBORw0KGgo=")
            .expect("fixture image admits");
        let send = wire::intent::SteerSend {
            fanout: "5f72a0b1-6273-4495-9f46-0718293a4b5c".parse().unwrap(),
            lane: "5f72a0b1-6273-4495-9f46-0718293a4b5d".parse().unwrap(),
            message: media.markdown().unwrap(),
            attachments: vec![media],
        };
        assert!(matches!(
            admit_send_content(&send),
            Err(Outcome::Refusal(refusal)) if refusal.reason() == "steer::media-duplicated"
        ));
    }
}
