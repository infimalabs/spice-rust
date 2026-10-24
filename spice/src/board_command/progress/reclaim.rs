//! Exact renewal and same-team recovery for one active task claim.

use super::*;

pub(super) struct ReclaimWindow {
    until: Instant,
    lease: LeaseHead,
    residency: ReclaimResidency,
    /// The lease sits exactly at the indefinite approval horizon, so this
    /// reclaim acknowledges the generation and moves no expiry.
    held: bool,
}

#[derive(Clone)]
enum ReclaimResidency {
    Exact,
    Rebound(Rebound),
}

/// The seat a same-team recovery moves the claim onto.
#[derive(Clone)]
struct Rebound {
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
    ancestor: wire::CanonicalUuid,
    epoch: wire::CanonicalUuid,
    source: body::SourceBoundary,
}

impl ReclaimWindow {
    pub(super) const fn expiry(&self) -> Instant {
        self.until
    }

    pub(super) const fn held(&self) -> bool {
        self.held
    }

    pub(super) const fn producer(&self) -> Producer {
        match &self.residency {
            ReclaimResidency::Exact => Producer::BusTimer,
            ReclaimResidency::Rebound(_) => Producer::CommandDoor,
        }
    }
}

pub(super) fn reclaim_entry<'a>(
    snapshot: &'a ValidatedSnapshot,
    renew: &ClaimRenew,
) -> Result<&'a Entry, Outcome<CommandAnswer>> {
    task_entry(&snapshot.entries, &renew.task).ok_or_else(|| {
        refusal(
            "task::unknown",
            Some(renew.task.to_string()),
            "read board and reclaim one current task handle",
        )
    })
}

pub(super) fn reclaim_window(
    bus: &Bus,
    entry: &Entry,
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    renew: &ClaimRenew,
    at: Instant,
) -> Result<ReclaimWindow, Outcome<CommandAnswer>> {
    let claim = active_claim(entry, command, at)?;
    let lease = entry.lease().ok_or_else(|| {
        protocol_fault(
            "claim::lease-missing",
            "an active claim head has no durable lease head",
        )
    })?;
    let residency = reclaim_residency(bus, snapshot, command, renew, claim, lease)?;
    let seconds = i64::try_from(renew.lease_seconds)
        .ok()
        .filter(|seconds| *seconds > 0)
        .ok_or_else(|| {
            refusal(
                "claim::lease-seconds-invalid",
                Some(renew.lease_seconds.to_string()),
                "request a positive representable lease duration",
            )
        })?;
    let requested = instant_after(at, seconds)?;
    // A claim iced on human approval holds its lease at the indefinite
    // horizon, which no request can pass. An exact reclaim of that hold is
    // the holder acknowledging the task's generation, so it is admitted and
    // leaves the lease exactly where the hold put it: settling it to the
    // ordinary maximum would end the approval hold an hour later.
    if matches!(residency, ReclaimResidency::Exact)
        && lease.expiry == spice::claim_lease::indefinite_horizon()
    {
        return Ok(ReclaimWindow {
            until: lease.expiry,
            lease: lease.clone(),
            residency,
            held: true,
        });
    }
    // Every accepted holder reclaim gets a fresh one-hour maximum. This keeps
    // the automatic-claim horizon intact while letting a manual claim roll the
    // same horizon forward without ownership churn.
    let maximum = std::cmp::max(lease.maximum, instant_after(at, RECLAIM_HORIZON_SECONDS)?);
    let requested = std::cmp::min(requested, maximum);
    let until = match residency {
        ReclaimResidency::Exact if requested <= lease.expiry => {
            return Err(refusal(
                "claim::lease-nonextending",
                Some(lease.expiry.to_string()),
                "request a duration that extends the current expiry",
            ));
        }
        ReclaimResidency::Rebound(_) => std::cmp::max(requested, lease.expiry),
        ReclaimResidency::Exact => requested,
    };
    let mut lease = lease.clone();
    lease.maximum = maximum;
    Ok(ReclaimWindow {
        until,
        lease,
        residency,
        held: false,
    })
}

fn active_claim<'a>(
    entry: &'a Entry,
    command: &wire::AdmittedCommand,
    at: Instant,
) -> Result<&'a ClaimHead, Outcome<CommandAnswer>> {
    let claim = entry.claim().ok_or_else(|| {
        reclaim_authority_refusal("claim::head-missing", Some(entry.handle().to_string()))
    })?;
    let Actor::Lane(_) = command.actor() else {
        return Err(refusal(
            "claim::lane-required",
            Some(command.actor().to_string()),
            "reclaim through the lane capability that owns the claim",
        ));
    };
    if claim.disposition != ClaimDisposition::Active || claim.actor != *command.actor() {
        return Err(reclaim_authority_refusal(
            "claim::ownership-refused",
            Some(claim.actor.to_string()),
        ));
    }
    board::Active::admit(entry, command.actor(), at).map_err(|error| {
        reclaim_authority_refusal("claim::active-refused", Some(error.to_string()))
    })?;
    Ok(claim)
}

fn reclaim_residency(
    bus: &Bus,
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    renew: &ClaimRenew,
    claim: &ClaimHead,
    lease: &LeaseHead,
) -> Result<ReclaimResidency, Outcome<CommandAnswer>> {
    let Actor::Lane(lane) = command.actor() else {
        unreachable!("active claim admission requires a lane actor")
    };
    let residency = &snapshot.answer.residency.body;
    let seat = residency.lanes.iter().find(|seat| seat.lane == *lane);
    let occupant = seat
        .filter(|seat| seat.disposition == "occupied")
        .and_then(|seat| seat.occupant);
    if claim.lane != *lane || residency.epoch != command.epoch() || occupant.is_none() {
        return Err(moved_refusal(renew));
    }
    let occupant = occupant.expect("occupied current seat has an occupant");
    if claim.epoch == command.epoch()
        && lease.epoch == command.epoch()
        && occupant == claim.occupant
    {
        return Ok(ReclaimResidency::Exact);
    }
    if claim.epoch != lease.epoch {
        return Err(protocol_fault(
            "claim::lease-epoch-mismatch",
            "active claim and lease generations disagree before recovery",
        ));
    }
    if !seat.is_some_and(|seat| seat.lineage.contains(&claim.occupant)) {
        return Err(moved_refusal(renew));
    }
    let source = bus
        .source_boundary(lane)
        .map_err(|error| store_fault("claim::rebound-boundary-unreadable", &error.to_string()))?;
    Ok(ReclaimResidency::Rebound(Rebound {
        lane: *lane,
        occupant,
        ancestor: claim.occupant,
        epoch: command.epoch(),
        source,
    }))
}

fn moved_refusal(renew: &ClaimRenew) -> Outcome<CommandAnswer> {
    reclaim_authority_refusal("claim::residency-moved", Some(renew.task.to_string()))
}

pub(super) fn append_reclaim(
    write: &mut JournalWrite,
    entry: &Entry,
    command: &wire::AdmittedCommand,
    at: Instant,
    window: ReclaimWindow,
) -> Result<(), Outcome<CommandAnswer>> {
    let ReclaimWindow {
        until,
        lease,
        residency,
        held: _,
    } = window;
    match residency {
        ReclaimResidency::Exact => append_renewal(write, entry, command, at, until, lease),
        ReclaimResidency::Rebound(rebound) => {
            append_rebound(write, entry, at, until, lease, rebound)
        }
    }
}

fn append_renewal(
    write: &mut JournalWrite,
    entry: &Entry,
    command: &wire::AdmittedCommand,
    at: Instant,
    until: Instant,
    mut lease: LeaseHead,
) -> Result<(), Outcome<CommandAnswer>> {
    let renewed = write
        .fact(
            body::ClaimRenewed {
                task: entry.born().clone(),
                epoch: command.epoch(),
                until,
                maximum: lease.maximum,
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("claim::reclaim-build", &error.to_string()))?;
    lease.renewal = at;
    lease.expiry = until;
    write.lease_head(entry.born().clone(), renewed, move |_| lease);
    acknowledge(write, entry, renewed);
    Ok(())
}

/// Record that this explicit reclaim acknowledges the task's generation.
///
/// Only a stale claim is rewritten, so reclaiming a current claim remains the
/// lease-only renewal it always was.
fn acknowledge(write: &mut JournalWrite, entry: &Entry, renewed: journal::PendingFact) {
    let claim = entry
        .claim()
        .expect("reclaim admission requires a claim")
        .clone();
    if !claim.is_stale(entry.workflow()) {
        return;
    }
    let acknowledged = entry.workflow().generation;
    write.claim_head(entry.born().clone(), renewed, move |fact| ClaimHead {
        fact,
        acknowledged,
        ..claim
    });
}

fn append_rebound(
    write: &mut JournalWrite,
    entry: &Entry,
    at: Instant,
    until: Instant,
    mut lease: LeaseHead,
    rebound: Rebound,
) -> Result<(), Outcome<CommandAnswer>> {
    let Rebound {
        lane,
        occupant,
        ancestor,
        epoch,
        source,
    } = rebound;
    let rebound = write
        .fact(
            body::ClaimRebound {
                task: entry.born().clone(),
                lane,
                occupant,
                ancestor,
                epoch,
                until,
                maximum: lease.maximum,
                source,
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("claim::rebound-build", &error.to_string()))?;
    let mut claim = entry
        .claim()
        .expect("rebound admission requires a claim")
        .clone();
    claim.occupant = occupant;
    claim.epoch = epoch;
    claim.acknowledged = entry.workflow().generation;
    write.claim_head(entry.born().clone(), rebound, move |fact| ClaimHead {
        fact,
        ..claim
    });
    lease.renewal = at;
    lease.expiry = until;
    lease.epoch = epoch;
    write.lease_head(entry.born().clone(), rebound, move |_| lease);
    Ok(())
}
