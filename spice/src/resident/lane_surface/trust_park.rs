//! Keep a lane's held claim parked exactly while its worktree needs an
//! ungranted executable configuration.
//!
//! The configured gate checks trust only when the worktree's configuration
//! declares pre-commit commands, and a lane can neither pend nor grant. So
//! when an occupant's turn ends while its lane holds a live claim, the resident
//! reads that worktree's current trust-gated digest. An ungranted digest is
//! pended for the operator when it has no trust head, becomes the held task's
//! trust requirement, and ices the live lease, exactly as `task require
//! --trust-grant` records them; the wake projection then leaves the claim
//! alone until the grant lands. Once the worktree no longer needs an ungranted
//! digest, for example because the lane backed its change out, a requirement
//! naming a digest this worktree pended is dropped, and the indefinite lease
//! thaws on the claim's next wake.

use super::automatic::{HeldClaimAuthority, guard_held_claim, held_claim_authority};
use super::*;
use journal::{GuardError, JournalHead};
use wire::{Digest, EnvironmentRequirement, EnvironmentRequirements, TrustHead, TrustState};

/// Reconcile the held claim of `lane` on `worktree` with that worktree's
/// current trust need, answering whether the held task's requirements changed.
///
/// `need` reads the worktree's current executable-configuration digest when
/// its configuration declares pre-commit commands, or `None` when it declares
/// none; it runs only once `lane` holds a live claim.
pub(in crate::resident) fn reconcile_trust_park(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    worktree: &str,
    need: impl FnOnce() -> CommandResult<Option<Digest>>,
    at: wire::Instant,
) -> CommandResult<bool> {
    let snapshot = bus
        .task_snapshot()
        .map_err(|error| store_fault("lifecycle::claim-heads-unreadable", &error.to_string()))?;
    let Some(authority) = held_claim_authority(bus, Some(&snapshot), lane)? else {
        return Ok(false);
    };
    if authority.lease.body().expiry <= at {
        return Ok(false);
    }
    let need = need()?;
    let requirements = &authority.statement.body().requirements;
    let required = trust_requirement(requirements);
    let Some(change) = decide(required, snapshot.trust(), worktree, need.as_ref()) else {
        return Ok(false);
    };
    let next = match change {
        Change::Park(digest) => {
            if !snapshot
                .trust()
                .iter()
                .any(|head| head.body().digest == digest)
            {
                pend(bus, epoch, worktree, &digest, at)?;
            }
            with_trust(requirements, Some(digest))
        }
        Change::Drop => with_trust(requirements, None),
    }?;
    rewrite(bus, &authority, next, at).map(|()| true)
}

/// How the held task's trust slot moves at a turn end.
#[derive(Debug, Eq, PartialEq)]
enum Change {
    /// Name the worktree's ungranted current digest.
    Park(Digest),
    /// Remove a digest this worktree pended and no longer needs.
    Drop,
}

/// The move from the task's `required` trust digest given the worktree's
/// current trust-gated digest `need`, or `None` when nothing should change.
///
/// Requirements hold one entry per environmental fact, so the trust slot names
/// one digest. An ungranted need replaces the worktree's own older digest or a
/// granted one, but never an ungranted digest another worktree pended, which
/// already parks the task on a grant this lane did not cause. A need that is
/// granted or absent drops only a digest this worktree pended.
fn decide(
    required: Option<&Digest>,
    trust: &[JournalHead<TrustHead>],
    worktree: &str,
    need: Option<&Digest>,
) -> Option<Change> {
    let head = |digest: &Digest| {
        trust
            .iter()
            .map(JournalHead::body)
            .find(|head| &head.digest == digest)
    };
    let ours = |digest: &Digest| {
        head(digest).is_some_and(|head| head.worktree.as_deref() == Some(worktree))
    };
    let granted =
        |digest: &Digest| head(digest).is_some_and(|head| head.state == TrustState::Granted);
    match need.filter(|digest| !granted(digest)) {
        Some(blocking) => {
            let foreign = required
                .is_some_and(|digest| digest != blocking && !granted(digest) && !ours(digest));
            (required != Some(blocking) && !foreign).then(|| Change::Park(blocking.clone()))
        }
        None => required
            .filter(|digest| Some(*digest) != need && ours(digest))
            .map(|_| Change::Drop),
    }
}

fn trust_requirement(requirements: &EnvironmentRequirements) -> Option<&Digest> {
    requirements
        .as_slice()
        .iter()
        .find_map(|requirement| match requirement {
            EnvironmentRequirement::TrustGrant { value } => Some(value),
            _ => None,
        })
}

/// `requirements` with its trust slot replaced by `trust`.
fn with_trust(
    requirements: &EnvironmentRequirements,
    trust: Option<Digest>,
) -> CommandResult<EnvironmentRequirements> {
    let mut next = requirements
        .as_slice()
        .iter()
        .filter(|requirement| !matches!(requirement, EnvironmentRequirement::TrustGrant { .. }))
        .cloned()
        .chain(trust.map(EnvironmentRequirement::trust_grant))
        .collect::<Vec<_>>();
    next.sort_by_key(EnvironmentRequirement::fact);
    EnvironmentRequirements::new(next)
        .map_err(|error| store_fault("task::trust-park-invalid", &error.to_string()))
}

/// Pend `digest` for `worktree` so the operator sees the decision it awaits.
fn pend(
    bus: &Bus,
    epoch: CanonicalUuid,
    worktree: &str,
    digest: &Digest,
    at: wire::Instant,
) -> CommandResult<()> {
    let previous = crate::resident::latest_surface_digest(bus)?;
    let origin = format!("resident:{epoch}")
        .parse::<wire::Origin>()
        .map_err(|error| store_fault("trust::origin-invalid", &error.to_string()))?;
    let mut write = JournalWrite::new(Actor::Spice, origin, at, Producer::CommandDoor);
    let guarded = digest.clone();
    write.guard(JournalGuard::new(move |read| {
        if read.trust_head(&guarded)?.is_some() {
            return Err(GuardError::refused("trust::digest-already-pended"));
        }
        Ok(())
    }));
    let pended = write
        .fact(
            wire::body::TrustPended {
                surface: crate::trust_command::SURFACE_NAME.to_owned(),
                digest: digest.clone(),
                worktree: Some(worktree.to_owned()),
                previous,
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("trust::fact-invalid", &error.to_string()))?;
    let (digest, worktree) = (digest.clone(), worktree.to_owned());
    write.trust_head(pended, move |_| TrustHead {
        digest,
        state: TrustState::Pended,
        worktree: Some(worktree),
    });
    commit(bus, write).map(|_| ())
}

/// Record `requirements` on the held task, icing a finite lease when they
/// carry an unsatisfied trust grant.
fn rewrite(
    bus: &Bus,
    authority: &HeldClaimAuthority,
    requirements: EnvironmentRequirements,
    at: wire::Instant,
) -> CommandResult<()> {
    let lease = authority.lease.body();
    let iced = (trust_requirement(&requirements).is_some()
        && !spice::claim_lease::is_indefinite(lease))
    .then(|| spice::claim_lease::iced(lease));
    let origin = format!("task:{}", authority.task)
        .parse::<wire::Origin>()
        .map_err(|error| store_fault("task::origin-invalid", &error.to_string()))?;
    let mut write = JournalWrite::new(Actor::Spice, origin, at, Producer::CommandDoor);
    guard_held_claim(&mut write, authority);
    let revision = u32::try_from(authority.statement.position().get())
        .unwrap_or(u32::MAX)
        .saturating_add(1);
    let edited = write
        .fact(
            wire::body::TaskEdited {
                intent: None,
                task: authority.task.clone(),
                revision,
                dependencies: None,
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("task::trust-park-build", &error.to_string()))?;
    let statement = wire::TaskHead {
        requirements,
        ..authority.statement.body().clone()
    };
    write.task_head(edited, move |position| wire::TaskHead {
        revision: position,
        ..statement
    });
    if let Some(iced) = iced {
        write.lease_head(authority.task.clone(), edited, move |_| iced);
    }
    commit(bus, write).map(|_| ())
}
