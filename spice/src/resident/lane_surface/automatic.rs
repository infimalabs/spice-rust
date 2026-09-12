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
    let topology_revision = observed.body().topology_revision;
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
    if let Err(outcome) = super::super::steering::lane_capability(row) {
        return typed_outcome(outcome);
    }
    let resume = match launch_resume(paths, row) {
        Ok(resume) => resume,
        Err(outcome) => return typed_outcome(outcome),
    };
    let fast_mode = match super::super::settings_surface::fast_mode(bus) {
        Ok(fast_mode) => fast_mode,
        Err(outcome) => return typed_outcome(outcome),
    };
    let occupant = new_uuid();
    let launch = match super::super::supervisor::launch(
        paths,
        OccupantLaunchRequest {
            worktree: &row.worktree,
            vendor: &row.vendor,
            epoch,
            lane: row.lane,
            occupant,
            resume,
            fast_mode,
        },
        Arc::clone(&runtime.capture),
    ) {
        Ok(launch) => launch,
        Err(outcome) => return typed_outcome(outcome),
    };
    let proven = ProvenOccupant {
        occupant: launch.occupant,
        group: launch.group,
        session: launch.session,
        git: launch.git,
    };
    match seats.seat_detached(request.lane, occupant, launch.process) {
        Ok(_) => {}
        Err(error) => return typed_environment_fault("lane::wake-failed", &error.to_string()),
    }
    let answer = LaneChangeAnswer {
        lane: request.lane,
        topology_revision,
        from: row.disposition.clone(),
        to: OCCUPIED.to_owned(),
        git: Some(proven.git.clone().into_boxed_str()),
        rtk: proven
            .session
            .launch
            .as_ref()
            .and_then(|launch| launch.rtk.clone()),
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
    wake_automatic_with(
        bus,
        paths,
        epoch,
        seats,
        runtime,
        WakeRequest {
            lane,
            occupant,
            decision,
            reserved,
            renewal: None,
        },
    )
}

pub(super) fn wake_renewal(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    runtime: &Runtime<'_>,
    renewal: &wire::LaneRenewalRecord,
    occupant: CanonicalUuid,
) -> CommandResult<AutomaticWake> {
    wake_automatic_with(
        bus,
        paths,
        epoch,
        seats,
        runtime,
        WakeRequest {
            lane: renewal.lane,
            occupant,
            decision: None,
            reserved: None,
            renewal: Some(renewal),
        },
    )
}

struct WakeRequest<'a> {
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    decision: Option<&'a bus::TaskSnapshot>,
    reserved: Option<&'a crate::board_command::AutomaticClaim>,
    renewal: Option<&'a wire::LaneRenewalRecord>,
}

struct WakeAdmission {
    expected_cursor: Option<wire::Seq>,
    observed: journal::JournalHead<wire::ResidencyHead>,
    row: wire::LaneResidency,
    claim: Option<AutomaticClaimAuthority>,
    resume: vendor::Resume,
    fast_mode: bool,
}

fn wake_automatic_with(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    runtime: &Runtime<'_>,
    request: WakeRequest<'_>,
) -> CommandResult<AutomaticWake> {
    let admitted = admit_automatic_wake(bus, paths, epoch, seats, &request)?;
    let launch = match super::super::supervisor::launch(
        paths,
        OccupantLaunchRequest {
            worktree: &admitted.row.worktree,
            vendor: &admitted.row.vendor,
            epoch,
            lane: request.lane,
            occupant: request.occupant,
            resume: admitted.resume,
            fast_mode: admitted.fast_mode,
        },
        Arc::clone(&runtime.capture),
    ) {
        Ok(launch) => launch,
        Err(outcome) => return Ok(AutomaticWake::Unstarted(outcome)),
    };
    let proven = ProvenOccupant {
        occupant: launch.occupant,
        group: launch.group,
        session: launch.session,
        git: launch.git,
    };
    if let Err(error) = seats.seat_detached(request.lane, request.occupant, launch.process) {
        return Ok(AutomaticWake::Unstarted(super::super::environment_fault(
            "lifecycle::wake-launch-failed",
            &error.to_string(),
        )));
    }
    if let Err(outcome) = commit_automatic_wake(
        bus,
        epoch,
        &admitted.observed,
        &admitted.row,
        &proven,
        WakeCommitAuthority {
            expected_cursor: admitted.expected_cursor,
            claim: admitted.claim.as_ref(),
            renewal: request.renewal,
        },
    ) {
        stop_lane(seats, request.lane);
        return Err(outcome);
    }
    Ok(AutomaticWake::Started)
}

fn admit_automatic_wake(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &Seats,
    request: &WakeRequest<'_>,
) -> CommandResult<WakeAdmission> {
    let expected_cursor = request.decision.and_then(bus::TaskSnapshot::cursor);
    let observed = current_residency(bus, epoch)?;
    let row = observed
        .body()
        .lanes
        .iter()
        .find(|row| row.lane == request.lane)
        .cloned()
        .ok_or_else(|| {
            store_fault(
                "lifecycle::wake-lane-missing",
                "the selected automatic wake lane disappeared",
            )
        })?;
    if row.occupant.is_some() || seats.occupied(request.lane) {
        return Err(automatic_refusal(
            "lifecycle::wake-capacity-moved",
            "lane",
            VACANT,
            &row.disposition,
            "reconcile after observing the current occupant",
        ));
    }
    if row.life == "drain" && request.renewal.is_none() {
        return Err(automatic_refusal(
            "lifecycle::wake-life-ended",
            "lane.life",
            "steer or drive",
            &row.life,
            "seat new capacity instead of re-seating a terminal lane",
        ));
    }
    super::super::steering::lane_capability(&row)?;
    let claim = automatic_claim_authority(bus, request.decision, request.lane)?;
    if let Some(reserved) = request.reserved
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
    let resume = match request.renewal {
        Some(_) => vendor::Resume::Fresh,
        None => launch_resume(paths, &row)?,
    };
    let fast_mode = super::super::settings_surface::fast_mode(bus)?;
    Ok(WakeAdmission {
        expected_cursor,
        observed,
        row,
        claim,
        resume,
        fast_mode,
    })
}

#[derive(Clone)]
struct AutomaticClaimAuthority {
    task: wire::TaskStamp,
    statement: journal::JournalHead<wire::TaskHead>,
    claim: journal::JournalHead<wire::ClaimHead>,
    lease: journal::JournalHead<wire::LeaseHead>,
    source: wire::body::SourceBoundary,
    automatic: bool,
    thaw: bool,
    trust_blocked: Option<(wire::EnvironmentRequirements, bool)>,
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
    let facts = snapshot.facts().iter().cloned().collect::<Vec<_>>();
    let environment = snapshot.environment();
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
        let indefinite = spice::claim_lease::is_indefinite(lease.body());
        let requirements = heads.statement().body().requirements.clone();
        let trust_blocked = requirements.has_unsatisfied_trust_grant(&environment);
        authority = Some(AutomaticClaimAuthority {
            task: heads.statement().body().born.clone(),
            statement: heads.statement().clone(),
            claim: claim.clone(),
            lease: lease.clone(),
            source: bus.source_boundary(&lane).map_err(|error| {
                store_fault("lifecycle::claim-boundary-unreadable", &error.to_string())
            })?,
            automatic: crate::board_command::is_automatic_claim(claim.body(), &facts),
            thaw: indefinite && !trust_blocked,
            trust_blocked: indefinite.then_some((requirements, trust_blocked)),
        });
    }
    Ok(authority)
}

fn guard_automatic_claim(write: &mut JournalWrite, authority: &AutomaticClaimAuthority) {
    let task = authority.task.clone();
    let statement = authority.statement.clone();
    let claim = authority.claim.clone();
    let lease = authority.lease.clone();
    let trust_blocked = authority.trust_blocked.clone();
    write.guard(JournalGuard::new(move |read| {
        if read.task_head(&task)? != Some(statement.clone()) {
            return Err(journal::GuardError::refused(
                "lifecycle::held-claim-task-moved",
            ));
        }
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
        if let Some((requirements, expected)) = trust_blocked.as_ref() {
            let mut environment = wire::Environment::journal_schema(journal::SCHEMA_VERSION)
                .expect("the compiled journal schema is positive");
            if let Some(digest) = requirements.as_slice().iter().find_map(|requirement| {
                let wire::EnvironmentRequirement::TrustGrant { value } = requirement else {
                    return None;
                };
                Some(value)
            }) {
                environment = environment.with_trust_heads(
                    read.trust_head(digest)?
                        .into_iter()
                        .map(journal::JournalHead::into_body),
                );
            }
            if requirements.has_unsatisfied_trust_grant(&environment) != *expected {
                return Err(journal::GuardError::refused(
                    "lifecycle::held-claim-trust-moved",
                ));
            }
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
    Ok(carried)
}

fn update_automatic_lease(
    write: &mut JournalWrite,
    authority: &AutomaticClaimAuthority,
    epoch: CanonicalUuid,
    at: wire::Instant,
    cause: PendingFact,
    reset_maximum: bool,
) -> CommandResult<PendingFact> {
    let lease = promoted_lease(authority.lease.body(), epoch, at, reset_maximum)?;
    let task = authority.task.clone();
    write.lease_head(task, cause, move |_| lease);
    Ok(cause)
}

fn promoted_lease(
    observed: &wire::LeaseHead,
    epoch: CanonicalUuid,
    at: wire::Instant,
    reset_maximum: bool,
) -> CommandResult<wire::LeaseHead> {
    if at >= observed.expiry {
        return Err(store_fault(
            "lifecycle::claim-promotion-expired",
            "the automatic claim expired before startup became ready",
        ));
    }
    let deadline = at
        .get()
        .checked_add(jiff::SignedDuration::from_secs(
            crate::board_command::AUTOMATIC_ROLLING_SECONDS,
        ))
        .map_err(|error| store_fault("lifecycle::claim-promotion-invalid", &error.to_string()))?;
    let until = wire::Instant::new(deadline)
        .map_err(|error| store_fault("lifecycle::claim-promotion-invalid", &error.to_string()))?;
    let maximum = at
        .get()
        .checked_add(jiff::SignedDuration::from_secs(
            crate::board_command::AUTOMATIC_MAXIMUM_SECONDS,
        ))
        .map_err(|error| store_fault("lifecycle::claim-promotion-invalid", &error.to_string()))?;
    let maximum = wire::Instant::new(maximum)
        .map_err(|error| store_fault("lifecycle::claim-promotion-invalid", &error.to_string()))?;
    let mut lease = observed.clone();
    lease.epoch = epoch;
    lease.renewal = at;
    lease.expiry = until;
    lease.maximum = if reset_maximum {
        maximum
    } else {
        std::cmp::max(maximum, observed.maximum)
    };
    Ok(lease)
}

fn should_promote_lease(authority: &AutomaticClaimAuthority) -> bool {
    authority.thaw
        || (authority.automatic && !spice::claim_lease::is_indefinite(authority.lease.body()))
}

struct WakeCommitAuthority<'a> {
    expected_cursor: Option<wire::Seq>,
    claim: Option<&'a AutomaticClaimAuthority>,
    renewal: Option<&'a wire::LaneRenewalRecord>,
}

fn commit_automatic_wake(
    bus: &Bus,
    epoch: CanonicalUuid,
    observed: &journal::JournalHead<wire::ResidencyHead>,
    row: &wire::LaneResidency,
    proven: &ProvenOccupant,
    authority: WakeCommitAuthority<'_>,
) -> CommandResult<()> {
    let WakeCommitAuthority {
        expected_cursor,
        claim,
        renewal,
    } = authority;
    let at = wire::Instant::new(jiff::Timestamp::now())
        .map_err(|error| store_fault("lifecycle::claim-promotion-clock", &error.to_string()))?;
    let (mut next, mut write, started) =
        prepare_wake_commit(epoch, observed, row, proven, expected_cursor, claim)?;
    let mut head = match claim {
        Some(claim) if claim.claim.body().occupant != proven.occupant => {
            carry_automatic_claim(&mut write, claim, row.lane, proven.occupant, epoch, started)?
        }
        _ => started,
    };
    if let Some(claim) = claim {
        if should_promote_lease(claim) && (claim.thaw || renewal.is_none()) {
            head = update_automatic_lease(&mut write, claim, epoch, at, head, claim.thaw)?;
        } else if claim.claim.body().occupant != proven.occupant {
            let mut lease = claim.lease.body().clone();
            lease.epoch = epoch;
            let task = claim.task.clone();
            write.lease_head(task, head, move |_| lease);
        }
    }
    if let Some(renewal) = renewal {
        let successor =
            super::renewal::successor_record(renewal, &proven.session, next.topology_revision)?;
        super::renewal::replace_renewal(&mut next, row.lane, Some(successor.clone()));
        head = write
            .fact(
                wire::body::LaneRenewalChanged {
                    lineage_id: successor.lineage_id,
                    lane: successor.lane,
                    renewal: Some(successor),
                },
                FactCause::Pending(head),
            )
            .map_err(|error| store_fault("lane::renewal-fact-invalid", &error.to_string()))?;
    }
    write.residency_head(head, move |_| next);
    commit(bus, write).map(|_| ())
}

fn prepare_wake_commit(
    epoch: CanonicalUuid,
    observed: &journal::JournalHead<wire::ResidencyHead>,
    row: &wire::LaneResidency,
    proven: &ProvenOccupant,
    expected_cursor: Option<wire::Seq>,
    claim: Option<&AutomaticClaimAuthority>,
) -> CommandResult<(wire::ResidencyHead, JournalWrite, PendingFact)> {
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
    Ok((next, write, started))
}

#[cfg(test)]
mod tests;
