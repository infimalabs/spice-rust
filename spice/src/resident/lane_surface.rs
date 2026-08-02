//! Resident-owned lane seating, lifecycle, restart, and recall handlers.

use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bus::Bus;
use journal::{FactCause, JournalWrite};
use lane::capture::{CaptureFailure, CaptureRecord, CaptureReporter};
use lane::occupant::{Occupant, OccupantWait, Seats};
use sill::{
    Cancellation, Deadlines, Environment, Observation, RestartDisposition, Spec, WaitOutcome,
    WaitPolicy,
};
use spice::repository::Repository;
use wire::intent::{
    CommandAnswer, LaneAction, LaneAnswer, LaneChangeAnswer, LaneRecall, LaneSeat, LaneState,
    RecallAnswer, RecallEntry,
};
use wire::{
    Actor, CanonicalUuid, Command, Fault, FaultClass, Outcome, Producer, Refusal, Reply,
    ReplyPosition, ResidentState, SeqRange,
};

use super::shutdown::{current_residency, guard_residency};
use super::{
    CommandResult, RuntimePaths, commit, new_uuid, seq, store_fault, typed_outcome,
    typed_store_fault, write,
};

const OCCUPIED: &str = "occupied";
const VACANT: &str = "vacant";
const DEGRADED: &str = "degraded";
const RECALL_LIMIT: usize = 16;
const CAPTURE_LIMIT: usize = 1024 * 1024;
const OUTPUT_DRAIN: Duration = Duration::from_secs(2);
const REAP: Duration = Duration::from_secs(2);
const TERMINATION_GRACE: Duration = Duration::from_secs(1);
const RECORD_QUEUE: usize = 64;

pub(super) struct Runtime<'a> {
    capabilities: &'a canvas::axum::Capabilities,
    discovery: &'a canvas::Discovery,
    capture: Arc<dyn CaptureReporter>,
}

impl<'a> Runtime<'a> {
    pub(super) const fn new(
        capabilities: &'a canvas::axum::Capabilities,
        discovery: &'a canvas::Discovery,
        capture: Arc<dyn CaptureReporter>,
    ) -> Self {
        Self {
            capabilities,
            discovery,
            capture,
        }
    }
}

/// Reconstruct every durable unfinished seat beneath the new resident epoch.
pub(super) fn restore_seats(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    capture: &Arc<dyn CaptureReporter>,
) -> CommandResult<()> {
    let durable = current_residency(bus, epoch)?.body().lanes.clone();
    for row in durable {
        if row.occupant.is_none() {
            continue;
        }
        super::steering::lane_capability(&row)?;
        let occupant = new_uuid();
        let launch = occupant_launch(
            paths,
            &row.worktree,
            &row.vendor,
            row.lane,
            occupant,
            Arc::clone(capture),
        )?;
        let group = seats
            .seat_observed(row.lane, occupant, launch.spec, launch.observation)
            .map_err(|error| store_fault("lane::restart-launch-failed", &error.to_string()))?
            .group();
        if let Err(outcome) = commit_restored(bus, epoch, &row, occupant, group) {
            drop(seats.vacate(row.lane));
            return Err(outcome);
        }
    }
    Ok(())
}

/// Read the current lane table at one residency barrier.
pub(super) fn read(
    bus: &Bus,
    _paths: &RuntimePaths,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
) -> Outcome<CommandAnswer> {
    if !matches!(command.actor(), Actor::Operator) {
        return authority_refusal(command.actor(), "operator");
    }
    let head = match current_residency(bus, epoch) {
        Ok(head) => head,
        Err(outcome) => return typed_outcome(outcome),
    };
    Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(head.position()),
        body: CommandAnswer::Lane(Box::new(answer(head.body(), None))),
    })
}

/// Seat one new lane, capability, and first supervised occupant.
pub(super) fn seat(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    runtime: &Runtime<'_>,
    command: &wire::AdmittedCommand,
    request: &LaneSeat,
) -> Outcome<CommandAnswer> {
    if !matches!(command.actor(), Actor::Operator) {
        return authority_refusal(command.actor(), "operator");
    }
    let observed = match admit_seat(bus, paths, epoch, request) {
        Ok(observed) => observed,
        Err(outcome) => return outcome,
    };
    let prepared = match prepare_seat(paths, seats, runtime, request) {
        Ok(prepared) => prepared,
        Err(outcome) => return outcome,
    };
    let answer = LaneChangeAnswer {
        lane: request.lane,
        from: "absent".to_owned(),
        to: OCCUPIED.to_owned(),
    };
    let proposal = propose_seat(
        command,
        request,
        &observed,
        epoch,
        prepared.capability,
        prepared.occupant,
        prepared.group,
    );
    let proposal = match proposal {
        Ok(proposal) => proposal,
        Err(outcome) => {
            compensate_seat(paths, seats, runtime, request.lane, prepared.capability);
            return typed_outcome(outcome);
        }
    };
    match commit(bus, proposal) {
        Ok(committed) => committed_lane_reply(committed, answer),
        Err(outcome) => {
            compensate_seat(paths, seats, runtime, request.lane, prepared.capability);
            typed_outcome(outcome)
        }
    }
}

fn admit_seat(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    request: &LaneSeat,
) -> Result<journal::JournalHead<wire::ResidencyHead>, Outcome<CommandAnswer>> {
    if !matches!(request.life.as_str(), "steer" | "drive" | "drain") {
        return Err(lane_refusal(
            "lane::life-invalid",
            Some("steer, drive, or drain".to_owned()),
            Some(request.life.clone()),
            "select one closed lane life",
        ));
    }
    let observed = current_residency(bus, epoch).map_err(typed_outcome)?;
    if observed.body().state != ResidentState::Seated {
        return Err(lane_refusal(
            "lane::resident-not-seated",
            Some("seated".to_owned()),
            Some(format!("{:?}", observed.body().state).to_lowercase()),
            "wait for a seated resident generation before adding a lane",
        ));
    }
    if observed
        .body()
        .lanes
        .iter()
        .any(|row| row.lane == request.lane)
    {
        return Err(lane_refusal(
            "lane::already-seated",
            None,
            Some(request.lane.to_string()),
            "retry lane add so the client mints a fresh lane identity",
        ));
    }
    if observed
        .body()
        .lanes
        .iter()
        .any(|row| row.worktree == request.worktree)
    {
        return Err(lane_refusal(
            "lane::worktree-occupied",
            Some("one lane per worktree".to_owned()),
            Some(request.worktree.clone()),
            "drain the existing lane or choose another linked worktree",
        ));
    }
    validate_worktree(paths, &request.worktree).map_err(typed_outcome)?;
    Ok(observed)
}

struct PreparedSeat {
    capability: CanonicalUuid,
    occupant: CanonicalUuid,
    group: u32,
}

fn prepare_seat(
    paths: &RuntimePaths,
    seats: &mut Seats,
    runtime: &Runtime<'_>,
    request: &LaneSeat,
) -> Result<PreparedSeat, Outcome<CommandAnswer>> {
    let capability = new_uuid();
    let actor = Actor::Lane(request.lane);
    runtime
        .capabilities
        .insert(capability, actor.clone())
        .map_err(|error| {
            typed_environment_fault("lane::capability-unavailable", &error.to_string())
        })?;
    if let Err(outcome) =
        super::steering::publish_lane_surface(paths, runtime.discovery, request.lane, capability)
    {
        runtime.capabilities.remove(capability, &actor);
        return Err(typed_outcome(outcome));
    }
    let occupant = new_uuid();
    let launch = occupant_launch(
        paths,
        &request.worktree,
        &request.vendor,
        request.lane,
        occupant,
        Arc::clone(&runtime.capture),
    )
    .map_err(|outcome| {
        remove_seat_surface(paths, runtime, request.lane, capability);
        typed_outcome(outcome)
    })?;
    let group = seats
        .seat_observed(request.lane, occupant, launch.spec, launch.observation)
        .map(lane::occupant::Occupant::group)
        .map_err(|error| {
            remove_seat_surface(paths, runtime, request.lane, capability);
            typed_environment_fault("lane::launch-failed", &error.to_string())
        })?;
    Ok(PreparedSeat {
        capability,
        occupant,
        group,
    })
}

fn compensate_seat(
    paths: &RuntimePaths,
    seats: &mut Seats,
    runtime: &Runtime<'_>,
    lane: CanonicalUuid,
    capability: CanonicalUuid,
) {
    drop(seats.vacate(lane));
    remove_seat_surface(paths, runtime, lane, capability);
}

fn remove_seat_surface(
    paths: &RuntimePaths,
    runtime: &Runtime<'_>,
    lane: CanonicalUuid,
    capability: CanonicalUuid,
) {
    runtime.capabilities.remove(capability, &Actor::Lane(lane));
    super::steering::remove_lane_surface(paths, lane);
}

/// Drain or wake one current lane.
pub(super) fn state(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    runtime: &Runtime<'_>,
    command: &wire::AdmittedCommand,
    request: &LaneState,
) -> Outcome<CommandAnswer> {
    if !matches!(command.actor(), Actor::Operator) {
        return authority_refusal(command.actor(), "operator");
    }
    match request.action {
        LaneAction::Drain => drain(bus, epoch, seats, command, request),
        LaneAction::Wake => wake(bus, paths, epoch, seats, runtime, command, request),
    }
}

/// Admit one already-durable capture report from the current occupant.
///
/// Stale reports can race a completed drain or renewal and are discarded by
/// exact occupant identity. Session recognition consumes this same event seam.
pub(super) fn capture_record(seats: &Seats, record: &CaptureRecord) -> CommandResult<()> {
    if seats
        .occupant(record.lane)
        .is_some_and(|occupant| occupant.occupant() == record.occupant)
    {
        return Ok(());
    }
    Ok(())
}

/// Vacate and durably degrade an occupant whose selected capture path failed.
pub(super) fn capture_failed(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    failure: &CaptureFailure,
) -> CommandResult<()> {
    let observed = current_residency(bus, epoch)?;
    let Some(row) = observed
        .body()
        .lanes
        .iter()
        .find(|row| row.lane == failure.lane)
    else {
        return Ok(());
    };
    if row.occupant != Some(failure.occupant) {
        return Ok(());
    }

    if seats
        .occupant(failure.lane)
        .is_some_and(|occupant| occupant.occupant() == failure.occupant)
    {
        drop(seats.vacate(failure.lane));
    }
    let mut next = observed.body().clone();
    let row = next
        .lanes
        .iter_mut()
        .find(|row| row.lane == failure.lane)
        .expect("observed capture lane remains in its cloned head");
    row.occupant = None;
    row.disposition = DEGRADED.to_owned();
    let mut write = write(Actor::Spice, epoch, Producer::ResidentRecognition)?;
    guard_residency(&mut write, &observed);
    let fact = write
        .fact(
            wire::body::LaneDegraded {
                lane: failure.lane,
                marker: format!(
                    "occupant {} capture failed: {}",
                    failure.occupant, failure.failure
                ),
            },
            FactCause::Existing(observed.position()),
        )
        .map_err(|error| store_fault("lane::capture-fact-invalid", &error.to_string()))?;
    write.residency_head(fact, move |_| next);
    commit(bus, write).map(|_| ())
}

/// Build a bounded continuity account under the authenticated principal.
pub(super) fn recall(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    request: &LaneRecall,
) -> Outcome<CommandAnswer> {
    let selected = match command.actor() {
        Actor::Operator => request.lane,
        Actor::Lane(lane) if request.lane.is_none_or(|requested| requested == *lane) => Some(*lane),
        Actor::Lane(lane) => {
            return lane_refusal(
                "recall::lane-authority",
                Some(lane.to_string()),
                request.lane.map(|lane| lane.to_string()),
                "a lane may recall only its own continuity account",
            );
        }
        Actor::Spice => return authority_refusal(command.actor(), "operator or lane"),
    };
    let head = match current_residency(bus, epoch) {
        Ok(head) => head,
        Err(outcome) => return typed_outcome(outcome),
    };
    if let Some(lane) = selected
        && !head.body().lanes.iter().any(|row| row.lane == lane)
    {
        return lane_refusal(
            "recall::lane-missing",
            Some("one current lane".to_owned()),
            Some(lane.to_string()),
            "read lane and retry with a current lane identity",
        );
    }
    let snapshot = match bus.fact_snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => return typed_store_fault("recall::journal-unreadable", &error.to_string()),
    };
    let barrier = head.position();
    let recent = snapshot
        .facts()
        .iter()
        .rev()
        .filter(|fact| fact.seq <= barrier)
        .filter(|fact| selected.is_none_or(|lane| relevant(fact, lane)))
        .take(RECALL_LIMIT)
        .map(|fact| RecallEntry {
            seq: fact.seq,
            kind: fact.fact.kind(),
            actor: fact.actor.clone(),
            origin: fact.origin.clone(),
        })
        .collect();
    let body = RecallAnswer {
        barrier,
        repository: paths.worktree.display().to_string(),
        lane: selected,
        residency: answer(head.body(), selected),
        recent,
    };
    Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(barrier),
        body: CommandAnswer::Recall(Box::new(body)),
    })
}

fn drain(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
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
    let Some(durable_occupant) = row.occupant else {
        return lane_refusal(
            "lane::already-vacant",
            Some(OCCUPIED.to_owned()),
            Some(row.disposition.clone()),
            "wake the lane before asking it to drain again",
        );
    };
    let Some(occupant) = seats.vacate(request.lane) else {
        return typed_store_fault(
            "lane::occupant-unowned",
            "durable lane occupant has no process group supervised by this resident",
        );
    };
    if occupant.occupant() != durable_occupant {
        drop(occupant);
        return typed_store_fault(
            "lane::occupant-moved",
            "supervised process group does not match durable residency",
        );
    }
    let evidence = match stop_occupant(occupant) {
        Ok(evidence) => evidence,
        Err(error) => return typed_environment_fault("lane::drain-failed", &error),
    };
    let answer = LaneChangeAnswer {
        lane: request.lane,
        from: OCCUPIED.to_owned(),
        to: VACANT.to_owned(),
    };
    let proposal = match propose_drain(
        command,
        request,
        &observed,
        epoch,
        durable_occupant,
        evidence,
        answer.clone(),
    ) {
        Ok(proposal) => proposal,
        Err(outcome) => return typed_outcome(outcome),
    };
    match commit(bus, proposal) {
        Ok(committed) => committed_lane_reply(committed, answer),
        Err(outcome) => typed_outcome(outcome),
    }
}

fn wake(
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
    if let Err(outcome) = super::steering::lane_capability(row) {
        return typed_outcome(outcome);
    }
    let occupant = new_uuid();
    let launch = match occupant_launch(
        paths,
        &row.worktree,
        &row.vendor,
        row.lane,
        occupant,
        Arc::clone(&runtime.capture),
    ) {
        Ok(launch) => launch,
        Err(outcome) => return typed_outcome(outcome),
    };
    let group = match seats.seat_observed(request.lane, occupant, launch.spec, launch.observation) {
        Ok(occupant) => occupant.group(),
        Err(error) => return typed_environment_fault("lane::wake-failed", &error.to_string()),
    };
    let answer = LaneChangeAnswer {
        lane: request.lane,
        from: row.disposition.clone(),
        to: OCCUPIED.to_owned(),
    };
    let proposal = propose_wake(
        command,
        request,
        &observed,
        epoch,
        occupant,
        group,
        answer.clone(),
    );
    let proposal = match proposal {
        Ok(proposal) => proposal,
        Err(outcome) => {
            drop(seats.vacate(request.lane));
            return typed_outcome(outcome);
        }
    };
    match commit(bus, proposal) {
        Ok(committed) => committed_lane_reply(committed, answer),
        Err(outcome) => {
            drop(seats.vacate(request.lane));
            typed_outcome(outcome)
        }
    }
}

fn propose_seat(
    command: &wire::AdmittedCommand,
    request: &LaneSeat,
    observed: &journal::JournalHead<wire::ResidencyHead>,
    epoch: CanonicalUuid,
    capability: CanonicalUuid,
    occupant: CanonicalUuid,
    group: u32,
) -> CommandResult<JournalWrite> {
    let mut next = observed.body().clone();
    next.lanes.push(wire::LaneResidency {
        lane: request.lane,
        worktree: request.worktree.clone(),
        life: request.life.clone(),
        vendor: request.vendor.clone(),
        occupant: Some(occupant),
        lineage: vec![occupant],
        capabilities: vec![capability],
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
                capability,
                worktree: request.worktree.clone(),
                life: request.life.clone(),
                vendor: request.vendor.clone(),
            },
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
            FactCause::Pending(seated),
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

fn propose_drain(
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

fn propose_wake(
    command: &wire::AdmittedCommand,
    request: &LaneState,
    observed: &journal::JournalHead<wire::ResidencyHead>,
    epoch: CanonicalUuid,
    occupant: CanonicalUuid,
    group: u32,
    answer: LaneChangeAnswer,
) -> CommandResult<JournalWrite> {
    let mut next = observed.body().clone();
    let row = next
        .lanes
        .iter_mut()
        .find(|row| row.lane == request.lane)
        .expect("observed lane remains in its cloned head");
    row.occupant = Some(occupant);
    row.lineage.push(occupant);
    row.disposition = OCCUPIED.to_owned();
    let mut write = write(
        command.actor().clone(),
        epoch,
        Producer::ResidentRecognition,
    )?;
    guard_residency(&mut write, observed);
    let started = write
        .fact(
            wire::body::LaneStarted {
                lane: request.lane,
                occupant,
                epoch,
                group,
            },
            FactCause::Existing(observed.position()),
        )
        .map_err(|error| store_fault("lane::fact-invalid", &error.to_string()))?;
    write.residency_head(started, move |_| next);
    command_result(&mut write, command, request.clone(), epoch, answer)?;
    Ok(write)
}

fn commit_restored(
    bus: &Bus,
    epoch: CanonicalUuid,
    durable: &wire::LaneResidency,
    occupant: CanonicalUuid,
    group: u32,
) -> CommandResult<()> {
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
        write
            .fact(
                wire::body::LaneRenewed {
                    lane: row.lane,
                    occupant,
                    ancestor,
                    epoch,
                    group,
                },
                FactCause::Pending(degraded),
            )
            .map_err(|error| store_fault("lane::restart-fact-invalid", &error.to_string()))?
    } else {
        write
            .fact(
                wire::body::LaneStarted {
                    lane: row.lane,
                    occupant,
                    epoch,
                    group,
                },
                FactCause::Existing(observed.position()),
            )
            .map_err(|error| store_fault("lane::restart-fact-invalid", &error.to_string()))?
    };
    row.occupant = Some(occupant);
    row.lineage.push(occupant);
    row.disposition = OCCUPIED.to_owned();
    write.residency_head(fact, move |_| next);
    commit(bus, write).map(|_| ())
}

fn command_result<B: serde::Serialize>(
    write: &mut JournalWrite,
    admitted: &wire::AdmittedCommand,
    body: B,
    epoch: CanonicalUuid,
    answer: LaneChangeAnswer,
) -> CommandResult<()> {
    write
        .command_result(
            Command {
                v: wire::WireVersion::CURRENT,
                id: admitted.id(),
                epoch: admitted.epoch(),
                kind: admitted.kind(),
                expected: admitted.expected().to_vec(),
                body,
            },
            epoch,
            answer,
        )
        .map_err(|error| store_fault("lane::receipt-invalid", &error.to_string()))
}

struct OccupantLaunch {
    spec: Spec,
    observation: Observation,
}

fn occupant_launch(
    paths: &RuntimePaths,
    worktree: &str,
    name: &wire::Name,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    reporter: Arc<dyn CaptureReporter>,
) -> CommandResult<OccupantLaunch> {
    let repository = Repository::discover(worktree)
        .map_err(|error| store_fault("lane::worktree-unresolved", &error.to_string()))?;
    if repository.common_dir() != paths.common || repository.worktree_root() != Path::new(worktree)
    {
        return Err(store_fault(
            "lane::worktree-moved",
            "durable lane worktree no longer names one current repository worktree root",
        ));
    }
    let resolved = spice::config::load_repository(&repository, &[])
        .map_err(|error| store_fault("lane::configuration-refused", &error.to_string()))?;
    let binary = resolved
        .root()
        .vendor()
        .entries()
        .get(name)
        .map_or(name.as_str(), |entry| entry.binary().as_str());
    let executable = std::env::current_exe()
        .map_err(|error| store_fault("lane::executable-unresolved", &error.to_string()))?;
    let executable = executable.display().to_string();
    let launch = vendor::Launch {
        cwd: repository.worktree_root().to_path_buf(),
        prompt: "[$spice](.agents/skills/spice/SKILL.md)".to_owned(),
        model: match name.as_str() {
            "claude" => "sonnet",
            "codex" => "gpt-5.2-codex",
            other => {
                return Err(Outcome::Refusal(Refusal {
                    reason: "lane::vendor-unsupported".to_owned(),
                    witness: Some("vendor".to_owned()),
                    expected: Some("claude or codex".to_owned()),
                    observed: Some(other.to_owned()),
                    remedy: "select a vendor with a landed Rust launch dialect".to_owned(),
                }));
            }
        }
        .to_owned(),
        effort: vendor::Effort::High,
        personality: None,
        fast_mode: false,
        attribution: vendor::Attribution::Harness,
        hook: vendor::SteeringHook {
            command: format!("{} inbox", shell_word(&executable)),
        },
        steering_key: Some(vendor::SteeringKey {
            token: new_uuid().to_string(),
        }),
        mcp: vendor::McpServer {
            name: "spice".to_owned(),
            command: executable,
            args: vec!["mcp".to_owned()],
        },
        resume: vendor::Resume::Fresh,
    };
    let record = match name.as_str() {
        "claude" => vendor::claude::record(binary, &launch),
        "codex" => vendor::codex::record(binary, &launch),
        _ => unreachable!("unsupported vendors return above"),
    }
    .map_err(|error| store_fault("lane::launch-invalid", &error.to_string()))?;
    let mut environment = ["HOME", "PATH", "SHELL", "TMPDIR", "USER"]
        .into_iter()
        .filter_map(|name| crate::bootstrap_environment(name).map(|value| (name.into(), value)))
        .collect::<Vec<(OsString, OsString)>>();
    environment.push(("SPICE_LANE".into(), lane.to_string().into())); // env-policy: allow
    let environment = Environment::new(environment)
        .map_err(|error| store_fault("lane::environment-invalid", &error.to_string()))?;
    let deadlines = Deadlines::new(resolved.root().lane().lease().get(), OUTPUT_DRAIN, REAP)
        .map_err(|error| store_fault("lane::deadlines-invalid", &error.to_string()))?;
    let wait = WaitPolicy::new(
        format!("lane {lane}"),
        Cancellation::Never,
        RestartDisposition::CallerDecides,
    )
    .map_err(|error| store_fault("lane::wait-invalid", &error.to_string()))?;
    let process = vendor::Process::new(
        repository.worktree_root(),
        environment,
        deadlines,
        wait,
        CAPTURE_LIMIT,
        TERMINATION_GRACE,
    )
    .map_err(|error| store_fault("lane::process-invalid", &error.to_string()))?;
    let spec = record.spec(&process);
    let tape = tape::Tape::open(paths.tape.join(lane.to_string()).join(name.as_str()))
        .map_err(|error| store_fault("lane::capture-tape-unavailable", &error.to_string()))?;
    let observation = lane::capture::observation(
        lane,
        occupant,
        record,
        tape,
        CAPTURE_LIMIT,
        RECORD_QUEUE,
        reporter,
    )
    .map_err(|error| store_fault("lane::capture-invalid", &error.to_string()))?;
    Ok(OccupantLaunch { spec, observation })
}

fn validate_worktree(paths: &RuntimePaths, worktree: &str) -> CommandResult<()> {
    let repository = Repository::discover(worktree)
        .map_err(|error| store_fault("lane::worktree-unresolved", &error.to_string()))?;
    if repository.common_dir() != paths.common {
        return Err(Outcome::Refusal(Refusal {
            reason: "lane::foreign-worktree".to_owned(),
            witness: Some("git-common-directory".to_owned()),
            expected: Some(paths.common.display().to_string()),
            observed: Some(repository.common_dir().display().to_string()),
            remedy: "choose one existing worktree from the resident repository".to_owned(),
        }));
    }
    if repository.worktree_root() != Path::new(worktree) {
        return Err(Outcome::Refusal(Refusal {
            reason: "lane::worktree-not-root".to_owned(),
            witness: Some("worktree".to_owned()),
            expected: Some(repository.worktree_root().display().to_string()),
            observed: Some(worktree.to_owned()),
            remedy: "name the canonical root of one existing worktree".to_owned(),
        }));
    }
    Ok(())
}

enum StopEvidence {
    Drained,
    Exited(i32),
}

fn stop_occupant(occupant: Occupant) -> Result<StopEvidence, String> {
    let occupant = match occupant
        .wait_for(Duration::ZERO)
        .map_err(|error| error.to_string())?
    {
        OccupantWait::Exited { outcome, .. } => {
            return Ok(StopEvidence::Exited(exit_code(&outcome)));
        }
        OccupantWait::Running(occupant) => occupant,
    };
    occupant.terminate().map_err(|error| error.to_string())?;
    let grace = occupant.termination_grace();
    match occupant
        .wait_for(grace)
        .map_err(|error| error.to_string())?
    {
        OccupantWait::Exited { outcome, .. } => Ok(stop_evidence(&outcome)),
        OccupantWait::Running(occupant) => {
            occupant.kill().map_err(|error| error.to_string())?;
            let reap = occupant.reap_bound();
            match occupant.wait_for(reap).map_err(|error| error.to_string())? {
                OccupantWait::Exited { outcome, .. } => {
                    Ok(StopEvidence::Exited(exit_code(&outcome)))
                }
                OccupantWait::Running(_) => {
                    Err("process group survived its configured reap bound".to_owned())
                }
            }
        }
    }
}

fn stop_evidence(outcome: &WaitOutcome) -> StopEvidence {
    if outcome.completion().status().success() {
        StopEvidence::Drained
    } else {
        StopEvidence::Exited(exit_code(outcome))
    }
}

fn exit_code(outcome: &WaitOutcome) -> i32 {
    outcome.completion().status().code().unwrap_or(-1)
}

fn answer(head: &wire::ResidencyHead, selected: Option<CanonicalUuid>) -> LaneAnswer {
    let lanes = head
        .lanes
        .iter()
        .filter(|row| selected.is_none_or(|lane| row.lane == lane))
        .cloned()
        .map(|mut row| {
            row.capabilities.clear();
            row
        })
        .collect();
    LaneAnswer {
        epoch: head.epoch,
        state: head.state,
        lanes,
    }
}

fn relevant(fact: &wire::Envelope<wire::AnyFact>, lane: CanonicalUuid) -> bool {
    fact.actor == Actor::Lane(lane)
        || fact
            .fact
            .body()
            .get("lane")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|named| named == lane.to_string())
}

fn committed_lane_reply(
    committed: bus::JournalCommit,
    answer: LaneChangeAnswer,
) -> Outcome<CommandAnswer> {
    Outcome::Reply(Reply {
        position: ReplyPosition::Committed(SeqRange {
            first: seq(committed.first()),
            last: seq(committed.last()),
        }),
        body: CommandAnswer::LaneChange(answer),
    })
}

fn authority_refusal(actor: &Actor, expected: &str) -> Outcome<CommandAnswer> {
    lane_refusal(
        "command::authority",
        Some(expected.to_owned()),
        Some(actor.to_string()),
        "reconnect with a capability admitted for this command family",
    )
}

fn missing_lane(lane: CanonicalUuid) -> Outcome<CommandAnswer> {
    lane_refusal(
        "lane::missing",
        Some("one current lane".to_owned()),
        Some(lane.to_string()),
        "read lane and retry with a current lane identity",
    )
}

fn lane_refusal(
    reason: &str,
    expected: Option<String>,
    observed: Option<String>,
    remedy: &str,
) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: Some("lane".to_owned()),
        expected,
        observed,
        remedy: remedy.to_owned(),
    })
}

fn typed_environment_fault(reason: &str, detail: &str) -> Outcome<CommandAnswer> {
    Outcome::Fault(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}

fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
