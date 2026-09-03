//! Exact renewal and same-lineage recovery for one active task claim.

use super::*;

pub(super) struct ReclaimWindow {
    until: Instant,
    lease: LeaseHead,
    residency: ReclaimResidency,
}

#[derive(Clone)]
enum ReclaimResidency {
    Exact,
    Rebound {
        lane: wire::CanonicalUuid,
        occupant: wire::CanonicalUuid,
        ancestor: wire::CanonicalUuid,
        epoch: wire::CanonicalUuid,
        source: body::SourceBoundary,
    },
}

impl ReclaimWindow {
    pub(super) const fn expiry(&self) -> Instant {
        self.until
    }

    pub(super) const fn producer(&self) -> Producer {
        match &self.residency {
            ReclaimResidency::Exact => Producer::BusTimer,
            ReclaimResidency::Rebound { .. } => Producer::CommandDoor,
        }
    }
}

pub(super) fn reclaim_entry<'a>(
    snapshot: &'a ValidatedSnapshot,
    renew: &ClaimRenew,
) -> Result<&'a Entry, Outcome<CommandAnswer>> {
    snapshot
        .entries
        .iter()
        .find(|entry| entry.handle() == &renew.task)
        .ok_or_else(|| {
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
    let claim = active_claim(entry, command, renew, at)?;
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
    let automatic = snapshot
        .facts
        .iter()
        .find(|fact| fact.seq == claim.claimed)
        .is_some_and(|fact| spice::claim_lease::is_automatic_claim_fact(claim, fact));
    let maximum = if automatic {
        std::cmp::max(lease.maximum, instant_after(at, AUTOMATIC_MAXIMUM_SECONDS)?)
    } else {
        lease.maximum
    };
    let requested = std::cmp::min(instant_after(at, seconds)?, maximum);
    let until = match residency {
        ReclaimResidency::Exact if requested <= lease.expiry => {
            if automatic || lease.expiry < maximum {
                return Err(refusal(
                    "claim::lease-nonextending",
                    Some(lease.expiry.to_string()),
                    "request a duration that extends the current expiry",
                ));
            }
            let command =
                wire::SpiceCommand::new("task", ["free".to_owned(), renew.task.to_string()])
                    .expect("task free is a structural Spice recovery command");
            return Err(Outcome::Refusal(Refusal::repairable(
                command,
                wire::RefusalDiagnostic::new(
                    "claim::lease-nonextending",
                    None,
                    Some("expiry before the durable maximum".to_owned()),
                    Some(format!(
                        "expiry={} maximum={}",
                        lease.expiry, lease.maximum
                    )),
                )
                .with_detail("the claim is already at its durable maximum; free it, then run spice task next for a fresh allocation lease"),
            )));
        }
        ReclaimResidency::Rebound { .. } => std::cmp::max(requested, lease.expiry),
        ReclaimResidency::Exact => requested,
    };
    let mut lease = lease.clone();
    lease.maximum = maximum;
    Ok(ReclaimWindow {
        until,
        lease,
        residency,
    })
}

fn active_claim<'a>(
    entry: &'a Entry,
    command: &wire::AdmittedCommand,
    renew: &ClaimRenew,
    at: Instant,
) -> Result<&'a ClaimHead, Outcome<CommandAnswer>> {
    let claim = entry.claim().ok_or_else(|| {
        reclaim_authority_refusal("claim::head-missing", Some(renew.task.to_string()))
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
    Ok(ReclaimResidency::Rebound {
        lane: *lane,
        occupant,
        ancestor: claim.occupant,
        epoch: command.epoch(),
        source,
    })
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
    match window.residency.clone() {
        ReclaimResidency::Exact => append_renewal(write, entry, command, at, window),
        ReclaimResidency::Rebound {
            lane,
            occupant,
            ancestor,
            epoch,
            source,
        } => append_rebound(
            write, entry, at, window, lane, occupant, ancestor, epoch, source,
        ),
    }
}

fn append_renewal(
    write: &mut JournalWrite,
    entry: &Entry,
    command: &wire::AdmittedCommand,
    at: Instant,
    window: ReclaimWindow,
) -> Result<(), Outcome<CommandAnswer>> {
    let renewed = write
        .fact(
            body::ClaimRenewed {
                task: entry.born().clone(),
                epoch: command.epoch(),
                until: window.until,
                maximum: window.lease.maximum,
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("claim::reclaim-build", &error.to_string()))?;
    let mut lease = window.lease;
    lease.renewal = at;
    lease.expiry = window.until;
    write.lease_head(entry.born().clone(), renewed, move |_| lease);
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    reason = "one rebound fact updates one claim and lease boundary"
)]
fn append_rebound(
    write: &mut JournalWrite,
    entry: &Entry,
    at: Instant,
    window: ReclaimWindow,
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
    ancestor: wire::CanonicalUuid,
    epoch: wire::CanonicalUuid,
    source: body::SourceBoundary,
) -> Result<(), Outcome<CommandAnswer>> {
    let rebound = write
        .fact(
            body::ClaimRebound {
                task: entry.born().clone(),
                lane,
                occupant,
                ancestor,
                epoch,
                until: window.until,
                maximum: window.lease.maximum,
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
    write.claim_head(entry.born().clone(), rebound, move |fact| ClaimHead {
        fact,
        ..claim
    });
    let mut lease = window.lease;
    lease.renewal = at;
    lease.expiry = window.until;
    lease.epoch = epoch;
    write.lease_head(entry.born().clone(), rebound, move |_| lease);
    Ok(())
}
