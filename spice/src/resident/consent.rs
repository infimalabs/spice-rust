fn dispatch_consent_decide(
    bus: &Bus,
    epoch: CanonicalUuid,
    policy: &consent::Policy,
    command: &wire::AdmittedCommand,
    decide: &ConsentDecide,
) -> Outcome<CommandAnswer> {
    let admission = match admit_consent_decision(bus, policy, command, decide) {
        Ok(admission) => admission,
        Err(outcome) => return outcome,
    };
    let (proposal, result) = match build_consent_decision(
        policy, command, decide, epoch, admission,
    ) {
        Ok(built) => built,
        Err(outcome) => return outcome,
    };
    match commit(bus, proposal) {
        Ok(committed) => Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(committed.first()),
                last: seq(committed.last()),
            }),
            body: CommandAnswer::Consent(result),
        }),
        Err(outcome) => typed_outcome(outcome),
    }
}

struct ConsentAdmission {
    pending: consent::PendingRequest,
    terminal: ConsentTerminal,
    witnesses: Vec<Witness>,
}

fn admit_consent_decision(
    bus: &Bus,
    policy: &consent::Policy,
    command: &wire::AdmittedCommand,
    decide: &ConsentDecide,
) -> Result<ConsentAdmission, Outcome<CommandAnswer>> {
    require_consent_decision(command, decide)?;
    let (_, queue) = consent_snapshot(bus, policy)?;
    let answer = queue
        .answer(policy)
        .map_err(|error| typed_protocol_fault("consent::queue-invalid", &error.to_string()))?;
    let pending_answer = answer
        .pending
        .iter()
        .find(|pending| pending.request == decide.request)
        .ok_or_else(|| consent_not_held(decide.request))?;
    let witnesses = consent::decision_witnesses(pending_answer, decide.answer)
        .map_err(Outcome::Refusal)?;
    if !same_witnesses(command.expected(), &witnesses) {
        return Err(Outcome::Refusal(Refusal {
            reason: "consent::witness-moved".to_owned(),
            witness: Some(decide.request.to_string()),
            expected: Some(format!("{witnesses:?}")),
            observed: Some(format!("{:?}", command.expected())),
            remedy: "re-read the consent queue and retry its exact current request".to_owned(),
        }));
    }
    let pending = queue
        .pending(decide.request)
        .expect("the wire queue answer and internal queue share pending identities")
        .clone();
    let terminal = consent_terminal(decide, &pending);
    Ok(ConsentAdmission {
        pending,
        terminal,
        witnesses,
    })
}

fn require_consent_decision(
    command: &wire::AdmittedCommand,
    decide: &ConsentDecide,
) -> Result<(), Outcome<CommandAnswer>> {
    if !matches!(command.actor(), Actor::Operator) {
        return Err(Outcome::Refusal(Refusal {
            reason: "command::authority".to_owned(),
            witness: Some("principal".to_owned()),
            expected: Some("operator".to_owned()),
            observed: Some(command.actor().to_string()),
            remedy: "reconnect with the current operator capability".to_owned(),
        }));
    }
    if !matches!(decide.answer, wire::Answer::Granted | wire::Answer::Denied) {
        return Err(Outcome::Refusal(Refusal {
            reason: "consent::ruling-invalid".to_owned(),
            witness: Some(decide.request.to_string()),
            expected: Some("granted or denied".to_owned()),
            observed: Some(format!("{:?}", decide.answer).to_lowercase()),
            remedy: "choose allow or deny for the queued request".to_owned(),
        }));
    }
    Ok(())
}

fn consent_terminal(
    decide: &ConsentDecide,
    pending: &consent::PendingRequest,
) -> ConsentTerminal {
    match decide.answer {
        wire::Answer::Granted => ConsentTerminal::Granted(wire::body::ConsentGranted {
            request: decide.request,
            lane: pending.lane(),
            rule: consent::OPERATOR.to_owned(),
            budget: None,
        }),
        wire::Answer::Denied => ConsentTerminal::Denied(wire::body::ConsentDenied {
            request: decide.request,
            lane: pending.lane(),
            rule: consent::OPERATOR.to_owned(),
        }),
        _ => unreachable!("non-terminal answers are refused before construction"),
    }
}

fn consent_decision_guard(
    policy: &consent::Policy,
    decide: &ConsentDecide,
    witnesses: &[Witness],
) -> JournalGuard {
    let guarded_policy = policy.clone();
    let guarded_expected = witnesses.to_vec();
    let guarded_request = decide.request;
    let guarded_answer = decide.answer;
    JournalGuard::new(move |read| {
        let snapshot = read
            .fact_snapshot()
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        let queue = consent::Queue::from_snapshot(&snapshot, &guarded_policy)
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        let answer = queue
            .answer(&guarded_policy)
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        let current = answer
            .pending
            .iter()
            .find(|pending| pending.request == guarded_request)
            .ok_or_else(|| GuardError::refused("consent::request-moved"))?;
        let current = consent::decision_witnesses(current, guarded_answer)
            .map_err(|_| GuardError::refused("consent::request-moved"))?;
        if !same_witnesses(&current, &guarded_expected) {
            return Err(GuardError::refused("consent::witness-moved"));
        }
        Ok(())
    })
}

fn build_consent_decision(
    policy: &consent::Policy,
    command: &wire::AdmittedCommand,
    decide: &ConsentDecide,
    epoch: CanonicalUuid,
    admission: ConsentAdmission,
) -> Result<(JournalWrite, ConsentAnswer), Outcome<CommandAnswer>> {
    let mut proposal = write_at_origin(
        command.actor().clone(),
        admission.pending.origin().clone(),
        Producer::CommandDoor,
    )
    .map_err(typed_outcome)?;
    proposal.guard(consent_decision_guard(
        policy,
        decide,
        &admission.witnesses,
    ));
    let fact = match admission.terminal {
        ConsentTerminal::Granted(body) => {
            proposal.fact(body, FactCause::Existing(admission.pending.fact()))
        }
        ConsentTerminal::Denied(body) => {
            proposal.fact(body, FactCause::Existing(admission.pending.fact()))
        }
    };
    fact.map_err(|error| typed_store_fault("consent::fact-invalid", &error.to_string()))?;
    let answer = ConsentAnswer {
        request: decide.request,
        answer: decide.answer,
    };
    attach_consent_receipt(&mut proposal, command, decide, epoch, &answer)?;
    Ok((proposal, answer))
}

fn attach_consent_receipt(
    proposal: &mut JournalWrite,
    command: &wire::AdmittedCommand,
    decide: &ConsentDecide,
    epoch: CanonicalUuid,
    answer: &ConsentAnswer,
) -> Result<(), Outcome<CommandAnswer>> {
    proposal
        .command_result(
            Command {
                v: wire::WireVersion::CURRENT,
                id: command.id(),
                epoch: command.epoch(),
                kind: CommandKind::ConsentDecide,
                expected: command.expected().to_vec(),
                body: decide.clone(),
            },
            epoch,
            answer.clone(),
        )
        .map_err(|error| typed_store_fault("consent::receipt-invalid", &error.to_string()))
}
