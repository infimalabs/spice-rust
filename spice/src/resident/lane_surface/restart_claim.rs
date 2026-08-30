//! Claim authority transferred while a resident generation restores its seats.

use super::*;

#[derive(Clone)]
pub(super) struct RestoredClaim {
    task: wire::TaskStamp,
    claim: journal::JournalHead<wire::ClaimHead>,
    lease: journal::JournalHead<wire::LeaseHead>,
    source: wire::body::SourceBoundary,
}

pub(super) fn observe_restored_claim(
    bus: &Bus,
    durable: &wire::LaneResidency,
) -> CommandResult<Option<RestoredClaim>> {
    let Some(ancestor) = durable.occupant else {
        return Ok(None);
    };
    let at = wire::Instant::new(jiff::Timestamp::now())
        .map_err(|error| store_fault("lane::restart-clock-invalid", &error.to_string()))?;
    let snapshot = bus
        .task_snapshot()
        .map_err(|error| store_fault("lane::restart-claim-heads-unreadable", &error.to_string()))?;
    let mut restored = None;
    let mut found = false;
    for heads in snapshot.tasks() {
        let Some(claim) = heads.claim() else { continue };
        if claim.body().disposition != wire::ClaimDisposition::Active {
            continue;
        }
        if claim.body().lane != durable.lane && claim.body().actor != Actor::Lane(durable.lane) {
            continue;
        }
        if found {
            return Err(store_fault(
                "lane::restart-claim-ambiguous",
                "one restored lane owns more than one active claim",
            ));
        }
        found = true;
        let actor_bound = matches!(claim.body().actor, Actor::Operator)
            || claim.body().actor == Actor::Lane(durable.lane);
        if !actor_bound || claim.body().lane != durable.lane || claim.body().occupant != ancestor {
            return Err(store_fault(
                "lane::restart-claim-lineage-moved",
                "active claim ownership differs from the restored predecessor lineage",
            ));
        }
        let lease = heads.lease().ok_or_else(|| {
            store_fault(
                "lane::restart-claim-lease-missing",
                "an active restored claim has no durable lease head",
            )
        })?;
        if lease.body().epoch != claim.body().epoch {
            return Err(store_fault(
                "lane::restart-claim-epoch-mismatch",
                "restored claim and lease generations disagree",
            ));
        }
        if at < lease.body().start || at >= lease.body().expiry {
            continue;
        }
        restored = Some(RestoredClaim {
            task: heads.statement().body().born.clone(),
            claim: claim.clone(),
            lease: lease.clone(),
            source: bus.source_boundary(&durable.lane).map_err(|error| {
                store_fault(
                    "lane::restart-claim-boundary-unreadable",
                    &error.to_string(),
                )
            })?,
        });
    }
    Ok(restored)
}

pub(super) fn guard_restored_claim(write: &mut JournalWrite, restored: &RestoredClaim) {
    let task = restored.task.clone();
    let claim = restored.claim.clone();
    let lease = restored.lease.clone();
    write.guard(JournalGuard::new(move |read| {
        if read.claim_head(&task)? != Some(claim.clone()) {
            return Err(journal::GuardError::refused(
                "lane::restart-claim-head-moved",
            ));
        }
        if read.lease_head(&task)? != Some(lease.clone()) {
            return Err(journal::GuardError::refused(
                "lane::restart-lease-head-moved",
            ));
        }
        Ok(())
    }));
}

pub(super) fn append_restored_claim(
    write: &mut JournalWrite,
    restored: &RestoredClaim,
    durable: &wire::LaneResidency,
    occupant: CanonicalUuid,
    epoch: CanonicalUuid,
    cause: PendingFact,
) -> CommandResult<PendingFact> {
    let ancestor = durable
        .occupant
        .expect("restored claims require a predecessor occupant");
    let carried = write
        .fact(
            wire::body::ClaimCarried {
                task: restored.task.clone(),
                lane: durable.lane,
                occupant,
                ancestor,
                epoch,
                source: restored.source.clone(),
            },
            FactCause::Pending(cause),
        )
        .map_err(|error| store_fault("lane::restart-claim-carry-invalid", &error.to_string()))?;
    let mut claim = restored.claim.body().clone();
    claim.occupant = occupant;
    claim.epoch = epoch;
    write.claim_head(restored.task.clone(), carried, move |position| {
        claim.fact = position;
        claim
    });
    let mut lease = restored.lease.body().clone();
    lease.epoch = epoch;
    write.lease_head(restored.task.clone(), carried, move |_| lease);
    Ok(carried)
}
