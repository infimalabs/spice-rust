//! Resident-owned lane seating, lifecycle, restart, and recall handlers.

use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant as Monotonic};

use bus::Bus;
use journal::{FactCause, JournalGuard, JournalWrite, PendingFact};
use lane::capture::{
    CaptureExit, CaptureFailure, CaptureRecord, CaptureReportError, CaptureReporter,
};
use lane::occupant::{Occupant, OccupantWait, Seats};
use sill::WaitOutcome;
use spice::repository::Repository;
use wire::intent::{
    CommandAnswer, LaneAction, LaneAnswer, LaneArrange, LaneChangeAnswer, LaneEffortRetune,
    LaneFastRetune, LaneRecall, LaneRetune, LaneSeat, LaneState, RecallAnswer, RecallEntry,
    RecallProvenance, TeamArrange, TeamConfigure, TeamLife,
};
use wire::{
    Actor, CanonicalUuid, Command, Fault, FaultClass, Outcome, Producer, Refusal, Reply,
    ReplyPosition, ResidentState, SeqRange,
};

mod automatic;
mod trust_park;
#[cfg(test)]
pub(in crate::resident) use trust_park::reconcile_trust_park;
mod ending;
mod jobs;
mod launch;
mod propose;
pub(super) use jobs::{catalog as job_catalog, control as job_control};
mod renewal;
mod reply;
mod restart_claim;
mod retune;
mod session;
mod team;

use automatic::wake;
pub(super) use automatic::{
    AutomaticLaunch, AutomaticWake, BeginAutomaticWake, PendingAutomaticWake, PendingWake,
    WakeLaunch, begin_automatic_wake, begin_wake, finish_automatic_wake, finish_wake,
};
use ending::{StopEvidence, drain, ending_fact, stop_lane, stop_occupant};
#[cfg(test)]
pub(in crate::resident) use launch::test_support::{InertProbes, LaunchFixture, stand_in_stream};
pub(super) use launch::{HostProbes, OccupantLaunch, OccupantLaunchRequest, occupant_launch};
#[cfg(test)]
pub(in crate::resident) use propose::tests::{
    assert_successor_address, seed_birth, seed_task_claim,
};
use propose::{commit_adopted, propose_drain, propose_remove, propose_seat, propose_wake};
pub(super) use renewal::{reconcile as reconcile_renewal, set as renew};
use reply::{
    StableRoot, answer, authority_refusal, command_result, committed_lane_reply, fact_lane,
    lane_refusal, missing_lane, next_topology_revision, recall_provenance, relevant,
    typed_environment_fault,
};
pub(super) use restart_claim::append_startup_claims;
pub(super) use retune::{
    LiveChange, LiveChanges, owe_global_fast_flips, record_fast_refusal, retune, retune_effort,
    retune_fast, seat_session,
};
pub(super) use session::{
    DeclarationReporter, DeclarationState, DeclarationWitness, SessionProof, SessionProofError,
    await_session, launch_resume, record_stream, resume_session, session_fact, session_source,
};
pub(super) use team::{arrange as arrange_teams, configure};

use super::read_materializations::CurrentReads;
use super::shutdown::{current_residency, guard_residency, rebase_lane};
use super::supervisor::terminal;
use super::{
    CommandEffects, CommandResult, RuntimePaths, commit, new_uuid, seq, store_fault, typed_outcome,
    typed_store_fault, write,
};

const OCCUPIED: &str = "occupied";
const VACANT: &str = "vacant";
const DEGRADED: &str = "degraded";
const RECALL_LIMIT: usize = 16;
/// Bytes one captured record may carry; capture truncates the rest.
pub(in crate::resident) const CAPTURE_LIMIT: usize = 1024 * 1024;
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
// A second declaration crosses a separate child write and capture callback.
// Keep the duplicate window bounded, but long enough to survive scheduler load.
const SESSION_SETTLE: Duration = Duration::from_millis(250);

/// Resolve the one durable subscription row for a team.
fn subscription_row(
    head: &wire::ResidencyHead,
    team: CanonicalUuid,
) -> CommandResult<Option<&wire::TeamSubscription>> {
    let mut durable = head
        .subscriptions
        .iter()
        .filter(|subscription| subscription.team == team);
    let subscription = durable.next();
    if durable.next().is_some() {
        return Err(store_fault(
            "team::subscription-ambiguous",
            "one team has more than one durable task-allocation lifetime",
        ));
    }
    Ok(subscription)
}

fn subscription_policy(
    head: &wire::ResidencyHead,
    team: CanonicalUuid,
) -> CommandResult<Option<TeamLife>> {
    Ok(subscription_row(head, team)?.map(|subscription| subscription.policy))
}

pub(super) struct Runtime<'a> {
    capabilities: &'a serve::axum::Capabilities,
    discovery: &'a serve::Discovery,
    capture: Arc<dyn CaptureReporter>,
    reads: &'a CurrentReads,
}

impl<'a> Runtime<'a> {
    pub(super) fn new(
        capabilities: &'a serve::axum::Capabilities,
        discovery: &'a serve::Discovery,
        capture: Arc<dyn CaptureReporter>,
        reads: &'a CurrentReads,
    ) -> Self {
        Self {
            capabilities,
            discovery,
            capture,
            reads,
        }
    }

    /// Bind and retire this generation's capabilities by the principal heads.
    pub(super) fn publish_roles(&self, bus: &Bus) -> CommandResult<()> {
        self.capabilities
            .publish(super::principal_surface::roles(bus)?)
            .map_err(|error| {
                super::environment_fault("serve::capability-invalid", &error.to_string())
            })
    }
}

/// Reconstruct every durable unfinished seat beneath the new resident epoch.
pub(super) fn restore_seats(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    reconcile_seats(bus, paths, epoch, seats, diagnostic)
}

/// Bring every durable occupant this resident is not seated behind back into
/// agreement with what its own supervisor recorded.
///
/// At startup that is every occupied row, since the new resident holds no
/// seats yet. Afterwards it is a row whose exit could not be accounted when it
/// happened -- the seat was already vacated when the commit lost its lock, so
/// the durable row is the only place the occupant still lives -- and the
/// supervisor's state file says what actually became of it. Re-deriving from
/// that file is the entire repair: nothing here remembers an attempt, so each
/// sweep judges the same durable inputs afresh, and a sweep that finds nothing
/// stale commits nothing. A lane this resident is seated behind is left to the
/// seat sweep in the runtime, which judges it against the same supervisor
/// state; here the seat is the live authority for it.
pub(super) fn reconcile_seats(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let durable = current_residency(bus, epoch)?.body().lanes.clone();
    for row in durable {
        let Some(durable_occupant) = row.occupant else {
            continue;
        };
        if seats.occupied(row.lane) {
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
        if let Some(adopted) = super::supervisor::adopt(paths, &row)? {
            match adopted {
                super::supervisor::Adoption::Running(adopted) => {
                    seats
                        .seat_detached(row.lane, adopted.occupant, adopted.process)
                        .map_err(|error| {
                            store_fault("lane::restart-adoption-failed", &error.to_string())
                        })?;
                    if let Err(outcome) = commit_adopted(bus, epoch, &row) {
                        let _detached = seats.vacate(row.lane);
                        return Err(outcome);
                    }
                }
                super::supervisor::Adoption::Exited(adopted) => {
                    seats
                        .seat_detached(row.lane, adopted.occupant, adopted.process)
                        .map_err(|error| {
                            store_fault("lane::restart-adoption-failed", &error.to_string())
                        })?;
                    if let Err(outcome) = commit_adopted(bus, epoch, &row) {
                        let _detached = seats.vacate(row.lane);
                        return Err(outcome);
                    }
                    occupant_exited(
                        bus,
                        paths,
                        epoch,
                        seats,
                        CaptureExit {
                            lane: row.lane,
                            occupant: adopted.occupant,
                        },
                        diagnostic,
                    )?;
                }
                super::supervisor::Adoption::Failed(failure) => {
                    commit_adopted(bus, epoch, &row)?;
                    supervisor_failed(bus, epoch, row.lane, durable_occupant, &failure)?;
                }
            }
            continue;
        }
        // No supervisor trace survived: the runtime directory that would hold
        // it is scratch space, cleared by routine cleanup, a reboot, or an
        // operator on purpose. Relaunching here cannot distinguish "this agent
        // legitimately needed to keep running" from "someone shut it down and
        // wants it left alone" — it always guessed the former, silently
        // resurrecting a possibly-unwanted occupant on the lane's last known
        // vendor. Treat the missing trace the same as an observed failure: carry
        // forward any claim this occupant held, then degrade to vacant so an
        // operator makes the call explicitly via `lane wake`.
        commit_adopted(bus, epoch, &row)?;
        supervisor_failed(
            bus,
            epoch,
            row.lane,
            durable_occupant,
            "no supervisor trace survived",
        )?;
    }
    Ok(())
}

/// Read the current lane table at one residency barrier.
pub(super) fn read(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &Seats,
    _command: &wire::AdmittedCommand,
) -> Outcome<CommandAnswer> {
    let (cursor, answer) = match current_answer(bus, paths, epoch, seats) {
        Ok(answer) => answer,
        Err(outcome) => return typed_outcome(outcome),
    };
    Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(cursor),
        body: CommandAnswer::Lane(Box::new(answer)),
    })
}

/// Build the operator lane read and its exact journal barrier.
pub(in crate::resident) fn current_answer(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &Seats,
) -> CommandResult<(wire::Seq, LaneAnswer)> {
    let (head, cursor) = bus
        .read(move |read| {
            let head = read.residency_head()?.ok_or_else(|| {
                journal::GuardError::Invalid(
                    "resident lane read found no residency head".to_owned(),
                )
            })?;
            let cursor = read.cursor()?.ok_or_else(|| {
                journal::GuardError::Invalid(
                    "resident lane read found no journal cursor".to_owned(),
                )
            })?;
            Ok((head, cursor))
        })
        .map_err(|error| store_fault("resident::lane-read-unreadable", &error.to_string()))?;
    if head.body().epoch != epoch {
        return Err(store_fault(
            "resident::epoch-moved",
            &format!("expected {epoch}, observed {}", head.body().epoch),
        ));
    }
    let mut answer = answer(head.body(), None, seats, paths)?;
    project_holds(bus, cursor, &mut answer)?;
    Ok((cursor, answer))
}

/// Give every row of one lane answer the hold current at `barrier`, the
/// journal barrier the answer itself was built at.
fn project_holds(bus: &Bus, barrier: wire::Seq, answer: &mut LaneAnswer) -> CommandResult<()> {
    let lanes: Vec<CanonicalUuid> = answer.lanes.iter().map(|row| row.lane).collect();
    let mut holds = bus
        .read(move |read| {
            let mut holds = std::collections::BTreeMap::new();
            for lane in lanes {
                if let Some(held) = read.lane_hold_at(lane, barrier)? {
                    holds.insert(lane, held);
                }
            }
            Ok(holds)
        })
        .map_err(|error| store_fault("resident::lane-hold-unreadable", &error.to_string()))?;
    for row in &mut answer.lanes {
        row.hold = holds.remove(&row.lane).map(|held| wire::intent::LaneHold {
            evidence: held.evidence,
            until: held.until,
        });
    }
    Ok(())
}

/// Seat one new vacant lane and publish its command surface.
pub(super) fn seat(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    _seats: &mut Seats,
    runtime: &Runtime<'_>,
    command: &wire::AdmittedCommand,
    request: &LaneSeat,
) -> Outcome<CommandAnswer> {
    let (observed, topology_revision) = match admit_seat(bus, paths, epoch, request) {
        Ok(observed) => observed,
        Err(outcome) => return outcome,
    };
    let capability = match prepare_seat_surface(paths, runtime, request) {
        Ok(prepared) => prepared,
        Err(outcome) => return outcome,
    };
    let answer = LaneChangeAnswer {
        lane: request.lane,
        topology_revision,
        from: "absent".to_owned(),
        to: VACANT.to_owned(),
        git: None,
        rtk: None,
    };
    let proposal = propose_seat(
        command,
        request,
        &observed,
        epoch,
        capability,
        answer.clone(),
    );
    let proposal = match proposal {
        Ok(proposal) => proposal,
        Err(outcome) => {
            remove_seat_surface(paths, runtime, request.lane, capability);
            return typed_outcome(outcome);
        }
    };
    match commit(bus, proposal) {
        Ok(committed) => committed_lane_reply(committed, answer),
        Err(outcome) => {
            remove_seat_surface(paths, runtime, request.lane, capability);
            typed_outcome(outcome)
        }
    }
}

/// Place one existing member immediately before another existing member.
///
/// The target row supplies the destination team. Moving the row itself keeps
/// its lane, occupant, session, worktree, and composer identity unchanged. The
/// residency head's vector is the durable order read back by the app.
pub(super) fn arrange(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &Seats,
    command: &wire::AdmittedCommand,
    request: &LaneArrange,
) -> Outcome<CommandAnswer> {
    let observed = match current_residency(bus, epoch) {
        Ok(head) => head,
        Err(outcome) => return typed_outcome(outcome),
    };
    let topology_revision = match next_topology_revision(observed.body(), request.topology_revision)
    {
        Ok(revision) => revision,
        Err(outcome) => return outcome,
    };
    let Arrangement {
        next,
        from_team,
        team,
    } = match rearrange(observed.body(), request) {
        Ok(arrangement) => arrangement,
        Err(outcome) => return outcome,
    };
    let mut next = next;
    next.topology_revision = topology_revision;
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
    team::reconcile_formations(observed.body(), &mut next, write.at());
    let lane_answer = match answer(&next, None, seats, paths) {
        Ok(answer) => answer,
        Err(outcome) => return typed_outcome(outcome),
    };
    guard_residency(&mut write, &observed);
    let fact = match write.fact(
        wire::body::LaneArranged {
            lane: request.lane,
            from_team,
            team,
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
    from_team: CanonicalUuid,
    team: CanonicalUuid,
}

/// Admit growth only within the team capacity; legacy larger teams can shrink.
fn admit_team_size(current_members: usize, members: usize) -> Result<(), Outcome<CommandAnswer>> {
    wire::grouping::admit_size(current_members, members).map_err(team::grouping_refusal)
}

/// Resolve one arrangement against the observed order, or refuse it.
///
/// Every way the request can fail to name a move is decided here, against one
/// snapshot and before any write is opened, so a refusal costs no journal bytes.
fn rearrange(
    observed: &wire::ResidencyHead,
    request: &LaneArrange,
) -> Result<Arrangement, Outcome<CommandAnswer>> {
    let teams = team::partition(observed)?;
    let (placed, moved) = teams
        .place_before(request.lane, request.before)
        .map_err(|error| match error {
            wire::grouping::GroupingError::PlaceSelf(_) => lane_refusal(
                "lane::arrange-self",
                Some("a different target lane".to_owned()),
                Some(request.lane.to_string()),
                "drop the primary composer on another seated member",
            ),
            wire::grouping::GroupingError::Absent(_) => lane_refusal(
                "lane::arrange-source-missing",
                Some("one current seated member".to_owned()),
                Some(request.lane.to_string()),
                "read lane state and retry with a current source member",
            ),
            wire::grouping::GroupingError::TargetAbsent(_) => lane_refusal(
                "lane::arrange-target-missing",
                Some("one current seated member".to_owned()),
                Some(request.before.to_string()),
                "read lane state and retry with a current target member",
            ),
            other => team::grouping_refusal(other),
        })?;
    let (Some(from_team), Some(team)) = (moved.from, moved.to) else {
        return Err(typed_store_fault(
            "lane::arrange-target-lost",
            "an arranged member stands in no team",
        ));
    };
    let mut next = observed.clone();
    next.lanes = team::placed(observed, &placed)?;
    if !placed.holds(from_team) {
        next.subscriptions
            .retain(|subscription| subscription.team != from_team);
    }
    Ok(Arrangement {
        next,
        from_team,
        team,
    })
}

fn admit_seat(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    request: &LaneSeat,
) -> Result<(journal::JournalHead<wire::ResidencyHead>, u64), Outcome<CommandAnswer>> {
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
    if model_malformed(request.model.as_ref()) {
        return Err(lane_refusal(
            "lane::model-invalid",
            Some("one nonblank model spelling without surrounding whitespace".to_owned()),
            request.model.clone(),
            "trim the model spelling or leave it to the vendor configuration",
        ));
    }
    if let Some(effort) = &request.effort
        && effort.parse::<vendor::Effort>().is_err()
    {
        return Err(lane_refusal(
            "lane::effort-invalid",
            Some(closed_efforts()),
            Some(effort.clone()),
            "select one closed effort or leave it to the vendor configuration",
        ));
    }
    if request.resume.is_some() {
        return Err(lane_refusal(
            "lane::resume-requires-wake",
            Some("lane add without resume".to_owned()),
            request.resume.clone(),
            "add the lane without launching it, then wake an established lane explicitly",
        ));
    }
    let observed = current_residency(bus, epoch).map_err(typed_outcome)?;
    let topology_revision = next_topology_revision(observed.body(), request.topology_revision)?;
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
    let current_members = observed.body().team_members(request.team).count();
    admit_team_size(current_members, current_members + 1)?;
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
    admit_worktree(paths, &request.worktree).map_err(typed_outcome)?;
    Ok((observed, topology_revision))
}

/// The closed effort vocabulary, spelled as a refusal expects it.
fn closed_efforts() -> String {
    vendor::Effort::ALL
        .iter()
        .map(|effort| effort.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether a model spelling could not be handed to a vendor as argv.
///
/// A seat carries this spelling to every launch it ever makes, so it is judged
/// once where it enters the seat -- when the lane is added and when the seat is
/// retuned -- rather than at the spawn that would otherwise inherit it.
pub(super) fn model_malformed(model: Option<&String>) -> bool {
    model.is_some_and(|model| model.is_empty() || model.trim() != model || model.contains('\0'))
}

struct ProvenOccupant {
    occupant: CanonicalUuid,
    group: u32,
    session: wire::LaneSession,
    source: CanonicalUuid,
    git: String,
    fresh_session: bool,
}

fn prepare_seat_surface(
    paths: &RuntimePaths,
    runtime: &Runtime<'_>,
    request: &LaneSeat,
) -> Result<CanonicalUuid, Outcome<CommandAnswer>> {
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
    Ok(capability)
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

/// Drain, wake, or remove one current lane.
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
        LaneAction::Remove => remove(bus, paths, epoch, seats, runtime, command, request),
    }
}

/// Remove one vacant lane seat from the durable topology.
pub(super) fn remove(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &Seats,
    runtime: &Runtime<'_>,
    command: &wire::AdmittedCommand,
    request: &LaneState,
) -> Outcome<CommandAnswer> {
    if matches!(command.actor(), Actor::Lane(lane) if *lane == request.lane) {
        return lane_refusal(
            "lane::remove-self",
            Some("a different lane or the operator".to_owned()),
            Some(request.lane.to_string()),
            "ask the operator to remove this lane after it drains",
        );
    }
    let observed = match current_residency(bus, epoch) {
        Ok(head) => head,
        Err(outcome) => return typed_outcome(outcome),
    };
    let Some(row) = observed
        .body()
        .lanes
        .iter()
        .find(|row| row.lane == request.lane)
        .cloned()
    else {
        return missing_lane(request.lane);
    };
    if let Err(refusal) = removable(bus, seats, &row) {
        return refusal;
    }
    let capability = match super::steering::lane_capability(&row) {
        Ok(capability) => capability,
        Err(outcome) => return typed_outcome(outcome),
    };
    let topology_revision = match observed.body().topology_revision.checked_add(1) {
        Some(revision) => revision,
        None => {
            return typed_store_fault(
                "lane::topology-revision-exhausted",
                "the durable topology revision cannot advance",
            );
        }
    };
    let answer = LaneChangeAnswer {
        lane: row.lane,
        topology_revision,
        from: row.disposition.clone(),
        to: "removed".to_owned(),
        git: None,
        rtk: None,
    };
    if let Err(outcome) = super::consent_surface::commit_lapse(bus, runtime.reads, row.lane) {
        return typed_outcome(outcome);
    }
    let proposal = match propose_remove(command, request, &observed, &row, answer.clone()) {
        Ok(proposal) => proposal,
        Err(outcome) => return typed_outcome(outcome),
    };
    let committed = match commit(bus, proposal) {
        Ok(committed) => committed,
        Err(outcome) => return typed_outcome(outcome),
    };
    runtime
        .capabilities
        .remove(capability, &Actor::Lane(row.lane));
    super::steering::remove_lane_surface(paths, row.lane);
    // The lane's live peer duties end with it; a lane that later takes its
    // place is another destination. Startup parks what a failure here leaves.
    if let Err(outcome) = super::steering::dead_letter_removed_lanes(bus, &mut io::sink()) {
        return typed_outcome(outcome);
    }
    committed_lane_reply(committed, answer)
}

/// A seat ends only once it is vacant and owns no active claim.
fn removable(
    bus: &Bus,
    seats: &Seats,
    row: &wire::LaneResidency,
) -> Result<(), Outcome<CommandAnswer>> {
    if row.occupant.is_some() || seats.occupied(row.lane) {
        return Err(lane_refusal(
            "lane::remove-occupied",
            Some(VACANT.to_owned()),
            Some(row.disposition.clone()),
            "drain the lane before removing it",
        ));
    }
    match active_claim_for_lane(bus, row.lane) {
        Ok(None) => Ok(()),
        Ok(Some(claim)) => Err(lane_refusal(
            "lane::remove-claimed",
            Some("no active claim owned by the lane".to_owned()),
            Some(claim.to_string()),
            "finish, review, or free the claimed task before removing the lane",
        )),
        Err(outcome) => Err(typed_outcome(outcome)),
    }
}

fn active_claim_for_lane(bus: &Bus, lane: CanonicalUuid) -> CommandResult<Option<wire::TaskStamp>> {
    let snapshot = bus
        .task_snapshot()
        .map_err(|error| store_fault("lane::claim-heads-unreadable", &error.to_string()))?;
    Ok(snapshot.tasks().iter().find_map(|task| {
        let claim = task.claim()?.body();
        (claim.lane == lane && claim.disposition == wire::ClaimDisposition::Active)
            .then(|| task.statement().body().born.clone())
    }))
}

/// Admit one already-durable capture report from the current occupant.
///
/// Stale reports can race a completed drain or renewal and are discarded by
/// exact occupant identity. Session recognition, taste recognition, and app
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
    degrade_occupant(
        bus,
        epoch,
        &observed,
        failure.lane,
        format!(
            "occupant {} capture failed: {}",
            failure.occupant, failure.failure
        ),
    )
}

/// Durably degrade an occupant whose detached supervisor could not be adopted.
pub(super) fn supervisor_failed(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    failure: &str,
) -> CommandResult<()> {
    let observed = current_residency(bus, epoch)?;
    let Some(row) = observed.body().lanes.iter().find(|row| row.lane == lane) else {
        return Ok(());
    };
    if row.occupant != Some(occupant) {
        return Ok(());
    }
    degrade_occupant(
        bus,
        epoch,
        &observed,
        lane,
        format!("occupant {occupant} supervisor failed: {failure}"),
    )
}

fn degrade_occupant(
    bus: &Bus,
    epoch: CanonicalUuid,
    observed: &journal::JournalHead<wire::ResidencyHead>,
    lane: CanonicalUuid,
    marker: String,
) -> CommandResult<()> {
    let mut next = observed.body().clone();
    let row = next
        .lanes
        .iter_mut()
        .find(|row| row.lane == lane)
        .expect("observed degraded lane remains in its cloned head");
    row.occupant = None;
    row.disposition = DEGRADED.to_owned();
    let mut write = write(Actor::Spice, epoch, Producer::ResidentRecognition)?;
    guard_residency(&mut write, observed);
    let fact = write
        .fact(
            wire::body::LaneDegraded { lane, marker },
            FactCause::Existing(observed.position()),
        )
        .map_err(|error| store_fault("lane::capture-fact-invalid", &error.to_string()))?;
    let fact = observed
        .body()
        .lanes
        .iter()
        .find(|row| row.lane == lane)
        .and_then(|row| row.occupant)
        .map_or(Ok(fact), |occupant| {
            renewal::advance_exit(&mut write, &mut next, lane, occupant, fact)
        })?;
    write.residency_head(fact, move |_| next);
    commit(bus, write).map(|_| ())
}

/// Reap and durably vacate the exact occupant named by a kernel exit event.
///
/// An exit that names neither the durable occupant nor the seated one is not
/// accounted, and the values that decided that are written down: a notice
/// dropped in silence leaves an occupied row nothing to explain it. The seat
/// is vacated before the commit, so a commit that loses its lock leaves the
/// durable row as the only place the occupant still lives; the seat sweep of
/// the next reconciliation reads that row against the supervisor state and
/// accounts the same exit from there.
pub(super) fn occupant_exited(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    exit: CaptureExit,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let observed = current_residency(bus, epoch)?;
    let row = observed
        .body()
        .lanes
        .iter()
        .find(|row| row.lane == exit.lane);
    let seated = seats.occupant(exit.lane).map(Occupant::occupant);
    let (vendor, worktree) = match row {
        Some(row) if row.occupant == Some(exit.occupant) && seated == Some(exit.occupant) => {
            (row.vendor.clone(), row.worktree.clone())
        }
        _ => {
            ending::report_unaccounted_exit(diagnostic, exit, row, seated);
            return Ok(());
        }
    };
    let occupant = seats
        .vacate(exit.lane)
        .expect("the exact supervised exit remains seated");
    let evidence = stop_occupant(occupant)
        .map_err(|error| super::environment_fault("lane::exit-reap-failed", &error))?;
    let mut next = observed.body().clone();
    let row = next
        .lanes
        .iter_mut()
        .find(|row| row.lane == exit.lane)
        .expect("observed exit lane remains in its cloned head");
    row.occupant = None;
    row.disposition = evidence.disposition().to_owned();
    let mut write = write(Actor::Spice, epoch, Producer::ResidentRecognition)?;
    guard_residency(&mut write, &observed);
    let fact = ending_fact(
        &mut write,
        exit.lane,
        exit.occupant,
        &evidence,
        FactCause::Existing(observed.position()),
    )?;
    let fact = renewal::advance_exit(&mut write, &mut next, exit.lane, exit.occupant, fact)?;
    write.residency_head(fact, move |_| next);
    commit(bus, write)?;
    // The transcript is complete once the exit is accounted, so an answer the
    // occupant left in its final prose is journaled here rather than waiting
    // for a successor to cite it.
    super::steering::dispose_at_exit(bus, paths, epoch, exit.lane, &vendor, diagnostic);
    // Held work stays parked exactly while its worktree needs an ungranted
    // executable configuration: the resident pends and requires that digest,
    // and drops a digest the worktree pended once it no longer needs it. A
    // failure here, including an unreadable configuration, leaves the claim as
    // it was.
    let need = || {
        let repository = Repository::discover(&worktree)
            .map_err(|error| store_fault("lane::worktree-unresolved", &error.to_string()))?;
        let loaded = spice::config::capture_repository(&repository, &[])
            .map_err(|error| store_fault("trust::surface-unreadable", &error.to_string()))?;
        let gated = !loaded.resolved().root().policy().pre_commit().is_empty();
        Ok(gated.then(|| {
            crate::trust_command::Surface::from_loaded(&loaded)
                .digest()
                .clone()
        }))
    };
    let reconciled = wire::Instant::new(jiff::Timestamp::now())
        .map_err(|error| store_fault("lane::exit-clock-invalid", &error.to_string()))
        .and_then(|at| {
            trust_park::reconcile_trust_park(bus, epoch, exit.lane, &worktree, need, at)
        });
    if let Err(error) = reconciled {
        let _ = writeln!(
            diagnostic,
            "held claim trust parking was not reconciled: {error:?}"
        );
    }
    Ok(())
}

pub(super) fn renewal_requested_exit(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> CommandResult<bool> {
    current_residency(bus, epoch).map(|head| renewal::requested_exit(head.body(), lane, occupant))
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
        _ if command.holds(wire::principal::PrincipalRole::Operator) => request.lane,
        Actor::Lane(lane) if request.lane.is_none_or(|requested| requested == *lane) => Some(*lane),
        Actor::Lane(lane) => {
            return lane_refusal(
                "recall::lane-authority",
                Some(lane.to_string()),
                request.lane.map(|lane| lane.to_string()),
                "a lane may recall only its own continuity account",
            );
        }
        actor => return authority_refusal(actor, "operator or lane"),
    };
    let snapshot = match bus.task_snapshot() {
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
    // A terminal steering fact answers for its directive's current indexed
    // standing, so no ledger is folded from history for the account.
    let mut roots = |key: &steer::Key| {
        let key = key.to_string();
        bus.read(move |read| read.steering_key(&key))
            .map(|head| head.map(|head| StableRoot::from(head.body())))
            .map_err(|error| store_fault("recall::steering-index-unreadable", &error.to_string()))
    };
    let roles = match bus.read(|read| read.principal_roles()) {
        Ok(roles) => roles,
        Err(error) => {
            return typed_store_fault("recall::principal-roles-unreadable", &error.to_string());
        }
    };
    let mut recent = Vec::new();
    for fact in snapshot
        .facts()
        .iter()
        .rev()
        .filter(|fact| fact.seq <= barrier)
        .filter(|fact| selected.is_none_or(|lane| relevant(fact, lane)))
        .take(RECALL_LIMIT)
    {
        let provenance = match recall_provenance(fact, &mut roots) {
            Ok(provenance) => provenance,
            Err(outcome) => return typed_outcome(outcome),
        };
        recent.push(RecallEntry {
            seq: fact.seq,
            kind: fact.fact.kind(),
            actor: roles.author(&fact.actor),
            origin: fact.origin.clone(),
            lane: fact_lane(fact),
            provenance,
        });
    }
    let mut residency = match answer(head.body(), selected, seats, paths) {
        Ok(answer) => answer,
        Err(outcome) => return typed_outcome(outcome),
    };
    if let Err(outcome) = project_holds(bus, barrier, &mut residency) {
        return typed_outcome(outcome);
    }
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

/// Admit only a durable lane worktree root inside this resident's Git common
/// directory. Every seat and every occupant launch passes here before touching
/// the worktree, so a resident started on a copied journal parks lanes that
/// name another repository's worktrees instead of driving them.
pub(in crate::resident) fn admit_worktree(
    paths: &RuntimePaths,
    worktree: &str,
) -> CommandResult<Repository> {
    let repository = Repository::discover(worktree)
        .map_err(|error| store_fault("lane::worktree-unresolved", &error.to_string()))?;
    if repository.common_dir() != paths.common {
        return Err(Outcome::Refusal(Refusal::bare(
            wire::RefusalDiagnostic::new(
                "lane::foreign-worktree".to_owned(),
                Some("git-common-directory".to_owned()),
                Some(paths.common.display().to_string()),
                Some(repository.common_dir().display().to_string()),
            ),
        )));
    }
    if repository.worktree_root() != Path::new(worktree) {
        return Err(Outcome::Refusal(Refusal::bare(
            wire::RefusalDiagnostic::new(
                "lane::worktree-not-root".to_owned(),
                Some("worktree".to_owned()),
                Some(repository.worktree_root().display().to_string()),
                Some(worktree.to_owned()),
            ),
        )));
    }
    Ok(repository)
}
