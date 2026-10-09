//! Manual and lifecycle-authority occupant wakes.

use super::*;

pub(in crate::resident) struct WakeLaunch {
    launch: super::super::supervisor::PendingLaunch,
    pending: PendingWake,
}

impl WakeLaunch {
    pub(in crate::resident) fn lane(&self) -> CanonicalUuid {
        self.pending.request.lane
    }

    pub(in crate::resident) fn into_parts(
        self,
    ) -> (super::super::supervisor::PendingLaunch, PendingWake) {
        (self.launch, self.pending)
    }
}

pub(in crate::resident) struct PendingWake {
    observed: journal::JournalHead<wire::ResidencyHead>,
    row: wire::LaneResidency,
    renewal: Option<wire::LaneRenewalRecord>,
    request: LaneState,
    occupant: CanonicalUuid,
    fresh_session: bool,
}

#[allow(
    clippy::too_many_arguments,
    reason = "one manual wake needs the whole command context"
)]
pub(in crate::resident) fn begin_wake(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &Seats,
    runtime: &Runtime<'_>,
    steering: &steer::Config,
    request: &LaneState,
) -> Result<WakeLaunch, Outcome<CommandAnswer>> {
    let observed = current_residency(bus, epoch).map_err(typed_outcome)?;
    let row = observed
        .body()
        .lanes
        .iter()
        .find(|row| row.lane == request.lane)
        .cloned()
        .ok_or_else(|| missing_lane(request.lane))?;
    if row.occupant.is_some() || seats.occupied(request.lane) {
        return Err(lane_refusal(
            "lane::already-occupied",
            Some(VACANT.to_owned()),
            Some(row.disposition.clone()),
            "drain the current occupant before waking a successor",
        ));
    }
    super::super::steering::lane_capability(&row).map_err(typed_outcome)?;
    let renewal =
        super::renewal::pending_for(observed.body(), request.lane).map_err(typed_outcome)?;
    let resume = wake_resume(bus, paths, epoch, steering, runtime, &row, renewal.as_ref())?;
    let fast_mode = super::super::settings_surface::lane_fast_mode(bus, observed.body(), &row)
        .map_err(typed_outcome)?;
    let occupant = new_uuid();
    let fresh_session = matches!(resume, vendor::Resume::Fresh);
    let launch = super::super::supervisor::begin_launch(
        paths,
        OccupantLaunchRequest {
            worktree: &row.worktree,
            vendor: &row.vendor,
            epoch,
            lane: row.lane,
            occupant,
            resume,
            fast_mode,
            model: row.model.as_deref(),
            effort: row.effort.as_deref(),
        },
        Arc::clone(&runtime.capture),
    )
    .map_err(typed_outcome)?;
    Ok(WakeLaunch {
        launch,
        pending: PendingWake {
            observed,
            row,
            renewal,
            request: request.clone(),
            occupant,
            fresh_session,
        },
    })
}

pub(in crate::resident) fn finish_wake(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    command: &wire::AdmittedCommand,
    pending: PendingWake,
    launched: CommandResult<super::super::supervisor::Launch>,
) -> Outcome<CommandAnswer> {
    let launch = match launched {
        Ok(launch) => launch,
        Err(outcome) => return typed_outcome(outcome),
    };
    let source = match session_source(paths, &launch.session) {
        Ok(source) => source,
        Err(outcome) => return typed_outcome(outcome),
    };
    let proven = ProvenOccupant {
        occupant: launch.occupant,
        group: launch.group,
        session: launch.session,
        source,
        git: launch.git,
        fresh_session: pending.fresh_session,
    };
    if let Err(error) = seats.seat_detached(pending.request.lane, pending.occupant, launch.process)
    {
        return typed_environment_fault("lane::wake-failed", &error.to_string());
    }
    let answer = LaneChangeAnswer {
        lane: pending.request.lane,
        topology_revision: pending.observed.body().topology_revision,
        from: pending.row.disposition.clone(),
        to: OCCUPIED.to_owned(),
        git: Some(proven.git.clone().into_boxed_str()),
        rtk: proven
            .session
            .launch
            .as_ref()
            .and_then(|launch| launch.rtk.clone()),
    };
    let proposal = propose_wake(
        command,
        &pending.request,
        &pending.observed,
        epoch,
        &proven,
        pending.renewal.as_ref(),
        answer.clone(),
    );
    let proposal = match proposal {
        Ok(proposal) => proposal,
        Err(outcome) => {
            stop_lane(seats, pending.request.lane);
            return typed_outcome(outcome);
        }
    };
    match commit(bus, proposal) {
        Ok(committed) => committed_lane_reply(committed, answer),
        Err(outcome) => {
            stop_lane(seats, pending.request.lane);
            typed_outcome(outcome)
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the synchronous test boundary exercises the same split launch continuation"
)]
pub(super) fn wake(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    runtime: &Runtime<'_>,
    steering: &steer::Config,
    command: &wire::AdmittedCommand,
    request: &LaneState,
) -> Outcome<CommandAnswer> {
    let launch = match begin_wake(bus, paths, epoch, seats, runtime, steering, request) {
        Ok(launch) => launch,
        Err(outcome) => return outcome,
    };
    let (launch, pending) = launch.into_parts();
    finish_wake(
        bus,
        paths,
        epoch,
        seats,
        command,
        pending,
        launch.establish(),
    )
}

/// The successor's resume posture, with any renewal context published first.
///
/// A replacement starts fresh: its predecessor's session is exactly what the
/// operator asked to leave behind, and the rehydration context reaches the
/// successor's first inbox read because it is published before the launch.
fn wake_resume(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    steering: &steer::Config,
    runtime: &Runtime<'_>,
    row: &wire::LaneResidency,
    renewal: Option<&wire::LaneRenewalRecord>,
) -> Result<vendor::Resume, Outcome<CommandAnswer>> {
    let Some(renewal) = renewal else {
        return launch_resume(paths, row).map_err(typed_outcome);
    };
    super::renewal::prepare(bus, paths, epoch, steering, runtime, renewal)
        .map_err(typed_outcome)?;
    Ok(vendor::Resume::Fresh)
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

struct WakeRequest<'a> {
    lane: CanonicalUuid,
    decision: Option<&'a bus::TaskSnapshot>,
    reserved: Option<&'a crate::board_command::AutomaticClaim>,
}

struct WakeAdmission {
    expected_cursor: Option<wire::Seq>,
    observed: journal::JournalHead<wire::ResidencyHead>,
    row: wire::LaneResidency,
    claim: Option<AutomaticClaimAuthority>,
    resume: vendor::Resume,
    fast_mode: bool,
    /// The durable replacement this start consumes, when the lane carries one.
    renewal: Option<wire::LaneRenewalRecord>,
}

pub(in crate::resident) enum BeginAutomaticWake {
    Launch(AutomaticLaunch),
    Unstarted(Outcome<()>),
}

pub(in crate::resident) struct AutomaticLaunch {
    launch: super::super::supervisor::PendingLaunch,
    pending: PendingAutomaticWake,
}

impl AutomaticLaunch {
    pub(in crate::resident) fn into_parts(
        self,
    ) -> (
        super::super::supervisor::PendingLaunch,
        PendingAutomaticWake,
    ) {
        (self.launch, self.pending)
    }
}

pub(in crate::resident) struct PendingAutomaticWake {
    admitted: WakeAdmission,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    fresh_session: bool,
}

#[allow(
    clippy::too_many_arguments,
    reason = "one automatic wake owns the complete resident launch boundary"
)]
pub(in crate::resident) fn begin_automatic_wake(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &Seats,
    runtime: &Runtime<'_>,
    steering: &steer::Config,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    decision: Option<&bus::TaskSnapshot>,
    reserved: Option<&crate::board_command::AutomaticClaim>,
) -> CommandResult<BeginAutomaticWake> {
    let request = WakeRequest {
        lane,
        decision,
        reserved,
    };
    let admitted = admit_automatic_wake(bus, paths, epoch, seats, &request)?;
    if let Some(renewal) = admitted.renewal.as_ref() {
        super::renewal::prepare(bus, paths, epoch, steering, runtime, renewal)?;
    }
    let fresh_session = matches!(admitted.resume, vendor::Resume::Fresh);
    let launch = match super::super::supervisor::begin_launch(
        paths,
        OccupantLaunchRequest {
            worktree: &admitted.row.worktree,
            vendor: &admitted.row.vendor,
            epoch,
            lane,
            occupant,
            resume: admitted.resume.clone(),
            fast_mode: admitted.fast_mode,
            model: admitted.row.model.as_deref(),
            effort: admitted.row.effort.as_deref(),
        },
        Arc::clone(&runtime.capture),
    ) {
        Ok(launch) => launch,
        Err(outcome) => return Ok(BeginAutomaticWake::Unstarted(outcome)),
    };
    Ok(BeginAutomaticWake::Launch(AutomaticLaunch {
        launch,
        pending: PendingAutomaticWake {
            admitted,
            lane,
            occupant,
            fresh_session,
        },
    }))
}

pub(in crate::resident) fn finish_automatic_wake(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    pending: PendingAutomaticWake,
    launched: CommandResult<super::super::supervisor::Launch>,
) -> CommandResult<AutomaticWake> {
    let launch = match launched {
        Ok(launch) => launch,
        Err(outcome) => return Ok(AutomaticWake::Unstarted(outcome)),
    };
    let source = match session_source(paths, &launch.session) {
        Ok(source) => source,
        Err(outcome) => return Ok(AutomaticWake::Unstarted(outcome)),
    };
    let proven = ProvenOccupant {
        occupant: launch.occupant,
        group: launch.group,
        session: launch.session,
        source,
        git: launch.git,
        fresh_session: pending.fresh_session,
    };
    if let Err(error) = seats.seat_detached(pending.lane, pending.occupant, launch.process) {
        return Ok(AutomaticWake::Unstarted(super::super::environment_fault(
            "lifecycle::wake-launch-failed",
            &error.to_string(),
        )));
    }
    if let Err(outcome) = commit_automatic_wake(
        bus,
        epoch,
        &pending.admitted.observed,
        &pending.admitted.row,
        &proven,
        WakeCommitAuthority {
            expected_cursor: pending.admitted.expected_cursor,
            claim: pending.admitted.claim.as_ref(),
            renewal: pending.admitted.renewal.as_ref(),
        },
    ) {
        stop_lane(seats, pending.lane);
        return Err(outcome);
    }
    Ok(AutomaticWake::Started)
}

#[cfg(test)]
#[allow(
    clippy::too_many_arguments,
    reason = "the synchronous test boundary exercises the same split launch continuation"
)]
pub(in crate::resident) fn wake_automatic(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    runtime: &Runtime<'_>,
    steering: &steer::Config,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    decision: Option<&bus::TaskSnapshot>,
    reserved: Option<&crate::board_command::AutomaticClaim>,
) -> CommandResult<AutomaticWake> {
    match begin_automatic_wake(
        bus, paths, epoch, seats, runtime, steering, lane, occupant, decision, reserved,
    )? {
        BeginAutomaticWake::Unstarted(outcome) => Ok(AutomaticWake::Unstarted(outcome)),
        BeginAutomaticWake::Launch(launch) => {
            let (launch, pending) = launch.into_parts();
            finish_automatic_wake(bus, paths, epoch, seats, pending, launch.establish())
        }
    }
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
    let renewal = super::renewal::pending_for(observed.body(), request.lane)?;
    let resume = if renewal.is_some() {
        vendor::Resume::Fresh
    } else {
        launch_resume(paths, &row)?
    };
    let fast_mode = super::super::settings_surface::lane_fast_mode(bus, observed.body(), &row)?;
    Ok(WakeAdmission {
        expected_cursor,
        observed,
        row,
        claim,
        resume,
        fast_mode,
        renewal,
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
    let appliance = spice::version::running().map_err(Outcome::Fault)?;
    let environment = snapshot
        .environment()
        .with_appliance(appliance.product.clone());
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
            automatic: crate::board_command::is_automatic_claim(claim.body(), snapshot.facts()),
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
            let environment = read.environment(requirements)?;
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
        head = super::renewal::append_successor(
            &mut write,
            &mut next,
            renewal,
            &proven.session,
            head,
        )?;
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
            session_fact(&proven.session, proven.source),
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
                fresh_session: Some(proven.fresh_session),
            },
            FactCause::Pending(declared),
        )
        .map_err(|error| store_fault("lifecycle::wake-fact-invalid", &error.to_string()))?;
    Ok((next, write, started))
}

#[cfg(test)]
mod tests;
