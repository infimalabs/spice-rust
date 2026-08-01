fn dispatch_trust_rule(
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    reference: &TrustRef,
) -> Outcome<CommandAnswer> {
    let surface = match crate::trust_command::Surface::load(repository) {
        Ok(surface) => surface,
        Err(error) => return crate::trust_command::load_failure(&error),
    };
    dispatch_trust_rule_with_surface(bus, repository, epoch, command, reference, surface)
}

fn dispatch_trust_rule_with_surface(
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    reference: &TrustRef,
    surface: crate::trust_command::Surface,
) -> Outcome<CommandAnswer> {
    dispatch_trust_rule_with_surface_and(bus, repository, epoch, command, reference, surface, || {})
}

fn dispatch_trust_rule_with_surface_and<F>(
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    reference: &TrustRef,
    surface: crate::trust_command::Surface,
    before_commit: F,
) -> Outcome<CommandAnswer>
where
    F: FnOnce(),
{
    let ruling = match admit_trust_ruling(bus, epoch, command, reference, &surface) {
        Ok(ruling) => ruling,
        Err(outcome) => return outcome,
    };
    let (proposal, answer) = match build_trust_proposal(
        repository,
        epoch,
        command,
        reference,
        surface,
        ruling,
    ) {
        Ok(built) => built,
        Err(outcome) => return outcome,
    };
    before_commit();
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

#[derive(Clone, Copy, Debug)]
struct TrustRuling {
    from: TrustState,
    to: TrustState,
    cause: Seq,
}

fn admit_trust_ruling(
    bus: &Bus,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    reference: &TrustRef,
    surface: &crate::trust_command::Surface,
) -> Result<TrustRuling, Outcome<CommandAnswer>> {
    let (from, to) = trust_transition(command)?;
    let cause = admit_trust_head(bus, command, reference, surface, from)?;
    require_trust_epoch(bus, epoch, command)?;
    Ok(TrustRuling { from, to, cause })
}

fn trust_transition(
    command: &wire::AdmittedCommand,
) -> Result<(TrustState, TrustState), Outcome<CommandAnswer>> {
    if !matches!(command.actor(), Actor::Operator) {
        return Err(Outcome::Refusal(Refusal {
            reason: "command::authority".to_owned(),
            witness: Some("principal".to_owned()),
            expected: Some("operator".to_owned()),
            observed: Some(command.actor().to_string()),
            remedy: "reconnect with the current operator capability".to_owned(),
        }));
    }
    Ok(match command.kind() {
        CommandKind::TrustGrant => (TrustState::Pended, TrustState::Granted),
        CommandKind::TrustRevoke => (TrustState::Granted, TrustState::Revoked),
        _ => unreachable!("trust dispatcher receives only grant or revoke"),
    })
}

fn admit_trust_head(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    reference: &TrustRef,
    surface: &crate::trust_command::Surface,
    from: TrustState,
) -> Result<Seq, Outcome<CommandAnswer>> {
    if reference.digest != *surface.digest() {
        return Err(trust_refusal(
            "trust::digest-not-current",
            Some(surface.digest().to_string()),
            Some(reference.digest.to_string()),
            "run spice trust and rule on its exact current digest",
        ));
    }
    let head = match bus.trust_head(&reference.digest) {
        Ok(Some(head)) => head,
        Ok(None) => {
            return Err(trust_refusal(
                "trust::digest-unknown",
                Some(reference.digest.to_string()),
                None,
                "restart the resident to pend the exact current digest before ruling",
            ));
        }
        Err(error) => {
            return Err(typed_store_fault(
                "trust::head-unreadable",
                &error.to_string(),
            ));
        }
    };
    if head.body().state != from {
        return Err(trust_refusal(
            "trust::standing-moved",
            Some(format!("{from:?}").to_lowercase()),
            Some(format!("{:?}", head.body().state).to_lowercase()),
            "re-read the current trust report before issuing another ruling",
        ));
    }
    let expected = Witness::Trust {
        digest: reference.digest.clone(),
        standing: from,
    };
    if command.expected() != [expected] {
        return Err(trust_refusal(
            "trust::witness-moved",
            Some(format!("{}:{from:?}", reference.digest).to_lowercase()),
            Some(format!("{:?}", command.expected())),
            "re-read the current trust head and retry its exact witness",
        ));
    }
    Ok(head.position())
}

fn require_trust_epoch(
    bus: &Bus,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
) -> Result<(), Outcome<CommandAnswer>> {
    let residency = match bus.residency_head() {
        Ok(Some(residency)) => residency,
        Ok(None) => {
            return Err(typed_store_fault(
                "resident::head-missing",
                "residency head is absent",
            ));
        }
        Err(error) => {
            return Err(typed_store_fault(
                "resident::head-unreadable",
                &error.to_string(),
            ));
        }
    };
    if command.epoch() != epoch || residency.body().epoch != epoch {
        return Err(typed_protocol_fault(
            "resident::epoch-moved",
            &format!(
                "request {}, resident {}, durable head {}",
                command.epoch(),
                epoch,
                residency.body().epoch
            ),
        ));
    }
    Ok(())
}

fn build_trust_proposal(
    repository: &Repository,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    reference: &TrustRef,
    surface: crate::trust_command::Surface,
    ruling: TrustRuling,
) -> Result<(JournalWrite, TrustAnswer), Outcome<CommandAnswer>> {
    let mut proposal = match write(command.actor().clone(), epoch, Producer::CommandDoor) {
        Ok(proposal) => proposal,
        Err(outcome) => return Err(typed_outcome(outcome)),
    };
    let guarded_repository = repository.clone();
    let guarded_digest = reference.digest.clone();
    let guarded_surface = surface;
    proposal.guard(JournalGuard::new(move |read| {
        require_current_surface(&guarded_repository, &guarded_surface)?;
        let current = read
            .trust_head(&guarded_digest)?
            .ok_or_else(|| GuardError::refused("trust::digest-unknown"))?;
        if current.body().state != ruling.from {
            return Err(GuardError::refused("trust::standing-moved"));
        }
        let residency = read
            .residency_head()?
            .ok_or_else(|| GuardError::refused("resident::head-missing"))?;
        if residency.body().epoch != epoch {
            return Err(GuardError::refused("resident::epoch-moved"));
        }
        Ok(())
    }));
    let answer = append_trust_change(&mut proposal, command, reference, ruling)?;
    attach_trust_receipt(&mut proposal, command, reference, epoch, &answer)?;
    Ok((proposal, answer))
}

fn append_trust_change(
    proposal: &mut JournalWrite,
    command: &wire::AdmittedCommand,
    reference: &TrustRef,
    ruling: TrustRuling,
) -> Result<TrustAnswer, Outcome<CommandAnswer>> {
    let fact = match command.kind() {
        CommandKind::TrustGrant => proposal.fact(
            wire::body::TrustGranted {
                surface: crate::trust_command::SURFACE_NAME.to_owned(),
                digest: reference.digest.clone(),
            },
            FactCause::Existing(ruling.cause),
        ),
        CommandKind::TrustRevoke => proposal.fact(
            wire::body::TrustRevoked {
                surface: crate::trust_command::SURFACE_NAME.to_owned(),
                digest: reference.digest.clone(),
            },
            FactCause::Existing(ruling.cause),
        ),
        _ => unreachable!("trust dispatcher receives only grant or revoke"),
    };
    let fact = match fact {
        Ok(fact) => fact,
        Err(error) => {
            return Err(typed_store_fault(
                "trust::fact-invalid",
                &error.to_string(),
            ));
        }
    };
    let digest = reference.digest.clone();
    proposal.trust_head(fact, move |_| TrustHead {
        digest,
        state: ruling.to,
    });
    Ok(TrustAnswer {
        digest: reference.digest.clone(),
        standing: ruling.to,
    })
}

fn attach_trust_receipt(
    proposal: &mut JournalWrite,
    command: &wire::AdmittedCommand,
    reference: &TrustRef,
    epoch: CanonicalUuid,
    answer: &TrustAnswer,
) -> Result<(), Outcome<CommandAnswer>> {
    if let Err(error) = proposal.command_result(
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
    ) {
        return Err(typed_store_fault(
            "trust::receipt-invalid",
            &error.to_string(),
        ));
    }
    Ok(())
}
