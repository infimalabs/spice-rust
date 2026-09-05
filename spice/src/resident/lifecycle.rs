//! One resident-owned automatic lifecycle decision authority.
//!
//! Durable commit notifications and explicit timer completions call
//! [`Runtime::reconcile`].  The resolver itself reads no clock: `now` is an
//! input, READY ages are projected from the fact that established each current
//! workflow head, and all future work is returned as one UTC deadline for the
//! resident timer worker.

use std::collections::BTreeMap;

use board::Entry;
use bus::Bus;
use journal::FactCause;
use lane::hold::{Death, Holds};
use lane::occupant::Seats;
use spice::repository::Repository;
use wire::{Actor, CanonicalUuid, ClaimDisposition, Instant, Outcome, Producer, Seq, TaskStamp};

use super::lane_surface;
use super::shutdown::{current_residency, guard_residency};
use super::{CommandResult, RuntimePaths, SurfaceConfig, commit, new_uuid, store_fault, write};

mod materialization;
use materialization::Projection;
#[cfg(test)]
mod startup_tests;

const NORMAL_READY: usize = 2;
const STARVATION_SECONDS: i64 = 180;
const SETTLE_SECONDS: i64 = 3;
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
struct Target {
    lane: CanonicalUuid,
    pending_inbox: bool,
    held_claim: bool,
    drain_queue: Available,
    available: Available,
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
    DeadLettered(Vec<super::steering::DeadLetter>),
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
    projection: Projection,
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

impl Runtime {
    pub(super) fn new(config: &lane::Config) -> Self {
        Self {
            backoff: Backoff::default(),
            holds: Holds::new(config.deaths()),
            projection: Projection::new(),
        }
    }

    /// Reconcile one authoritative snapshot and execute at most one wake.
    #[allow(
        clippy::too_many_arguments,
        reason = "the lifecycle authority composes every existing resident fold explicitly"
    )]
    pub(super) fn reconcile(
        &mut self,
        bus: &Bus,
        repository: &Repository,
        paths: &RuntimePaths,
        epoch: CanonicalUuid,
        seats: &mut Seats,
        lane_runtime: &lane_surface::Runtime<'_>,
        config: &SurfaceConfig,
        diagnostic: &mut dyn std::io::Write,
    ) -> CommandResult<Option<WakeDeadline>> {
        let now = Instant::new(jiff::Timestamp::now()).map_err(|error| {
            super::environment_fault("lifecycle::clock-invalid", &error.to_string())
        })?;
        let snapshot = bus
            .task_snapshot()
            .map_err(|error| store_fault("lifecycle::snapshot-unreadable", &error.to_string()))?;
        self.synchronize_holds(&snapshot, self.projection.cursor())?;
        self.projection.advance(&snapshot, &config.projects)?;
        let targets = project_targets(
            &snapshot,
            &self.projection,
            now,
            &self.holds,
            &config.allocation,
        )?;
        self.backoff.retain_targets(&targets);
        let task = match resolve(&targets, &self.backoff, now) {
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
                let result = self.attempt(
                    trigger,
                    lane,
                    bus,
                    repository,
                    paths,
                    epoch,
                    seats,
                    lane_runtime,
                    config,
                    &snapshot,
                );
                match result {
                    Ok(Attempted::Started) => {
                        self.backoff.release(lane);
                        let _ = writeln!(
                            diagnostic,
                            "lifecycle started trigger={} lane={}",
                            trigger.as_str(),
                            lane
                        );
                        None
                    }
                    Ok(Attempted::DeadLettered(parked)) => {
                        self.backoff.release(lane);
                        for directive in parked {
                            let _ = writeln!(
                                diagnostic,
                                "lifecycle dead-lettered trigger={} lane={} key={} directive={} requeue={}",
                                trigger.as_str(),
                                lane,
                                directive.key,
                                directive.directive,
                                directive.requeue,
                            );
                        }
                        None
                    }
                    Err(Outcome::Refusal(refusal)) => {
                        self.backoff
                            .refused(trigger, lane, refusal.reason(), now, diagnostic)?
                    }
                    Err(Outcome::Fault(fault)) => return Err(Outcome::Fault(fault)),
                    Err(Outcome::Reply(_)) => {
                        unreachable!("unit lifecycle failures carry no reply")
                    }
                }
            }
        };
        let hold = self.holds.next().map(|(lane, at)| WakeDeadline {
            lane,
            at,
            kind: WakeKind::Hold,
        });
        Ok(earliest(task, hold))
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
            wire::intent::Intent::LineageConfigure(_)
                | wire::intent::Intent::LineageArrange(_)
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

    /// Classify one observed occupant death through the shared hold reducer.
    pub(super) fn observe_death(
        &mut self,
        bus: &Bus,
        epoch: CanonicalUuid,
        seats: &Seats,
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
        config: &lane::Config,
    ) -> CommandResult<()> {
        let Some(activity) = seats
            .started(lane)
            .filter(|activity| activity.occupant() == occupant)
        else {
            return Ok(());
        };
        let death = if activity.at().elapsed() < config.young().get() {
            Death::Young
        } else {
            Death::Mature
        };
        let now = Instant::new(jiff::Timestamp::now()).map_err(|error| {
            super::environment_fault("lifecycle::clock-invalid", &error.to_string())
        })?;
        let seconds = i64::try_from(config.hold().get().as_secs()).map_err(|_| {
            super::environment_fault(
                "lifecycle::hold-range",
                "the configured lane hold does not fit a signed deadline",
            )
        })?;
        let until = after(now, seconds)?;
        let Some(held) = self.holds.died(lane, death, until) else {
            return Ok(());
        };
        let observed = current_residency(bus, epoch)?;
        let mut proposal = write(Actor::Spice, epoch, Producer::BusTimer)?;
        guard_residency(&mut proposal, &observed);
        proposal
            .fact(held, FactCause::Existing(observed.position()))
            .map_err(|error| store_fault("lifecycle::hold-fact-invalid", &error.to_string()))?;
        match commit(bus, proposal) {
            Ok(_) => Ok(()),
            Err(outcome) => {
                let _ = self.holds.wake(lane);
                Err(outcome)
            }
        }
    }

    /// Release one hold from its explicit timer event.
    pub(super) fn expire_hold(
        &mut self,
        bus: &Bus,
        epoch: CanonicalUuid,
        lane: CanonicalUuid,
    ) -> CommandResult<()> {
        let now = Instant::new(jiff::Timestamp::now()).map_err(|error| {
            super::environment_fault("lifecycle::clock-invalid", &error.to_string())
        })?;
        let until = self.holds.held(lane);
        let resumed = self.holds.expire(lane, now);
        self.commit_resume(bus, epoch, lane, until, resumed)
    }

    /// Release one hold because an operator explicitly requested wake.
    pub(super) fn operator_wake(
        &mut self,
        bus: &Bus,
        epoch: CanonicalUuid,
        lane: CanonicalUuid,
    ) -> CommandResult<()> {
        self.backoff.release(lane);
        let until = self.holds.held(lane);
        let resumed = self.holds.wake(lane);
        self.commit_resume(bus, epoch, lane, until, resumed)
    }

    fn commit_resume(
        &mut self,
        bus: &Bus,
        epoch: CanonicalUuid,
        lane: CanonicalUuid,
        until: Option<Instant>,
        resumed: Option<wire::body::LaneResumed>,
    ) -> CommandResult<()> {
        let Some(resumed) = resumed else {
            return Ok(());
        };
        let observed = current_residency(bus, epoch)?;
        let mut proposal = write(Actor::Spice, epoch, Producer::BusTimer)?;
        guard_residency(&mut proposal, &observed);
        proposal
            .fact(resumed, FactCause::Existing(observed.position()))
            .map_err(|error| store_fault("lifecycle::resume-fact-invalid", &error.to_string()))?;
        match commit(bus, proposal) {
            Ok(_) => Ok(()),
            Err(outcome) => {
                if let Some(until) = until {
                    self.holds.absorb_held(&wire::body::LaneHeld {
                        lane,
                        evidence: "durable hold restored after refused release".to_owned(),
                        until,
                    });
                }
                Err(outcome)
            }
        }
    }

    fn synchronize_holds(
        &mut self,
        snapshot: &bus::TaskSnapshot,
        cursor: Option<Seq>,
    ) -> CommandResult<()> {
        for envelope in snapshot.facts().after(cursor) {
            match envelope.fact.kind() {
                wire::FactKind::LaneHeld => {
                    let held = fact_body::<wire::body::LaneHeld>(envelope)?;
                    self.holds.absorb_held(&held);
                }
                wire::FactKind::LaneResumed => {
                    let resumed = fact_body::<wire::body::LaneResumed>(envelope)?;
                    self.holds.absorb_resumed(&resumed);
                }
                _ => {}
            }
        }
        Ok(())
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "one lifecycle attempt owns the complete resident effect boundary"
    )]
    fn attempt(
        &mut self,
        trigger: Trigger,
        lane: CanonicalUuid,
        bus: &Bus,
        repository: &Repository,
        paths: &RuntimePaths,
        epoch: CanonicalUuid,
        seats: &mut Seats,
        lane_runtime: &lane_surface::Runtime<'_>,
        config: &SurfaceConfig,
        snapshot: &bus::TaskSnapshot,
    ) -> CommandResult<Attempted> {
        let cursor = snapshot
            .cursor()
            .expect("reconciliation admitted one nonempty task snapshot");
        let occupant = new_uuid();
        match trigger {
            Trigger::PendingInbox => {
                let wake = lane_surface::wake_automatic(
                    bus,
                    paths,
                    epoch,
                    seats,
                    lane_runtime,
                    lane,
                    occupant,
                    Some(snapshot),
                    None,
                );
                match wake {
                    Ok(lane_surface::AutomaticWake::Started) => Ok(Attempted::Started),
                    Ok(lane_surface::AutomaticWake::Unstarted(failure)) => {
                        super::steering::dead_letter_pending(
                            bus, paths, epoch, lane, cursor, &failure,
                        )
                        .map(Attempted::DeadLettered)
                    }
                    Err(failure) if pending_start_failure_is_terminal(&failure) => {
                        super::steering::dead_letter_pending(
                            bus, paths, epoch, lane, cursor, &failure,
                        )
                        .map(Attempted::DeadLettered)
                    }
                    Err(failure) => Err(failure),
                }
            }
            Trigger::HeldClaim => match lane_surface::wake_automatic(
                bus,
                paths,
                epoch,
                seats,
                lane_runtime,
                lane,
                occupant,
                Some(snapshot),
                None,
            )? {
                lane_surface::AutomaticWake::Started => Ok(Attempted::Started),
                lane_surface::AutomaticWake::Unstarted(failure) => Err(failure),
            },
            Trigger::DrainQueue | Trigger::AvailableWork => {
                let claim = crate::board_command::claim_automatic(
                    bus,
                    snapshot,
                    repository,
                    epoch,
                    lane,
                    occupant,
                    &config.projects,
                    &config.allocation,
                )
                .map_err(unit_outcome)?;
                let launched = lane_surface::wake_automatic(
                    bus,
                    paths,
                    epoch,
                    seats,
                    lane_runtime,
                    lane,
                    occupant,
                    None,
                    Some(&claim),
                );
                match launched {
                    Ok(lane_surface::AutomaticWake::Started) => Ok(Attempted::Started),
                    Ok(lane_surface::AutomaticWake::Unstarted(launch)) | Err(launch) => {
                        match crate::board_command::release_automatic(bus, &claim) {
                            Ok(()) => Err(launch),
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
    "lifecycle::snapshot-moved",
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

fn project_targets(
    snapshot: &bus::TaskSnapshot,
    projection: &Projection,
    now: Instant,
    holds: &Holds,
    allocation: &board::NextConfig,
) -> CommandResult<Vec<Target>> {
    let Some(residency) = snapshot.residency() else {
        return Ok(Vec::new());
    };
    if !residency.body().lanes.iter().any(|row| {
        row.occupant.is_none()
            && row.disposition != "occupied"
            && row.life != "drain"
            && holds.seats(row.lane)
    }) {
        return Ok(Vec::new());
    }
    let entries = projection.entries();
    let projected_residency = projection
        .project_residency(residency.body())
        .map_err(|error| store_fault("lifecycle::subscription-history-invalid", &error))?;
    let ready_at = projection.ready_at();
    let mut rows = projected_residency.lanes.iter().collect::<Vec<_>>();
    rows.sort_by_key(|row| row.lane);
    let mut targets = Vec::new();
    for row in rows {
        if row.occupant.is_some()
            || row.disposition == "occupied"
            || row.life == "drain"
            || !holds.seats(row.lane)
        {
            continue;
        }
        let pending_inbox = projection.has_operator_steering(row.lane);
        let mut held = entries.iter().filter(|entry| {
            entry.claim().is_some_and(|claim| {
                claim.disposition == ClaimDisposition::Active && claim.lane == row.lane
            })
        });
        let held_claim = match held.next() {
            Some(entry) => {
                if held.next().is_some() {
                    return Err(store_fault(
                        "lifecycle::held-claim-ambiguous",
                        "one vacant lane carries more than one active claim",
                    ));
                }
                entry
                    .lease()
                    .ok_or_else(|| {
                        store_fault(
                            "lifecycle::held-claim-lease-missing",
                            "an active held claim has no durable lease head",
                        )
                    })?
                    .expiry
                    > now
            }
            None => false,
        };
        let mut subscriptions = projected_residency
            .subscriptions
            .iter()
            .filter(|subscription| subscription.lineage_id == row.lineage_id);
        let subscription = subscriptions.next();
        if subscriptions.next().is_some() {
            return Err(store_fault(
                "lifecycle::subscription-ambiguous",
                "one lifecycle target has more than one durable task subscription",
            ));
        }
        let actor = Actor::Lane(row.lane);
        let first_drain_member = !projected_residency.lanes.iter().any(|member| {
            member.lineage_id == row.lineage_id
                && member.occupant.is_some()
                && member.disposition == "occupied"
        });
        let (drain_queue, available) = match subscription {
            Some(subscription) if subscription.policy == wire::intent::LineageLife::Drain => (
                drain_available(
                    snapshot,
                    entries,
                    ready_at,
                    &actor,
                    subscription,
                    allocation,
                    first_drain_member,
                )?,
                Available::None,
            ),
            Some(subscription) if subscription.policy == wire::intent::LineageLife::Drive => (
                Available::None,
                available_deadline(entries, ready_at, &actor, subscription)?,
            ),
            Some(_) | None => (Available::None, Available::None),
        };
        targets.push(Target {
            lane: row.lane,
            pending_inbox,
            held_claim,
            drain_queue,
            available,
        });
    }
    Ok(targets)
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

fn available_deadline(
    entries: &[Entry],
    ready_at: &BTreeMap<TaskStamp, Instant>,
    actor: &Actor,
    subscription: &wire::LineageSubscription,
) -> CommandResult<Available> {
    let mut eligible = entries
        .iter()
        .filter(|entry| board::selectable_entry(entries, entry, actor, subscription))
        .filter_map(|entry| ready_at.get(entry.born()).copied())
        .collect::<Vec<_>>();
    available_from_ready_instants(&mut eligible)
}

fn drain_available(
    snapshot: &bus::TaskSnapshot,
    entries: &[Entry],
    ready_at: &BTreeMap<TaskStamp, Instant>,
    actor: &Actor,
    subscription: &wire::LineageSubscription,
    allocation: &board::NextConfig,
    first_member: bool,
) -> CommandResult<Available> {
    let recent_cell = board::recent_claim_cell(entries, snapshot.facts().iter(), actor);
    if board::select(
        entries,
        actor,
        subscription,
        allocation.band(),
        recent_cell.as_ref(),
    )
    .is_none()
    {
        return Ok(Available::None);
    }

    let mut eligible = entries
        .iter()
        .filter(|entry| board::selectable_entry(entries, entry, actor, subscription))
        .filter_map(|entry| ready_at.get(entry.born()).copied())
        .collect::<Vec<_>>();
    drain_from_ready_instants(&mut eligible, first_member)
}

fn drain_from_ready_instants(
    eligible: &mut [Instant],
    first_member: bool,
) -> CommandResult<Available> {
    eligible.sort();
    if eligible.is_empty() {
        Ok(Available::None)
    } else if first_member || eligible.len() >= NORMAL_READY {
        Ok(Available::Ready)
    } else {
        after(eligible[0], STARVATION_SECONDS).map(Available::At)
    }
}

fn available_from_ready_instants(eligible: &mut [Instant]) -> CommandResult<Available> {
    eligible.sort();
    if eligible.is_empty() {
        Ok(Available::None)
    } else if eligible.len() < NORMAL_READY {
        after(eligible[0], STARVATION_SECONDS).map(Available::At)
    } else {
        after(
            *eligible.last().expect("nonempty eligible set"),
            SETTLE_SECONDS,
        )
        .map(Available::At)
    }
}

fn resolve(targets: &[Target], backoff: &Backoff, now: Instant) -> Resolution {
    let mut candidates = Vec::new();
    for target in targets {
        let (trigger, available) = if target.pending_inbox {
            (Trigger::PendingInbox, Available::Ready)
        } else if target.held_claim {
            (Trigger::HeldClaim, Available::Ready)
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
        candidates.push((trigger, target.lane, due));
    }
    for trigger in [
        Trigger::PendingInbox,
        Trigger::HeldClaim,
        Trigger::DrainQueue,
        Trigger::AvailableWork,
    ] {
        if let Some((_, lane, _)) = candidates
            .iter()
            .find(|(candidate, _, due)| *candidate == trigger && *due <= now)
        {
            return Resolution::Attempt {
                trigger,
                lane: *lane,
            };
        }
    }
    candidates
        .into_iter()
        .min_by_key(|(trigger, lane, deadline)| (*deadline, *lane, *trigger as u8))
        .map_or(Resolution::Idle, |(trigger, lane, deadline)| {
            Resolution::Wait {
                trigger,
                lane,
                deadline,
            }
        })
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

    fn lane(last: u8) -> CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
            .parse()
            .expect("fixture lane")
    }

    fn instant(second: u8) -> Instant {
        format!("2026-08-01T07:00:{second:02}.000Z")
            .parse()
            .expect("fixture instant")
    }

    fn target(lane: u8) -> Target {
        Target {
            lane: super::tests::lane(lane),
            pending_inbox: false,
            held_claim: false,
            drain_queue: Available::None,
            available: Available::None,
        }
    }

    #[test]
    fn resolver_orders_inbox_then_held_claim_then_available_work() {
        let mut row = target(1);
        row.pending_inbox = true;
        row.held_claim = true;
        row.available = Available::Ready;
        assert_eq!(
            resolve(&[row], &Backoff::default(), instant(10)),
            Resolution::Attempt {
                trigger: Trigger::PendingInbox,
                lane: lane(1),
            }
        );
        row.pending_inbox = false;
        assert_eq!(
            resolve(&[row], &Backoff::default(), instant(10)),
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
        held.held_claim = true;
        let mut inbox = target(3);
        inbox.pending_inbox = true;
        assert_eq!(
            resolve(&[available, held, inbox], &Backoff::default(), instant(10),),
            Resolution::Attempt {
                trigger: Trigger::PendingInbox,
                lane: lane(3),
            }
        );
        inbox.pending_inbox = false;
        assert_eq!(
            resolve(&[available, held, inbox], &Backoff::default(), instant(10),),
            Resolution::Attempt {
                trigger: Trigger::HeldClaim,
                lane: lane(2),
            }
        );
    }

    #[test]
    fn one_resolution_starts_exactly_one_lane() {
        let mut first = target(1);
        first.available = Available::Ready;
        let mut second = target(2);
        second.available = Available::Ready;
        assert_eq!(
            resolve(&[second, first], &Backoff::default(), instant(10),),
            Resolution::Attempt {
                trigger: Trigger::AvailableWork,
                lane: lane(2),
            }
        );
    }

    #[test]
    fn durable_age_and_settle_deadlines_wait_for_their_real_event() {
        let mut row = target(1);
        row.available = Available::At(instant(12));
        assert_eq!(
            resolve(&[row], &Backoff::default(), instant(10)),
            Resolution::Wait {
                trigger: Trigger::AvailableWork,
                lane: lane(1),
                deadline: instant(12),
            }
        );
        assert!(matches!(
            resolve(&[row], &Backoff::default(), instant(12)),
            Resolution::Attempt { .. }
        ));
    }

    #[test]
    fn two_ready_tasks_settle_and_one_ready_task_ages_to_starvation() {
        let mut none = Vec::new();
        assert_eq!(
            available_from_ready_instants(&mut none).expect("empty fold resolves"),
            Available::None
        );
        let mut lone = vec![instant(0)];
        assert_eq!(
            available_from_ready_instants(&mut lone).expect("lone fold resolves"),
            Available::At(instant_after_minutes(3))
        );
        let mut burst = vec![instant(0), instant(7)];
        assert_eq!(
            available_from_ready_instants(&mut burst).expect("burst fold resolves"),
            Available::At(instant(10))
        );
    }

    #[test]
    fn drain_capacity_starts_first_member_then_scales_for_age_or_two_tasks() {
        let mut none = Vec::new();
        assert_eq!(
            drain_from_ready_instants(&mut none, true).expect("empty drain fold resolves"),
            Available::None
        );
        let mut first = vec![instant(0)];
        assert_eq!(
            drain_from_ready_instants(&mut first, true).expect("first drain member resolves"),
            Available::Ready
        );
        let mut next = vec![instant(0)];
        assert_eq!(
            drain_from_ready_instants(&mut next, false).expect("next drain member resolves"),
            Available::At(instant_after_minutes(3))
        );
        let mut backlog = vec![instant(0), instant(7)];
        assert_eq!(
            drain_from_ready_instants(&mut backlog, false).expect("drain backlog resolves"),
            Available::Ready
        );
    }

    #[test]
    fn every_refusal_trigger_has_one_closed_label() {
        assert_eq!(Trigger::PendingInbox.as_str(), "pending-inbox");
        assert_eq!(Trigger::HeldClaim.as_str(), "held-claim");
        assert_eq!(Trigger::DrainQueue.as_str(), "drain-queue");
        assert_eq!(Trigger::AvailableWork.as_str(), "available-work");
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
            "lifecycle::snapshot-moved",
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
                "lifecycle::snapshot-moved",
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
        assert_eq!(resolve(&[row], &backoff, instant(10)), Resolution::Idle);
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
            row.held_claim = trigger == Trigger::HeldClaim;
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
                resolve(&[row], &backoff, instant(10)),
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
            resolve(&[waiting, ready], &Backoff::default(), instant(10),),
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
            resolve(&[retrying, sibling], &backoff, instant(10)),
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
        assert_eq!(resolve(&[row], &backoff, instant(10)), Resolution::Idle,);

        row.held_claim = true;
        assert_eq!(
            resolve(&[row], &backoff, instant(10)),
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
