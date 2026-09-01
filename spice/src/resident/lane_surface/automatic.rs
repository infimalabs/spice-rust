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
    let resume = launch_resume(paths, &row)?;
    let launch = match super::super::supervisor::launch(
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
    let proven = ProvenOccupant {
        occupant: launch.occupant,
        group: launch.group,
        session: launch.session,
        git: launch.git,
    };
    match seats.seat_detached(lane, occupant, launch.process) {
        Ok(_) => {}
        Err(error) => {
            return Ok(AutomaticWake::Unstarted(super::super::environment_fault(
                "lifecycle::wake-launch-failed",
                &error.to_string(),
            )));
        }
    }
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
    task: wire::TaskStamp,
    claim: journal::JournalHead<wire::ClaimHead>,
    lease: journal::JournalHead<wire::LeaseHead>,
    source: wire::body::SourceBoundary,
    automatic: bool,
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
            task: heads.statement().body().born.clone(),
            claim: claim.clone(),
            lease: lease.clone(),
            source: bus.source_boundary(&lane).map_err(|error| {
                store_fault("lifecycle::claim-boundary-unreadable", &error.to_string())
            })?,
            automatic: crate::board_command::is_automatic_claim(claim.body(), &facts),
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
    Ok(carried)
}

fn update_automatic_lease(
    write: &mut JournalWrite,
    authority: &AutomaticClaimAuthority,
    epoch: CanonicalUuid,
    at: wire::Instant,
    cause: PendingFact,
) -> CommandResult<PendingFact> {
    let lease = promoted_lease(authority.lease.body(), epoch, at)?;
    let task = authority.task.clone();
    write.lease_head(task, cause, move |_| lease);
    Ok(cause)
}

fn promoted_lease(
    observed: &wire::LeaseHead,
    epoch: CanonicalUuid,
    at: wire::Instant,
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
    let mut lease = observed.clone();
    lease.epoch = epoch;
    lease.renewal = at;
    lease.expiry = until;
    lease.maximum = until;
    Ok(lease)
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
    let at = wire::Instant::new(jiff::Timestamp::now())
        .map_err(|error| store_fault("lifecycle::claim-promotion-clock", &error.to_string()))?;
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
    let mut head = match claim {
        Some(claim) if claim.claim.body().occupant != proven.occupant => {
            carry_automatic_claim(&mut write, claim, row.lane, proven.occupant, epoch, started)?
        }
        _ => started,
    };
    if let Some(claim) = claim {
        if claim.automatic {
            head = update_automatic_lease(&mut write, claim, epoch, at, head)?;
        } else if claim.claim.body().occupant != proven.occupant {
            let mut lease = claim.lease.body().clone();
            lease.epoch = epoch;
            let task = claim.task.clone();
            write.lease_head(task, head, move |_| lease);
        }
    }
    write.residency_head(head, move |_| next);
    commit(bus, write).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::super::launch::test_support::{AdvisoryRtkFixture, assert_advisory_health};
    use super::*;

    #[test]
    fn automatic_wake_commits_advisory_rtk_for_capability_free_readback_without_reprobe() {
        let fixture = AdvisoryRtkFixture::new();
        let bus = Bus::start(journal::Journal::in_memory().expect("fixture journal opens"))
            .expect("fixture bus starts");
        seed_vacant_lane(&bus, &fixture, "drive");
        let capabilities = canvas::axum::Capabilities::default();
        let discovery = canvas::Discovery {
            endpoint: "http://127.0.0.1:1".to_owned(),
            capability: fixture.capability,
            epoch: fixture.epoch,
        };
        let runtime = Runtime::new(&capabilities, &discovery, fixture.reporter());
        let mut seats = Seats::new();

        let result = wake_automatic(
            &bus,
            &fixture.paths,
            fixture.epoch,
            &mut seats,
            &runtime,
            fixture.lane,
            fixture.occupant,
            None,
            None,
        )
        .expect("automatic advisory-RTK launch commits");

        match result {
            AutomaticWake::Started => {}
            AutomaticWake::Unstarted(outcome) => {
                panic!("automatic advisory-RTK launch did not start: {outcome:?}")
            }
        }
        fixture.assert_occupant_started_with_rtk();
        fixture.assert_one_configured_probe_from_launch_worktree();
        let residency = current_residency(&bus, fixture.epoch).expect("committed residency reads");
        let answer = answer(residency.body(), None, &seats).expect("capability-free lane readback");
        let row = answer.lanes.first().expect("automatic lane projects");
        assert_advisory_health(row.rtk.as_ref().expect("readback retains RTK health"));
        let encoded = serde_json::to_string(&answer).expect("lane readback encodes");
        assert!(
            !encoded.contains(&fixture.capability.to_string()),
            "{encoded}"
        );
        fixture.assert_one_configured_probe_from_launch_worktree();

        stop_lane(&mut seats, fixture.lane);
        bus.verify().expect("automatic RTK authority verifies");
        bus.shutdown().expect("fixture bus stops");
    }

    #[test]
    fn explicit_wake_starts_an_already_seated_drain_lane() {
        let fixture = AdvisoryRtkFixture::new();
        let bus = Bus::start(journal::Journal::in_memory().expect("fixture journal opens"))
            .expect("fixture bus starts");
        seed_vacant_lane(&bus, &fixture, "drain");
        let capabilities = canvas::axum::Capabilities::default();
        let discovery = canvas::Discovery {
            endpoint: "http://127.0.0.1:1".to_owned(),
            capability: fixture.capability,
            epoch: fixture.epoch,
        };
        let runtime = Runtime::new(&capabilities, &discovery, fixture.reporter());
        let request = LaneState {
            lane: fixture.lane,
            action: LaneAction::Wake,
            worktree: None,
        };
        let command = admitted_operator_wake(&fixture, &request);
        let mut seats = Seats::new();

        let outcome = wake(
            &bus,
            &fixture.paths,
            fixture.epoch,
            &mut seats,
            &runtime,
            &command,
            &request,
        );

        assert!(matches!(outcome, Outcome::Reply(_)), "{outcome:?}");
        let residency = current_residency(&bus, fixture.epoch).expect("committed residency reads");
        let row = residency
            .body()
            .lanes
            .first()
            .expect("explicit lane remains seated");
        assert_eq!(row.life, "drain");
        let occupant = row.occupant.expect("explicit wake records its occupant");
        assert_eq!(row.lineage.last(), Some(&occupant));
        assert!(seats.occupied(fixture.lane));
        stop_lane(&mut seats, fixture.lane);
        bus.verify().expect("explicit wake authority verifies");
        bus.shutdown().expect("fixture bus stops");
    }

    #[test]
    fn automatic_wake_refuses_an_already_seated_drain_lane() {
        let fixture = AdvisoryRtkFixture::new();
        let bus = Bus::start(journal::Journal::in_memory().expect("fixture journal opens"))
            .expect("fixture bus starts");
        seed_vacant_lane(&bus, &fixture, "drain");
        let capabilities = canvas::axum::Capabilities::default();
        let discovery = canvas::Discovery {
            endpoint: "http://127.0.0.1:1".to_owned(),
            capability: fixture.capability,
            epoch: fixture.epoch,
        };
        let runtime = Runtime::new(&capabilities, &discovery, fixture.reporter());

        let outcome = wake_automatic(
            &bus,
            &fixture.paths,
            fixture.epoch,
            &mut Seats::new(),
            &runtime,
            fixture.lane,
            fixture.occupant,
            None,
            None,
        );

        match outcome {
            Err(Outcome::Refusal(refusal)) => {
                assert_eq!(refusal.reason(), "lifecycle::wake-life-ended");
            }
            Err(Outcome::Fault(fault)) => panic!("automatic wake faulted: {fault:?}"),
            Err(Outcome::Reply(_)) | Ok(_) => panic!("automatic wake started a Drain lane"),
        }
        bus.verify().expect("automatic refusal verifies");
        bus.shutdown().expect("fixture bus stops");
    }

    fn admitted_operator_wake(
        fixture: &AdvisoryRtkFixture,
        request: &LaneState,
    ) -> wire::AdmittedCommand {
        Command {
            v: wire::WireVersion::CURRENT,
            id: fixture.capability,
            epoch: fixture.epoch,
            kind: wire::CommandKind::LaneState,
            expected: Vec::new(),
            body: serde_json::to_value(request).expect("lane intent encodes"),
        }
        .admit(&wire::Accepted {
            wire: wire::WireVersion::CURRENT,
            actor: Actor::Operator,
            epoch: fixture.epoch,
        })
        .expect("operator wake intent admits")
    }

    fn seed_vacant_lane(bus: &Bus, fixture: &AdvisoryRtkFixture, life: &str) {
        let mut write = write(Actor::Spice, fixture.epoch, Producer::CommandDoor)
            .expect("fixture write starts");
        let seated = write
            .fact(
                wire::body::UpSeated {
                    epoch: fixture.epoch,
                },
                FactCause::Root,
            )
            .expect("fixture resident seats");
        let root = fixture.root.display().to_string();
        let vendor = fixture.vendor.clone();
        let epoch = fixture.epoch;
        let lane = fixture.lane;
        let capability = fixture.capability;
        let life = life.to_owned();
        write.residency_head(seated, move |_| wire::ResidencyHead {
            epoch,
            state: ResidentState::Seated,
            topology_revision: 1,
            lanes: vec![wire::LaneResidency {
                lane,
                lineage_id: lane,
                name: None,
                worktree: root,
                life,
                vendor,
                occupant: None,
                lineage: Vec::new(),
                capabilities: vec![capability],
                session: None,
                disposition: VACANT.to_owned(),
            }],
            subscriptions: vec![wire::LineageSubscription {
                lineage_id: lane,
                policy: wire::intent::LineageLife::Drive,
                manual_projects: Vec::new(),
                auto_projects: Vec::new(),
            }],
        });
        commit(bus, write).expect("fixture residency commits");
    }

    #[test]
    fn startup_ready_promotes_provisional_lease_to_one_hundred_seconds() {
        let epoch = "018f82d4-6f59-7d06-92e3-bce9131163aa"
            .parse()
            .expect("fixture epoch");
        let prior_epoch = "018f82d4-6f59-7d06-92e3-bce9131163ab"
            .parse()
            .expect("fixture prior epoch");
        let start: wire::Instant = "2026-08-30T08:00:00.000Z".parse().expect("fixture start");
        let provisional = wire::Instant::new(
            start
                .get()
                .checked_add(jiff::SignedDuration::from_secs(
                    crate::board_command::AUTOMATIC_PROVISIONAL_SECONDS,
                ))
                .expect("provisional deadline shifts"),
        )
        .expect("provisional deadline");
        let ready: wire::Instant = "2026-08-30T08:00:10.000Z"
            .parse()
            .expect("fixture ready time");
        let promoted = promoted_lease(
            &wire::LeaseHead {
                start,
                renewal: start,
                maximum: provisional,
                expiry: provisional,
                epoch: prior_epoch,
            },
            epoch,
            ready,
        )
        .expect("startup promotion succeeds");

        assert_eq!(promoted.start, start);
        assert_eq!(promoted.renewal, ready);
        assert_eq!(promoted.epoch, epoch);
        assert_eq!(promoted.expiry, promoted.maximum);
        assert_eq!(
            promoted.expiry,
            wire::Instant::new(
                ready
                    .get()
                    .checked_add(jiff::SignedDuration::from_secs(
                        crate::board_command::AUTOMATIC_ROLLING_SECONDS,
                    ))
                    .expect("rolling deadline shifts")
            )
            .expect("rolling deadline")
        );
    }

    #[test]
    fn startup_ready_cannot_revive_an_expired_provisional_lease() {
        let epoch = "018f82d4-6f59-7d06-92e3-bce9131163aa"
            .parse()
            .expect("fixture epoch");
        let start: wire::Instant = "2026-08-30T08:00:00.000Z".parse().expect("fixture start");
        let expiry: wire::Instant = "2026-08-30T08:01:00.000Z".parse().expect("fixture expiry");
        let observed = wire::LeaseHead {
            start,
            renewal: start,
            maximum: expiry,
            expiry,
            epoch,
        };

        let refusal = promoted_lease(&observed, epoch, expiry)
            .expect_err("an expired provisional lease cannot be promoted");

        assert!(format!("{refusal:?}").contains("lifecycle::claim-promotion-expired"));
    }
}
