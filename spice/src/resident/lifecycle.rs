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
use wire::{Actor, CanonicalUuid, ClaimDisposition, Instant, Outcome, Producer, Seq, TaskHandle};

use super::lane_surface;
use super::shutdown::{current_residency, guard_residency};
use super::{CommandResult, RuntimePaths, SurfaceConfig, commit, new_uuid, store_fault, write};

const NORMAL_READY: usize = 2;
const STARVATION_SECONDS: i64 = 180;
const SETTLE_SECONDS: i64 = 3;
const RETRY_SECONDS: i64 = 5;

/// Closed automatic wake arms, in resolver order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Trigger {
    PendingInbox,
    HeldClaim,
    AvailableWork,
}

impl Trigger {
    const fn as_str(self) -> &'static str {
        match self {
            Self::PendingInbox => "pending-inbox",
            Self::HeldClaim => "held-claim",
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

/// Process-local retry state; durable eligibility remains in the bus.
pub(super) struct Runtime {
    retry: BTreeMap<CanonicalUuid, Instant>,
    holds: Holds,
    hold_cursor: Option<Seq>,
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
            retry: BTreeMap::new(),
            holds: Holds::new(config.deaths()),
            hold_cursor: None,
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
        self.synchronize_holds(&snapshot)?;
        snapshot.cursor().ok_or_else(|| {
            store_fault(
                "lifecycle::snapshot-empty",
                "a running resident has no durable lifecycle snapshot cursor",
            )
        })?;
        let targets = project_targets(&snapshot, &config.projects, now, &self.holds)?;
        let task = match resolve(&targets, &self.retry, now) {
            Resolution::Idle => None,
            Resolution::Wait {
                trigger,
                lane,
                deadline,
            } => {
                let _ = writeln!(
                    diagnostic,
                    "lifecycle waiting trigger={} lane={} deadline={}",
                    trigger.as_str(),
                    lane,
                    deadline
                );
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
                        self.retry.remove(&lane);
                        let _ = writeln!(
                            diagnostic,
                            "lifecycle started trigger={} lane={}",
                            trigger.as_str(),
                            lane
                        );
                        None
                    }
                    Ok(Attempted::DeadLettered(parked)) => {
                        self.retry.remove(&lane);
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
                        let deadline = after(now, RETRY_SECONDS)?;
                        self.retry.insert(lane, deadline);
                        let _ = writeln!(
                            diagnostic,
                            "lifecycle refused trigger={} lane={} reason={} retry={}",
                            trigger.as_str(),
                            lane,
                            refusal.reason(),
                            deadline
                        );
                        Some(WakeDeadline {
                            lane,
                            at: deadline,
                            kind: WakeKind::Task,
                        })
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

    fn synchronize_holds(&mut self, snapshot: &bus::TaskSnapshot) -> CommandResult<()> {
        for envelope in snapshot
            .facts()
            .facts()
            .iter()
            .filter(|envelope| self.hold_cursor.is_none_or(|cursor| envelope.seq > cursor))
        {
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
        self.hold_cursor = snapshot.cursor();
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
            Trigger::AvailableWork => {
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

fn pending_start_failure_is_terminal(failure: &Outcome<()>) -> bool {
    matches!(
        failure,
        Outcome::Refusal(refusal)
            if !matches!(
                refusal.reason(),
                "lifecycle::wake-capacity-moved"
                    | "lifecycle::snapshot-moved"
                    | "lifecycle::held-claim-head-moved"
                    | "lifecycle::held-claim-lease-moved"
            )
    )
}

fn project_targets(
    snapshot: &bus::TaskSnapshot,
    projects: &board::ProjectPolicy,
    now: Instant,
    holds: &Holds,
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
    let entries = snapshot
        .tasks()
        .iter()
        .map(|heads| {
            Entry::new(
                heads.statement().body().clone(),
                heads.workflow().body().clone(),
                heads.claim().map(|head| head.body().clone()),
                heads.lease().map(|head| head.body().clone()),
                projects,
            )
            .map_err(|error| {
                store_fault(
                    "lifecycle::task-head-invalid",
                    &format!(
                        "task {} current heads are invalid: {error}",
                        heads.statement().body().handle
                    ),
                )
            })
        })
        .collect::<CommandResult<Vec<_>>>()?;
    let mut projected_residency = residency.body().clone();
    board::project_subscription_history(
        &mut projected_residency,
        &entries,
        snapshot.facts().facts(),
    )
    .map_err(|error| store_fault("lifecycle::subscription-history-invalid", &error))?;
    let ledger = steer::Ledger::from_snapshot(snapshot.facts())
        .map_err(|error| store_fault("lifecycle::steering-invalid", &error.to_string()))?;
    let ready_at = ready_instants(snapshot, &entries)?;
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
        let pending_inbox = !ledger.pending(row.lane).is_empty();
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
        let available = match subscription {
            Some(subscription) => {
                available_deadline(&entries, &ready_at, &Actor::Lane(row.lane), subscription)?
            }
            None => Available::None,
        };
        targets.push(Target {
            lane: row.lane,
            pending_inbox,
            held_claim,
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

fn ready_instants(
    snapshot: &bus::TaskSnapshot,
    entries: &[Entry],
) -> CommandResult<BTreeMap<TaskHandle, Instant>> {
    let heads = snapshot
        .tasks()
        .iter()
        .map(|heads| (heads.statement().body().handle.clone(), heads))
        .collect::<BTreeMap<_, _>>();
    let facts = snapshot
        .facts()
        .facts()
        .iter()
        .map(|fact| (fact.seq, fact.at))
        .collect::<BTreeMap<_, _>>();
    entries
        .iter()
        .map(|entry| {
            let task = &entry.statement().handle;
            let task_heads = heads.get(task).ok_or_else(|| {
                store_fault(
                    "lifecycle::task-head-missing",
                    &format!("validated task {task} has no storage heads"),
                )
            })?;
            let mut transition = task_heads.workflow().position();
            if entry
                .claim()
                .is_some_and(|claim| claim.disposition != ClaimDisposition::Active)
                && let Some(claim) = task_heads.claim()
            {
                transition = transition.max(claim.position());
            }
            for dependency in &entry.statement().dependencies {
                let dependency = heads.get(dependency).ok_or_else(|| {
                    store_fault(
                        "lifecycle::dependency-head-missing",
                        &format!("task {task} dependency {dependency} has no storage heads"),
                    )
                })?;
                transition = transition.max(dependency.workflow().position());
            }
            let at = facts.get(&transition).copied().ok_or_else(|| {
                store_fault(
                    "lifecycle::ready-fact-missing",
                    &format!(
                        "task {task} READY transition head {transition} has no fact at the snapshot barrier"
                    ),
                )
            })?;
            Ok((task.clone(), at))
        })
        .collect()
}

fn available_deadline(
    entries: &[Entry],
    ready_at: &BTreeMap<TaskHandle, Instant>,
    actor: &Actor,
    subscription: &wire::LineageSubscription,
) -> CommandResult<Available> {
    if subscription.policy != wire::intent::LineageLife::Drain {
        return Ok(Available::None);
    }
    let mut eligible = entries
        .iter()
        .filter(|entry| board::selectable_entry(entries, entry, actor, subscription))
        .filter_map(|entry| ready_at.get(&entry.statement().handle).copied())
        .collect::<Vec<_>>();
    available_from_ready_instants(&mut eligible)
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

fn resolve(
    targets: &[Target],
    retry: &BTreeMap<CanonicalUuid, Instant>,
    now: Instant,
) -> Resolution {
    let mut candidates = Vec::new();
    for target in targets {
        let (trigger, available) = if target.pending_inbox {
            (Trigger::PendingInbox, Available::Ready)
        } else if target.held_claim {
            (Trigger::HeldClaim, Available::Ready)
        } else {
            (Trigger::AvailableWork, target.available)
        };
        let eligible_at = match available {
            Available::None => continue,
            Available::At(deadline) if deadline > now => deadline,
            Available::At(_) | Available::Ready => now,
        };
        let due = retry
            .get(&target.lane)
            .copied()
            .filter(|deadline| *deadline > eligible_at)
            .unwrap_or(eligible_at);
        candidates.push((trigger, target.lane, due));
    }
    for trigger in [
        Trigger::PendingInbox,
        Trigger::HeldClaim,
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
            resolve(&[row], &BTreeMap::new(), instant(10)),
            Resolution::Attempt {
                trigger: Trigger::PendingInbox,
                lane: lane(1),
            }
        );
        row.pending_inbox = false;
        assert_eq!(
            resolve(&[row], &BTreeMap::new(), instant(10)),
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
            resolve(&[available, held, inbox], &BTreeMap::new(), instant(10)),
            Resolution::Attempt {
                trigger: Trigger::PendingInbox,
                lane: lane(3),
            }
        );
        inbox.pending_inbox = false;
        assert_eq!(
            resolve(&[available, held, inbox], &BTreeMap::new(), instant(10)),
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
            resolve(&[second, first], &BTreeMap::new(), instant(10)),
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
            resolve(&[row], &BTreeMap::new(), instant(10)),
            Resolution::Wait {
                trigger: Trigger::AvailableWork,
                lane: lane(1),
                deadline: instant(12),
            }
        );
        assert!(matches!(
            resolve(&[row], &BTreeMap::new(), instant(12)),
            Resolution::Attempt { .. }
        ));
    }

    #[test]
    fn two_ready_tasks_settle_and_one_ready_task_ages_to_starvation() {
        let mut none = Vec::new();
        assert_eq!(
            available_from_ready_instants(&mut none).expect("empty projection resolves"),
            Available::None
        );
        let mut lone = vec![instant(0)];
        assert_eq!(
            available_from_ready_instants(&mut lone).expect("lone projection resolves"),
            Available::At(instant_after_minutes(3))
        );
        let mut burst = vec![instant(0), instant(7)];
        assert_eq!(
            available_from_ready_instants(&mut burst).expect("burst projection resolves"),
            Available::At(instant(10))
        );
    }

    #[test]
    fn every_refusal_trigger_has_one_closed_label() {
        assert_eq!(Trigger::PendingInbox.as_str(), "pending-inbox");
        assert_eq!(Trigger::HeldClaim.as_str(), "held-claim");
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
    fn cheap_declines_do_not_spend_the_shared_retry_bucket() {
        let row = target(1);
        let retry = BTreeMap::from([(lane(1), instant(30))]);
        assert_eq!(resolve(&[row], &retry, instant(10)), Resolution::Idle);
    }

    #[test]
    fn every_attempted_arm_inherits_the_one_retry_deadline() {
        for trigger in [
            Trigger::PendingInbox,
            Trigger::HeldClaim,
            Trigger::AvailableWork,
        ] {
            let mut row = target(1);
            row.pending_inbox = trigger == Trigger::PendingInbox;
            row.held_claim = trigger == Trigger::HeldClaim;
            row.available = if trigger == Trigger::AvailableWork {
                Available::Ready
            } else {
                Available::None
            };
            let retry = BTreeMap::from([(lane(1), instant(30))]);
            assert_eq!(
                resolve(&[row], &retry, instant(10)),
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
            resolve(&[waiting, ready], &BTreeMap::new(), instant(10)),
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
        let retry = BTreeMap::from([(lane(1), instant(30))]);
        assert_eq!(
            resolve(&[retrying, sibling], &retry, instant(10)),
            Resolution::Attempt {
                trigger: Trigger::AvailableWork,
                lane: lane(2),
            }
        );
    }

    fn instant_after_minutes(minutes: u8) -> Instant {
        format!("2026-08-01T07:{minutes:02}:00.000Z")
            .parse()
            .expect("fixture instant")
    }
}
