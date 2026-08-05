//! Resident-owned lane seating, lifecycle, restart, and recall handlers.

use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use bus::Bus;
use journal::{FactCause, JournalWrite};
use lane::capture::{CaptureFailure, CaptureRecord, CaptureReportError, CaptureReporter};
use lane::occupant::{Occupant, OccupantWait, Seats};
use sill::WaitOutcome;
use spice::repository::Repository;
use wire::intent::{
    CommandAnswer, LaneAction, LaneAnswer, LaneChangeAnswer, LaneRecall, LaneSeat, LaneState,
    RecallAnswer, RecallEntry, RecallProvenance,
};
use wire::{
    Actor, CanonicalUuid, Command, Fault, FaultClass, Outcome, Producer, Refusal, Reply,
    ReplyPosition, ResidentState, SeqRange,
};

mod launch;
mod propose;
mod reply;
mod session;

use launch::{OccupantLaunchRequest, occupant_launch};
use propose::{commit_restored, propose_drain, propose_seat, propose_wake};
use reply::{
    answer, authority_refusal, command_result, committed_lane_reply, fact_lane, lane_refusal,
    missing_lane, recall_provenance, relevant, typed_environment_fault,
};
use session::{DeclarationReporter, DeclarationState, SessionProofError, await_session};
pub(super) use session::{record_stream, resume_session, session_fact};

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
const SESSION_PROOF: Duration = Duration::from_secs(10);
const SESSION_SETTLE: Duration = Duration::from_millis(25);

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
        let resume = resume_session(paths, &row)?;
        let occupant = new_uuid();
        let launch = occupant_launch(
            paths,
            OccupantLaunchRequest {
                worktree: &row.worktree,
                vendor: &row.vendor,
                epoch,
                lane: row.lane,
                occupant,
                resume,
            },
            Arc::clone(capture),
        )?;
        let session_receiver = launch.session;
        let group = seats
            .seat_observed(row.lane, occupant, launch.spec, launch.observation)
            .map_err(|error| store_fault("lane::restart-launch-failed", &error.to_string()))?
            .group();
        let session = match await_session(session_receiver) {
            Ok(session) => session,
            Err(error) => {
                stop_lane(seats, row.lane);
                return Err(Outcome::Refusal(error.refusal()));
            }
        };
        let proven = ProvenOccupant {
            occupant,
            group,
            session,
        };
        if let Err(outcome) = commit_restored(bus, epoch, &row, &proven) {
            stop_lane(seats, row.lane);
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
    _command: &wire::AdmittedCommand,
) -> Outcome<CommandAnswer> {
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
    let observed = match admit_seat(bus, paths, epoch, request) {
        Ok(observed) => observed,
        Err(outcome) => return outcome,
    };
    let prepared = match prepare_seat(paths, epoch, seats, runtime, request) {
        Ok(prepared) => prepared,
        Err(outcome) => return outcome,
    };
    let answer = LaneChangeAnswer {
        lane: request.lane,
        from: "absent".to_owned(),
        to: OCCUPIED.to_owned(),
    };
    let proposal = propose_seat(command, request, &observed, epoch, &prepared);
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
    proven: ProvenOccupant,
}

struct ProvenOccupant {
    occupant: CanonicalUuid,
    group: u32,
    session: wire::LaneSession,
}

fn prepare_seat(
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
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
        OccupantLaunchRequest {
            worktree: &request.worktree,
            vendor: &request.vendor,
            epoch,
            lane: request.lane,
            occupant,
            resume: vendor::Resume::Fresh,
        },
        Arc::clone(&runtime.capture),
    )
    .map_err(|outcome| {
        remove_seat_surface(paths, runtime, request.lane, capability);
        typed_outcome(outcome)
    })?;
    let session_receiver = launch.session;
    let group = seats
        .seat_observed(request.lane, occupant, launch.spec, launch.observation)
        .map(lane::occupant::Occupant::group)
        .map_err(|error| {
            remove_seat_surface(paths, runtime, request.lane, capability);
            typed_environment_fault("lane::launch-failed", &error.to_string())
        })?;
    let session = await_session(session_receiver).map_err(|error| {
        stop_lane(seats, request.lane);
        remove_seat_surface(paths, runtime, request.lane, capability);
        error.outcome()
    })?;
    Ok(PreparedSeat {
        capability,
        proven: ProvenOccupant {
            occupant,
            group,
            session,
        },
    })
}

fn compensate_seat(
    paths: &RuntimePaths,
    seats: &mut Seats,
    runtime: &Runtime<'_>,
    lane: CanonicalUuid,
    capability: CanonicalUuid,
) {
    stop_lane(seats, lane);
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
    match request.action {
        LaneAction::Drain => drain(bus, epoch, seats, command, request),
        LaneAction::Wake => wake(bus, paths, epoch, seats, runtime, command, request),
    }
}

/// Admit one already-durable capture report from the current occupant.
///
/// Stale reports can race a completed drain or renewal and are discarded by
/// exact occupant identity. Session recognition consumes this same event seam.
pub(super) fn capture_record(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &Seats,
    record: &CaptureRecord,
) -> CommandResult<()> {
    if !seats
        .occupant(record.lane)
        .is_some_and(|occupant| occupant.occupant() == record.occupant)
    {
        return Ok(());
    }
    if let Some(declaration) = &record.declaration {
        let observed = current_residency(bus, epoch)?;
        let current = observed
            .body()
            .lanes
            .iter()
            .find(|row| row.lane == record.lane)
            .and_then(|row| row.session.as_ref())
            .ok_or_else(|| {
                store_fault(
                    "lane::session-head-missing",
                    "a live declaration has no durable session authority",
                )
            })?;
        let same = current.occupant == record.occupant
            && current.identity == declaration.session.as_str()
            && current.stream == record_stream(record.stream)
            && current.locus.member == declaration.locus.member
            && current.locus.offset == declaration.locus.offset
            && current.locus.length == declaration.locus.length;
        if !same {
            return Err(store_fault(
                "lane::session-head-moved",
                concat!(
                    "the live declaration does not equal its committed occupant, ",
                    "identity, stream, and locus",
                ),
            ));
        }
    }
    let compactions = record
        .decoded
        .events()
        .iter()
        .filter_map(|event| match event {
            listen::Event::Compaction(boundary) => Some(boundary.locus),
            _ => None,
        })
        .collect::<Vec<_>>();
    if !compactions.is_empty() {
        let observed = current_residency(bus, epoch)?;
        let current = observed
            .body()
            .lanes
            .iter()
            .find(|row| row.lane == record.lane)
            .ok_or_else(|| {
                store_fault(
                    "lane::compaction-head-missing",
                    "a live compaction has no durable lane authority",
                )
            })?;
        if current.occupant != Some(record.occupant) {
            return Err(store_fault(
                "lane::compaction-occupant-moved",
                "the compaction occupant does not equal durable residency",
            ));
        }
        let mut write = write(Actor::Spice, epoch, Producer::ResidentRecognition)?;
        guard_residency(&mut write, &observed);
        let mut cause = FactCause::Existing(observed.position());
        for locus in compactions {
            let fact = write
                .fact(
                    wire::body::LaneCompacted {
                        lane: record.lane,
                        occupant: record.occupant,
                        stream: record_stream(record.stream).to_owned(),
                        member: locus.member,
                        offset: locus.offset,
                        length: locus.length,
                    },
                    cause,
                )
                .map_err(|error| {
                    store_fault("lane::compaction-fact-invalid", &error.to_string())
                })?;
            cause = FactCause::Pending(fact);
        }
        commit(bus, write)?;
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
    let snapshot = match bus.canvas_snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return typed_store_fault("recall::journal-unreadable", &error.to_string());
        }
    };
    let Some(head) = snapshot.residency() else {
        return typed_store_fault("resident::head-missing", "residency head is absent");
    };
    if head.body().epoch != epoch {
        return typed_store_fault(
            "resident::epoch-moved",
            &format!("expected {epoch}, observed {}", head.body().epoch),
        );
    }
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
    let Some(barrier) = snapshot.cursor() else {
        return typed_store_fault(
            "recall::journal-empty",
            "a running resident has no durable fact cursor",
        );
    };
    let mut recent = Vec::new();
    for fact in snapshot
        .facts()
        .facts()
        .iter()
        .rev()
        .filter(|fact| fact.seq <= barrier)
        .filter(|fact| selected.is_none_or(|lane| relevant(fact, lane)))
        .take(RECALL_LIMIT)
    {
        let provenance = match recall_provenance(fact) {
            Ok(provenance) => provenance,
            Err(outcome) => return typed_outcome(outcome),
        };
        recent.push(RecallEntry {
            seq: fact.seq,
            kind: fact.fact.kind(),
            actor: fact.actor.clone(),
            origin: fact.origin.clone(),
            lane: fact_lane(fact),
            provenance,
        });
    }
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
    let session_receiver = launch.session;
    let group = match seats.seat_observed(request.lane, occupant, launch.spec, launch.observation) {
        Ok(occupant) => occupant.group(),
        Err(error) => return typed_environment_fault("lane::wake-failed", &error.to_string()),
    };
    let session = match await_session(session_receiver) {
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
    };
    let answer = LaneChangeAnswer {
        lane: request.lane,
        from: row.disposition.clone(),
        to: OCCUPIED.to_owned(),
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

fn stop_lane(seats: &mut Seats, lane: CanonicalUuid) {
    if let Some(occupant) = seats.vacate(lane) {
        let _stopped = stop_occupant(occupant);
    }
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
