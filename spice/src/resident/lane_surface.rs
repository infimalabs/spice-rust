//! Resident-owned lane seating, lifecycle, restart, and recall handlers.

use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant as Monotonic};

use bus::Bus;
use journal::{FactCause, JournalWrite};
use lane::capture::{CaptureFailure, CaptureRecord, CaptureReportError, CaptureReporter};
use lane::occupant::{Occupant, OccupantWait, Seats};
use sill::WaitOutcome;
use spice::repository::Repository;
use wire::intent::{
    CommandAnswer, LaneAction, LaneAnswer, LaneArrange, LaneChangeAnswer, LaneRecall, LaneSeat,
    LaneState, LineageArrange, LineageConfigure, LineageLife, RecallAnswer, RecallEntry,
    RecallProvenance,
};
use wire::{
    Actor, CanonicalUuid, Command, Fault, FaultClass, Outcome, Producer, Refusal, Reply,
    ReplyPosition, ResidentState, SeqRange,
};

mod launch;
mod lineage;
mod propose;
mod reply;
mod session;

pub(super) use launch::{OccupantLaunchRequest, occupant_launch};
pub(super) use lineage::{arrange as arrange_lineages, configure};
use propose::{commit_restored, propose_drain, propose_seat, propose_wake};
use reply::{
    answer, authority_refusal, command_result, committed_lane_reply, fact_lane, lane_refusal,
    missing_lane, recall_provenance, relevant, typed_environment_fault,
};
pub(super) use session::{
    DeclarationReporter, DeclarationState, DeclarationWitness, SessionProof, await_session,
    record_stream, resume_session, session_fact,
};

use super::shutdown::{current_residency, guard_residency};
use super::{
    CommandResult, RuntimePaths, commit, new_uuid, seq, store_fault, typed_outcome,
    typed_protocol_fault, typed_store_fault, write,
};

const OCCUPIED: &str = "occupied";
const VACANT: &str = "vacant";
const DEGRADED: &str = "degraded";
const RECALL_LIMIT: usize = 16;
const CAPTURE_LIMIT: usize = 1024 * 1024;
const OUTPUT_DRAIN: Duration = Duration::from_secs(2);
const REAP: Duration = Duration::from_secs(2);
/// How long an occupant may take to end itself before the resident kills it.
///
/// One second is short on purpose, and it is a fixed number on purpose. Nothing
/// durable is owed to this window: the transcript is taped by the resident as
/// the vendor emits it, one committed record per newline boundary, so every
/// byte the occupant already wrote outlives the kill and a record cut mid-line
/// is closed as [`sill::RecordBoundary::Terminal`] rather than dropped. An
/// occupant has no flush to finish here, only an exit to perform, so a grace
/// that could see the machine's load would buy nothing this one does not
/// already give.
///
/// What the window cannot promise is that a loaded machine schedules the
/// occupant's own signal handler inside it. That is why being killed has to
/// stay cheap rather than being made rarer by a larger number.
const TERMINATION_GRACE: Duration = Duration::from_secs(1);
const RECORD_QUEUE: usize = 64;
const SESSION_PROOF: Duration = Duration::from_secs(10);
const SESSION_SETTLE: Duration = Duration::from_millis(25);

/// Resolve the one durable subscription row for a lineage.
fn subscription_row(
    head: &wire::ResidencyHead,
    lineage_id: CanonicalUuid,
) -> CommandResult<Option<&wire::LineageSubscription>> {
    let mut durable = head
        .subscriptions
        .iter()
        .filter(|subscription| subscription.lineage_id == lineage_id);
    let subscription = durable.next();
    if durable.next().is_some() {
        return Err(store_fault(
            "lineage::subscription-ambiguous",
            "one lineage has more than one durable task-subscription policy",
        ));
    }
    Ok(subscription)
}

fn subscription_policy(
    head: &wire::ResidencyHead,
    lineage_id: CanonicalUuid,
) -> CommandResult<Option<LineageLife>> {
    Ok(subscription_row(head, lineage_id)?.map(|subscription| subscription.policy))
}

pub(super) struct Runtime<'a> {
    capabilities: &'a canvas::axum::Capabilities,
    discovery: &'a canvas::Discovery,
    capture: Arc<dyn CaptureReporter>,
}

fn admit_manual_projects(
    projects: &board::ProjectPolicy,
    requested: Option<&[String]>,
) -> Result<Option<Vec<String>>, Outcome<CommandAnswer>> {
    let Some(requested) = requested else {
        return Ok(None);
    };
    let mut admitted = Vec::with_capacity(requested.len());
    for project in requested {
        if let Err(error) = projects.admit_manual_creation(project) {
            return Err(lane_refusal(
                "lineage::manual-project-invalid",
                Some("one admitted public task project".to_owned()),
                Some(project.clone()),
                &error.to_string(),
            ));
        }
        if !admitted.contains(project) {
            admitted.push(project.clone());
        }
    }
    admitted.sort();
    Ok(Some(admitted))
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
        if super::is_simulated_lane(&row) {
            if row.session.is_some() {
                return Err(store_fault(
                    "lane::simulated-session-present",
                    "a simulated lane carries real vendor session authority",
                ));
            }
            continue;
        }
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
        let git = launch.git;
        let session_proof = launch.session;
        let group = seats
            .seat_observed(row.lane, occupant, launch.spec, launch.observation)
            .map_err(|error| store_fault("lane::restart-launch-failed", &error.to_string()))?
            .group();
        let session = match await_session(session_proof) {
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
            git,
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
    seats: &Seats,
    _command: &wire::AdmittedCommand,
) -> Outcome<CommandAnswer> {
    let head = match current_residency(bus, epoch) {
        Ok(head) => head,
        Err(outcome) => return typed_outcome(outcome),
    };
    let answer = match answer(head.body(), None, seats) {
        Ok(answer) => answer,
        Err(outcome) => return typed_outcome(outcome),
    };
    Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(head.position()),
        body: CommandAnswer::Lane(Box::new(answer)),
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
        git: Some(prepared.proven.git.clone().into_boxed_str()),
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

/// Place one existing member immediately before another existing member.
///
/// The target row supplies the destination lineage. Moving the row itself keeps
/// its lane, occupant, session, worktree, and composer identity unchanged. The
/// residency head's vector is the durable order read back by Canvas.
pub(super) fn arrange(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &Seats,
    command: &wire::AdmittedCommand,
    request: &LaneArrange,
) -> Outcome<CommandAnswer> {
    let observed = match current_residency(bus, epoch) {
        Ok(head) => head,
        Err(outcome) => return typed_outcome(outcome),
    };
    let Arrangement {
        next,
        from_lineage,
        lineage,
    } = match rearrange(observed.body(), request) {
        Ok(arrangement) => arrangement,
        Err(outcome) => return outcome,
    };
    let lane_answer = match answer(&next, None, seats) {
        Ok(answer) => answer,
        Err(outcome) => return typed_outcome(outcome),
    };
    // Read before the write is opened, so an unreadable boundary refuses the
    // whole arrangement instead of committing a move whose attribution split
    // nothing can reconstruct.
    let sources = match bus.lane_source_boundaries(&[request.lane]) {
        Ok(sources) => sources,
        Err(error) => {
            return typed_store_fault("lane::arrange-boundary-unreadable", &error.to_string());
        }
    };
    let mut write = match write(
        command.actor().clone(),
        epoch,
        Producer::ResidentRecognition,
    ) {
        Ok(write) => write,
        Err(outcome) => return typed_outcome(outcome),
    };
    guard_residency(&mut write, &observed);
    let fact = match write.fact(
        wire::body::LaneArranged {
            lane: request.lane,
            from_lineage_id: from_lineage,
            lineage_id: lineage,
            before: request.before,
            sources,
        },
        FactCause::Existing(observed.position()),
    ) {
        Ok(fact) => fact,
        Err(error) => return typed_store_fault("lane::arrange-fact-invalid", &error.to_string()),
    };
    write.residency_head(fact, move |_| next);
    if let Err(error) = write.command_result(
        Command {
            v: wire::WireVersion::CURRENT,
            id: command.id(),
            epoch: command.epoch(),
            kind: command.kind(),
            expected: command.expected().to_vec(),
            body: *request,
        },
        epoch,
        lane_answer.clone(),
    ) {
        return typed_store_fault("lane::arrange-receipt-invalid", &error.to_string());
    }
    match commit(bus, write) {
        Ok(committed) => Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(committed.first()),
                last: seq(committed.last()),
            }),
            body: CommandAnswer::Lane(Box::new(lane_answer)),
        }),
        Err(outcome) => typed_outcome(outcome),
    }
}

/// One arrangement resolved against the observed head.
struct Arrangement {
    next: wire::ResidencyHead,
    from_lineage: CanonicalUuid,
    lineage: CanonicalUuid,
}

/// Resolve one arrangement against the observed order, or refuse it.
///
/// Every way the request can fail to name a move is decided here, against one
/// snapshot and before any write is opened, so a refusal costs no journal bytes.
fn rearrange(
    observed: &wire::ResidencyHead,
    request: &LaneArrange,
) -> Result<Arrangement, Outcome<CommandAnswer>> {
    if request.lane == request.before {
        return Err(lane_refusal(
            "lane::arrange-self",
            Some("a different target lane".to_owned()),
            Some(request.lane.to_string()),
            "drop the primary composer on another seated member",
        ));
    }
    let Some(source_index) = observed
        .lanes
        .iter()
        .position(|row| row.lane == request.lane)
    else {
        return Err(lane_refusal(
            "lane::arrange-source-missing",
            Some("one current seated member".to_owned()),
            Some(request.lane.to_string()),
            "read lane state and retry with a current source member",
        ));
    };
    let Some(target) = observed.lanes.iter().find(|row| row.lane == request.before) else {
        return Err(lane_refusal(
            "lane::arrange-target-missing",
            Some("one current seated member".to_owned()),
            Some(request.before.to_string()),
            "read lane state and retry with a current target member",
        ));
    };
    let lineage = target.lineage_id;
    let from_lineage = observed.lanes[source_index].lineage_id;
    let mut next = observed.clone();
    let mut source = next.lanes.remove(source_index);
    let Some(target_index) = next.lanes.iter().position(|row| row.lane == request.before) else {
        return Err(typed_store_fault(
            "lane::arrange-target-lost",
            "the target member disappeared while arranging one residency snapshot",
        ));
    };
    source.lineage_id = lineage;
    next.lanes.insert(target_index, source);
    if from_lineage != lineage && !next.lanes.iter().any(|row| row.lineage_id == from_lineage) {
        next.subscriptions
            .retain(|subscription| subscription.lineage_id != from_lineage);
    }
    Ok(Arrangement {
        next,
        from_lineage,
        lineage,
    })
}

fn admit_seat(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    request: &LaneSeat,
) -> Result<journal::JournalHead<wire::ResidencyHead>, Outcome<CommandAnswer>> {
    if request
        .name
        .as_ref()
        .is_some_and(|name| name.is_empty() || name.trim() != name)
    {
        return Err(lane_refusal(
            "lane::name-invalid",
            Some("one nonblank display name without surrounding whitespace".to_owned()),
            request.name.clone(),
            "trim the agent display name or leave it unconfigured",
        ));
    }
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
    git: String,
}

fn prepare_seat(
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    runtime: &Runtime<'_>,
    request: &LaneSeat,
) -> Result<PreparedSeat, Outcome<CommandAnswer>> {
    let resume = request.resume.as_ref().map_or_else(
        || Ok(vendor::Resume::Fresh),
        |session| {
            vendor::SessionId::new(session.clone())
                .map(vendor::Resume::Session)
                .map_err(|error| typed_protocol_fault("lane::resume-invalid", &error.to_string()))
        },
    )?;
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
            resume,
        },
        Arc::clone(&runtime.capture),
    )
    .map_err(|outcome| {
        remove_seat_surface(paths, runtime, request.lane, capability);
        typed_outcome(outcome)
    })?;
    let git = launch.git;
    let session_proof = launch.session;
    let group = seats
        .seat_observed(request.lane, occupant, launch.spec, launch.observation)
        .map(lane::occupant::Occupant::group)
        .map_err(|error| {
            remove_seat_surface(paths, runtime, request.lane, capability);
            typed_environment_fault("lane::launch-failed", &error.to_string())
        })?;
    let session = await_session(session_proof).map_err(|error| {
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
            git,
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
/// exact occupant identity. Session recognition, taste recognition, and Canvas
/// message rendering consume this same admission decision.
pub(super) fn capture_record(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    record: &CaptureRecord,
) -> CommandResult<bool> {
    if !seats
        .occupant(record.lane)
        .is_some_and(|occupant| occupant.occupant() == record.occupant)
    {
        return Ok(false);
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
    let admitted = seats.observe_activity(record.lane, record.occupant, Monotonic::now());
    debug_assert!(admitted, "the current capture record remains supervised");
    Ok(true)
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
    seats: &Seats,
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
    let residency = match answer(head.body(), selected, seats) {
        Ok(answer) => answer,
        Err(outcome) => return typed_outcome(outcome),
    };
    let body = RecallAnswer {
        barrier,
        repository: paths.worktree.display().to_string(),
        lane: selected,
        residency,
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
        git: None,
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

/// End a lane's occupant without publishing how it ended.
///
/// The dropped [`StopEvidence`] is the point rather than an oversight. Every
/// caller here is a compensation path — `await_session`, `commit_restored`,
/// `propose_wake`, or the commit itself failed — so the seating this unwinds
/// was never committed. An ending fact names the residency it closes, and there
/// is no committed residency to close: publishing one would assert a lane life
/// the journal never recorded beginning, which is a worse record than silence.
///
/// The distinction this discards is only owed where an occupancy was durable,
/// which is [`stop_occupant`]'s other caller, and that one carries the evidence
/// through to a fact.
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

/// How an occupant's residency ended, as the resident itself observed it.
///
/// The three arms are three different endings, not three values of one.
/// [`Self::Killed`] carries no code because a signal this resident sent left the
/// occupant no status of its own to report.
enum StopEvidence {
    Drained,
    Exited(i32),
    /// A signal this resident sent ended it, at either signalling stage.
    ///
    /// Not only the kill: an occupant that never trapped the termination signal
    /// dies from that signal instead, and this resident is equally the reason
    /// for the ending in both cases.
    Killed,
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
                // The wait outcome is deliberately not read for a code here.
                // This resident sent the signal that ended it, so whatever
                // status the platform reports is a description of that signal
                // rather than anything the occupant decided.
                OccupantWait::Exited { .. } => Ok(StopEvidence::Killed),
                OccupantWait::Running(_) => {
                    Err("process group survived its configured reap bound".to_owned())
                }
            }
        }
    }
}

/// Classify an ending this resident's termination signal is responsible for.
///
/// The only caller is the wait that follows [`Occupant::terminate`], so every
/// outcome reaching here was already signalled by this resident. A code means
/// the occupant trapped the signal and chose what to report; its absence means
/// the signal is what ended it, which is the same ending the kill stage records.
/// Reporting that as an exit would publish a status the occupant never chose.
fn stop_evidence(outcome: &WaitOutcome) -> StopEvidence {
    let status = outcome.completion().status();
    if status.success() {
        return StopEvidence::Drained;
    }
    match status.code() {
        Some(code) => StopEvidence::Exited(code),
        None => StopEvidence::Killed,
    }
}

/// The status of an occupant that was already gone before this resident acted.
///
/// This is the one place a missing code still becomes a number, and it is the
/// one place that number says something true: nothing here had been signalled by
/// this resident, so a signal death is one it did not cause. Since a Unix status
/// code is 0 to 255 and never negative, the substitute cannot collide with a
/// status an occupant chose.
fn exit_code(outcome: &WaitOutcome) -> i32 {
    outcome.completion().status().code().unwrap_or(-1)
}
