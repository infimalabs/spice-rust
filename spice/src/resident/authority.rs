fn dispatch_command(
    bus: &Bus,
    repository: &Repository,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    config: &SurfaceConfig,
    seats: &mut Seats,
    command: &wire::AdmittedCommand,
) -> Outcome<CommandAnswer> {
    if let Some(outcome) = crate::board_command::dispatch(bus, command) {
        return outcome;
    }
    match (command.kind(), command.body()) {
        (CommandKind::UpChange, Intent::UpChange(change)) => {
            dispatch_up_change(bus, paths, epoch, seats, command, change)
        }
        (CommandKind::ReleasePublish, Intent::ReleasePublish(publish)) => {
            dispatch_release_publish(bus, paths, epoch, command, publish)
        }
        (CommandKind::SteerInbox, Intent::SteerInbox(inbox)) => {
            steering::inbox(bus, epoch, command, inbox)
        }
        (CommandKind::SteerSend, Intent::SteerSend(send)) => {
            steering::send(bus, paths, epoch, &config.steering, command, send)
        }
        (CommandKind::SteerCancel, Intent::DirectiveRef(cancel)) => {
            steering::cancel(bus, paths, epoch, command, cancel)
        }
        (CommandKind::SteerDispose, Intent::SteerDispose(dispose)) => {
            steering::dispose(bus, paths, epoch, command, dispose)
        }
        (CommandKind::ConsentRead, Intent::ConsentRead(_)) => {
            dispatch_consent_read(bus, &config.consent)
        }
        (CommandKind::ConsentPolicy, Intent::ConsentPolicy(_)) => {
            dispatch_consent_policy(bus, &config.consent)
        }
        (CommandKind::ConsentDecide, Intent::ConsentDecide(decide)) => {
            dispatch_consent_decide(bus, epoch, &config.consent, command, decide)
        }
        (CommandKind::TrustGrant | CommandKind::TrustRevoke, Intent::TrustRef(reference)) => {
            dispatch_trust_rule(bus, repository, epoch, command, reference)
        }
        _ => Outcome::Refusal(Refusal {
            reason: "command::handler-not-landed".to_owned(),
            witness: None,
            expected: Some(command.kind().to_string()),
            observed: None,
            remedy: "land the registered resident handler before exposing this CLI leaf".to_owned(),
        }),
    }
}

fn dispatch_consent_read(bus: &Bus, policy: &consent::Policy) -> Outcome<CommandAnswer> {
    let (snapshot, queue) = match consent_snapshot(bus, policy) {
        Ok(state) => state,
        Err(outcome) => return outcome,
    };
    let answer = match queue.answer(policy) {
        Ok(answer) => answer,
        Err(error) => {
            return typed_protocol_fault("consent::queue-invalid", &error.to_string());
        }
    };
    Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(
            snapshot
                .horizon()
                .expect("resident startup always precedes a socket snapshot"),
        ),
        body: CommandAnswer::ConsentQueue(answer),
    })
}

fn dispatch_consent_policy(bus: &Bus, policy: &consent::Policy) -> Outcome<CommandAnswer> {
    let (snapshot, queue) = match consent_snapshot(bus, policy) {
        Ok(state) => state,
        Err(outcome) => return outcome,
    };
    Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(
            snapshot
                .horizon()
                .expect("resident startup always precedes a socket snapshot"),
        ),
        body: CommandAnswer::ConsentPolicy(queue.policy_answer(policy)),
    })
}

struct ConsentPlan {
    pending: consent::PendingRequest,
    exact: Vec<Witness>,
    terminal: ConsentTerminal,
}

fn dispatch_consent_decide(
    bus: &Bus,
    epoch: CanonicalUuid,
    policy: &consent::Policy,
    command: &wire::AdmittedCommand,
    decide: &ConsentDecide,
) -> Outcome<CommandAnswer> {
    let plan = match prepare_consent_decision(bus, policy, command, decide) {
        Ok(plan) => plan,
        Err(outcome) => return outcome,
    };
    let (proposal, answer) = match propose_consent_decision(epoch, policy, command, decide, plan) {
        Ok(proposed) => proposed,
        Err(outcome) => return outcome,
    };
    commit_consent_decision(bus, proposal, answer)
}

fn prepare_consent_decision(
    bus: &Bus,
    policy: &consent::Policy,
    command: &wire::AdmittedCommand,
    decide: &ConsentDecide,
) -> Result<ConsentPlan, Outcome<CommandAnswer>> {
    require_operator(command)?;
    require_terminal_consent_answer(decide)?;
    let (_, queue) = consent_snapshot(bus, policy)?;
    let answer = queue
        .answer(policy)
        .map_err(|error| typed_protocol_fault("consent::queue-invalid", &error.to_string()))?;
    let pending_answer = answer
        .pending
        .iter()
        .find(|pending| pending.request == decide.request)
        .ok_or_else(|| consent_not_held(decide.request))?;
    let exact = consent::decision_witnesses(pending_answer, decide.answer)
        .map_err(Outcome::Refusal)?;
    if !same_witnesses(command.expected(), &exact) {
        return Err(Outcome::Refusal(Refusal {
            reason: "consent::witness-moved".to_owned(),
            witness: Some(decide.request.to_string()),
            expected: Some(format!("{exact:?}")),
            observed: Some(format!("{:?}", command.expected())),
            remedy: "re-read the consent queue and retry its exact current request".to_owned(),
        }));
    }
    let pending = queue
        .pending(decide.request)
        .expect("the wire queue answer and internal queue share pending identities")
        .clone();
    let terminal = consent_terminal(decide, &pending);
    Ok(ConsentPlan {
        pending,
        exact,
        terminal,
    })
}

fn require_operator(command: &wire::AdmittedCommand) -> Result<(), Outcome<CommandAnswer>> {
    if matches!(command.actor(), Actor::Operator) {
        return Ok(());
    }
    Err(Outcome::Refusal(Refusal {
        reason: "command::authority".to_owned(),
        witness: Some("principal".to_owned()),
        expected: Some("operator".to_owned()),
        observed: Some(command.actor().to_string()),
        remedy: "reconnect with the current operator capability".to_owned(),
    }))
}

fn require_terminal_consent_answer(
    decide: &ConsentDecide,
) -> Result<(), Outcome<CommandAnswer>> {
    if matches!(decide.answer, wire::Answer::Granted | wire::Answer::Denied) {
        return Ok(());
    }
    Err(Outcome::Refusal(Refusal {
        reason: "consent::ruling-invalid".to_owned(),
        witness: Some(decide.request.to_string()),
        expected: Some("granted or denied".to_owned()),
        observed: Some(format!("{:?}", decide.answer).to_lowercase()),
        remedy: "choose allow or deny for the queued request".to_owned(),
    }))
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
        _ => unreachable!("terminal answer was validated"),
    }
}

fn propose_consent_decision(
    epoch: CanonicalUuid,
    policy: &consent::Policy,
    command: &wire::AdmittedCommand,
    decide: &ConsentDecide,
    plan: ConsentPlan,
) -> Result<(JournalWrite, ConsentAnswer), Outcome<CommandAnswer>> {
    let mut proposal = write_at_origin(
        command.actor().clone(),
        plan.pending.origin().clone(),
        Producer::CommandDoor,
    )
    .map_err(typed_outcome)?;
    guard_consent_decision(&mut proposal, policy, decide, &plan.exact);
    let fact = match plan.terminal {
        ConsentTerminal::Granted(body) => {
            proposal.fact(body, FactCause::Existing(plan.pending.fact()))
        }
        ConsentTerminal::Denied(body) => {
            proposal.fact(body, FactCause::Existing(plan.pending.fact()))
        }
    };
    fact.map_err(|error| typed_store_fault("consent::fact-invalid", &error.to_string()))?;
    let answer = ConsentAnswer {
        request: decide.request,
        answer: decide.answer,
    };
    record_consent_result(&mut proposal, epoch, command, decide, &answer)?;
    Ok((proposal, answer))
}

fn guard_consent_decision(
    proposal: &mut JournalWrite,
    policy: &consent::Policy,
    decide: &ConsentDecide,
    exact: &[Witness],
) {
    let guarded_policy = policy.clone();
    let guarded_expected = exact.to_vec();
    let guarded_request = decide.request;
    let guarded_answer = decide.answer;
    proposal.guard(JournalGuard::new(move |read| {
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
        let witnesses = consent::decision_witnesses(current, guarded_answer)
            .map_err(|_| GuardError::refused("consent::request-moved"))?;
        if same_witnesses(&witnesses, &guarded_expected) {
            Ok(())
        } else {
            Err(GuardError::refused("consent::witness-moved"))
        }
    }));
}

fn record_consent_result(
    proposal: &mut JournalWrite,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    decide: &ConsentDecide,
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

fn commit_consent_decision(
    bus: &Bus,
    proposal: JournalWrite,
    answer: ConsentAnswer,
) -> Outcome<CommandAnswer> {
    match commit(bus, proposal) {
        Ok(committed) => Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(committed.first()),
                last: seq(committed.last()),
            }),
            body: CommandAnswer::Consent(answer),
        }),
        Err(outcome) => typed_outcome(outcome),
    }
}

struct TrustPlan {
    from: TrustState,
    to: TrustState,
    repository: Repository,
    surface: crate::trust_command::Surface,
    head: journal::JournalHead<TrustHead>,
}

fn dispatch_trust_rule(
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    reference: &TrustRef,
) -> Outcome<CommandAnswer> {
    let plan = match prepare_trust_rule(bus, repository, epoch, command, reference) {
        Ok(plan) => plan,
        Err(outcome) => return outcome,
    };
    let (proposal, answer) = match propose_trust_rule(epoch, command, reference, plan) {
        Ok(proposed) => proposed,
        Err(outcome) => return outcome,
    };
    commit_trust_rule(bus, proposal, answer)
}

fn prepare_trust_rule(
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    reference: &TrustRef,
) -> Result<TrustPlan, Outcome<CommandAnswer>> {
    require_operator(command)?;
    let (from, to) = trust_transition(command.kind());
    let surface = current_trust_surface(repository, reference)?;
    let head = current_trust_head(bus, reference, from)?;
    require_trust_witness(command, reference, from)?;
    require_trust_epoch(bus, command, epoch)?;
    Ok(TrustPlan {
        from,
        to,
        repository: repository.clone(),
        surface,
        head,
    })
}

fn trust_transition(kind: CommandKind) -> (TrustState, TrustState) {
    match kind {
        CommandKind::TrustGrant => (TrustState::Pended, TrustState::Granted),
        CommandKind::TrustRevoke => (TrustState::Granted, TrustState::Revoked),
        _ => unreachable!("trust dispatcher receives only grant or revoke"),
    }
}

fn current_trust_surface(
    repository: &Repository,
    reference: &TrustRef,
) -> Result<crate::trust_command::Surface, Outcome<CommandAnswer>> {
    let surface = crate::trust_command::Surface::load(repository)
        .map_err(|error| crate::trust_command::load_failure(&error))?;
    if reference.digest == *surface.digest() {
        return Ok(surface);
    }
    Err(trust_refusal(
        "trust::digest-not-current",
        Some(surface.digest().to_string()),
        Some(reference.digest.to_string()),
        "run spice trust and rule on its exact current digest",
    ))
}

fn current_trust_head(
    bus: &Bus,
    reference: &TrustRef,
    from: TrustState,
) -> Result<journal::JournalHead<TrustHead>, Outcome<CommandAnswer>> {
    let head = bus
        .trust_head(&reference.digest)
        .map_err(|error| typed_store_fault("trust::head-unreadable", &error.to_string()))?
        .ok_or_else(|| {
            trust_refusal(
                "trust::digest-unknown",
                Some(reference.digest.to_string()),
                None,
                "restart the resident to pend the exact current digest before ruling",
            )
        })?;
    if head.body().state == from {
        return Ok(head);
    }
    Err(trust_refusal(
        "trust::standing-moved",
        Some(format!("{from:?}").to_lowercase()),
        Some(format!("{:?}", head.body().state).to_lowercase()),
        "re-read the current trust report before issuing another ruling",
    ))
}

fn require_trust_witness(
    command: &wire::AdmittedCommand,
    reference: &TrustRef,
    from: TrustState,
) -> Result<(), Outcome<CommandAnswer>> {
    let expected = Witness::Trust {
        digest: reference.digest.clone(),
        standing: from,
    };
    if command.expected() == [expected] {
        return Ok(());
    }
    Err(trust_refusal(
        "trust::witness-moved",
        Some(format!("{}:{from:?}", reference.digest).to_lowercase()),
        Some(format!("{:?}", command.expected())),
        "re-read the current trust head and retry its exact witness",
    ))
}

fn require_trust_epoch(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    epoch: CanonicalUuid,
) -> Result<(), Outcome<CommandAnswer>> {
    let residency = bus
        .residency_head()
        .map_err(|error| typed_store_fault("resident::head-unreadable", &error.to_string()))?
        .ok_or_else(|| typed_store_fault("resident::head-missing", "residency head is absent"))?;
    if command.epoch() == epoch && residency.body().epoch == epoch {
        return Ok(());
    }
    Err(typed_protocol_fault(
        "resident::epoch-moved",
        &format!(
            "request {}, resident {}, durable head {}",
            command.epoch(),
            epoch,
            residency.body().epoch
        ),
    ))
}

fn propose_trust_rule(
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    reference: &TrustRef,
    plan: TrustPlan,
) -> Result<(JournalWrite, TrustAnswer), Outcome<CommandAnswer>> {
    let mut proposal =
        write(command.actor().clone(), epoch, Producer::CommandDoor).map_err(typed_outcome)?;
    guard_trust_rule(&mut proposal, epoch, reference, &plan);
    let fact = match command.kind() {
        CommandKind::TrustGrant => proposal.fact(
            wire::body::TrustGranted {
                surface: crate::trust_command::SURFACE_NAME.to_owned(),
                digest: reference.digest.clone(),
            },
            FactCause::Existing(plan.head.position()),
        ),
        CommandKind::TrustRevoke => proposal.fact(
            wire::body::TrustRevoked {
                surface: crate::trust_command::SURFACE_NAME.to_owned(),
                digest: reference.digest.clone(),
            },
            FactCause::Existing(plan.head.position()),
        ),
        _ => unreachable!("trust dispatcher receives only grant or revoke"),
    }
    .map_err(|error| typed_store_fault("trust::fact-invalid", &error.to_string()))?;
    let digest = reference.digest.clone();
    proposal.trust_head(fact, move |_| TrustHead {
        digest,
        state: plan.to,
    });
    let answer = TrustAnswer {
        digest: reference.digest.clone(),
        standing: plan.to,
    };
    record_trust_result(&mut proposal, epoch, command, reference, &answer)?;
    Ok((proposal, answer))
}

fn guard_trust_rule(
    proposal: &mut JournalWrite,
    epoch: CanonicalUuid,
    reference: &TrustRef,
    plan: &TrustPlan,
) {
    let guarded_repository = plan.repository.clone();
    let guarded_surface = plan.surface.clone();
    let guarded_digest = reference.digest.clone();
    let from = plan.from;
    proposal.guard(JournalGuard::new(move |read| {
        let current = crate::trust_command::Surface::load(&guarded_repository)
            .map_err(|_| GuardError::refused("trust::surface-moved"))?;
        if current != guarded_surface {
            return Err(GuardError::refused("trust::surface-moved"));
        }
        let current = read
            .trust_head(&guarded_digest)?
            .ok_or_else(|| GuardError::refused("trust::digest-unknown"))?;
        if current.body().state != from {
            return Err(GuardError::refused("trust::standing-moved"));
        }
        let residency = read
            .residency_head()?
            .ok_or_else(|| GuardError::refused("resident::head-missing"))?;
        if residency.body().epoch == epoch {
            Ok(())
        } else {
            Err(GuardError::refused("resident::epoch-moved"))
        }
    }));
}

fn record_trust_result(
    proposal: &mut JournalWrite,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    reference: &TrustRef,
    answer: &TrustAnswer,
) -> Result<(), Outcome<CommandAnswer>> {
    proposal
        .command_result(
            Command {
                v: wire::WireVersion::CURRENT,
                id: command.id(),
                epoch: command.epoch(),
                kind: command.kind(),
                expected: command.expected().to_vec(),
                body: reference.clone(),
            },
            epoch,
            answer.clone(),
        )
        .map_err(|error| typed_store_fault("trust::receipt-invalid", &error.to_string()))
}

fn commit_trust_rule(
    bus: &Bus,
    proposal: JournalWrite,
    answer: TrustAnswer,
) -> Outcome<CommandAnswer> {
    match commit(bus, proposal) {
        Ok(committed) => Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(committed.first()),
                last: seq(committed.last()),
            }),
            body: CommandAnswer::Trust(answer),
        }),
        Err(outcome) => typed_outcome(outcome),
    }
}

enum ConsentTerminal {
    Granted(wire::body::ConsentGranted),
    Denied(wire::body::ConsentDenied),
}

fn consent_snapshot(
    bus: &Bus,
    policy: &consent::Policy,
) -> Result<(journal::FactSnapshot, consent::Queue), Outcome<CommandAnswer>> {
    let snapshot = bus
        .fact_snapshot()
        .map_err(|error| typed_store_fault("consent::journal-unreadable", &error.to_string()))?;
    let queue = consent::Queue::from_snapshot(&snapshot, policy)
        .map_err(|error| typed_protocol_fault("consent::queue-invalid", &error.to_string()))?;
    Ok((snapshot, queue))
}

fn same_witnesses(left: &[Witness], right: &[Witness]) -> bool {
    let mut left = left.to_vec();
    let mut right = right.to_vec();
    left.sort_unstable();
    right.sort_unstable();
    left == right
}

fn consent_not_held(request: CanonicalUuid) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal {
        reason: "consent::request-not-held".to_owned(),
        witness: Some(request.to_string()),
        expected: Some("a request in the current consent queue".to_owned()),
        observed: Some("absent or terminal".to_owned()),
        remedy: "re-read the consent queue and decide a request that is still pending".to_owned(),
    })
}

fn trust_refusal(
    reason: &str,
    expected: Option<String>,
    observed: Option<String>,
    remedy: &str,
) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: Some("executable-policy".to_owned()),
        expected,
        observed,
        remedy: remedy.to_owned(),
    })
}
