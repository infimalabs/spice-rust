//! One resident-owned automatic lifecycle decision authority.
//!
//! Durable commit notifications and explicit timer completions call
//! [`Runtime::reconcile`].  The resolver itself reads no clock: `now` is an
//! input, READY ages are projected from the fact that established each current
//! workflow head, and all future work is returned as one UTC deadline for the
//! resident timer worker.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use board::Entry;
use bus::Bus;
use journal::FactCause;
use lane::hold::{Death, Holds};
use lane::occupant::{Activity, Seats};
use spice::repository::Repository;
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, Instant, Outcome, Producer, ReplyPosition, Seq,
    TaskStamp,
};

use super::lane_surface;
use super::shutdown::{current_residency, guard_residency};
use super::{CommandResult, RuntimePaths, SurfaceConfig, commit, new_uuid, store_fault, write};

mod availability;
mod holds;
mod launch;
mod matching;
mod materialization;
use availability::{BoardView, ready_instant, selected_ready};
use matching::match_targets;
use materialization::Projection;
pub(super) mod generation_notices;
mod review_feedback;
mod targets;
pub(in crate::resident) use launch::PendingLaunch;
use launch::{Launching, pending_launch};
use targets::{await_regular_jobs, project_targets};
#[cfg(test)]
mod job_hold_tests;
#[cfg(test)]
mod startup_tests;

const NORMAL_READY: usize = 2;
const STARVATION_SECONDS: i64 = 180;
const SETTLE_SECONDS: i64 = board::HANDOFF_SECONDS;
const RETRY_SECONDS: i64 = 5;

/// Closed automatic wake arms, in resolver order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Trigger {
    PendingInbox,
    HeldClaim,
    DrainQueue,
    AvailableWork,
}

impl Trigger {
    const fn as_str(self) -> &'static str {
        match self {
            Self::PendingInbox => "pending-inbox",
            Self::HeldClaim => "held-claim",
            Self::DrainQueue => "drain-queue",
            Self::AvailableWork => "available-work",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Available {
    None,
    At(Instant),
    Ready,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SelectedReady {
    at: Instant,
    eligible: usize,
    handoff: bool,
    handoff_wait: bool,
    /// The selection is in the allocator's last-resort class for this actor.
    last_resort: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Target<'a> {
    lane: CanonicalUuid,
    /// Journal position of the newest claim this lane won; `None` if it never has.
    last_claim: Option<Seq>,
    pending_inbox: bool,
    held_claim: Available,
    drain_queue: Available,
    available: Available,
    /// The work the drain-queue or available-work trigger would claim is in
    /// this lane's last-resort class: its own independent handoff or review,
    /// or a row it freed.
    last_resort: bool,
    /// The task the allocator selected for this lane's drain-queue or
    /// available-work trigger, which a claim would select again.
    task: Option<&'a Entry>,
    /// How well the lane's retained work matches `task`, scored only under
    /// the best-match order; absent when nothing scores.
    matched: Option<f64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Resolution {
    Idle,
    Wait {
        trigger: Trigger,
        lane: CanonicalUuid,
        deadline: Instant,
    },
    Attempt {
        trigger: Trigger,
        lane: CanonicalUuid,
    },
}

enum Attempted {
    Started,
    DeadLettered(super::steering::DeadLettered),
    Launching(Launching),
}

/// Process-local lifecycle backoff; durable eligibility remains in the bus.
#[derive(Default)]
struct Backoff {
    timed: BTreeMap<CanonicalUuid, Instant>,
    parked: BTreeMap<CanonicalUuid, Trigger>,
}

impl Backoff {
    fn retain_targets(&mut self, targets: &[Target]) {
        self.timed
            .retain(|lane, _| targets.iter().any(|target| target.lane == *lane));
        self.parked
            .retain(|lane, _| targets.iter().any(|target| target.lane == *lane));
    }

    fn release(&mut self, lane: CanonicalUuid) {
        self.timed.remove(&lane);
        self.parked.remove(&lane);
    }

    fn release_all(&mut self) {
        self.timed.clear();
        self.parked.clear();
    }

    fn due(&self, lane: CanonicalUuid, trigger: Trigger, eligible_at: Instant) -> Option<Instant> {
        if self.parked.get(&lane) == Some(&trigger) {
            return None;
        }
        Some(
            self.timed
                .get(&lane)
                .copied()
                .filter(|deadline| *deadline > eligible_at)
                .unwrap_or(eligible_at),
        )
    }

    fn refused(
        &mut self,
        trigger: Trigger,
        lane: CanonicalUuid,
        reason: &str,
        now: Instant,
        diagnostic: &mut dyn std::io::Write,
    ) -> CommandResult<Option<WakeDeadline>> {
        if !start_refusal_is_retryable(reason) {
            self.timed.remove(&lane);
            self.parked.insert(lane, trigger);
            let _ = writeln!(
                diagnostic,
                "lifecycle parked trigger={} lane={} reason={}",
                trigger.as_str(),
                lane,
                reason,
            );
            return Ok(None);
        }

        let deadline = after(now, RETRY_SECONDS)?;
        self.timed.insert(lane, deadline);
        let _ = writeln!(
            diagnostic,
            "lifecycle refused trigger={} lane={} reason={} retry={}",
            trigger.as_str(),
            lane,
            reason,
            deadline,
        );
        Ok(Some(WakeDeadline {
            lane,
            at: deadline,
            kind: WakeKind::Task,
        }))
    }
}

pub(super) struct Runtime {
    backoff: Backoff,
    holds: Holds,
    launching: BTreeSet<CanonicalUuid>,
    /// How long after its seating an occupant's unsettled death is young.
    young: Duration,
    /// How long a hold keeps the lane from its next wake.
    hold: Duration,
    projection: Projection,
}

/// Exact admitted command generation that released automatic lifecycle work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ReconciliationWitness {
    command: CanonicalUuid,
    epoch: CanonicalUuid,
    position: Option<ReplyPosition>,
}

impl ReconciliationWitness {
    pub(super) const fn new(
        command: CanonicalUuid,
        epoch: CanonicalUuid,
        position: Option<ReplyPosition>,
    ) -> Self {
        Self {
            command,
            epoch,
            position,
        }
    }

    const fn awaits_commit(self) -> bool {
        matches!(self.position, Some(ReplyPosition::Committed(_)))
    }

    const fn reached_by(self, last: u64) -> bool {
        match self.position {
            Some(ReplyPosition::Committed(range)) => last >= range.last.get(),
            Some(ReplyPosition::Snapshot(cursor)) => last > cursor.get(),
            Some(ReplyPosition::Direct {}) | None => true,
        }
    }
}

/// One constant-size lifecycle demand waiting for an admitted resident event.
#[derive(Default)]
pub(super) struct Reconciliation {
    pending: Option<ReconciliationWitness>,
}

impl Reconciliation {
    pub(super) fn request(&mut self, witness: ReconciliationWitness) {
        if self.pending != Some(witness) {
            self.pending = Some(witness);
        }
    }

    pub(super) fn control_enabled(&self) -> bool {
        self.pending.is_none_or(|witness| !witness.awaits_commit())
    }

    pub(super) fn control_settles(&self) -> bool {
        self.pending.is_some_and(|witness| !witness.awaits_commit())
    }

    pub(super) fn settle(&mut self) {
        self.pending = None;
    }

    pub(super) fn settle_commit(&mut self, commit: bus::JournalCommit) {
        let reached = self
            .pending
            .is_some_and(|witness| witness.reached_by(commit.last()));
        if reached {
            self.settle();
        }
    }
}

/// The one future event the timer runtime must arm after reconciliation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct WakeDeadline {
    pub(super) lane: CanonicalUuid,
    pub(super) at: Instant,
    pub(super) kind: WakeKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WakeKind {
    Task,
    Hold,
}

/// The resident authority a lifecycle reconciliation reads and wakes through.
pub(super) struct Resident<'a> {
    pub(super) bus: &'a Bus,
    pub(super) repository: &'a Repository,
    pub(super) paths: &'a RuntimePaths,
    pub(super) epoch: CanonicalUuid,
    pub(super) lane_runtime: &'a lane_surface::Runtime<'a>,
    pub(super) config: &'a SurfaceConfig,
    /// The retained session profiles the best-match order scores; absent
    /// until message startup restores them.
    pub(super) profiles: Option<&'a std::sync::Mutex<super::message_surface::MessageState>>,
}

impl Runtime {
    pub(super) fn new(config: &lane::Config) -> Self {
        Self {
            backoff: Backoff::default(),
            launching: BTreeSet::new(),
            holds: Holds::new(config.deaths()),
            young: config.young().get(),
            hold: config.hold().get(),
            projection: Projection::new(),
        }
    }

    /// Reconcile one authoritative snapshot and execute at most one wake.
    pub(super) fn reconcile(
        &mut self,
        resident: &Resident<'_>,
        seats: &mut Seats,
        launches: &mut super::launch_runtime::Runtime,
        diagnostic: &mut dyn std::io::Write,
    ) -> CommandResult<Option<WakeDeadline>> {
        let now = current_instant()?;
        self.reconcile_at(now, resident, seats, launches, diagnostic)
    }

    fn reconcile_at(
        &mut self,
        now: Instant,
        resident: &Resident<'_>,
        seats: &mut Seats,
        launches: &mut super::launch_runtime::Runtime,
        diagnostic: &mut dyn std::io::Write,
    ) -> CommandResult<Option<WakeDeadline>> {
        let &Resident {
            bus,
            paths,
            epoch,
            config,
            profiles,
            ..
        } = resident;
        lane_surface::reconcile_renewal(bus, paths, epoch)?;
        let mut snapshot = bus
            .task_snapshot()
            .map_err(|error| store_fault("lifecycle::snapshot-unreadable", &error.to_string()))?;
        let cursor = self.projection.cursor();
        self.synchronize_holds(&snapshot, cursor)?;
        if cursor.is_none() || snapshot.facts().after(cursor).next().is_some() {
            super::supervisor::terminal::reconcile_services(
                paths,
                &snapshot,
                cursor.is_none(),
                diagnostic,
            );
        }
        if generation_notices::reconcile(
            bus,
            paths,
            epoch,
            &snapshot,
            cursor,
            config.steering.expiry().get(),
        )? {
            snapshot = bus.task_snapshot().map_err(|error| {
                store_fault("lifecycle::snapshot-unreadable", &error.to_string())
            })?;
            self.synchronize_holds(&snapshot, self.projection.cursor())?;
        }
        self.projection
            .observe_roles(super::principal_surface::roles(bus)?);
        loop {
            self.projection.advance(&snapshot, &config.projects)?;
            if !self.release_changed_vendor_hold(bus, epoch, &snapshot)? {
                break;
            }
            snapshot = bus.task_snapshot().map_err(|error| {
                store_fault("lifecycle::snapshot-unreadable", &error.to_string())
            })?;
            self.synchronize_holds(&snapshot, self.projection.cursor())?;
        }
        let mut targets = project_targets(
            &snapshot,
            &self.projection,
            now,
            &self.holds,
            &config.allocation,
        )?;
        await_regular_jobs(&mut targets, now, |lane| {
            super::supervisor::terminal::Jobs::for_lane(paths, lane).finite_horizon()
        });
        targets.retain(|target| !self.launching.contains(&target.lane));
        self.backoff.retain_targets(&targets);
        if config.lane.wake_order() == lane::WakeOrder::BestMatch
            && let Some(profiles) = profiles
        {
            match_targets(&mut targets, self.projection.entries(), profiles)?;
        }
        let task = match resolve(&targets, &self.backoff, now, config.lane.wake_order()) {
            Resolution::Idle => None,
            Resolution::Wait { lane, deadline, .. } => {
                // A wait is unchanged process-local scheduler state, not a
                // lifecycle event. Durable commits can reconcile the same
                // deadline thousands of times while a transcript is active;
                // the timer runtime already coalesces that identical arm.
                Some(WakeDeadline {
                    lane,
                    at: deadline,
                    kind: WakeKind::Task,
                })
            }
            Resolution::Attempt { trigger, lane } => {
                let result = self.attempt(trigger, lane, resident, seats, &snapshot);
                self.settle_attempt(now, trigger, lane, result, launches, diagnostic)?
            }
        };
        let hold = self.holds.next().map(|(lane, at)| WakeDeadline {
            lane,
            at,
            kind: WakeKind::Hold,
        });
        Ok(earliest(task, hold))
    }

    fn settle_attempt(
        &mut self,
        now: Instant,
        trigger: Trigger,
        lane: CanonicalUuid,
        result: CommandResult<Attempted>,
        launches: &mut super::launch_runtime::Runtime,
        diagnostic: &mut dyn std::io::Write,
    ) -> CommandResult<Option<WakeDeadline>> {
        match result {
            Ok(Attempted::Started) => {
                self.backoff.release(lane);
                let _ = writeln!(
                    diagnostic,
                    "lifecycle started trigger={} lane={}",
                    trigger.as_str(),
                    lane
                );
                Ok(None)
            }
            Ok(Attempted::Launching(launching)) => {
                self.launching.insert(lane);
                launches.start_automatic(launching.launch, launching.pending);
                let _ = writeln!(
                    diagnostic,
                    "lifecycle establishing trigger={} lane={}",
                    trigger.as_str(),
                    lane
                );
                Ok(None)
            }
            Ok(Attempted::DeadLettered(dead)) => {
                self.backoff.release(lane);
                dead.log(diagnostic, trigger.as_str(), lane);
                Ok(None)
            }
            Err(Outcome::Refusal(refusal)) => {
                self.backoff
                    .refused(trigger, lane, refusal.reason(), now, diagnostic)
            }
            Err(Outcome::Fault(fault)) => Err(Outcome::Fault(fault)),
            Err(Outcome::Reply(_)) => {
                unreachable!("unit lifecycle failures carry no reply")
            }
        }
    }

    /// A same-lane command is the event that can make a parked local
    /// precondition current again. It also supersedes an elapsed-time retry.
    pub(super) fn observe_command(&mut self, actor: &Actor) -> bool {
        if let Actor::Lane(lane) = actor {
            self.backoff.release(*lane);
            return true;
        }
        false
    }

    /// A topology command can change which team policy governs any parked
    /// member, so it makes every local startup refusal eligible for retry.
    pub(super) fn observe_topology(&mut self, command: &wire::AdmittedCommand) -> bool {
        if self.observe_command(command.actor()) {
            return true;
        }
        if matches!(
            command.body(),
            wire::intent::Intent::TeamConfigure(_)
                | wire::intent::Intent::TeamArrange(_)
                | wire::intent::Intent::LaneArrange(_)
        ) {
            // A deliberate topology or team-policy change is new startup
            // authority. Retry every member against the committed shape rather
            // than retaining a refusal parked under the preceding policy.
            self.backoff.release_all();
            return true;
        }
        false
    }

    pub(super) fn release_all(&mut self) {
        self.backoff.release_all();
    }

    pub(super) fn launch_pending(&self, lane: CanonicalUuid) -> bool {
        self.launching.contains(&lane)
    }

    pub(super) fn begin_external_launch(&mut self, lane: CanonicalUuid) {
        self.launching.insert(lane);
    }

    pub(super) fn finish_external_launch(&mut self, lane: CanonicalUuid) {
        self.launching.remove(&lane);
    }

    fn attempt(
        &mut self,
        trigger: Trigger,
        lane: CanonicalUuid,
        resident: &Resident<'_>,
        seats: &mut Seats,
        snapshot: &bus::TaskSnapshot,
    ) -> CommandResult<Attempted> {
        let &Resident {
            bus,
            repository,
            paths,
            epoch,
            lane_runtime,
            config,
            ..
        } = resident;
        let occupant = new_uuid();
        match trigger {
            Trigger::PendingInbox => {
                let woken = super::steering::woken_inbox(bus, lane)?;
                let wake = lane_surface::begin_automatic_wake(
                    bus,
                    paths,
                    epoch,
                    seats,
                    lane_runtime,
                    &lane_surface::WakeRequest {
                        lane,
                        occupant,
                        decision: Some(snapshot),
                        reserved: None,
                        inbox: true,
                    },
                );
                match wake {
                    Ok(lane_surface::BeginAutomaticWake::Launch(launch)) => {
                        Ok(pending_launch(trigger, lane, Some(woken), None, launch))
                    }
                    Ok(lane_surface::BeginAutomaticWake::Unstarted(failure)) => {
                        super::steering::dead_letter_pending(
                            bus, paths, epoch, lane, &woken, &failure,
                        )
                        .map(Attempted::DeadLettered)
                    }
                    Err(failure) if pending_start_failure_is_terminal(&failure) => {
                        super::steering::dead_letter_pending(
                            bus, paths, epoch, lane, &woken, &failure,
                        )
                        .map(Attempted::DeadLettered)
                    }
                    Err(failure) => Err(failure),
                }
            }
            Trigger::HeldClaim => match lane_surface::begin_automatic_wake(
                bus,
                paths,
                epoch,
                seats,
                lane_runtime,
                &lane_surface::WakeRequest {
                    lane,
                    occupant,
                    decision: Some(snapshot),
                    reserved: None,
                    inbox: false,
                },
            )? {
                lane_surface::BeginAutomaticWake::Launch(launch) => {
                    Ok(pending_launch(trigger, lane, None, None, launch))
                }
                lane_surface::BeginAutomaticWake::Unstarted(failure) => Err(failure),
            },
            Trigger::DrainQueue | Trigger::AvailableWork => {
                let claim = crate::board_command::claim_automatic(
                    bus,
                    snapshot,
                    repository,
                    crate::board_command::AutomaticSeat {
                        epoch,
                        lane,
                        occupant,
                    },
                    crate::board_command::AllocationPolicy {
                        projects: &config.projects,
                        allocation: &config.allocation,
                    },
                )
                .map_err(unit_outcome)?;
                let launched = lane_surface::begin_automatic_wake(
                    bus,
                    paths,
                    epoch,
                    seats,
                    lane_runtime,
                    &lane_surface::WakeRequest {
                        lane,
                        occupant,
                        decision: None,
                        reserved: Some(&claim),
                        inbox: false,
                    },
                );
                match launched {
                    Ok(lane_surface::BeginAutomaticWake::Launch(launch)) => {
                        Ok(pending_launch(trigger, lane, None, Some(claim), launch))
                    }
                    Ok(lane_surface::BeginAutomaticWake::Unstarted(failure)) | Err(failure) => {
                        match crate::board_command::release_automatic(bus, repository, &claim) {
                            Ok(()) => Err(failure),
                            Err(release) => Err(unit_outcome(release)),
                        }
                    }
                }
            }
        }
    }
}

fn unit_outcome(outcome: Outcome<wire::intent::CommandAnswer>) -> Outcome<()> {
    match outcome {
        Outcome::Reply(_) => unreachable!("a failing lifecycle fold returned a reply"),
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

const RETRYABLE_START_REFUSALS: [&str; 4] = [
    "lifecycle::wake-capacity-moved",
    "lifecycle::wake-inbox-moved",
    "lifecycle::held-claim-head-moved",
    "lifecycle::held-claim-lease-moved",
];

fn start_refusal_is_retryable(reason: &str) -> bool {
    RETRYABLE_START_REFUSALS.contains(&reason)
}

fn pending_start_failure_is_terminal(failure: &Outcome<()>) -> bool {
    matches!(
        failure,
        Outcome::Refusal(refusal)
            if !start_refusal_is_retryable(refusal.reason())
    )
}

fn earliest(first: Option<WakeDeadline>, second: Option<WakeDeadline>) -> Option<WakeDeadline> {
    match (first, second) {
        (Some(first), Some(second)) => Some(
            if (first.at, first.lane, first.kind as u8)
                <= (second.at, second.lane, second.kind as u8)
            {
                first
            } else {
                second
            },
        ),
        (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
        (None, None) => None,
    }
}

fn fact_body<T: serde::de::DeserializeOwned>(
    envelope: &wire::Envelope<wire::AnyFact>,
) -> CommandResult<T> {
    let bytes = wire::encode(envelope.fact.body())
        .map_err(|error| store_fault("lifecycle::fact-invalid", &error.to_string()))?;
    wire::decode(&bytes).map_err(|error| store_fault("lifecycle::fact-invalid", &error.to_string()))
}

fn drain_from_selected(
    selected: Option<SelectedReady>,
    first_member: bool,
) -> CommandResult<Available> {
    match selected {
        None => Ok(Available::None),
        Some(selected) if selected.handoff_wait => Ok(Available::At(selected.at)),
        Some(selected) if selected.handoff => Ok(Available::Ready),
        Some(selected) if first_member || selected.eligible >= NORMAL_READY => {
            after(selected.at, SETTLE_SECONDS).map(Available::At)
        }
        Some(selected) => after(selected.at, STARVATION_SECONDS).map(Available::At),
    }
}

fn available_from_selected(selected: Option<SelectedReady>) -> CommandResult<Available> {
    match selected {
        None => Ok(Available::None),
        Some(selected) if selected.handoff_wait => Ok(Available::At(selected.at)),
        Some(selected) if selected.handoff => Ok(Available::Ready),
        Some(selected) if selected.eligible < NORMAL_READY => {
            after(selected.at, STARVATION_SECONDS).map(Available::At)
        }
        Some(selected) => after(selected.at, SETTLE_SECONDS).map(Available::At),
    }
}

/// Wake the highest-precedence due trigger. Among lanes due for the same
/// trigger, a lane whose work is ordinary wakes before one whose work is its
/// own last resort, so an idle peer takes a review before its author does;
/// under best-match, lanes due for the same work trigger then rank by how well
/// each lane's retained work matches its selected task, a scored lane above an
/// unscored one; `order` then ranks lanes by their newest durable claim, and
/// lane id breaks exact ties. The same rank settles which lane a wait names.
fn resolve(
    targets: &[Target],
    backoff: &Backoff,
    now: Instant,
    order: lane::WakeOrder,
) -> Resolution {
    let mut candidates = Vec::new();
    for target in targets {
        let (trigger, available) = if target.pending_inbox {
            (Trigger::PendingInbox, Available::Ready)
        } else if target.held_claim != Available::None {
            (Trigger::HeldClaim, target.held_claim)
        } else if target.drain_queue != Available::None {
            (Trigger::DrainQueue, target.drain_queue)
        } else {
            (Trigger::AvailableWork, target.available)
        };
        let eligible_at = match available {
            Available::None => continue,
            Available::At(deadline) if deadline > now => deadline,
            Available::At(_) | Available::Ready => now,
        };
        let Some(due) = backoff.due(target.lane, trigger, eligible_at) else {
            continue;
        };
        candidates.push((trigger, target, due));
    }
    for trigger in [
        Trigger::PendingInbox,
        Trigger::HeldClaim,
        Trigger::DrainQueue,
        Trigger::AvailableWork,
    ] {
        if let Some((_, target, _)) = candidates
            .iter()
            .filter(|(candidate, _, due)| *candidate == trigger && *due <= now)
            .min_by(|(_, left, _), (_, right, _)| {
                wake_rank(order, (trigger, left), (trigger, right))
            })
        {
            return Resolution::Attempt {
                trigger,
                lane: target.lane,
            };
        }
    }
    candidates
        .into_iter()
        .min_by(
            |(left_trigger, left, left_due), (right_trigger, right, right_due)| {
                left_due
                    .cmp(right_due)
                    .then_with(|| wake_rank(order, (*left_trigger, left), (*right_trigger, right)))
                    .then_with(|| (*left_trigger as u8).cmp(&(*right_trigger as u8)))
            },
        )
        .map_or(Resolution::Idle, |(trigger, target, deadline)| {
            Resolution::Wait {
                trigger,
                lane: target.lane,
                deadline,
            }
        })
}

/// Which of two due lanes wakes first: ordinary work before a lane's own last
/// resort, then the higher match when both are due for the same work trigger
/// under best-match, then `order`'s claim recency, then lane id.
fn wake_rank(
    order: lane::WakeOrder,
    (left_trigger, left): (Trigger, &Target),
    (right_trigger, right): (Trigger, &Target),
) -> std::cmp::Ordering {
    let work = |trigger| matches!(trigger, Trigger::DrainQueue | Trigger::AvailableWork);
    let last_resort = |trigger, target: &Target| work(trigger) && target.last_resort;
    let matched = if order == lane::WakeOrder::BestMatch
        && left_trigger == right_trigger
        && work(left_trigger)
    {
        // Cosines are never negative, so an unscored lane ranks below all.
        let score = |target: &Target| target.matched.unwrap_or(f64::NEG_INFINITY);
        score(right).total_cmp(&score(left))
    } else {
        std::cmp::Ordering::Equal
    };
    let claims = match order {
        // `None` sorts first: a lane that never claimed is the oldest.
        lane::WakeOrder::LeastRecentlyClaimed => left.last_claim.cmp(&right.last_claim),
        // Reversed, `None` sorts last: a lane that never claimed is the coldest.
        lane::WakeOrder::BestMatch | lane::WakeOrder::MostRecentlyClaimed => {
            right.last_claim.cmp(&left.last_claim)
        }
    };
    last_resort(left_trigger, left)
        .cmp(&last_resort(right_trigger, right))
        .then(matched)
        .then(claims)
        .then_with(|| left.lane.cmp(&right.lane))
}

fn current_instant() -> CommandResult<Instant> {
    Instant::new(jiff::Timestamp::now())
        .map_err(|error| super::environment_fault("lifecycle::clock-invalid", &error.to_string()))
}
fn after(at: Instant, seconds: i64) -> CommandResult<Instant> {
    let at = at
        .get()
        .checked_add(jiff::SignedDuration::from_secs(seconds))
        .map_err(|error| {
            super::environment_fault("lifecycle::deadline-range", &error.to_string())
        })?;
    Instant::new(at).map_err(|error| {
        super::environment_fault("lifecycle::deadline-invalid", &error.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn lane(last: u8) -> CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
            .parse()
            .expect("fixture lane")
    }

    pub(super) fn instant(second: u8) -> Instant {
        format!("2026-08-01T07:00:{second:02}.000Z")
            .parse()
            .expect("fixture instant")
    }

    pub(super) const ROTATE: lane::WakeOrder = lane::WakeOrder::LeastRecentlyClaimed;

    pub(super) fn target(lane: u8) -> Target<'static> {
        Target {
            lane: super::tests::lane(lane),
            last_claim: None,
            pending_inbox: false,
            held_claim: Available::None,
            drain_queue: Available::None,
            available: Available::None,
            last_resort: false,
            task: None,
            matched: None,
        }
    }

    #[test]
    fn resolver_orders_inbox_then_held_claim_then_available_work() {
        let mut row = target(1);
        row.pending_inbox = true;
        row.held_claim = Available::Ready;
        row.available = Available::At(instant(30));
        assert_eq!(
            resolve(&[row], &Backoff::default(), instant(10), ROTATE),
            Resolution::Attempt {
                trigger: Trigger::PendingInbox,
                lane: lane(1),
            }
        );
        row.pending_inbox = false;
        assert_eq!(
            resolve(&[row], &Backoff::default(), instant(10), ROTATE),
            Resolution::Attempt {
                trigger: Trigger::HeldClaim,
                lane: lane(1),
            }
        );
    }

    #[test]
    fn resolver_orders_due_triggers_globally_across_lanes() {
        let mut available = target(1);
        available.available = Available::Ready;
        let mut held = target(2);
        held.held_claim = Available::Ready;
        let mut inbox = target(3);
        inbox.pending_inbox = true;
        assert_eq!(
            resolve(
                &[available, held, inbox],
                &Backoff::default(),
                instant(10),
                ROTATE
            ),
            Resolution::Attempt {
                trigger: Trigger::PendingInbox,
                lane: lane(3),
            }
        );
        inbox.pending_inbox = false;
        assert_eq!(
            resolve(
                &[available, held, inbox],
                &Backoff::default(),
                instant(10),
                ROTATE
            ),
            Resolution::Attempt {
                trigger: Trigger::HeldClaim,
                lane: lane(2),
            }
        );
    }

    #[test]
    fn durable_age_and_settle_deadlines_wait_for_their_real_event() {
        let mut row = target(1);
        row.available = Available::At(instant(12));
        assert_eq!(
            resolve(&[row], &Backoff::default(), instant(10), ROTATE),
            Resolution::Wait {
                trigger: Trigger::AvailableWork,
                lane: lane(1),
                deadline: instant(12),
            }
        );
        assert!(matches!(
            resolve(&[row], &Backoff::default(), instant(12), ROTATE),
            Resolution::Attempt { .. }
        ));

        let snapshot = ReconciliationWitness::new(
            lane(2),
            lane(3),
            Some(ReplyPosition::Snapshot(Seq::new(4).expect("snapshot"))),
        );
        assert!(!snapshot.reached_by(4) && snapshot.reached_by(5));
        let mut reconciliation = Reconciliation::default();
        reconciliation.request(snapshot);
        assert!(reconciliation.control_enabled() && reconciliation.control_settles());
        let committed = ReconciliationWitness::new(
            lane(2),
            lane(3),
            Some(ReplyPosition::Committed(wire::SeqRange {
                first: Seq::new(5).expect("commit start"),
                last: Seq::new(6).expect("commit end"),
            })),
        );
        assert!(!committed.reached_by(5) && committed.reached_by(6));
        reconciliation.request(committed);
        assert!(!reconciliation.control_enabled() && !reconciliation.control_settles());
        reconciliation.settle();
        assert!(reconciliation.control_enabled() && !reconciliation.control_settles());
    }

    #[test]
    fn selected_ready_task_settles_ages_or_hands_off_immediately() {
        assert_eq!(
            available_from_selected(None).expect("empty fold resolves"),
            Available::None
        );
        assert_eq!(
            available_from_selected(Some(SelectedReady {
                at: instant(0),
                eligible: 1,
                handoff: false,
                handoff_wait: false,
                last_resort: false,
            }))
            .expect("lone fold resolves"),
            Available::At(instant_after_minutes(3))
        );
        assert_eq!(
            available_from_selected(Some(SelectedReady {
                at: instant(0),
                eligible: 2,
                handoff: false,
                handoff_wait: false,
                last_resort: false,
            }))
            .expect("burst fold resolves"),
            Available::At(instant(3))
        );
        assert_eq!(
            available_from_selected(Some(SelectedReady {
                at: instant(0),
                eligible: 1,
                handoff: true,
                handoff_wait: false,
                last_resort: false,
            }))
            .expect("phase handoff resolves"),
            Available::Ready
        );
        assert_eq!(
            available_from_selected(Some(SelectedReady {
                at: instant(3),
                eligible: 0,
                handoff: false,
                handoff_wait: true,
                last_resort: false,
            }))
            .expect("blocked handoff deadline resolves"),
            Available::At(instant(3))
        );
    }

    #[test]
    fn drain_capacity_settles_scales_or_hands_off_immediately() {
        assert_eq!(
            drain_from_selected(None, true).expect("empty drain fold resolves"),
            Available::None
        );
        assert_eq!(
            drain_from_selected(
                Some(SelectedReady {
                    at: instant(0),
                    eligible: 1,
                    handoff: false,
                    handoff_wait: false,
                    last_resort: false,
                }),
                true,
            )
            .expect("first drain member resolves"),
            Available::At(instant(3))
        );
        assert_eq!(
            drain_from_selected(
                Some(SelectedReady {
                    at: instant(0),
                    eligible: 1,
                    handoff: false,
                    handoff_wait: false,
                    last_resort: false,
                }),
                false,
            )
            .expect("next drain member resolves"),
            Available::At(instant_after_minutes(3))
        );
        assert_eq!(
            drain_from_selected(
                Some(SelectedReady {
                    at: instant(0),
                    eligible: 2,
                    handoff: false,
                    handoff_wait: false,
                    last_resort: false,
                }),
                false,
            )
            .expect("drain backlog resolves"),
            Available::At(instant(3))
        );
        assert_eq!(
            drain_from_selected(
                Some(SelectedReady {
                    at: instant(0),
                    eligible: 1,
                    handoff: true,
                    handoff_wait: false,
                    last_resort: false,
                }),
                false,
            )
            .expect("drain phase handoff resolves"),
            Available::Ready
        );
        assert_eq!(
            drain_from_selected(
                Some(SelectedReady {
                    at: instant(3),
                    eligible: 0,
                    handoff: false,
                    handoff_wait: true,
                    last_resort: false,
                }),
                false,
            )
            .expect("blocked drain handoff deadline resolves"),
            Available::At(instant(3))
        );
    }

    #[test]
    fn pending_parks_permanent_start_refusals_but_retries_moved_authority() {
        let failure = |reason: &str| {
            Outcome::Refusal(wire::Refusal::bare(
                wire::RefusalDiagnostic::new(reason.to_owned(), None, None, None)
                    .with_detail("fixture diagnostic"),
            ))
        };
        assert!(pending_start_failure_is_terminal(&failure(
            "lane::session-authority-missing"
        )));
        assert!(pending_start_failure_is_terminal(&failure(
            "git::claim-loose-commits"
        )));
        for reason in [
            "lifecycle::wake-capacity-moved",
            "lifecycle::wake-inbox-moved",
            "lifecycle::held-claim-head-moved",
            "lifecycle::held-claim-lease-moved",
        ] {
            assert!(!pending_start_failure_is_terminal(&failure(reason)));
        }
    }

    #[test]
    fn permanent_refusal_has_no_timer_while_authority_movement_retries() {
        let selected = lane(1);
        let mut parked = Backoff {
            timed: BTreeMap::from([(selected, instant(30))]),
            ..Backoff::default()
        };
        let mut diagnostic = Vec::new();
        assert_eq!(
            parked
                .refused(
                    Trigger::AvailableWork,
                    selected,
                    "git::claim-loose-commits",
                    instant(10),
                    &mut diagnostic,
                )
                .expect("permanent refusal parks"),
            None,
        );
        assert!(!parked.timed.contains_key(&selected));
        assert_eq!(parked.parked.get(&selected), Some(&Trigger::AvailableWork));
        assert!(
            String::from_utf8(diagnostic)
                .expect("diagnostic is UTF-8")
                .contains("lifecycle parked trigger=available-work")
        );

        let mut retrying = Backoff::default();
        let wake = retrying
            .refused(
                Trigger::HeldClaim,
                selected,
                "lifecycle::wake-inbox-moved",
                instant(10),
                &mut Vec::new(),
            )
            .expect("movement refusal retries");
        assert_eq!(
            wake,
            Some(WakeDeadline {
                lane: selected,
                at: instant(15),
                kind: WakeKind::Task,
            })
        );
        assert_eq!(retrying.timed.get(&selected), Some(&instant(15)));
        assert!(!retrying.parked.contains_key(&selected));
    }

    #[test]
    fn cheap_declines_do_not_spend_the_shared_retry_bucket() {
        let row = target(1);
        let backoff = Backoff {
            timed: BTreeMap::from([(lane(1), instant(30))]),
            ..Backoff::default()
        };
        assert_eq!(
            resolve(&[row], &backoff, instant(10), ROTATE),
            Resolution::Idle
        );
    }

    #[test]
    fn every_attempted_arm_inherits_the_one_retry_deadline() {
        for trigger in [
            Trigger::PendingInbox,
            Trigger::HeldClaim,
            Trigger::DrainQueue,
            Trigger::AvailableWork,
        ] {
            let mut row = target(1);
            row.pending_inbox = trigger == Trigger::PendingInbox;
            row.held_claim = if trigger == Trigger::HeldClaim {
                Available::Ready
            } else {
                Available::None
            };
            row.drain_queue = if trigger == Trigger::DrainQueue {
                Available::Ready
            } else {
                Available::None
            };
            row.available = if trigger == Trigger::AvailableWork {
                Available::Ready
            } else {
                Available::None
            };
            let backoff = Backoff {
                timed: BTreeMap::from([(lane(1), instant(30))]),
                ..Backoff::default()
            };
            assert_eq!(
                resolve(&[row], &backoff, instant(10), ROTATE),
                Resolution::Wait {
                    trigger,
                    lane: lane(1),
                    deadline: instant(30),
                }
            );
        }
    }

    #[test]
    fn sibling_target_can_run_while_another_waits() {
        let mut waiting = target(1);
        waiting.available = Available::At(instant(30));
        let mut ready = target(2);
        ready.available = Available::Ready;
        assert_eq!(
            resolve(&[waiting, ready], &Backoff::default(), instant(10), ROTATE),
            Resolution::Attempt {
                trigger: Trigger::AvailableWork,
                lane: lane(2),
            }
        );
    }

    #[test]
    fn one_lanes_retry_does_not_block_a_ready_sibling() {
        let mut retrying = target(1);
        retrying.pending_inbox = true;
        let mut sibling = target(2);
        sibling.available = Available::Ready;
        let backoff = Backoff {
            timed: BTreeMap::from([(lane(1), instant(30))]),
            ..Backoff::default()
        };
        assert_eq!(
            resolve(&[retrying, sibling], &backoff, instant(10), ROTATE),
            Resolution::Attempt {
                trigger: Trigger::AvailableWork,
                lane: lane(2),
            }
        );
    }

    #[test]
    fn permanent_refusal_parks_only_the_trigger_that_failed() {
        let mut row = target(1);
        row.available = Available::Ready;
        let backoff = Backoff {
            parked: BTreeMap::from([(lane(1), Trigger::AvailableWork)]),
            ..Backoff::default()
        };
        assert_eq!(
            resolve(&[row], &backoff, instant(10), ROTATE),
            Resolution::Idle,
        );

        row.held_claim = Available::Ready;
        assert_eq!(
            resolve(&[row], &backoff, instant(10), ROTATE),
            Resolution::Attempt {
                trigger: Trigger::HeldClaim,
                lane: lane(1),
            },
        );
    }

    #[test]
    fn same_lane_command_releases_timed_and_parked_backoff() {
        let selected = lane(1);
        let sibling = lane(2);
        let mut runtime = Runtime::new(&lane::Config::default());
        runtime.backoff.timed.insert(selected, instant(30));
        runtime.backoff.timed.insert(sibling, instant(30));
        runtime
            .backoff
            .parked
            .insert(selected, Trigger::AvailableWork);
        runtime.backoff.parked.insert(sibling, Trigger::HeldClaim);

        assert!(runtime.observe_command(&Actor::Lane(selected)));

        assert!(!runtime.backoff.timed.contains_key(&selected));
        assert!(!runtime.backoff.parked.contains_key(&selected));
        assert_eq!(runtime.backoff.timed.get(&sibling), Some(&instant(30)));
        assert_eq!(
            runtime.backoff.parked.get(&sibling),
            Some(&Trigger::HeldClaim)
        );
    }

    #[test]
    fn backoff_drops_lanes_absent_from_the_current_projection() {
        let retained = lane(1);
        let removed = lane(2);
        let mut backoff = Backoff {
            timed: BTreeMap::from([(retained, instant(30)), (removed, instant(30))]),
            parked: BTreeMap::from([
                (retained, Trigger::AvailableWork),
                (removed, Trigger::HeldClaim),
            ]),
        };

        backoff.retain_targets(&[target(1)]);

        assert_eq!(backoff.timed.len(), 1);
        assert_eq!(backoff.parked.len(), 1);
        assert!(backoff.timed.contains_key(&retained));
        assert!(backoff.parked.contains_key(&retained));
    }

    fn instant_after_minutes(minutes: u8) -> Instant {
        format!("2026-08-01T07:{minutes:02}:00.000Z")
            .parse()
            .expect("fixture instant")
    }
}

#[cfg(test)]
mod materialization_tests;
#[cfg(test)]
mod wake_order_tests;
