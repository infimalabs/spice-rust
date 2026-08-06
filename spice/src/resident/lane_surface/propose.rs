//! Intent proposals for seating, drain, wake, and restart commitment.

use super::*;

/// Propose the lane, capability, and first occupant one seat admits.
pub(super) fn propose_seat(
    command: &wire::AdmittedCommand,
    request: &LaneSeat,
    observed: &journal::JournalHead<wire::ResidencyHead>,
    epoch: CanonicalUuid,
    prepared: &PreparedSeat,
) -> CommandResult<JournalWrite> {
    let capability = prepared.capability;
    let occupant = prepared.proven.occupant;
    let group = prepared.proven.group;
    let session = &prepared.proven.session;
    let mut next = observed.body().clone();
    next.lanes.push(wire::LaneResidency {
        lane: request.lane,
        team: request.team,
        worktree: request.worktree.clone(),
        life: request.life.clone(),
        vendor: request.vendor.clone(),
        occupant: Some(occupant),
        lineage: vec![occupant],
        capabilities: vec![capability],
        session: Some(session.clone()),
        disposition: OCCUPIED.to_owned(),
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
                team: request.team,
                capability,
                worktree: request.worktree.clone(),
                life: request.life.clone(),
                vendor: request.vendor.clone(),
            },
            FactCause::Existing(observed.position()),
        )
        .map_err(|error| store_fault("lane::fact-invalid", &error.to_string()))?;
    let declared = write
        .fact(session_fact(session), FactCause::Pending(seated))
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
    command_result(
        &mut write,
        command,
        request.clone(),
        epoch,
        LaneChangeAnswer {
            lane: request.lane,
            from: "absent".to_owned(),
            to: OCCUPIED.to_owned(),
        },
    )?;
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
    row.disposition = VACANT.to_owned();
    let mut write = write(
        command.actor().clone(),
        epoch,
        Producer::ResidentRecognition,
    )?;
    guard_residency(&mut write, observed);
    let fact = match evidence {
        StopEvidence::Drained => write.fact(
            wire::body::LaneDrained {
                lane: request.lane,
                occupant,
            },
            FactCause::Existing(observed.position()),
        ),
        StopEvidence::Exited(code) => write.fact(
            wire::body::LaneExited {
                lane: request.lane,
                occupant,
                code,
            },
            FactCause::Existing(observed.position()),
        ),
    }
    .map_err(|error| store_fault("lane::fact-invalid", &error.to_string()))?;
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

/// Commit the restored residency one recovered seat proved.
pub(super) fn commit_restored(
    bus: &Bus,
    epoch: CanonicalUuid,
    durable: &wire::LaneResidency,
    proven: &ProvenOccupant,
) -> CommandResult<()> {
    let occupant = proven.occupant;
    let group = proven.group;
    let session = &proven.session;
    let observed = current_residency(bus, epoch)?;
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
    let fact = if let Some(ancestor) = row.occupant {
        let degraded = write
            .fact(
                wire::body::LaneDegraded {
                    lane: row.lane,
                    marker: "prior resident generation left no supervised process group".to_owned(),
                },
                FactCause::Existing(observed.position()),
            )
            .map_err(|error| store_fault("lane::restart-fact-invalid", &error.to_string()))?;
        let declared = write
            .fact(session_fact(session), FactCause::Pending(degraded))
            .map_err(|error| store_fault("lane::restart-fact-invalid", &error.to_string()))?;
        write
            .fact(
                wire::body::LaneRenewed {
                    lane: row.lane,
                    occupant,
                    ancestor,
                    epoch,
                    group,
                },
                FactCause::Pending(declared),
            )
            .map_err(|error| store_fault("lane::restart-fact-invalid", &error.to_string()))?
    } else {
        let declared = write
            .fact(
                session_fact(session),
                FactCause::Existing(observed.position()),
            )
            .map_err(|error| store_fault("lane::restart-fact-invalid", &error.to_string()))?;
        write
            .fact(
                wire::body::LaneStarted {
                    lane: row.lane,
                    occupant,
                    epoch,
                    group,
                },
                FactCause::Pending(declared),
            )
            .map_err(|error| store_fault("lane::restart-fact-invalid", &error.to_string()))?
    };
    row.occupant = Some(occupant);
    row.lineage.push(occupant);
    row.session = Some(session.clone());
    row.disposition = OCCUPIED.to_owned();
    write.residency_head(fact, move |_| next);
    commit(bus, write).map(|_| ())
}
