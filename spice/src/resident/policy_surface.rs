//! Resident-owned consent decisions and executable-policy rulings.
//! Trust rulings commit through one generation-guarded path.

use bus::Bus;
use journal::{FactCause, GuardError, JournalGuard, JournalHead, JournalWrite};
use spice::repository::Repository;
use wire::intent::{CommandAnswer, ConsentAnswer, ConsentDecide, TrustAnswer, TrustRef};
use wire::{
    CanonicalUuid, Command, CommandKind, Outcome, Producer, Refusal, Reply, ReplyPosition,
    SeqRange, TrustHead, TrustState, Witness,
};

use super::{
    commit, seq, typed_outcome, typed_protocol_fault, typed_store_fault, write, write_at_origin,
};

type SurfaceResult<T> = Result<T, Outcome<CommandAnswer>>;

pub(super) fn read(bus: &Bus, policy: &consent::Policy) -> Outcome<CommandAnswer> {
    let (snapshot, queue) = match consent_snapshot(bus, policy) {
        Ok(state) => state,
        Err(outcome) => return outcome,
    };
    let answer = match queue.answer(policy) {
        Ok(answer) => answer,
        Err(error) => return typed_protocol_fault("consent::queue-invalid", &error.to_string()),
    };
    Outcome::Reply(Reply {
        position: snapshot_position(&snapshot),
        body: CommandAnswer::ConsentQueue(answer),
    })
}

pub(super) fn policy(bus: &Bus, policy: &consent::Policy) -> Outcome<CommandAnswer> {
    let (snapshot, queue) = match consent_snapshot(bus, policy) {
        Ok(state) => state,
        Err(outcome) => return outcome,
    };
    Outcome::Reply(Reply {
        position: snapshot_position(&snapshot),
        body: CommandAnswer::ConsentPolicy(queue.policy_answer(policy)),
    })
}

pub(super) fn decide(
    bus: &Bus,
    epoch: CanonicalUuid,
    policy: &consent::Policy,
    command: &wire::AdmittedCommand,
    decide: &ConsentDecide,
) -> Outcome<CommandAnswer> {
    let (queue, exact) = match admit_decision(bus, policy, command, decide) {
        Ok(admission) => admission,
        Err(outcome) => return outcome,
    };
    let pending = queue
        .pending(decide.request)
        .expect("the wire queue answer and internal queue share pending identities");
    let terminal = ConsentTerminal::from_decision(decide, pending.lane());
    let mut proposal = match write_at_origin(
        command.actor().clone(),
        pending.origin().clone(),
        Producer::CommandDoor,
    ) {
        Ok(proposal) => proposal,
        Err(outcome) => return typed_outcome(outcome),
    };
    proposal.guard(consent_guard(policy.clone(), exact, decide));
    if let Err(error) = terminal.record(&mut proposal, pending.fact()) {
        return typed_store_fault("consent::fact-invalid", &error.to_string());
    }
    let answer = ConsentAnswer {
        request: decide.request,
        answer: decide.answer,
    };
    if let Err(error) = consent_receipt(&mut proposal, command, decide, epoch, &answer) {
        return typed_store_fault("consent::receipt-invalid", &error.to_string());
    }
    committed_answer(bus, proposal, CommandAnswer::Consent(answer))
}

pub(super) fn trust_rule(
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    reference: &TrustRef,
) -> Outcome<CommandAnswer> {
    trust_rule_with(bus, repository, epoch, command, reference, || {})
}

pub(super) fn trust_rule_with<F>(
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    reference: &TrustRef,
    before_commit: F,
) -> Outcome<CommandAnswer>
where
    F: FnOnce(),
{
    let admission = match admit_trust(bus, repository, epoch, command, reference) {
        Ok(admission) => admission,
        Err(outcome) => return outcome,
    };
    let mut proposal = match write(command.actor().clone(), epoch, Producer::CommandDoor) {
        Ok(proposal) => proposal,
        Err(outcome) => return typed_outcome(outcome),
    };
    proposal.guard(trust_guard(
        repository.clone(),
        admission.surface,
        reference.digest.clone(),
        admission.from,
        epoch,
    ));
    let fact = match trust_fact(
        &mut proposal,
        command.kind(),
        reference,
        admission.head.position(),
    ) {
        Ok(fact) => fact,
        Err(outcome) => return outcome,
    };
    let digest = reference.digest.clone();
    let standing = admission.to;
    proposal.trust_head(fact, move |_| TrustHead {
        digest,
        state: standing,
    });
    let answer = TrustAnswer {
        digest: reference.digest.clone(),
        standing,
    };
    if let Err(error) = trust_receipt(&mut proposal, command, reference, epoch, &answer) {
        return typed_store_fault("trust::receipt-invalid", &error.to_string());
    }
    // Commit evaluates the surface-generation guard after this deterministic race seam.
    before_commit();
    committed_answer(bus, proposal, CommandAnswer::Trust(answer))
}

fn admit_decision(
    bus: &Bus,
    policy: &consent::Policy,
    command: &wire::AdmittedCommand,
    decide: &ConsentDecide,
) -> SurfaceResult<(consent::Queue, Vec<Witness>)> {
    if !matches!(decide.answer, wire::Answer::Granted | wire::Answer::Denied) {
        return Err(Outcome::Refusal(Refusal::bare(
            wire::RefusalDiagnostic::new(
                "consent::ruling-invalid".to_owned(),
                Some(decide.request.to_string()),
                Some("granted or denied".to_owned()),
                Some(format!("{:?}", decide.answer).to_lowercase()),
            ),
        )));
    }
    let (_, queue) = consent_snapshot(bus, policy)?;
    let answer = queue
        .answer(policy)
        .map_err(|error| typed_protocol_fault("consent::queue-invalid", &error.to_string()))?;
    let pending = answer
        .pending
        .iter()
        .find(|pending| pending.request == decide.request)
        .ok_or_else(|| consent_not_held(decide.request))?;
    let exact = consent::decision_witnesses(pending, decide.answer).map_err(Outcome::Refusal)?;
    if !same_witnesses(command.expected(), &exact) {
        return Err(Outcome::Refusal(Refusal::bare(
            wire::RefusalDiagnostic::new(
                "consent::witness-moved".to_owned(),
                Some(decide.request.to_string()),
                Some(format!("{exact:?}")),
                Some(format!("{:?}", command.expected())),
            ),
        )));
    }
    Ok((queue, exact))
}

fn consent_guard(
    policy: consent::Policy,
    expected: Vec<Witness>,
    decide: &ConsentDecide,
) -> JournalGuard {
    let request = decide.request;
    let ruling = decide.answer;
    JournalGuard::new(move |read| {
        let snapshot = read
            .fact_snapshot()
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        let queue = consent::Queue::from_snapshot(&snapshot, &policy)
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        let answer = queue
            .answer(&policy)
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        let current = answer
            .pending
            .iter()
            .find(|pending| pending.request == request)
            .ok_or_else(|| GuardError::refused("consent::request-moved"))?;
        let witnesses = consent::decision_witnesses(current, ruling)
            .map_err(|_| GuardError::refused("consent::request-moved"))?;
        if !same_witnesses(&witnesses, &expected) {
            return Err(GuardError::refused("consent::witness-moved"));
        }
        Ok(())
    })
}

fn consent_receipt(
    proposal: &mut JournalWrite,
    command: &wire::AdmittedCommand,
    decide: &ConsentDecide,
    epoch: CanonicalUuid,
    answer: &ConsentAnswer,
) -> Result<(), journal::BuildError> {
    proposal.command_result(
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
}

struct TrustAdmission {
    from: TrustState,
    to: TrustState,
    surface: crate::trust_command::Surface,
    head: JournalHead<TrustHead>,
}

fn admit_trust(
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    reference: &TrustRef,
) -> SurfaceResult<TrustAdmission> {
    let (from, to) = trust_transition(command.kind());
    let surface = crate::trust_command::Surface::load(repository)
        .map_err(|error| crate::trust_command::load_failure(&error))?;
    let head = admit_trust_head(bus, command, reference, &surface, from)?;
    admit_epoch(bus, command, epoch)?;
    Ok(TrustAdmission {
        from,
        to,
        surface,
        head,
    })
}

fn admit_trust_head(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    reference: &TrustRef,
    surface: &crate::trust_command::Surface,
    from: TrustState,
) -> SurfaceResult<JournalHead<TrustHead>> {
    if reference.digest != *surface.digest() {
        return Err(trust_refusal(
            "trust::digest-not-current",
            Some(surface.digest().to_string()),
            Some(reference.digest.to_string()),
            "run spice trust and rule on its exact current digest",
        ));
    }
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
    Ok(head)
}

fn admit_epoch(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    epoch: CanonicalUuid,
) -> SurfaceResult<()> {
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

fn trust_guard(
    repository: Repository,
    surface: crate::trust_command::Surface,
    digest: wire::Digest,
    from: TrustState,
    epoch: CanonicalUuid,
) -> JournalGuard {
    JournalGuard::new(move |read| {
        super::require_current_surface(&repository, &surface)?;
        let current = read
            .trust_head(&digest)?
            .ok_or_else(|| GuardError::refused("trust::digest-unknown"))?;
        if current.body().state != from {
            return Err(GuardError::refused("trust::standing-moved"));
        }
        let residency = read
            .residency_head()?
            .ok_or_else(|| GuardError::refused("resident::head-missing"))?;
        if residency.body().epoch != epoch {
            return Err(GuardError::refused("resident::epoch-moved"));
        }
        Ok(())
    })
}

fn trust_fact(
    proposal: &mut JournalWrite,
    kind: CommandKind,
    reference: &TrustRef,
    cause: wire::Seq,
) -> SurfaceResult<journal::PendingFact> {
    let fact = match kind {
        CommandKind::TrustGrant => proposal.fact(
            wire::body::TrustGranted {
                surface: crate::trust_command::SURFACE_NAME.to_owned(),
                digest: reference.digest.clone(),
            },
            FactCause::Existing(cause),
        ),
        CommandKind::TrustRevoke => proposal.fact(
            wire::body::TrustRevoked {
                surface: crate::trust_command::SURFACE_NAME.to_owned(),
                digest: reference.digest.clone(),
            },
            FactCause::Existing(cause),
        ),
        _ => unreachable!("trust dispatcher receives only grant or revoke"),
    };
    fact.map_err(|error| typed_store_fault("trust::fact-invalid", &error.to_string()))
}

fn trust_receipt(
    proposal: &mut JournalWrite,
    command: &wire::AdmittedCommand,
    reference: &TrustRef,
    epoch: CanonicalUuid,
    answer: &TrustAnswer,
) -> Result<(), journal::BuildError> {
    proposal.command_result(
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
}

fn committed_answer(
    bus: &Bus,
    proposal: JournalWrite,
    body: CommandAnswer,
) -> Outcome<CommandAnswer> {
    match commit(bus, proposal) {
        Ok(committed) => Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(committed.first()),
                last: seq(committed.last()),
            }),
            body,
        }),
        Err(outcome) => typed_outcome(outcome),
    }
}

fn trust_transition(kind: CommandKind) -> (TrustState, TrustState) {
    match kind {
        CommandKind::TrustGrant => (TrustState::Pended, TrustState::Granted),
        CommandKind::TrustRevoke => (TrustState::Granted, TrustState::Revoked),
        _ => unreachable!("trust dispatcher receives only grant or revoke"),
    }
}

enum ConsentTerminal {
    Granted(wire::body::ConsentGranted),
    Denied(wire::body::ConsentDenied),
}

impl ConsentTerminal {
    fn from_decision(decide: &ConsentDecide, lane: CanonicalUuid) -> Self {
        match decide.answer {
            wire::Answer::Granted => Self::Granted(wire::body::ConsentGranted {
                request: decide.request,
                lane,
                rule: consent::OPERATOR.to_owned(),
                budget: None,
            }),
            wire::Answer::Denied => Self::Denied(wire::body::ConsentDenied {
                request: decide.request,
                lane,
                rule: consent::OPERATOR.to_owned(),
            }),
            _ => unreachable!("non-terminal answers were rejected during admission"),
        }
    }

    fn record(
        self,
        proposal: &mut JournalWrite,
        cause: wire::Seq,
    ) -> Result<journal::PendingFact, journal::BuildError> {
        match self {
            Self::Granted(body) => proposal.fact(body, FactCause::Existing(cause)),
            Self::Denied(body) => proposal.fact(body, FactCause::Existing(cause)),
        }
    }
}

fn consent_snapshot(
    bus: &Bus,
    policy: &consent::Policy,
) -> SurfaceResult<(journal::FactSnapshot, consent::Queue)> {
    let snapshot = bus
        .fact_snapshot()
        .map_err(|error| typed_store_fault("consent::journal-unreadable", &error.to_string()))?;
    let queue = consent::Queue::from_snapshot(&snapshot, policy)
        .map_err(|error| typed_protocol_fault("consent::queue-invalid", &error.to_string()))?;
    Ok((snapshot, queue))
}

fn snapshot_position(snapshot: &journal::FactSnapshot) -> ReplyPosition {
    ReplyPosition::Snapshot(
        snapshot
            .horizon()
            .expect("resident startup always precedes a socket snapshot"),
    )
}

fn same_witnesses(left: &[Witness], right: &[Witness]) -> bool {
    let mut left = left.to_vec();
    let mut right = right.to_vec();
    left.sort_unstable();
    right.sort_unstable();
    left == right
}

fn consent_not_held(request: CanonicalUuid) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal::bare(wire::RefusalDiagnostic::new(
        "consent::request-not-held".to_owned(),
        Some(request.to_string()),
        Some("a request in the current consent queue".to_owned()),
        Some("absent or terminal".to_owned()),
    )))
}

fn trust_refusal(
    reason: &str,
    expected: Option<String>,
    observed: Option<String>,
    detail: &str,
) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(
            reason.to_owned(),
            Some("executable-policy".to_owned()),
            expected,
            observed,
        )
        .with_detail(detail),
    ))
}
