//! The operator's newest return decision as an active Design's next input.
//!
//! A return records why the operator sent one exact candidate version back.
//! Each active Design (open and standing in a working group) whose claim an
//! occupied lane holds is owed its newest return until that exact occupant's
//! supervisor confirms enqueue and the resident commits `room::relayed` for
//! the still-current claim. Failed or stale attempts remain owed. A holder
//! present at the return hears it, a Design returned while unheld is told to
//! its next claimant, a passive Design wakes and tells no one until a later
//! group entry leads to a claim, and a completed delivery survives restart.
//! Only the newest return is input; older ones stay history.

use std::collections::{BTreeMap, BTreeSet};

use super::*;
use wire::CanonicalUuid;
use wire::rooms::{
    DecisionOutcome, DecisionRef, DesignReturnDelivered, PreparationDecided, PreparationDecision,
    RoomAuthorityStamp,
};

/// One return an occupied holder of an active Design is owed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReturnOwed {
    pub(crate) lane: CanonicalUuid,
    pub(crate) occupant: CanonicalUuid,
    /// Fact that won the claim the delivery serves.
    pub(crate) claim: Seq,
    pub(crate) design: TaskHandle,
    pub(crate) decision: DecisionRef,
    pub(crate) candidate: RoomAuthorityStamp,
    pub(crate) reason: String,
}

/// Every return owed to the occupied holders in `occupied` (lane to its
/// current occupant), in Design order.
pub(crate) fn owed_returns(
    bus: &Bus,
    occupied: &BTreeMap<CanonicalUuid, CanonicalUuid>,
) -> Result<Vec<ReturnOwed>, Outcome<CommandAnswer>> {
    let snapshot = bus
        .task_snapshot()
        .map_err(|error| store_fault("room::return-snapshot-unreadable", &error.to_string()))?;
    let mut owed = Vec::new();
    let mut since: Option<Seq> = None;
    for heads in snapshot.tasks() {
        let statement = heads.statement().body();
        let workflow = heads.workflow().body();
        let Some(claim) = heads.claim().map(|head| head.body()) else {
            continue;
        };
        let Some(occupant) = occupied.get(&claim.lane) else {
            continue;
        };
        if statement.intent != Some(wire::TaskIntent::Design)
            || workflow.status != "pending"
            || workflow.completion.is_some()
            || claim.disposition != ClaimDisposition::Active
            || !snapshot.facts().grouped(&statement.born)
        {
            continue;
        }
        let Some((seq, decision)) = newest_return(bus, &statement.born)? else {
            continue;
        };
        since = Some(since.map_or(seq, |since| since.min(seq)));
        let DecisionOutcome::Returned { candidate, reason } = decision.outcome else {
            unreachable!("newest_return yields only returns");
        };
        owed.push(ReturnOwed {
            lane: claim.lane,
            occupant: *occupant,
            claim: claim.claimed,
            design: TaskHandle::for_project(&statement.project, &statement.born),
            decision: decision.decision,
            candidate: candidate.candidate,
            reason,
        });
    }
    let Some(since) = since else {
        return Ok(owed);
    };
    let delivered = delivered(bus, since)?;
    owed.retain(|owed| !delivered.contains(&owed.decision));
    Ok(owed)
}

/// Commit the durable record that consumes one owed return after its exact
/// occupant's supervisor has confirmed enqueue.
pub(crate) fn record_return(bus: &Bus, owed: &ReturnOwed) -> Result<(), Outcome<CommandAnswer>> {
    let at = current_instant()?;
    let origin = format!("lane:{}", owed.lane)
        .parse::<wire::Origin>()
        .map_err(|error| store_fault("room::return-origin-invalid", &error.to_string()))?;
    let mut write = JournalWrite::new(Actor::Spice, origin, at, Producer::ResidentRecognition);
    write
        .fact(
            DesignReturnDelivered {
                decision: owed.decision.clone(),
                lane: owed.lane,
                occupant: owed.occupant,
                claim: wire::SequenceCode::new(owed.claim.get()),
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("room::return-build", &error.to_string()))?;
    match bus
        .commit(write)
        .map_err(|error| store_fault("room::return-commit", &error.to_string()))?
    {
        CommitOutcome::Committed(_) => Ok(()),
        CommitOutcome::Refused(refusal) => {
            Err(store_fault("room::return-refused", &refusal.to_string()))
        }
        CommitOutcome::Fault(fault) => Err(store_fault("room::return-fault", &fault.to_string())),
    }
}

/// `design`'s newest return decision and its position, if any.
fn newest_return(
    bus: &Bus,
    design: &TaskStamp,
) -> Result<Option<(Seq, PreparationDecision)>, Outcome<CommandAnswer>> {
    let design = design.clone();
    bus.read(move |read| {
        let mut newest = None;
        let mut after = None;
        loop {
            let page =
                read.preparation_decisions(&design, after, journal::FactReadBudget::RETAINED)?;
            for envelope in page.rows {
                if let Some(decided) = envelope.fact.read::<PreparationDecided>()
                    && matches!(decided.decision.outcome, DecisionOutcome::Returned { .. })
                {
                    newest = Some((envelope.seq, decided.decision));
                }
            }
            match page.continuation {
                Some(continuation) => after = Some(continuation),
                None => return Ok(newest),
            }
        }
    })
    .map_err(|error| store_fault("room::return-unreadable", &error.to_string()))
}

/// Every return decision already delivered at or after `since`.
fn delivered(bus: &Bus, since: Seq) -> Result<BTreeSet<DecisionRef>, Outcome<CommandAnswer>> {
    bus.read(move |read| {
        let mut delivered = BTreeSet::new();
        let mut after = since.get().checked_sub(1).and_then(Seq::new);
        loop {
            let page = read.selected_fact_page(
                &[wire::FactKind::DesignReturnDelivered],
                after,
                None,
                journal::FactReadBudget::RETAINED,
            )?;
            delivered.extend(
                page.rows
                    .iter()
                    .filter_map(|envelope| envelope.fact.read::<DesignReturnDelivered>())
                    .map(|body| body.decision),
            );
            match page.continuation {
                Some(continuation) => after = Some(continuation),
                None => return Ok(delivered),
            }
        }
    })
    .map_err(|error| store_fault("room::return-unreadable", &error.to_string()))
}
