//! Resident-owned lane seating, lifecycle, restart, and recall handlers.

use std::ffi::OsString;
use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use bus::Bus;
use journal::{FactCause, JournalWrite};
use lane::capture::{CaptureFailure, CaptureRecord, CaptureReportError, CaptureReporter};
use lane::occupant::{Occupant, OccupantWait, Seats};
use sill::{
    Cancellation, Deadlines, Environment, Observation, RestartDisposition, Spec, WaitOutcome,
    WaitPolicy,
};
use spice::repository::Repository;
use wire::intent::{
    CommandAnswer, LaneAction, LaneAnswer, LaneChangeAnswer, LaneRecall, LaneSeat, LaneState,
    RecallAnswer, RecallEntry, RecallProvenance,
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
const SESSION_PROOF: Duration = Duration::from_secs(2);
const SESSION_SETTLE: Duration = Duration::from_millis(25);

#[derive(Clone, Debug, Eq, PartialEq)]
enum SessionProofError {
    Missing,
    Capture(String),
    CrossLane {
        expected: CanonicalUuid,
        observed: CanonicalUuid,
    },
    CrossOccupant {
        expected: CanonicalUuid,
        observed: CanonicalUuid,
    },
    WrongStream,
    Mismatched {
        expected: String,
        observed: String,
    },
    Duplicate {
        identity: String,
    },
}

impl SessionProofError {
    fn refusal(&self) -> Refusal {
        let (reason, expected, observed, remedy) = match self {
            Self::Missing => (
                "lane::session-missing",
                Some("one current selected-stream session declaration".to_owned()),
                None,
                "retry with a vendor that emits its current initialization record",
            ),
            Self::Capture(detail) => (
                "lane::session-capture-failed",
                Some("one intact selected-stream capture".to_owned()),
                Some(detail.clone()),
                "repair the capture path and retry the lane launch",
            ),
            Self::CrossLane { expected, observed } => (
                "lane::session-cross-lane",
                Some(expected.to_string()),
                Some(observed.to_string()),
                "use only the declaration emitted by this lane's supervised occupant",
            ),
            Self::CrossOccupant { expected, observed } => (
                "lane::session-cross-occupant",
                Some(expected.to_string()),
                Some(observed.to_string()),
                "use only the declaration emitted by the current supervised occupant",
            ),
            Self::WrongStream => (
                "lane::session-wrong-stream",
                Some("the vendor-selected record stream".to_owned()),
                Some("another child stream".to_owned()),
                "declare the session on the selected JSONL stream",
            ),
            Self::Mismatched { expected, observed } => (
                "lane::session-mismatched",
                Some(expected.clone()),
                Some(observed.clone()),
                "resume the exact durable session identity",
            ),
            Self::Duplicate { identity } => (
                "lane::session-duplicate",
                Some("one declaration per occupant".to_owned()),
                Some(identity.clone()),
                "emit the initialization declaration exactly once",
            ),
        };
        Refusal {
            reason: reason.to_owned(),
            witness: Some("session-declaration".to_owned()),
            expected,
            observed,
            remedy: remedy.to_owned(),
        }
    }

    fn outcome(&self) -> Outcome<CommandAnswer> {
        Outcome::Refusal(self.refusal())
    }
}

struct DeclarationState {
    reply: mpsc::SyncSender<Result<wire::LaneSession, SessionProofError>>,
    seen: Option<String>,
}

struct DeclarationReporter {
    inner: Arc<dyn CaptureReporter>,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    worktree: String,
    vendor: wire::Name,
    epoch: CanonicalUuid,
    stream: vendor::RecordStream,
    expected: Option<String>,
    state: Mutex<DeclarationState>,
}

impl DeclarationReporter {
    fn reject(&self, state: &mut DeclarationState, error: SessionProofError) -> CaptureReportError {
        let _sent = state.reply.try_send(Err(error.clone()));
        CaptureReportError::new(format!("session declaration refused: {error:?}"))
    }
}

impl CaptureReporter for DeclarationReporter {
    fn record(&self, record: CaptureRecord) -> Result<(), CaptureReportError> {
        let Some(declaration) = record.declaration.as_ref() else {
            return self.inner.record(record);
        };
        let mut state = self
            .state
            .lock()
            .map_err(|_| CaptureReportError::new("session proof lock is poisoned"))?;
        if let Some(identity) = state.seen.clone() {
            return Err(self.reject(&mut state, SessionProofError::Duplicate { identity }));
        }
        if record.lane != self.lane {
            return Err(self.reject(
                &mut state,
                SessionProofError::CrossLane {
                    expected: self.lane,
                    observed: record.lane,
                },
            ));
        }
        if record.occupant != self.occupant {
            return Err(self.reject(
                &mut state,
                SessionProofError::CrossOccupant {
                    expected: self.occupant,
                    observed: record.occupant,
                },
            ));
        }
        if record.stream != self.stream {
            return Err(self.reject(&mut state, SessionProofError::WrongStream));
        }
        let identity = declaration.session.as_str().to_owned();
        if let Some(expected) = &self.expected
            && expected != &identity
        {
            return Err(self.reject(
                &mut state,
                SessionProofError::Mismatched {
                    expected: expected.clone(),
                    observed: identity,
                },
            ));
        }
        let session = wire::LaneSession {
            lane: self.lane,
            occupant: self.occupant,
            worktree: self.worktree.clone(),
            vendor: self.vendor.clone(),
            epoch: self.epoch,
            stream: record_stream(self.stream).to_owned(),
            identity: identity.clone(),
            locus: wire::TapeLocus {
                member: declaration.locus.member,
                offset: declaration.locus.offset,
                length: declaration.locus.length,
            },
        };
        self.inner.record(record)?;
        state.seen = Some(identity);
        state.reply.try_send(Ok(session)).map_err(|error| {
            CaptureReportError::new(format!(
                "session proof receiver refused declaration: {error}"
            ))
        })
    }

    fn failed(&self, failure: CaptureFailure) {
        if let Ok(state) = self.state.lock() {
            let _sent = state
                .reply
                .try_send(Err(SessionProofError::Capture(failure.failure.to_string())));
        }
        self.inner.failed(failure);
    }
}

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

fn propose_seat(
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

fn session_fact(session: &wire::LaneSession) -> wire::body::LaneSessionDeclared {
    wire::body::LaneSessionDeclared {
        lane: session.lane,
        occupant: session.occupant,
        worktree: session.worktree.clone(),
        vendor: session.vendor.clone(),
        epoch: session.epoch,
        stream: session.stream.clone(),
        identity: session.identity.clone(),
        member: session.locus.member,
        offset: session.locus.offset,
        length: session.locus.length,
    }
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

fn commit_restored(
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
    session: mpsc::Receiver<Result<wire::LaneSession, SessionProofError>>,
}

struct OccupantLaunchRequest<'a> {
    worktree: &'a str,
    vendor: &'a wire::Name,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    resume: vendor::Resume,
}

fn occupant_launch(
    paths: &RuntimePaths,
    request: OccupantLaunchRequest<'_>,
    reporter: Arc<dyn CaptureReporter>,
) -> CommandResult<OccupantLaunch> {
    let OccupantLaunchRequest {
        worktree,
        vendor: name,
        epoch,
        lane,
        occupant,
        resume,
    } = request;
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
        resume,
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
    let stream = record.stream();
    let expected = record.resume().session_id().map(str::to_owned);
    let tape = tape::Tape::open(paths.tape.join(lane.to_string()).join(name.as_str()))
        .map_err(|error| store_fault("lane::capture-tape-unavailable", &error.to_string()))?;
    let (session_reply, session) = mpsc::sync_channel(4);
    let declaration_reporter: Arc<dyn CaptureReporter> = Arc::new(DeclarationReporter {
        inner: reporter,
        lane,
        occupant,
        worktree: repository.worktree_root().display().to_string(),
        vendor: name.clone(),
        epoch,
        stream,
        expected,
        state: Mutex::new(DeclarationState {
            reply: session_reply,
            seen: None,
        }),
    });
    let observation = lane::capture::observation(
        lane,
        occupant,
        record,
        tape,
        CAPTURE_LIMIT,
        RECORD_QUEUE,
        declaration_reporter,
    )
    .map_err(|error| store_fault("lane::capture-invalid", &error.to_string()))?;
    Ok(OccupantLaunch {
        spec,
        observation,
        session,
    })
}

fn record_stream(stream: vendor::RecordStream) -> &'static str {
    match stream {
        vendor::RecordStream::Stdout => "stdout",
        vendor::RecordStream::Stderr => "stderr",
    }
}

fn await_session(
    session: mpsc::Receiver<Result<wire::LaneSession, SessionProofError>>,
) -> Result<wire::LaneSession, SessionProofError> {
    match session.recv_timeout(SESSION_PROOF) {
        Ok(Ok(declared)) => match session.recv_timeout(SESSION_SETTLE) {
            Ok(Err(error)) => Err(error),
            Ok(Ok(duplicate)) => Err(SessionProofError::Duplicate {
                identity: duplicate.identity,
            }),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(declared),
            Err(mpsc::RecvTimeoutError::Disconnected) => Ok(declared),
        },
        Ok(Err(error)) => Err(error),
        Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {
            Err(SessionProofError::Missing)
        }
    }
}

fn stop_lane(seats: &mut Seats, lane: CanonicalUuid) {
    if let Some(occupant) = seats.vacate(lane) {
        let _stopped = stop_occupant(occupant);
    }
}

fn resume_session(
    paths: &RuntimePaths,
    row: &wire::LaneResidency,
) -> CommandResult<vendor::Resume> {
    let session = row.session.as_ref().ok_or_else(|| {
        session_head_refusal(
            "lane::session-authority-missing",
            Some("lane::declared authority from the current release".to_owned()),
            Some("legacy occupied lane without session authority".to_owned()),
            "drain and re-seat this lane under the current release",
        )
    })?;
    if session.lane != row.lane {
        return Err(session_head_refusal(
            "lane::session-cross-lane",
            Some(row.lane.to_string()),
            Some(session.lane.to_string()),
            "restore the session head bound to this exact lane",
        ));
    }
    if session.worktree != row.worktree {
        return Err(session_head_refusal(
            "lane::session-wrong-worktree",
            Some(row.worktree.clone()),
            Some(session.worktree.clone()),
            "restore the session head bound to this exact worktree",
        ));
    }
    if session.vendor != row.vendor {
        return Err(session_head_refusal(
            "lane::session-cross-vendor",
            Some(row.vendor.to_string()),
            Some(session.vendor.to_string()),
            "restore the session head admitted by this lane's vendor",
        ));
    }
    let expected_occupant = row.lineage.last().copied().ok_or_else(|| {
        session_head_refusal(
            "lane::session-lineage-missing",
            Some("the occupant that declared the session".to_owned()),
            None,
            "repair the durable lane lineage before resuming",
        )
    })?;
    if session.occupant != expected_occupant {
        return Err(session_head_refusal(
            "lane::session-cross-occupant",
            Some(expected_occupant.to_string()),
            Some(session.occupant.to_string()),
            "restore the declaration made by the lane's latest occupant",
        ));
    }
    if session.stream != "stdout" {
        return Err(session_head_refusal(
            "lane::session-wrong-stream",
            Some("stdout".to_owned()),
            Some(session.stream.clone()),
            "restore the locus from the vendor-selected JSONL stream",
        ));
    }

    let dialect = match row.vendor.as_str() {
        "claude" => listen::Dialect::Claude,
        "codex" => listen::Dialect::Codex,
        other => {
            return Err(session_head_refusal(
                "lane::session-vendor-unsupported",
                Some("claude or codex".to_owned()),
                Some(other.to_owned()),
                "select a vendor with a current session declaration dialect",
            ));
        }
    };
    let tape = tape::Tape::open_existing(
        paths
            .tape
            .join(row.lane.to_string())
            .join(row.vendor.as_str()),
    )
    .map_err(|error| {
        session_head_refusal(
            "lane::session-tape-moved",
            Some("the exact durable lane/vendor tape".to_owned()),
            Some(error.to_string()),
            "restore the preserved tape before resuming this lane",
        )
    })?;
    let locus = listen::Locus::new(
        session.locus.member,
        session.locus.offset,
        session.locus.length,
    );
    let bytes = tape.read(locus).map_err(|error| {
        session_head_refusal(
            "lane::session-tape-moved",
            Some(locus.to_string()),
            Some(error.to_string()),
            "restore the exact declaration bytes before resuming this lane",
        )
    })?;
    let decoded = listen::decode(dialect, &bytes, locus).map_err(|error| {
        session_head_refusal(
            "lane::session-tape-moved",
            Some(session.identity.clone()),
            Some(error.to_string()),
            "restore the exact declaration record before resuming this lane",
        )
    })?;
    let declared = decoded.events().iter().find_map(|event| match event {
        listen::Event::Session(declaration) => Some(declaration.identity.as_str()),
        _ => None,
    });
    if declared != Some(session.identity.as_str()) {
        return Err(session_head_refusal(
            "lane::session-tape-moved",
            Some(session.identity.clone()),
            declared.map(str::to_owned),
            "restore the declaration record named by the durable locus",
        ));
    }
    let identity = vendor::SessionId::new(session.identity.clone()).map_err(|error| {
        session_head_refusal(
            "lane::session-identity-invalid",
            Some("one nonempty vendor session identity".to_owned()),
            Some(error.to_string()),
            "repair the durable session authority before resuming",
        )
    })?;
    Ok(vendor::Resume::Session(identity))
}

fn session_head_refusal(
    reason: &str,
    expected: Option<String>,
    observed: Option<String>,
    remedy: &str,
) -> Outcome<()> {
    Outcome::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: Some("lane-session-head".to_owned()),
        expected,
        observed,
        remedy: remedy.to_owned(),
    })
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
    fact.actor == Actor::Lane(lane) || fact_lane(fact) == Some(lane)
}

fn fact_lane(fact: &wire::Envelope<wire::AnyFact>) -> Option<CanonicalUuid> {
    fact.fact
        .body()
        .get("lane")
        .and_then(serde_json::Value::as_str)
        .and_then(|lane| lane.parse().ok())
        .or(match fact.actor {
            Actor::Lane(lane) => Some(lane),
            Actor::Operator | Actor::Spice => None,
        })
}

fn recall_provenance(
    fact: &wire::Envelope<wire::AnyFact>,
) -> CommandResult<Option<RecallProvenance>> {
    let provenance = match fact.fact.kind() {
        wire::FactKind::LaneSessionDeclared => {
            let body: wire::body::LaneSessionDeclared = recall_body(fact)?;
            Some(RecallProvenance::Session {
                occupant: body.occupant,
                worktree: body.worktree,
                vendor: body.vendor,
                epoch: body.epoch,
                stream: body.stream,
                identity: body.identity,
                locus: wire::TapeLocus {
                    member: body.member,
                    offset: body.offset,
                    length: body.length,
                },
            })
        }
        wire::FactKind::LaneCompacted => {
            let body: wire::body::LaneCompacted = recall_body(fact)?;
            Some(RecallProvenance::Compaction {
                occupant: body.occupant,
                stream: body.stream,
                locus: wire::TapeLocus {
                    member: body.member,
                    offset: body.offset,
                    length: body.length,
                },
            })
        }
        wire::FactKind::SteerSent => {
            let body: wire::body::SteerSent = recall_body(fact)?;
            Some(steering_provenance(
                Some(body.directive),
                body.key,
                wire::Disposition::Live,
                None,
                None,
                None,
            ))
        }
        wire::FactKind::SteerResent => {
            let body: wire::body::SteerResent = recall_body(fact)?;
            Some(steering_provenance(
                Some(body.directive),
                body.key,
                wire::Disposition::Live,
                Some(body.ancestor),
                None,
                None,
            ))
        }
        wire::FactKind::SteerAcked => {
            let body: wire::body::SteerAcked = recall_body(fact)?;
            Some(steering_provenance(
                Some(body.directive),
                body.key,
                wire::Disposition::Acked,
                None,
                Some(body.epoch),
                Some(body.locus),
            ))
        }
        wire::FactKind::SteerDeclined => {
            let body: wire::body::SteerDeclined = recall_body(fact)?;
            Some(steering_provenance(
                Some(body.directive),
                body.key,
                wire::Disposition::Declined,
                None,
                Some(body.epoch),
                Some(body.locus),
            ))
        }
        wire::FactKind::SteerCanceled => {
            let body: wire::body::SteerCanceled = recall_body(fact)?;
            Some(steering_provenance(
                None,
                body.key,
                wire::Disposition::Canceled,
                None,
                None,
                None,
            ))
        }
        wire::FactKind::LaneRenewed => {
            let body: wire::body::LaneRenewed = recall_body(fact)?;
            Some(RecallProvenance::Renewal {
                occupant: body.occupant,
                ancestor: body.ancestor,
                epoch: body.epoch,
            })
        }
        _ => None,
    };
    Ok(provenance)
}

fn steering_provenance(
    directive: Option<CanonicalUuid>,
    key: String,
    disposition: wire::Disposition,
    ancestor: Option<String>,
    epoch: Option<CanonicalUuid>,
    locus: Option<wire::body::TranscriptLocus>,
) -> RecallProvenance {
    RecallProvenance::Steering {
        directive,
        key,
        disposition,
        ancestor,
        epoch,
        locus,
    }
}

fn recall_body<T: serde::de::DeserializeOwned>(
    fact: &wire::Envelope<wire::AnyFact>,
) -> CommandResult<T> {
    let bytes = wire::encode(fact.fact.body())
        .map_err(|error| store_fault("recall::fact-invalid", &error.to_string()))?;
    wire::decode(&bytes).map_err(|error| store_fault("recall::fact-invalid", &error.to_string()))
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
