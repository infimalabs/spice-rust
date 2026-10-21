//! Park a lane's held claim on the trust grant its own worktree pended.
//!
//! An executable-configuration trust refusal is the one point where lane
//! automation stops for the operator. When an occupant's turn ends while its
//! lane holds a claim and the lane's worktree still presents a digest it pended
//! and nobody granted as its current executable-configuration surface, the held
//! task acquires that grant as its trust requirement and its live lease is iced
//! on the indefinite horizon, exactly as `task require --trust-grant` records
//! them. A pended head stays pended after its worktree moves on, so only the
//! current surface names a live refusal. The held-claim wake projection already
//! parks a claim on an unsatisfied grant and thaws it once the grant lands, so
//! nothing wakes the lane for that task until the operator rules.

use super::automatic::{guard_held_claim, held_claim_authority};
use super::*;
use journal::JournalHead;
use wire::{EnvironmentRequirement, EnvironmentRequirements, TrustHead, TrustState};

/// Record `worktree`'s current surface digest as the held task's trust
/// requirement when that worktree pended it and it is still ungranted.
///
/// Answers whether a requirement was recorded. `surface` reads the worktree's
/// current digest and runs only once a live claim and a pended head from this
/// worktree exist. A lane without an active claim, a claim whose lease already
/// expired, a current surface this worktree has not pended or that is no
/// longer pended, a task already requiring that digest, and a task already
/// parked on another worktree's ungranted digest all record nothing.
pub(in crate::resident) fn park_on_pended_grant(
    bus: &Bus,
    lane: CanonicalUuid,
    worktree: &str,
    surface: impl FnOnce() -> CommandResult<wire::Digest>,
    at: wire::Instant,
) -> CommandResult<bool> {
    let snapshot = bus
        .task_snapshot()
        .map_err(|error| store_fault("lifecycle::claim-heads-unreadable", &error.to_string()))?;
    let Some(authority) = held_claim_authority(bus, Some(&snapshot), lane)? else {
        return Ok(false);
    };
    let lease = authority.lease.body();
    if lease.expiry <= at {
        return Ok(false);
    }
    let pended_here = |head: &JournalHead<TrustHead>| {
        head.body().state == TrustState::Pended && head.body().worktree.as_deref() == Some(worktree)
    };
    if !snapshot.trust().iter().any(pended_here) {
        return Ok(false);
    }
    let surface = surface()?;
    let statement = authority.statement.body();
    let Some(requirements) = parked(
        &statement.requirements,
        snapshot.trust(),
        worktree,
        &surface,
    ) else {
        return Ok(false);
    };
    let iced = (!spice::claim_lease::is_indefinite(lease)).then(|| spice::claim_lease::iced(lease));
    let origin = format!("task:{}", authority.task)
        .parse::<wire::Origin>()
        .map_err(|error| store_fault("task::origin-invalid", &error.to_string()))?;
    let mut write = JournalWrite::new(Actor::Spice, origin, at, Producer::CommandDoor);
    guard_held_claim(&mut write, &authority);
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
        .map_err(|error| store_fault("task::pended-grant-build", &error.to_string()))?;
    let statement = wire::TaskHead {
        requirements,
        ..statement.clone()
    };
    write.task_head(edited, move |position| wire::TaskHead {
        revision: position,
        ..statement
    });
    if let Some(iced) = iced {
        write.lease_head(authority.task.clone(), edited, move |_| iced);
    }
    commit(bus, write).map(|_| true)
}

/// The held task's requirements with its trust slot naming `surface`, the
/// worktree's current digest, when `worktree` pended it and it is still
/// ungranted, or `None` when nothing should change.
///
/// Requirements hold one entry per environmental fact, so the trust slot names
/// one digest. The current digest replaces an older one from the same worktree,
/// and a granted digest no longer parks anything, but an ungranted digest from
/// another worktree already parks the task on a grant this lane did not cause.
fn parked(
    requirements: &EnvironmentRequirements,
    trust: &[JournalHead<TrustHead>],
    worktree: &str,
    surface: &wire::Digest,
) -> Option<EnvironmentRequirements> {
    let ours = |head: &TrustHead| head.worktree.as_deref() == Some(worktree);
    let pended = &trust
        .iter()
        .map(JournalHead::body)
        .find(|head| &head.digest == surface)
        .filter(|head| head.state == TrustState::Pended && ours(head))?
        .digest;
    let current = requirements
        .as_slice()
        .iter()
        .find_map(|requirement| match requirement {
            EnvironmentRequirement::TrustGrant { value } => Some(value),
            _ => None,
        });
    if let Some(current) = current {
        let replaceable = trust
            .iter()
            .find(|head| &head.body().digest == current)
            .is_some_and(|head| head.body().state == TrustState::Granted || ours(head.body()));
        if current == pended || !replaceable {
            return None;
        }
    }
    let mut next = requirements
        .as_slice()
        .iter()
        .filter(|requirement| !matches!(requirement, EnvironmentRequirement::TrustGrant { .. }))
        .cloned()
        .collect::<Vec<_>>();
    next.push(EnvironmentRequirement::trust_grant(pended.clone()));
    next.sort_by_key(EnvironmentRequirement::fact);
    EnvironmentRequirements::new(next).ok()
}
