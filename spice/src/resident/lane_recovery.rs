//! Predecessor lane import and atomic authority transfer.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bus::Bus;
use journal::{FactCause, JournalGuard, JournalWrite, PendingFact};
use lane::capture::CaptureReporter;
use lane::occupant::{OccupantWait, Seats};
use spice::repository::Repository;
use wire::intent::{CommandAnswer, LaneChangeAnswer, LaneState};
use wire::{
    Actor, CanonicalUuid, Command, Outcome, Producer, Refusal, Reply, ReplyPosition, SeqRange,
};

#[cfg(test)]
use super::lane_surface::await_session;
use super::lane_surface::{OccupantLaunchRequest, resume_session, session_fact, session_source};
use super::shutdown::{current_residency, guard_residency, rebase_lane};
use super::{
    CommandResult, RuntimePaths, commit, new_uuid, seq, store_fault, typed_outcome, write,
};

const OCCUPIED: &str = "occupied";
const VACANT: &str = "vacant";

pub(super) struct Runtime {
    capture: Arc<dyn CaptureReporter>,
}

impl Runtime {
    pub(super) fn new(capture: Arc<dyn CaptureReporter>) -> Self {
        Self { capture }
    }
}

struct ImportContext {
    observed: journal::JournalHead<wire::ResidencyHead>,
    row: wire::LaneResidency,
    worktree: String,
    ancestor: CanonicalUuid,
    resume: vendor::Resume,
    carried: Option<CarriedClaim>,
}

struct ProvenOccupant {
    occupant: CanonicalUuid,
    group: u32,
    session: wire::LaneSession,
    source: CanonicalUuid,
    git: String,
}

#[derive(Clone)]
struct CarriedClaim {
    task: wire::TaskStamp,
    claim: journal::JournalHead<wire::ClaimHead>,
    lease: journal::JournalHead<wire::LeaseHead>,
    /// Accounted transcript the predecessor occupant stopped spending at.
    source: wire::body::SourceBoundary,
}

pub(super) struct ImportLaunch {
    launch: super::supervisor::PendingLaunch,
    pending: PendingImport,
}

impl ImportLaunch {
    pub(super) fn lane(&self) -> CanonicalUuid {
        self.pending.request.lane
    }

    pub(super) fn into_parts(self) -> (super::supervisor::PendingLaunch, PendingImport) {
        (self.launch, self.pending)
    }
}

pub(super) struct PendingImport {
    context: ImportContext,
    request: LaneState,
    occupant: CanonicalUuid,
}

pub(super) fn begin_import(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &Seats,
    runtime: &Runtime,
    request: &LaneState,
) -> Result<ImportLaunch, Outcome<CommandAnswer>> {
    if request.action != wire::intent::LaneAction::Wake {
        return Err(lane_refusal(
            "lane::state-shape",
            Some("wake action with successor worktree".into()),
            Some(format!("{:?}", request.action).to_lowercase()),
            "use lane import through its registered CLI contract",
        ));
    }
    let context = admit_import(bus, paths, epoch, seats, request).map_err(typed_outcome)?;
    let occupant = new_uuid();
    let fast_mode =
        super::settings_surface::lane_fast_mode(bus, context.observed.body(), &context.row)
            .map_err(typed_outcome)?;
    let launch = super::supervisor::begin_launch(
        paths,
        OccupantLaunchRequest {
            worktree: &context.worktree,
            vendor: &context.row.vendor,
            epoch,
            lane: context.row.lane,
            occupant,
            resume: context.resume.clone(),
            fast_mode,
            model: context.row.model.as_deref(),
            effort: context.row.effort.as_deref(),
        },
        Arc::clone(&runtime.capture),
    )
    .map_err(typed_outcome)?;
    Ok(ImportLaunch {
        launch,
        pending: PendingImport {
            context,
            request: request.clone(),
            occupant,
        },
    })
}

pub(super) fn finish_import(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    command: &wire::AdmittedCommand,
    mut pending: PendingImport,
    launched: CommandResult<super::supervisor::Launch>,
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
    };
    if let Err(error) =
        seats.seat_detached(pending.context.row.lane, pending.occupant, launch.process)
    {
        return typed_outcome(store_fault(
            "lane::import-launch-failed",
            &error.to_string(),
        ));
    }
    pending.context.observed =
        match rebase_lane(bus, epoch, &pending.context.observed, pending.request.lane) {
            Ok(observed) => observed,
            Err(outcome) => {
                stop_lane(seats, pending.request.lane);
                return typed_outcome(outcome);
            }
        };
    let answer = LaneChangeAnswer {
        lane: pending.request.lane,
        topology_revision: pending.context.observed.body().topology_revision,
        from: pending.context.row.worktree.clone(),
        to: pending.context.worktree.clone(),
        git: Some(proven.git.clone().into_boxed_str()),
        rtk: proven.session.launch.rtk.clone(),
    };
    let proposal = propose_import(
        command,
        epoch,
        &pending.request,
        &pending.context,
        &proven,
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
        Ok(committed) => committed_reply(committed, answer),
        Err(outcome) => {
            stop_lane(seats, pending.request.lane);
            typed_outcome(outcome)
        }
    }
}

#[cfg(test)]
pub(super) fn import(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    runtime: &Runtime,
    command: &wire::AdmittedCommand,
    request: &LaneState,
) -> Outcome<CommandAnswer> {
    let launch = match begin_import(bus, paths, epoch, seats, runtime, request) {
        Ok(launch) => launch,
        Err(outcome) => return outcome,
    };
    let (launch, pending) = launch.into_parts();
    finish_import(
        bus,
        paths,
        epoch,
        seats,
        command,
        pending,
        launch.establish(),
    )
}

fn admit_import(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &Seats,
    request: &LaneState,
) -> CommandResult<ImportContext> {
    let worktree = request.worktree.clone().ok_or_else(|| {
        lane_refusal_unit(
            "lane::import-worktree-missing",
            Some("one canonical linked worktree root".into()),
            None,
            "name the worktree receiving this vacant lineage seat",
        )
    })?;
    let observed = current_residency(bus, epoch)?;
    let row = observed
        .body()
        .lanes
        .iter()
        .find(|row| row.lane == request.lane)
        .cloned()
        .ok_or_else(|| {
            lane_refusal_unit(
                "lane::missing",
                Some("one current lane".into()),
                Some(request.lane.to_string()),
                "read lane and retry with a current lane identity",
            )
        })?;
    admit_row(paths, seats, observed.body(), &row, &worktree)?;
    super::steering::lane_capability(&row)?;
    let ancestor = row.lineage.last().copied().ok_or_else(|| {
        store_fault(
            "lane::lineage-missing",
            "a durable lane import has no predecessor occupant lineage",
        )
    })?;
    let resume = resume_session(paths, &row)?;
    let at = wire::Instant::new(jiff::Timestamp::now())
        .map_err(|error| store_fault("lane::clock-invalid", &error.to_string()))?;
    let carried = observe_carried_claim(bus, row.lane, ancestor, at)?;
    Ok(ImportContext {
        observed,
        row,
        worktree,
        ancestor,
        resume,
        carried,
    })
}

fn admit_row(
    paths: &RuntimePaths,
    seats: &Seats,
    residency: &wire::ResidencyHead,
    row: &wire::LaneResidency,
    worktree: &str,
) -> CommandResult<()> {
    if row.occupant.is_some() || seats.occupied(row.lane) {
        return Err(lane_refusal_unit(
            "lane::import-occupied",
            Some(VACANT.into()),
            Some(row.disposition.clone()),
            "drain the predecessor before importing",
        ));
    }
    if worktree == row.worktree {
        return Err(lane_refusal_unit(
            "lane::import-unmoved",
            Some("a different linked worktree".into()),
            Some(worktree.into()),
            "use lane wake in the prior worktree",
        ));
    }
    if residency
        .lanes
        .iter()
        .any(|other| other.lane != row.lane && other.worktree == worktree)
    {
        return Err(lane_refusal_unit(
            "lane::worktree-occupied",
            Some("one lane per worktree".into()),
            Some(worktree.into()),
            "relocate the lane already bound there",
        ));
    }
    validate_worktree(paths, worktree)
}

fn propose_import(
    command: &wire::AdmittedCommand,
    epoch: CanonicalUuid,
    request: &LaneState,
    context: &ImportContext,
    proven: &ProvenOccupant,
    answer: LaneChangeAnswer,
) -> CommandResult<JournalWrite> {
    let mut next = context.observed.body().clone();
    let row = next
        .lanes
        .iter_mut()
        .find(|row| row.lane == request.lane)
        .expect("observed imported lane remains in its cloned head");
    row.worktree.clone_from(&context.worktree);
    row.occupant = Some(proven.occupant);
    row.lineage.push(proven.occupant);
    super::lane_surface::seat_session(row, proven.session.clone());
    row.disposition = OCCUPIED.into();
    let mut proposal = write(
        command.actor().clone(),
        epoch,
        Producer::ResidentRecognition,
    )?;
    guard_residency(&mut proposal, &context.observed);
    if let Some(carried) = &context.carried {
        guard_carried_claim(&mut proposal, carried);
    }
    let started = import_facts(&mut proposal, epoch, context, proven)?;
    let head = match &context.carried {
        Some(carried) => append_claim_carry(
            &mut proposal,
            carried,
            context,
            proven.occupant,
            epoch,
            started,
        )?,
        None => started,
    };
    proposal.residency_head(head, move |_| next);
    command_result(&mut proposal, command, request.clone(), epoch, answer)?;
    Ok(proposal)
}

fn import_facts(
    write: &mut JournalWrite,
    epoch: CanonicalUuid,
    context: &ImportContext,
    proven: &ProvenOccupant,
) -> CommandResult<PendingFact> {
    let imported = write
        .fact(
            wire::body::LaneImported {
                lane: context.row.lane,
                ancestor: context.ancestor,
                from_worktree: context.row.worktree.clone(),
                worktree: context.worktree.clone(),
                vendor: context.row.vendor.clone(),
                identity: proven.session.identity.clone(),
                epoch,
            },
            FactCause::Existing(context.observed.position()),
        )
        .map_err(|error| store_fault("lane::import-fact-invalid", &error.to_string()))?;
    let declared = write
        .fact(
            session_fact(&proven.session, proven.source),
            FactCause::Pending(imported),
        )
        .map_err(|error| store_fault("lane::import-fact-invalid", &error.to_string()))?;
    write
        .fact(
            wire::body::LaneStarted {
                lane: context.row.lane,
                occupant: proven.occupant,
                epoch,
                group: proven.group,
                fresh_session: Some(matches!(context.resume, vendor::Resume::Fresh)),
            },
            FactCause::Pending(declared),
        )
        .map_err(|error| store_fault("lane::import-fact-invalid", &error.to_string()))
}

fn observe_carried_claim(
    bus: &Bus,
    lane: CanonicalUuid,
    ancestor: CanonicalUuid,
    at: wire::Instant,
) -> CommandResult<Option<CarriedClaim>> {
    let snapshot = bus
        .task_snapshot()
        .map_err(|error| store_fault("lane::claim-heads-unreadable", &error.to_string()))?;
    let mut carried = None;
    for heads in snapshot.tasks() {
        let Some(claim) = heads.claim() else { continue };
        if claim.body().disposition != wire::ClaimDisposition::Active {
            continue;
        }
        if claim.body().lane != lane && claim.body().actor != Actor::Lane(lane) {
            continue;
        }
        if carried.is_some() {
            return Err(store_fault(
                "lane::claim-ambiguous",
                "one lane owns more than one active claim",
            ));
        }
        let actor_bound = wire::principal::ActingPrincipal::of(&claim.body().actor).is_some()
            || claim.body().actor == Actor::Lane(lane);
        if !actor_bound || claim.body().lane != lane || claim.body().occupant != ancestor {
            return Err(store_fault(
                "lane::claim-lineage-moved",
                "active claim ownership differs from predecessor lineage",
            ));
        }
        let lease = heads.lease().ok_or_else(|| {
            store_fault(
                "lane::claim-lease-missing",
                "an imported active claim has no durable lease head",
            )
        })?;
        if lease.body().epoch != claim.body().epoch {
            return Err(store_fault(
                "lane::claim-epoch-mismatch",
                "claim and lease generations disagree",
            ));
        }
        if at < lease.body().start || at >= lease.body().expiry {
            return Err(lane_refusal_unit(
                "lane::claim-lease-stale",
                Some(format!(
                    "{} <= import instant < {}",
                    lease.body().start,
                    lease.body().expiry
                )),
                Some(at.to_string()),
                "reallocate an expired claim before importing its lane",
            ));
        }
        carried = Some((
            heads.statement().body().born.clone(),
            claim.clone(),
            lease.clone(),
        ));
    }
    // Read once the lane is known to carry a claim, and at the same barrier the
    // claim and lease heads were observed at, so the successor takes over at the
    // position its predecessor is guarded against having moved past.
    carried
        .map(|(task, claim, lease)| {
            Ok(CarriedClaim {
                task,
                claim,
                lease,
                source: bus.source_boundary(&lane).map_err(|error| {
                    store_fault("lane::claim-boundary-unreadable", &error.to_string())
                })?,
            })
        })
        .transpose()
}

fn guard_carried_claim(write: &mut JournalWrite, carried: &CarriedClaim) {
    let task = carried.task.clone();
    let claim = carried.claim.clone();
    let lease = carried.lease.clone();
    write.guard(JournalGuard::new(move |read| {
        if read.claim_head(&task)? != Some(claim.clone()) {
            return Err(journal::GuardError::refused("lane::claim-head-moved"));
        }
        if read.lease_head(&task)? != Some(lease.clone()) {
            return Err(journal::GuardError::refused("lane::lease-head-moved"));
        }
        Ok(())
    }));
}

fn append_claim_carry(
    write: &mut JournalWrite,
    carried: &CarriedClaim,
    context: &ImportContext,
    occupant: CanonicalUuid,
    epoch: CanonicalUuid,
    cause: PendingFact,
) -> CommandResult<PendingFact> {
    let fact = write
        .fact(
            wire::body::ClaimCarried {
                task: carried.task.clone(),
                lane: context.row.lane,
                occupant,
                ancestor: context.ancestor,
                epoch,
                source: carried.source.clone(),
            },
            FactCause::Pending(cause),
        )
        .map_err(|error| store_fault("lane::claim-carry-invalid", &error.to_string()))?;
    let mut claim = carried.claim.body().clone();
    claim.occupant = occupant;
    claim.epoch = epoch;
    write.claim_head(carried.task.clone(), fact, move |position| {
        claim.fact = position;
        claim
    });
    let mut lease = carried.lease.body().clone();
    lease.epoch = epoch;
    write.lease_head(carried.task.clone(), fact, move |_| lease);
    Ok(fact)
}

/// End a recovering lane's occupant without publishing how it ended.
///
/// Dropping the ending is deliberate here for the same reason it is in the lane
/// surface's namesake: every caller is a compensation path — `propose_import`,
/// the commit, or `await_session` failed — so the seating being unwound was
/// never committed, and an ending fact would name a residency the journal never
/// recorded beginning.
///
/// This one does not even classify the ending, because it has no reader to
/// classify it for. Where a kill is durable it is published as
/// [`wire::body::LaneKilled`] rather than folded into an exit status the
/// occupant never chose.
fn stop_lane(seats: &mut Seats, lane: CanonicalUuid) {
    let Some(occupant) = seats.vacate(lane) else {
        return;
    };
    let occupant = match occupant.wait_for(Duration::ZERO) {
        Ok(OccupantWait::Running(occupant)) => occupant,
        Ok(OccupantWait::Exited { .. }) | Err(_) => return,
    };
    if occupant.terminate().is_err() {
        return;
    }
    let grace = occupant.termination_grace();
    let occupant = match occupant.wait_for(grace) {
        Ok(OccupantWait::Running(occupant)) => occupant,
        Ok(OccupantWait::Exited { .. }) | Err(_) => return,
    };
    if occupant.kill().is_ok() {
        let reap = occupant.reap_bound();
        let _waited = occupant.wait_for(reap);
    }
}

fn validate_worktree(paths: &RuntimePaths, worktree: &str) -> CommandResult<()> {
    let repository = Repository::discover(worktree)
        .map_err(|error| store_fault("lane::worktree-unresolved", &error.to_string()))?;
    if repository.common_dir() != paths.common {
        return Err(lane_refusal_unit(
            "lane::foreign-worktree",
            Some(paths.common.display().to_string()),
            Some(repository.common_dir().display().to_string()),
            "choose a resident repository worktree",
        ));
    }
    if repository.worktree_root() != Path::new(worktree) {
        return Err(lane_refusal_unit(
            "lane::worktree-not-root",
            Some(repository.worktree_root().display().to_string()),
            Some(worktree.into()),
            "name the canonical worktree root",
        ));
    }
    Ok(())
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

fn committed_reply(
    committed: bus::JournalCommit,
    answer: LaneChangeAnswer,
) -> Outcome<CommandAnswer> {
    Outcome::Reply(Reply {
        position: ReplyPosition::Committed(SeqRange {
            first: seq(committed.first()),
            last: seq(committed.last()),
        }),
        body: CommandAnswer::LaneChange(Box::new(answer)),
    })
}

fn lane_refusal_unit(
    reason: &str,
    expected: Option<String>,
    observed: Option<String>,
    detail: &str,
) -> Outcome<()> {
    Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(reason, Some("lane".into()), expected, observed)
            .with_detail(detail),
    ))
}

fn lane_refusal(
    reason: &str,
    expected: Option<String>,
    observed: Option<String>,
    detail: &str,
) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(reason, Some("lane".into()), expected, observed)
            .with_detail(detail),
    ))
}

#[cfg(test)]
#[path = "lane_recovery_tests.rs"]
mod tests;
