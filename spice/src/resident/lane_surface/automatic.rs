//! Manual and lifecycle-authority occupant wakes.

use super::*;

pub(super) fn wake(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    runtime: &Runtime<'_>,
    command: &wire::AdmittedCommand,
    request: &LaneState,
) -> Outcome<CommandAnswer> {
    let observed = match current_residency(bus, epoch) {
        Ok(head) => head,
        Err(outcome) => return typed_outcome(outcome),
    };
    let Some(row) = observed
        .body()
        .lanes
        .iter()
        .find(|row| row.lane == request.lane)
    else {
        return missing_lane(request.lane);
    };
    if row.occupant.is_some() || seats.occupied(request.lane) {
        return lane_refusal(
            "lane::already-occupied",
            Some(VACANT.to_owned()),
            Some(row.disposition.clone()),
            "drain the current occupant before waking a successor",
        );
    }
    if row.life == "drain" {
        return lane_refusal(
            "lane::life-ended",
            Some("steer or drive".to_owned()),
            Some(row.life.clone()),
            "seat a new lane because drain life is terminal",
        );
    }
    if let Err(outcome) = super::super::steering::lane_capability(row) {
        return typed_outcome(outcome);
    }
    let resume = match resume_session(paths, row) {
        Ok(resume) => resume,
        Err(outcome) => return typed_outcome(outcome),
    };
    let occupant = new_uuid();
    let launch = match occupant_launch(
        paths,
        OccupantLaunchRequest {
            worktree: &row.worktree,
            vendor: &row.vendor,
            epoch,
            lane: row.lane,
            occupant,
            resume,
        },
        Arc::clone(&runtime.capture),
    ) {
        Ok(launch) => launch,
        Err(outcome) => return typed_outcome(outcome),
    };
    let git = launch.git;
    let session_proof = launch.session;
    let group = match seats.seat_observed(request.lane, occupant, launch.spec, launch.observation) {
        Ok(occupant) => occupant.group(),
        Err(error) => return typed_environment_fault("lane::wake-failed", &error.to_string()),
    };
    let session = match await_session(session_proof) {
        Ok(session) => session,
        Err(error) => {
            stop_lane(seats, request.lane);
            return error.outcome();
        }
    };
    let proven = ProvenOccupant {
        occupant,
        group,
        session,
        git,
    };
    let answer = LaneChangeAnswer {
        lane: request.lane,
        from: row.disposition.clone(),
        to: OCCUPIED.to_owned(),
        git: Some(proven.git.clone().into_boxed_str()),
    };
    let proposal = propose_wake(command, request, &observed, epoch, &proven, answer.clone());
    let proposal = match proposal {
        Ok(proposal) => proposal,
        Err(outcome) => {
            stop_lane(seats, request.lane);
            return typed_outcome(outcome);
        }
    };
    match commit(bus, proposal) {
        Ok(committed) => committed_lane_reply(committed, answer),
        Err(outcome) => {
            stop_lane(seats, request.lane);
            typed_outcome(outcome)
        }
    }
}

/// Launch one occupant selected by the resident lifecycle authority.
///
/// An available-work caller passes the exact claim it minted before launch.
/// A pending-inbox caller passes none; if the lane instead carries an older
/// live claim, this path transfers that claim to the successor in the same
/// resident-recognition transaction that starts it.
/// Whether launch crossed the session-proof boundary.
pub(in crate::resident) enum AutomaticWake {
    Started,
    Unstarted(Outcome<()>),
}

fn automatic_refusal(
    reason: &str,
    witness: &str,
    expected: &str,
    observed: &str,
    detail: &str,
) -> Outcome<()> {
    Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(
            reason.to_owned(),
            Some(witness.to_owned()),
            Some(expected.to_owned()),
            Some(observed.to_owned()),
        )
        .with_detail(detail),
    ))
}

#[allow(
    clippy::too_many_arguments,
    reason = "one automatic wake owns the complete resident launch boundary"
)]
pub(in crate::resident) fn wake_automatic(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    runtime: &Runtime<'_>,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    decision: Option<&bus::TaskSnapshot>,
    reserved: Option<&crate::board_command::AutomaticClaim>,
) -> CommandResult<AutomaticWake> {
    let expected_cursor = decision.and_then(bus::TaskSnapshot::cursor);
    let observed = current_residency(bus, epoch)?;
    let row = observed
        .body()
        .lanes
        .iter()
        .find(|row| row.lane == lane)
        .cloned()
        .ok_or_else(|| {
            store_fault(
                "lifecycle::wake-lane-missing",
                "the selected automatic wake lane disappeared",
            )
        })?;
    if row.occupant.is_some() || seats.occupied(lane) {
        return Err(automatic_refusal(
            "lifecycle::wake-capacity-moved",
            "lane",
            VACANT,
            &row.disposition,
            "reconcile after observing the current occupant",
        ));
    }
    if row.life == "drain" {
        return Err(automatic_refusal(
            "lifecycle::wake-life-ended",
            "lane.life",
            "steer or drive",
            &row.life,
            "seat new capacity instead of re-seating a terminal lane",
        ));
    }
    super::super::steering::lane_capability(&row)?;
    let claim = automatic_claim_authority(bus, decision, lane)?;
    if let Some(reserved) = reserved
        && !claim.as_ref().is_some_and(|claim| {
            claim.task == reserved.task
                && claim.claim.body().lane == reserved.lane
                && claim.claim.body().occupant == reserved.occupant
        })
    {
        return Err(store_fault(
            "lifecycle::available-work-claim-moved",
            "the exact pre-launch claim is no longer active",
        ));
    }
    let resume = resume_session(paths, &row)?;
    let launch = match occupant_launch(
        paths,
        OccupantLaunchRequest {
            worktree: &row.worktree,
            vendor: &row.vendor,
            epoch,
            lane,
            occupant,
            resume,
        },
        Arc::clone(&runtime.capture),
    ) {
        Ok(launch) => launch,
        Err(outcome) => return Ok(AutomaticWake::Unstarted(outcome)),
    };
    let git = launch.git;
    let session_proof = launch.session;
    let group = match seats.seat_observed(lane, occupant, launch.spec, launch.observation) {
        Ok(occupant) => occupant.group(),
        Err(error) => {
            return Ok(AutomaticWake::Unstarted(super::super::environment_fault(
                "lifecycle::wake-launch-failed",
                &error.to_string(),
            )));
        }
    };
    let session = match await_session(session_proof) {
        Ok(session) => session,
        Err(error) => {
            stop_lane(seats, lane);
            return Ok(AutomaticWake::Unstarted(Outcome::Refusal(error.refusal())));
        }
    };
    let proven = ProvenOccupant {
        occupant,
        group,
        session,
        git,
    };
    if let Err(outcome) = commit_automatic_wake(
        bus,
        epoch,
        &observed,
        &row,
        &proven,
        expected_cursor,
        claim.as_ref(),
    ) {
        stop_lane(seats, lane);
        return Err(outcome);
    }
    Ok(AutomaticWake::Started)
}

#[derive(Clone)]
struct AutomaticClaimAuthority {
    task: wire::TaskHandle,
    claim: journal::JournalHead<wire::ClaimHead>,
    lease: journal::JournalHead<wire::LeaseHead>,
    source: wire::body::SourceBoundary,
}

fn automatic_claim_authority(
    bus: &Bus,
    decision: Option<&bus::TaskSnapshot>,
    lane: CanonicalUuid,
) -> CommandResult<Option<AutomaticClaimAuthority>> {
    let current = if decision.is_none() {
        Some(bus.task_snapshot().map_err(|error| {
            store_fault("lifecycle::claim-heads-unreadable", &error.to_string())
        })?)
    } else {
        None
    };
    let snapshot = decision
        .or(current.as_ref())
        .expect("automatic claim authority has a supplied or current snapshot");
    let mut authority = None;
    for heads in snapshot.tasks() {
        let Some(claim) = heads.claim() else { continue };
        if claim.body().disposition != wire::ClaimDisposition::Active || claim.body().lane != lane {
            continue;
        }
        if authority.is_some() {
            return Err(store_fault(
                "lifecycle::held-claim-ambiguous",
                "one vacant lane carries more than one active claim",
            ));
        }
        let lease = heads.lease().ok_or_else(|| {
            store_fault(
                "lifecycle::held-claim-lease-missing",
                "an active held claim has no durable lease head",
            )
        })?;
        if lease.body().epoch != claim.body().epoch {
            return Err(store_fault(
                "lifecycle::held-claim-epoch-mismatch",
                "held claim and lease generations disagree",
            ));
        }
        authority = Some(AutomaticClaimAuthority {
            task: heads.statement().body().handle.clone(),
            claim: claim.clone(),
            lease: lease.clone(),
            source: bus.source_boundary(&lane).map_err(|error| {
                store_fault("lifecycle::claim-boundary-unreadable", &error.to_string())
            })?,
        });
    }
    Ok(authority)
}

fn guard_automatic_claim(write: &mut JournalWrite, authority: &AutomaticClaimAuthority) {
    let task = authority.task.clone();
    let claim = authority.claim.clone();
    let lease = authority.lease.clone();
    write.guard(JournalGuard::new(move |read| {
        if read.claim_head(&task)? != Some(claim.clone()) {
            return Err(journal::GuardError::refused(
                "lifecycle::held-claim-head-moved",
            ));
        }
        if read.lease_head(&task)? != Some(lease.clone()) {
            return Err(journal::GuardError::refused(
                "lifecycle::held-claim-lease-moved",
            ));
        }
        Ok(())
    }));
}

fn carry_automatic_claim(
    write: &mut JournalWrite,
    authority: &AutomaticClaimAuthority,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    epoch: CanonicalUuid,
    cause: PendingFact,
) -> CommandResult<PendingFact> {
    let ancestor = authority.claim.body().occupant;
    let carried = write
        .fact(
            wire::body::ClaimCarried {
                task: authority.task.clone(),
                lane,
                occupant,
                ancestor,
                epoch,
                source: authority.source.clone(),
            },
            FactCause::Pending(cause),
        )
        .map_err(|error| store_fault("lifecycle::claim-carry-invalid", &error.to_string()))?;
    let mut claim = authority.claim.body().clone();
    claim.occupant = occupant;
    claim.epoch = epoch;
    let task = authority.task.clone();
    write.claim_head(task.clone(), carried, move |position| {
        claim.fact = position;
        claim
    });
    let mut lease = authority.lease.body().clone();
    lease.epoch = epoch;
    write.lease_head(task, carried, move |_| lease);
    Ok(carried)
}

fn commit_automatic_wake(
    bus: &Bus,
    epoch: CanonicalUuid,
    observed: &journal::JournalHead<wire::ResidencyHead>,
    row: &wire::LaneResidency,
    proven: &ProvenOccupant,
    expected_cursor: Option<wire::Seq>,
    claim: Option<&AutomaticClaimAuthority>,
) -> CommandResult<()> {
    let mut next = observed.body().clone();
    let next_row = next
        .lanes
        .iter_mut()
        .find(|candidate| candidate.lane == row.lane)
        .expect("observed automatic lane remains in its cloned head");
    next_row.occupant = Some(proven.occupant);
    next_row.lineage.push(proven.occupant);
    next_row.session = Some(proven.session.clone());
    next_row.disposition = OCCUPIED.to_owned();
    let mut write = write(Actor::Spice, epoch, Producer::ResidentRecognition)?;
    guard_residency(&mut write, observed);
    if let Some(expected) = expected_cursor {
        write.guard(JournalGuard::new(move |read| {
            if read.cursor()? == Some(expected) {
                Ok(())
            } else {
                Err(journal::GuardError::refused("lifecycle::snapshot-moved"))
            }
        }));
    }
    if let Some(claim) = claim {
        guard_automatic_claim(&mut write, claim);
    }
    let declared = write
        .fact(
            session_fact(&proven.session),
            FactCause::Existing(observed.position()),
        )
        .map_err(|error| store_fault("lifecycle::wake-fact-invalid", &error.to_string()))?;
    let started = write
        .fact(
            wire::body::LaneStarted {
                lane: row.lane,
                occupant: proven.occupant,
                epoch,
                group: proven.group,
            },
            FactCause::Pending(declared),
        )
        .map_err(|error| store_fault("lifecycle::wake-fact-invalid", &error.to_string()))?;
    let head = match claim {
        Some(claim) if claim.claim.body().occupant != proven.occupant => {
            carry_automatic_claim(&mut write, claim, row.lane, proven.occupant, epoch, started)?
        }
        _ => started,
    };
    write.residency_head(head, move |_| next);
    commit(bus, write).map(|_| ())
}
