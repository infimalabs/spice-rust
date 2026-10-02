//! Resident-owned consent decisions and executable-policy rulings.
//! Trust rulings commit through one generation-guarded path.

use std::path::{Path, PathBuf};

use bus::Bus;
use journal::{FactCause, GuardError, GuardRefusal, JournalGuard, JournalHead, JournalWrite};
use spice::repository::Repository;
use wire::intent::{CommandAnswer, ConsentAnswer, ConsentDecide, TrustAnswer, TrustRef};
use wire::{
    Actor, CanonicalUuid, Command, CommandKind, Outcome, Producer, Refusal, Reply, ReplyPosition,
    SeqRange, SpiceCommand, TrustHead, TrustState, Witness,
};

use super::{
    commit, read_materializations::CurrentReads, seq, typed_outcome, typed_protocol_fault,
    typed_store_fault, write, write_at_origin,
};

type SurfaceResult<T> = Result<T, Outcome<CommandAnswer>>;

pub(super) fn read(reads: &CurrentReads, policy: &consent::Policy) -> Outcome<CommandAnswer> {
    let generation = match reads.current() {
        Ok(generation) => generation,
        Err(error) => {
            return typed_store_fault("consent::materialization-unavailable", &error);
        }
    };
    let answer = match generation.consent().answer(policy) {
        Ok(answer) => answer,
        Err(error) => return typed_protocol_fault("consent::queue-invalid", &error.to_string()),
    };
    Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(generation.cursor()),
        body: CommandAnswer::ConsentQueue(answer),
    })
}

pub(super) fn policy(reads: &CurrentReads, policy: &consent::Policy) -> Outcome<CommandAnswer> {
    let generation = match reads.current() {
        Ok(generation) => generation,
        Err(error) => {
            return typed_store_fault("consent::materialization-unavailable", &error);
        }
    };
    Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(generation.cursor()),
        body: CommandAnswer::ConsentPolicy(generation.consent().policy_answer(policy)),
    })
}

/// Rule on one queued request, and let the ruling reach the lane that asked.
pub(super) fn decide(
    bus: &Bus,
    reads: &CurrentReads,
    paths: &super::RuntimePaths,
    epoch: CanonicalUuid,
    policy: &consent::Policy,
    command: &wire::AdmittedCommand,
    decide: &ConsentDecide,
) -> Outcome<CommandAnswer> {
    let generation = match admit_decision(reads, policy, command, decide) {
        Ok(admission) => admission,
        Err(outcome) => return outcome,
    };
    let pending = generation
        .consent()
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
    proposal.guard(consent_guard(generation.cursor()));
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
    let outcome = committed_answer(bus, proposal, CommandAnswer::Consent(answer));
    if matches!(outcome, Outcome::Reply(_)) {
        let confirmed = decide.answer == wire::Answer::Granted;
        super::supervisor::command::answer(paths, pending.lane(), decide.request, confirmed);
    }
    outcome
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

pub(super) fn trust_pend(
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    reference: &TrustRef,
) -> Outcome<CommandAnswer> {
    trust_pend_with(bus, repository, epoch, command, reference, || {})
}

pub(super) fn trust_pend_with<F>(
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
    let admission = match admit_trust_pend(bus, repository, epoch, command, reference) {
        Ok(admission) => admission,
        Err(outcome) => return outcome,
    };
    let TrustPendAdmission {
        worktree,
        surface,
        previous,
    } = admission;
    let mut proposal = match write(command.actor().clone(), epoch, Producer::CommandDoor) {
        Ok(proposal) => proposal,
        Err(outcome) => return typed_outcome(outcome),
    };
    let guard_context = TrustRefusalContext::pend(reference).with_worktree(worktree.clone());
    proposal.guard(trust_pend_guard(
        repository.common_dir().to_path_buf(),
        guard_context,
        surface,
        reference.digest.clone(),
        epoch,
    ));
    let fact = match proposal.fact(
        wire::body::TrustPended {
            surface: crate::trust_command::SURFACE_NAME.to_owned(),
            digest: reference.digest.clone(),
            worktree: Some(worktree.clone()),
            previous,
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return typed_store_fault("trust::fact-invalid", &error.to_string()),
    };
    let digest = reference.digest.clone();
    proposal.trust_head(fact, move |_| TrustHead {
        digest,
        state: TrustState::Pended,
        worktree: Some(worktree),
    });
    let answer = TrustAnswer {
        digest: reference.digest.clone(),
        standing: TrustState::Pended,
    };
    if let Err(error) = trust_receipt(&mut proposal, command, reference, epoch, &answer) {
        return typed_store_fault("trust::receipt-invalid", &error.to_string());
    }
    before_commit();
    committed_answer(bus, proposal, CommandAnswer::Trust(answer))
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
    let mut proposal = match write_at_origin(
        command.actor().clone(),
        admission.origin,
        Producer::CommandDoor,
    ) {
        Ok(proposal) => proposal,
        Err(outcome) => return typed_outcome(outcome),
    };
    let guard_context = TrustRefusalContext::rule(command.kind(), reference)
        .with_worktree(admission.worktree.clone());
    proposal.guard(trust_guard(
        repository.common_dir().to_path_buf(),
        guard_context,
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
    let worktree = admission.worktree;
    proposal.trust_head(fact, move |_| TrustHead {
        digest,
        state: standing,
        worktree: Some(worktree),
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
    reads: &CurrentReads,
    policy: &consent::Policy,
    command: &wire::AdmittedCommand,
    decide: &ConsentDecide,
) -> SurfaceResult<std::sync::Arc<super::read_materializations::Generation>> {
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
    let generation = reads
        .current()
        .map_err(|error| typed_store_fault("consent::materialization-unavailable", &error))?;
    let answer = generation
        .consent()
        .answer(policy)
        .map_err(|error| typed_protocol_fault("consent::queue-invalid", &error.to_string()))?;
    let pending = answer
        .pending
        .iter()
        .find(|pending| pending.request == decide.request)
        .ok_or_else(|| consent_not_held(decide.request))?;
    let exact = pending
        .decision_witnesses(decide.answer)
        .map_err(Outcome::Refusal)?;
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
    Ok(generation)
}

/// Hold a consent write to the generation whose queue decided it.
pub(super) fn consent_guard(expected: wire::Seq) -> JournalGuard {
    JournalGuard::new(move |read| {
        let observed = read.cursor()?;
        if observed != Some(expected) {
            return Err(GuardError::refused("consent::materialization-moved"));
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
    origin: wire::Origin,
    worktree: String,
}

struct TrustPendAdmission {
    worktree: String,
    surface: crate::trust_command::Surface,
    previous: Option<wire::Digest>,
}

#[derive(Clone)]
struct TrustRefusalContext {
    worktree: Option<String>,
    recovery: SpiceCommand,
}

impl TrustRefusalContext {
    fn pend(reference: &TrustRef) -> Self {
        Self {
            worktree: reference.worktree.clone(),
            recovery: trust_recovery("pend", None),
        }
    }

    fn rule(kind: CommandKind, reference: &TrustRef) -> Self {
        let ruling = match kind {
            CommandKind::TrustGrant => "grant",
            CommandKind::TrustRevoke => "revoke",
            _ => unreachable!("trust ruling context receives only grant or revoke"),
        };
        Self {
            worktree: reference.worktree.clone(),
            recovery: trust_recovery(ruling, Some(&reference.digest)),
        }
    }

    fn with_worktree(&self, worktree: impl Into<String>) -> Self {
        Self {
            worktree: Some(worktree.into()),
            recovery: self.recovery.clone(),
        }
    }

    fn pend_again(&self) -> Self {
        Self {
            worktree: self.worktree.clone(),
            recovery: trust_recovery("pend", None),
        }
    }

    fn report(&self) -> Self {
        Self {
            worktree: self.worktree.clone(),
            recovery: trust_recovery("report", None),
        }
    }

    fn detail(&self, detail: &str) -> String {
        let worktree = self
            .worktree
            .as_deref()
            .map_or_else(|| "unknown".to_owned(), |worktree| format!("{worktree:?}"));
        format!("producer_worktree={worktree}; {detail}")
    }

    fn refusal(
        &self,
        reason: &str,
        expected: Option<String>,
        observed: Option<String>,
        detail: &str,
    ) -> Outcome<CommandAnswer> {
        let diagnostic = wire::RefusalDiagnostic::new(
            reason.to_owned(),
            Some("executable-policy".to_owned()),
            expected,
            observed,
        )
        .with_detail(self.detail(detail));
        Outcome::Refusal(Refusal::repairable(self.recovery.clone(), diagnostic))
    }

    fn guard_refusal(
        &self,
        reason: &str,
        expected: Option<String>,
        observed: Option<String>,
        detail: &str,
    ) -> GuardError {
        GuardError::Refused(
            GuardRefusal::new(reason)
                .with_evidence("executable-policy", expected, observed)
                .with_detail(self.detail(detail))
                .with_recovery(self.recovery.clone()),
        )
    }
}

fn trust_recovery(action: &str, digest: Option<&wire::Digest>) -> SpiceCommand {
    let arguments = match (action, digest) {
        ("report", None) => Vec::new(),
        (action, Some(digest)) => vec![action.to_owned(), digest.to_string()],
        (action, None) => vec![action.to_owned()],
    };
    SpiceCommand::new("trust", arguments).expect("trust recovery has one stable subcommand")
}

fn admit_trust_pend(
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    reference: &TrustRef,
) -> SurfaceResult<TrustPendAdmission> {
    let context = TrustRefusalContext::pend(reference);
    if !matches!(command.actor(), Actor::Operator) {
        return Err(context.refusal(
            "trust::operator-required",
            Some("operator".to_owned()),
            Some(command.actor().to_string()),
            "submit trust pend through the current operator discovery",
        ));
    }
    let worktree = reference.worktree.clone().ok_or_else(|| {
        context.refusal(
            "trust::worktree-missing",
            Some("the canonical producing worktree root".to_owned()),
            None,
            "rerun `spice trust pend` from the worktree that produced the digest",
        )
    })?;
    let context = context.with_worktree(worktree.clone());
    let producing_repository = admit_trust_worktree(repository, &worktree, &context)?;
    let surface = crate::trust_command::Surface::load(&producing_repository)
        .map_err(|error| crate::trust_command::load_failure(&error))?;
    if reference.digest != *surface.digest() {
        return Err(context.pend_again().refusal(
            "trust::digest-not-current",
            Some(surface.digest().to_string()),
            Some(reference.digest.to_string()),
            &format!(
                "rerun `spice trust pend` from worktree {}",
                producing_repository.worktree_root().display(),
            ),
        ));
    }
    let expected = Witness::Trust {
        digest: reference.digest.clone(),
        standing: TrustState::Pended,
    };
    if command.expected() != [expected] {
        return Err(context.refusal(
            "trust::witness-moved",
            Some(format!("{}:pended", reference.digest)),
            Some(format!("{:?}", command.expected())),
            "recapture the current executable surface before pending it",
        ));
    }
    admit_epoch(bus, command, epoch, &context)?;
    match bus.trust_head(&reference.digest) {
        Ok(None) => {}
        Ok(Some(_)) => {
            return Err(context.report().refusal(
                "trust::digest-already-pended",
                Some("one previously unseen current digest".to_owned()),
                Some(reference.digest.to_string()),
                "read the current trust report before issuing another ruling",
            ));
        }
        Err(error) => {
            return Err(typed_store_fault(
                "trust::head-unreadable",
                &error.to_string(),
            ));
        }
    }
    let previous = super::latest_surface_digest(bus).map_err(typed_outcome)?;
    Ok(TrustPendAdmission {
        worktree,
        surface,
        previous,
    })
}

fn admit_trust(
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    reference: &TrustRef,
) -> SurfaceResult<TrustAdmission> {
    let context = TrustRefusalContext::rule(command.kind(), reference);
    if reference.worktree.is_some() {
        return Err(context.refusal(
            "trust::worktree-unexpected",
            Some("grant and revoke name only the exact digest".to_owned()),
            reference.worktree.clone(),
            "retry the unchanged exact-digest trust ruling command",
        ));
    }
    let (from, to) = trust_transition(command.kind());
    let head = admit_trust_head(bus, command, reference, from, &context)?;
    let worktree = head.body().worktree.clone().ok_or_else(|| {
        context.pend_again().refusal(
            "trust::worktree-unbound",
            Some("schema 12 trust authority with a producing worktree".to_owned()),
            Some(format!("schema 11 digest {}", reference.digest)),
            "use the release that owns journal schema 11 to retire the legacy ruling, then pend the current digest with this release",
        )
    })?;
    let context = context.with_worktree(worktree.clone());
    let producing_repository = admit_trust_worktree(repository, &worktree, &context)?;
    let surface = crate::trust_command::Surface::load(&producing_repository)
        .map_err(|error| crate::trust_command::load_failure(&error))?;
    if reference.digest != *surface.digest() {
        return Err(context.pend_again().refusal(
            "trust::digest-not-current",
            Some(surface.digest().to_string()),
            Some(reference.digest.to_string()),
            &format!(
                "run spice trust from producing worktree {worktree} and rule on its exact current digest"
            ),
        ));
    }
    let origin = trust_head_origin(bus, &head)?;
    admit_epoch(bus, command, epoch, &context)?;
    Ok(TrustAdmission {
        from,
        to,
        surface,
        head,
        origin,
        worktree,
    })
}

fn admit_trust_worktree(
    resident_repository: &Repository,
    worktree: &str,
    context: &TrustRefusalContext,
) -> SurfaceResult<Repository> {
    let repository = Repository::discover(worktree).map_err(|error| {
        context.pend_again().refusal(
            "trust::worktree-unresolved",
            Some("one current linked worktree root".to_owned()),
            Some(worktree.to_owned()),
            &format!("restore the producing worktree and retry: {error}"),
        )
    })?;
    if repository.worktree_root() != Path::new(worktree) {
        return Err(context.pend_again().refusal(
            "trust::worktree-not-root",
            Some(repository.worktree_root().display().to_string()),
            Some(worktree.to_owned()),
            "rerun `spice trust pend` from the canonical producing worktree root",
        ));
    }
    if repository.common_dir() != resident_repository.common_dir() {
        return Err(context.pend_again().refusal(
            "trust::foreign-worktree",
            Some(resident_repository.common_dir().display().to_string()),
            Some(repository.common_dir().display().to_string()),
            "run `spice trust pend` through the resident that owns the producing repository",
        ));
    }
    Ok(repository)
}

fn trust_head_origin(bus: &Bus, head: &JournalHead<TrustHead>) -> SurfaceResult<wire::Origin> {
    let page = bus
        .app_facts(
            journal::FactPageStart::At(head.position()),
            head.position(),
            journal::FactReadBudget::new(1, journal::MAX_RETAINED_FACT_PAGE_BYTES)
                .expect("one trust-head fact is a repository budget"),
        )
        .map_err(|error| typed_store_fault("trust::journal-unreadable", &error.to_string()))?;
    page.rows
        .into_iter()
        .next()
        .filter(|fact| fact.seq == head.position())
        .map(|fact| fact.origin.clone())
        .ok_or_else(|| {
            typed_store_fault(
                "trust::head-fact-missing",
                &format!(
                    "trust head position {} is absent from its exact bounded fact page",
                    head.position()
                ),
            )
        })
}

fn admit_trust_head(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    reference: &TrustRef,
    from: TrustState,
    context: &TrustRefusalContext,
) -> SurfaceResult<JournalHead<TrustHead>> {
    let head = bus
        .trust_head(&reference.digest)
        .map_err(|error| typed_store_fault("trust::head-unreadable", &error.to_string()))?
        .ok_or_else(|| {
            context.pend_again().refusal(
                "trust::digest-unknown",
                Some(reference.digest.to_string()),
                None,
                "restart the resident to pend the exact current digest before ruling",
            )
        })?;
    if head.body().state != from {
        let context = match head.body().worktree.as_deref() {
            Some(worktree) => context.with_worktree(worktree),
            None => context.clone(),
        };
        return Err(context.report().refusal(
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
        let context = match head.body().worktree.as_deref() {
            Some(worktree) => context.with_worktree(worktree),
            None => context.clone(),
        };
        return Err(context.refusal(
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
    context: &TrustRefusalContext,
) -> SurfaceResult<()> {
    let residency = bus
        .residency_head()
        .map_err(|error| typed_store_fault("resident::head-unreadable", &error.to_string()))?
        .ok_or_else(|| typed_store_fault("resident::head-missing", "residency head is absent"))?;
    if command.epoch() == epoch && residency.body().epoch == epoch {
        return Ok(());
    }
    Err(context.refusal(
        "resident::epoch-moved",
        Some(epoch.to_string()),
        Some(format!(
            "request {}, resident {}, durable head {}",
            command.epoch(),
            epoch,
            residency.body().epoch
        )),
        "retry the exact trust command through the current resident discovery",
    ))
}

fn trust_guard(
    common: PathBuf,
    context: TrustRefusalContext,
    surface: crate::trust_command::Surface,
    digest: wire::Digest,
    from: TrustState,
    epoch: CanonicalUuid,
) -> JournalGuard {
    let worktree = context
        .worktree
        .clone()
        .expect("admitted trust ruling has a producing worktree");
    JournalGuard::new(move |read| {
        let repository = guarded_trust_worktree(&common, &worktree, &context)?;
        super::require_current_surface(&repository, &surface).map_err(|_| {
            context.pend_again().guard_refusal(
                "trust::surface-moved",
                Some(surface.digest().to_string()),
                None,
                "recapture and pend the current executable policy before ruling",
            )
        })?;
        let current = read.trust_head(&digest)?.ok_or_else(|| {
            context.pend_again().guard_refusal(
                "trust::digest-unknown",
                Some(digest.to_string()),
                None,
                "pend the current executable-policy digest before ruling",
            )
        })?;
        if current.body().state != from {
            return Err(context.report().guard_refusal(
                "trust::standing-moved",
                Some(format!("{from:?}").to_lowercase()),
                Some(format!("{:?}", current.body().state).to_lowercase()),
                "read the current trust report before issuing another ruling",
            ));
        }
        if current.body().worktree.as_deref() != Some(worktree.as_str()) {
            return Err(context.report().guard_refusal(
                "trust::worktree-moved",
                Some(worktree.clone()),
                current.body().worktree.clone(),
                "read the current trust report before issuing another ruling",
            ));
        }
        let residency = read
            .residency_head()?
            .ok_or_else(|| GuardError::refused("resident::head-missing"))?;
        if residency.body().epoch != epoch {
            return Err(context.guard_refusal(
                "resident::epoch-moved",
                Some(epoch.to_string()),
                Some(residency.body().epoch.to_string()),
                "retry the exact trust ruling through the current resident discovery",
            ));
        }
        Ok(())
    })
}

fn trust_pend_guard(
    common: PathBuf,
    context: TrustRefusalContext,
    surface: crate::trust_command::Surface,
    digest: wire::Digest,
    epoch: CanonicalUuid,
) -> JournalGuard {
    let worktree = context
        .worktree
        .clone()
        .expect("admitted trust pend has a producing worktree");
    JournalGuard::new(move |read| {
        let repository = guarded_trust_worktree(&common, &worktree, &context)?;
        super::require_current_surface(&repository, &surface).map_err(|_| {
            context.guard_refusal(
                "trust::surface-moved",
                Some(surface.digest().to_string()),
                None,
                "recapture and pend the current executable policy",
            )
        })?;
        if read.trust_head(&digest)?.is_some() {
            return Err(context.report().guard_refusal(
                "trust::digest-already-pended",
                Some("one previously unseen current digest".to_owned()),
                Some(digest.to_string()),
                "read the current trust report before issuing another ruling",
            ));
        }
        let residency = read
            .residency_head()?
            .ok_or_else(|| GuardError::refused("resident::head-missing"))?;
        if residency.body().epoch != epoch {
            return Err(context.guard_refusal(
                "resident::epoch-moved",
                Some(epoch.to_string()),
                Some(residency.body().epoch.to_string()),
                "retry trust pend through the current resident discovery",
            ));
        }
        Ok(())
    })
}

fn guarded_trust_worktree(
    common: &Path,
    worktree: &str,
    context: &TrustRefusalContext,
) -> Result<Repository, GuardError> {
    let repository = Repository::discover(worktree).map_err(|error| {
        context.pend_again().guard_refusal(
            "trust::worktree-unresolved",
            Some("one current linked worktree root".to_owned()),
            Some(worktree.to_owned()),
            &format!("restore the producing worktree before retrying: {error}"),
        )
    })?;
    if repository.worktree_root() != Path::new(worktree) {
        return Err(context.pend_again().guard_refusal(
            "trust::worktree-not-root",
            Some(repository.worktree_root().display().to_string()),
            Some(worktree.to_owned()),
            "rerun trust pend from the canonical producing worktree root",
        ));
    }
    if repository.common_dir() != common {
        return Err(context.pend_again().guard_refusal(
            "trust::foreign-worktree",
            Some(common.display().to_string()),
            Some(repository.common_dir().display().to_string()),
            "run trust pend through the resident that owns the producing repository",
        ));
    }
    Ok(repository)
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
