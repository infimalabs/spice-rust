//! Intent proposals for seating, drain, wake, removal, and restart commitment.

use super::restart_claim::{append_restored_claim, guard_restored_claim, observe_restored_claim};
use super::*;

/// Propose the lane, capability, and first occupant one seat admits.
pub(super) fn propose_seat(
    command: &wire::AdmittedCommand,
    request: &LaneSeat,
    observed: &journal::JournalHead<wire::ResidencyHead>,
    epoch: CanonicalUuid,
    capability: CanonicalUuid,
    answer: LaneChangeAnswer,
) -> CommandResult<JournalWrite> {
    let mut next = observed.body().clone();
    next.topology_revision = answer.topology_revision;
    if subscription_policy(observed.body(), request.lineage_id)?.is_none() {
        if observed
            .body()
            .lanes
            .iter()
            .any(|row| row.lineage_id == request.lineage_id)
        {
            return Err(store_fault(
                "lineage::subscription-missing",
                "an existing lineage has no durable task-subscription policy",
            ));
        }
        let policy = LineageLife::from_wire(&request.life).ok_or_else(|| {
            store_fault(
                "lineage::subscription-invalid",
                "a seated lane has no closed task-subscription policy",
            )
        })?;
        next.subscriptions.push(wire::LineageSubscription {
            lineage_id: request.lineage_id,
            policy,
            manual_projects: Vec::new(),
            auto_projects: Vec::new(),
        });
        next.subscriptions
            .sort_by_key(|subscription| subscription.lineage_id);
    }
    next.lanes.push(wire::LaneResidency {
        lane: request.lane,
        lineage_id: request.lineage_id,
        name: request.name.clone(),
        worktree: request.worktree.clone(),
        life: request.life.clone(),
        vendor: request.vendor.clone(),
        occupant: None,
        lineage: Vec::new(),
        capabilities: vec![capability],
        session: None,
        disposition: VACANT.to_owned(),
    });
    next.lanes.sort_by_key(|row| row.lane);
    let mut write = write(
        command.actor().clone(),
        epoch,
        Producer::ResidentRecognition,
    )?;
    guard_residency(&mut write, observed);
    let seated = write
        .fact(
            wire::body::LaneSeated {
                lane: request.lane,
                lineage_id: request.lineage_id,
                capability,
                name: request.name.clone(),
                worktree: request.worktree.clone(),
                life: request.life.clone(),
                vendor: request.vendor.clone(),
            },
            FactCause::Existing(observed.position()),
        )
        .map_err(|error| store_fault("lane::fact-invalid", &error.to_string()))?;
    write.residency_head(seated, move |_| next);
    command_result(&mut write, command, request.clone(), epoch, answer)?;
    Ok(write)
}

/// Propose the vacancy and closure one drained lane records.
pub(super) fn propose_drain(
    command: &wire::AdmittedCommand,
    request: &LaneState,
    observed: &journal::JournalHead<wire::ResidencyHead>,
    epoch: CanonicalUuid,
    occupant: CanonicalUuid,
    evidence: StopEvidence,
    answer: LaneChangeAnswer,
) -> CommandResult<JournalWrite> {
    let mut next = observed.body().clone();
    let row = next
        .lanes
        .iter_mut()
        .find(|row| row.lane == request.lane)
        .expect("observed lane remains in its cloned head");
    row.occupant = None;
    row.disposition = evidence.disposition().to_owned();
    let mut write = write(
        command.actor().clone(),
        epoch,
        Producer::ResidentRecognition,
    )?;
    guard_residency(&mut write, observed);
    let fact = ending_fact(
        &mut write,
        request.lane,
        occupant,
        &evidence,
        FactCause::Existing(observed.position()),
    )?;
    let fact = super::renewal::advance_exit(&mut write, &mut next, request.lane, occupant, fact)?;
    write.residency_head(fact, move |_| next);
    command_result(&mut write, command, request.clone(), epoch, answer)?;
    Ok(write)
}

/// Propose the occupant and residency one woken lane records.
pub(super) fn propose_wake(
    command: &wire::AdmittedCommand,
    request: &LaneState,
    observed: &journal::JournalHead<wire::ResidencyHead>,
    epoch: CanonicalUuid,
    proven: &ProvenOccupant,
    answer: LaneChangeAnswer,
) -> CommandResult<JournalWrite> {
    let occupant = proven.occupant;
    let group = proven.group;
    let session = &proven.session;
    let mut next = observed.body().clone();
    let row = next
        .lanes
        .iter_mut()
        .find(|row| row.lane == request.lane)
        .expect("observed lane remains in its cloned head");
    row.occupant = Some(occupant);
    row.lineage.push(occupant);
    row.session = Some(session.clone());
    row.disposition = OCCUPIED.to_owned();
    let mut write = write(
        command.actor().clone(),
        epoch,
        Producer::ResidentRecognition,
    )?;
    guard_residency(&mut write, observed);
    let declared = write
        .fact(
            session_fact(session),
            FactCause::Existing(observed.position()),
        )
        .map_err(|error| store_fault("lane::fact-invalid", &error.to_string()))?;
    let started = write
        .fact(
            wire::body::LaneStarted {
                lane: request.lane,
                occupant,
                epoch,
                group,
            },
            FactCause::Pending(declared),
        )
        .map_err(|error| store_fault("lane::fact-invalid", &error.to_string()))?;
    write.residency_head(started, move |_| next);
    command_result(&mut write, command, request.clone(), epoch, answer)?;
    Ok(write)
}

/// Propose removing one vacant lane from the resident topology.
pub(super) fn propose_remove(
    command: &wire::AdmittedCommand,
    request: &LaneState,
    observed: &journal::JournalHead<wire::ResidencyHead>,
    row: &wire::LaneResidency,
    answer: LaneChangeAnswer,
) -> CommandResult<JournalWrite> {
    let mut next = observed.body().clone();
    next.topology_revision = answer.topology_revision;
    let removed_lineage = row.lineage_id;
    next.lanes.retain(|lane| lane.lane != request.lane);
    if !next
        .lanes
        .iter()
        .any(|lane| lane.lineage_id == removed_lineage)
    {
        next.subscriptions
            .retain(|subscription| subscription.lineage_id != removed_lineage);
    }
    let mut write = write(
        command.actor().clone(),
        observed.body().epoch,
        Producer::ResidentRecognition,
    )?;
    guard_residency(&mut write, observed);
    let fact = write
        .fact(
            wire::body::LaneRemoved {
                lane: row.lane,
                lineage_id: row.lineage_id,
                worktree: row.worktree.clone(),
            },
            FactCause::Existing(observed.position()),
        )
        .map_err(|error| store_fault("lane::remove-fact-invalid", &error.to_string()))?;
    write.residency_head(fact, move |_| next);
    command_result(
        &mut write,
        command,
        request.clone(),
        observed.body().epoch,
        answer,
    )?;
    Ok(write)
}

/// Commit the restored residency one recovered seat proved.
pub(super) fn commit_restored(
    bus: &Bus,
    epoch: CanonicalUuid,
    durable: &wire::LaneResidency,
    proven: &ProvenOccupant,
) -> CommandResult<()> {
    let occupant = proven.occupant;
    let session = &proven.session;
    let observed = current_residency(bus, epoch)?;
    let carried = observe_restored_claim(bus, durable)?;
    let mut next = observed.body().clone();
    let row = next
        .lanes
        .iter_mut()
        .find(|row| row.lane == durable.lane)
        .ok_or_else(|| {
            store_fault(
                "lane::restart-moved",
                &format!("lane {} disappeared during restart", durable.lane),
            )
        })?;
    let mut write = write(Actor::Spice, epoch, Producer::ResidentRecognition)?;
    guard_residency(&mut write, &observed);
    if let Some(carried) = &carried {
        guard_restored_claim(&mut write, carried);
    }
    let mut fact = append_restored_lane(
        &mut write,
        row,
        proven,
        epoch,
        FactCause::Existing(observed.position()),
    )?;
    if let Some(carried) = &carried {
        fact = append_restored_claim(
            &mut write,
            carried,
            durable,
            occupant,
            epoch,
            FactCause::Pending(fact),
        )?;
    }
    row.occupant = Some(occupant);
    row.lineage.push(occupant);
    row.session = Some(session.clone());
    row.disposition = OCCUPIED.to_owned();
    write.residency_head(fact, move |_| next);
    commit(bus, write).map(|_| ())
}

/// Append the session and lane facts for one restored process.
fn append_restored_lane(
    write: &mut JournalWrite,
    row: &wire::LaneResidency,
    proven: &ProvenOccupant,
    epoch: CanonicalUuid,
    cause: FactCause,
) -> CommandResult<PendingFact> {
    let declared_cause = if let Some(ancestor) = row.occupant {
        let degraded = write
            .fact(
                wire::body::LaneDegraded {
                    lane: row.lane,
                    marker: "prior resident generation left no supervised process group".to_owned(),
                },
                cause,
            )
            .map_err(|error| store_fault("lane::restart-fact-invalid", &error.to_string()))?;
        let declared = write
            .fact(session_fact(&proven.session), FactCause::Pending(degraded))
            .map_err(|error| store_fault("lane::restart-fact-invalid", &error.to_string()))?;
        return write
            .fact(
                wire::body::LaneRenewed {
                    lane: row.lane,
                    occupant: proven.occupant,
                    ancestor,
                    epoch,
                    group: proven.group,
                },
                FactCause::Pending(declared),
            )
            .map_err(|error| store_fault("lane::restart-fact-invalid", &error.to_string()));
    } else {
        cause
    };
    let declared = write
        .fact(session_fact(&proven.session), declared_cause)
        .map_err(|error| store_fault("lane::restart-fact-invalid", &error.to_string()))?;
    write
        .fact(
            wire::body::LaneStarted {
                lane: row.lane,
                occupant: proven.occupant,
                epoch,
                group: proven.group,
            },
            FactCause::Pending(declared),
        )
        .map_err(|error| store_fault("lane::restart-fact-invalid", &error.to_string()))
}

/// Carry only epoch-bound claim authority when the same detached supervisor
/// and occupant remain live across resident replacement.
///
/// The startup transaction already retained the lane row while advancing the
/// residency epoch. No lane-start or session fact is owed here: neither the
/// process nor its vendor session restarted. An active claim is the sole piece
/// of authority that is resident-generation-bound, so it alone is rewritten.
pub(super) fn commit_adopted(
    bus: &Bus,
    epoch: CanonicalUuid,
    durable: &wire::LaneResidency,
) -> CommandResult<()> {
    let Some(occupant) = durable.occupant else {
        return Err(store_fault(
            "lane::restart-adoption-vacant",
            "a detached supervisor cannot be adopted into a vacant lane",
        ));
    };
    let Some(carried) = observe_restored_claim(bus, durable)? else {
        return Ok(());
    };
    if carried.is_bound_to(occupant, epoch) {
        return Ok(());
    }
    let observed = current_residency(bus, epoch)?;
    let mut write = write(Actor::Spice, epoch, Producer::ResidentRecognition)?;
    guard_residency(&mut write, &observed);
    guard_restored_claim(&mut write, &carried);
    append_restored_claim(
        &mut write,
        &carried,
        durable,
        occupant,
        epoch,
        FactCause::Existing(observed.position()),
    )?;
    commit(bus, write).map(|_| ())
}

#[cfg(test)]
#[path = "propose_tests.rs"]
pub(in crate::resident::lane_surface) mod tests;
