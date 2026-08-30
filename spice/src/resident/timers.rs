//! Resident-owned fold of durable deadlines into process-local waits.

use std::collections::{BTreeMap, BTreeSet};
use std::thread::{self, JoinHandle};

use board::ProjectPolicy;
use bus::timer::{TimerEvent, TimerId, TimerKind, TimerWorker};
use bus::{Bus, CommitOutcome, JournalTransaction};
use journal::{FactCause, GuardError, JournalGuard, JournalWrite};
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, Instant, Producer, ResidentState, TaskHandle, intent,
};

use super::{CommandResult, environment_fault, new_uuid, steering, store_fault};

const CLAIM_WARNING_SECONDS: i64 = 299;
const WARNING_COMMIT_RETRIES: usize = 4;

/// Process-local waits mirror durable claim, steering, and lifecycle authority.
/// Each fired event re-proves its captured durable state before appending, so
/// restart catch-up and stale races use the same guarded transition path.
pub(super) struct Runtime {
    worker: Option<TimerWorker>,
    events: tokio::sync::mpsc::UnboundedReceiver<TimerEvent>,
    forwarder: Option<JoinHandle<()>>,
    projects: ProjectPolicy,
    steering: steer::Config,
    claims: BTreeMap<TaskHandle, ArmedClaim>,
    renewals: BTreeMap<TaskHandle, ArmedRenewal>,
    warnings: BTreeMap<TaskHandle, ArmedWarning>,
    directives: BTreeMap<CanonicalUuid, ArmedDirective>,
    wake: Option<ArmedWake>,
    synchronized: Option<wire::Seq>,
}

struct TimerSnapshot {
    tasks: Vec<journal::TaskHeads>,
    residency: Option<journal::JournalHead<wire::ResidencyHead>>,
    steering: Vec<journal::JournalHead<wire::SteeringHead>>,
    facts: journal::FactSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ClaimAuthority {
    task: TaskHandle,
    occupant: CanonicalUuid,
    epoch: CanonicalUuid,
    expiry: Instant,
}

struct ArmedClaim {
    id: TimerId,
    authority: ClaimAuthority,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RenewalAuthority {
    task: TaskHandle,
    stamp: wire::TaskStamp,
    claim: journal::JournalHead<wire::ClaimHead>,
    lease: journal::JournalHead<wire::LeaseHead>,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    epoch: CanonicalUuid,
    deadline: Instant,
}

struct ArmedRenewal {
    id: TimerId,
    authority: RenewalAuthority,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct WarningAuthority {
    task: TaskHandle,
    stamp: wire::TaskStamp,
    claim: journal::JournalHead<wire::ClaimHead>,
    lease: journal::JournalHead<wire::LeaseHead>,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    epoch: CanonicalUuid,
    deadline: Instant,
    expiry: Instant,
}

struct ArmedWarning {
    id: TimerId,
    authority: WarningAuthority,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DirectiveAuthority {
    deadline: Instant,
    body: steering::SteeringDeadline,
}

struct ArmedDirective {
    id: TimerId,
    authority: DirectiveAuthority,
}

struct ArmedWake {
    id: TimerId,
    lane: CanonicalUuid,
    deadline: Instant,
    kind: super::lifecycle::WakeKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Fired {
    Stale,
    ClaimLapse,
    ClaimRenew,
    ClaimWarning(WarningDelivery),
    Steering,
    TaskWake(CanonicalUuid),
    LaneResume(CanonicalUuid),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct WarningDelivery {
    stamp: wire::TaskStamp,
    claim: wire::Seq,
    lease: wire::Seq,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    epoch: CanonicalUuid,
    expiry: Instant,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct WarningGeneration {
    stamp: wire::TaskStamp,
    claim: wire::Seq,
    lease: wire::Seq,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    epoch: CanonicalUuid,
    expiry: Instant,
}

impl From<&WarningDelivery> for WarningGeneration {
    fn from(delivery: &WarningDelivery) -> Self {
        Self {
            stamp: delivery.stamp.clone(),
            claim: delivery.claim,
            lease: delivery.lease,
            lane: delivery.lane,
            occupant: delivery.occupant,
            epoch: delivery.epoch,
            expiry: delivery.expiry,
        }
    }
}

impl From<&wire::body::ClaimLeaseWarned> for WarningGeneration {
    fn from(warning: &wire::body::ClaimLeaseWarned) -> Self {
        Self {
            stamp: warning.task.clone(),
            claim: warning.claim,
            lease: warning.lease,
            lane: warning.lane,
            occupant: warning.occupant,
            epoch: warning.epoch,
            expiry: warning.expiry,
        }
    }
}

impl WarningGeneration {
    fn current(
        &self,
        heads: Option<&journal::TaskHeads>,
        residency: Option<&journal::JournalHead<wire::ResidencyHead>>,
        at: Instant,
    ) -> bool {
        if at >= self.expiry {
            return false;
        }
        let Some(heads) = heads else {
            return false;
        };
        let (Some(claim), Some(lease), Some(residency)) = (heads.claim(), heads.lease(), residency)
        else {
            return false;
        };
        claim.position() == self.claim
            && lease.position() == self.lease
            && claim.body().disposition == ClaimDisposition::Active
            && claim.body().lane == self.lane
            && claim.body().occupant == self.occupant
            && claim.body().epoch == self.epoch
            && lease.body().epoch == self.epoch
            && lease.body().expiry == self.expiry
            && residency.body().state == ResidentState::Seated
            && residency.body().epoch == self.epoch
            && current_claim_occupant(residency.body(), claim.body())
    }

    fn current_residency(
        &self,
        read: &journal::JournalRead<'_>,
        at: Instant,
    ) -> Result<Option<journal::JournalHead<wire::ResidencyHead>>, GuardError> {
        let heads = read.task_heads_for(&self.stamp)?;
        let residency = read.residency_head()?;
        Ok(self
            .current(heads.as_ref(), residency.as_ref(), at)
            .then_some(residency)
            .flatten())
    }
}

impl WarningDelivery {
    pub(super) const fn lane(&self) -> CanonicalUuid {
        self.lane
    }

    pub(super) fn current_residency(
        &self,
        read: &journal::JournalRead<'_>,
    ) -> Result<Option<journal::JournalHead<wire::ResidencyHead>>, GuardError> {
        let at = Instant::new(jiff::Timestamp::now())
            .map_err(|error| GuardError::Invalid(format!("warning clock is invalid: {error}")))?;
        WarningGeneration::from(self).current_residency(read, at)
    }
}

impl Runtime {
    pub(super) fn start(projects: ProjectPolicy, steering: steer::Config) -> CommandResult<Self> {
        let (worker, events) = TimerWorker::start()
            .map_err(|error| environment_fault("bus::timer-start-failed", &error.to_string()))?;
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let forwarder = match thread::Builder::new()
            .name("spice-timer-events".to_owned())
            .spawn(move || {
                while let Ok(event) = events.recv() {
                    if sender.send(event).is_err() {
                        break;
                    }
                }
            }) {
            Ok(forwarder) => forwarder,
            Err(error) => {
                drop(worker);
                return Err(environment_fault(
                    "bus::timer-forwarder-failed",
                    &error.to_string(),
                ));
            }
        };
        Ok(Self {
            worker: Some(worker),
            events: receiver,
            forwarder: Some(forwarder),
            projects,
            steering,
            claims: BTreeMap::new(),
            renewals: BTreeMap::new(),
            warnings: BTreeMap::new(),
            directives: BTreeMap::new(),
            wake: None,
            synchronized: None,
        })
    }

    pub(super) async fn recv(&mut self) -> Option<TimerEvent> {
        self.events.recv().await
    }

    /// Make the sole automatic lifecycle wait match the resolver's next event.
    pub(super) fn synchronize_wake(
        &mut self,
        desired: Option<super::lifecycle::WakeDeadline>,
    ) -> CommandResult<()> {
        if self.wake.as_ref().is_some_and(|armed| {
            desired.is_some_and(|desired| {
                armed.lane == desired.lane
                    && armed.deadline == desired.at
                    && armed.kind == desired.kind
            })
        }) {
            return Ok(());
        }
        if let Some(armed) = self.wake.take() {
            self.worker()?
                .cancel(armed.id)
                .map_err(|error| store_fault("bus::timer-cancel-failed", &error.to_string()))?;
        }
        if let Some(desired) = desired {
            let id = self
                .worker()?
                .arm(
                    desired.lane.to_string(),
                    match desired.kind {
                        super::lifecycle::WakeKind::Task => TimerKind::TaskWake,
                        super::lifecycle::WakeKind::Hold => TimerKind::LaneResume,
                    },
                    desired.at,
                )
                .map_err(|error| store_fault("bus::timer-arm-failed", &error.to_string()))?;
            self.wake = Some(ArmedWake {
                id,
                lane: desired.lane,
                deadline: desired.at,
                kind: desired.kind,
            });
        }
        Ok(())
    }

    /// Make process-local waits exactly match current lapse-capable claim heads.
    pub(super) fn synchronize(&mut self, bus: &Bus) -> CommandResult<BTreeSet<CanonicalUuid>> {
        let mut snapshot = timer_snapshot(bus)?;
        let (refresh, changed) = cancel_stale_warnings(bus, &snapshot)?;
        if changed {
            snapshot = timer_snapshot(bus)?;
        }
        let synchronized = snapshot.facts.horizon();
        let desired = desired_claims(&snapshot.tasks, snapshot.residency.as_ref())?;
        let desired_renewals = desired_renewals(
            &snapshot.tasks,
            snapshot.residency.as_ref(),
            snapshot.facts.facts(),
        )?;
        let desired_warnings = desired_warnings(
            &snapshot.tasks,
            snapshot.residency.as_ref(),
            snapshot.facts.facts(),
        )?;
        let desired_directives = desired_directives(&snapshot.steering, &self.steering)?;
        let stale = self
            .claims
            .iter()
            .filter_map(|(task, armed)| {
                (desired.get(task) != Some(&armed.authority)).then_some(task.clone())
            })
            .collect::<Vec<_>>();
        for task in stale {
            let armed = self
                .claims
                .remove(&task)
                .expect("stale task came from the armed map");
            self.worker()?
                .cancel(armed.id)
                .map_err(|error| store_fault("bus::timer-cancel-failed", &error.to_string()))?;
        }
        for (task, authority) in desired {
            if self.claims.contains_key(&task) {
                continue;
            }
            let id = self
                .worker()?
                .arm(task.to_string(), TimerKind::ClaimLapse, authority.expiry)
                .map_err(|error| store_fault("bus::timer-arm-failed", &error.to_string()))?;
            self.claims.insert(task, ArmedClaim { id, authority });
        }
        self.synchronize_renewals(desired_renewals)?;
        self.synchronize_warnings(desired_warnings)?;
        let stale = self
            .directives
            .iter()
            .filter_map(|(directive, armed)| {
                (desired_directives.get(directive) != Some(&armed.authority)).then_some(*directive)
            })
            .collect::<Vec<_>>();
        for directive in stale {
            let armed = self
                .directives
                .remove(&directive)
                .expect("stale directive came from the armed map");
            self.worker()?
                .cancel(armed.id)
                .map_err(|error| store_fault("bus::timer-cancel-failed", &error.to_string()))?;
        }
        for (directive, authority) in desired_directives {
            if self.directives.contains_key(&directive) {
                continue;
            }
            let kind = match authority.body.transition {
                steering::DeadlineTransition::Escalate { .. } => TimerKind::SteerEscalate,
                steering::DeadlineTransition::Lapse => TimerKind::SteerLapse,
            };
            let id = self
                .worker()?
                .arm(directive.to_string(), kind, authority.deadline)
                .map_err(|error| store_fault("bus::timer-arm-failed", &error.to_string()))?;
            self.directives
                .insert(directive, ArmedDirective { id, authority });
        }
        self.synchronized = synchronized;
        Ok(refresh)
    }

    /// Whether a queued notification is already represented by the last fold.
    ///
    /// Synchronization reads through the writer's current cursor, which can
    /// cover several notifications already waiting in the broadcast channel.
    /// Replaying each of those notifications would decode the complete fact
    /// stream again without observing any newer authority.
    pub(super) fn covers(&self, commit: bus::JournalCommit) -> bool {
        self.synchronized
            .is_some_and(|horizon| horizon.get() >= commit.last())
    }

    fn synchronize_warnings(
        &mut self,
        desired: BTreeMap<TaskHandle, WarningAuthority>,
    ) -> CommandResult<()> {
        let stale = self
            .warnings
            .iter()
            .filter_map(|(task, armed)| {
                (desired.get(task) != Some(&armed.authority)).then_some(task.clone())
            })
            .collect::<Vec<_>>();
        for task in stale {
            let armed = self
                .warnings
                .remove(&task)
                .expect("stale task came from the warning map");
            self.worker()?
                .cancel(armed.id)
                .map_err(|error| store_fault("bus::timer-cancel-failed", &error.to_string()))?;
        }
        for (task, authority) in desired {
            if self.warnings.contains_key(&task) {
                continue;
            }
            let id = self
                .worker()?
                .arm(
                    task.to_string(),
                    TimerKind::ClaimWarning,
                    authority.deadline,
                )
                .map_err(|error| store_fault("bus::timer-arm-failed", &error.to_string()))?;
            self.warnings.insert(task, ArmedWarning { id, authority });
        }
        Ok(())
    }

    fn synchronize_renewals(
        &mut self,
        desired: BTreeMap<TaskHandle, RenewalAuthority>,
    ) -> CommandResult<()> {
        let stale = self
            .renewals
            .iter()
            .filter_map(|(task, armed)| {
                (desired.get(task) != Some(&armed.authority)).then_some(task.clone())
            })
            .collect::<Vec<_>>();
        for task in stale {
            let armed = self
                .renewals
                .remove(&task)
                .expect("stale task came from the renewal map");
            self.worker()?
                .cancel(armed.id)
                .map_err(|error| store_fault("bus::timer-cancel-failed", &error.to_string()))?;
        }
        for (task, authority) in desired {
            if self.renewals.contains_key(&task) {
                continue;
            }
            let id = self
                .worker()?
                .arm(task.to_string(), TimerKind::ClaimRenew, authority.deadline)
                .map_err(|error| store_fault("bus::timer-arm-failed", &error.to_string()))?;
            self.renewals.insert(task, ArmedRenewal { id, authority });
        }
        Ok(())
    }

    /// Consume one fired event using only authority captured when it was armed.
    pub(super) fn consume(&mut self, bus: &Bus, event: TimerEvent) -> CommandResult<Fired> {
        match event.kind() {
            TimerKind::TaskWake | TimerKind::LaneResume => self.consume_lifecycle(event),
            TimerKind::SteerEscalate | TimerKind::SteerLapse => {
                self.consume_steering(bus, event).map(|()| Fired::Steering)
            }
            TimerKind::ClaimLapse => self.consume_claim(bus, event),
            TimerKind::ClaimRenew => self.consume_renewal(bus, event),
            TimerKind::ClaimWarning => self.consume_warning(bus, event),
            kind => Err(environment_fault(
                "bus::timer-kind-unhandled",
                &format!("production received unarmed timer kind {kind:?}"),
            )),
        }
    }

    fn consume_lifecycle(&mut self, event: TimerEvent) -> CommandResult<Fired> {
        let fired_kind = event.kind();
        let Some(armed) = self.wake.take() else {
            return Ok(Fired::Stale);
        };
        if armed.id != event.id() {
            self.wake = Some(armed);
            return Ok(Fired::Stale);
        }
        if armed.lane.to_string() != event.owner()
            || armed.deadline != event.deadline()
            || match armed.kind {
                super::lifecycle::WakeKind::Task => fired_kind != TimerKind::TaskWake,
                super::lifecycle::WakeKind::Hold => fired_kind != TimerKind::LaneResume,
            }
        {
            return Err(environment_fault(
                "bus::timer-authority-mismatch",
                "fired lifecycle timer disagreed with its captured authority",
            ));
        }
        Ok(match armed.kind {
            super::lifecycle::WakeKind::Task => Fired::TaskWake(armed.lane),
            super::lifecycle::WakeKind::Hold => Fired::LaneResume(armed.lane),
        })
    }

    fn consume_claim(&mut self, bus: &Bus, event: TimerEvent) -> CommandResult<Fired> {
        let Some(task) = self
            .claims
            .iter()
            .find_map(|(task, armed)| (armed.id == event.id()).then_some(task.clone()))
        else {
            return Ok(Fired::Stale);
        };
        let armed = self
            .claims
            .remove(&task)
            .expect("event task came from the armed map");
        if event.owner() != armed.authority.task.to_string()
            || event.deadline() != armed.authority.expiry
        {
            return Err(environment_fault(
                "bus::timer-authority-mismatch",
                "fired timer identity disagreed with its captured claim authority",
            ));
        }
        let at = Instant::new(jiff::Timestamp::now())
            .map_err(|error| environment_fault("bus::timer-clock-invalid", &error.to_string()))?;
        let request = board::lapse::Request {
            projects: self.projects.clone(),
            command: new_uuid(),
            occupant: armed.authority.occupant,
            epoch: armed.authority.epoch,
            expiry: armed.authority.expiry,
            at,
            intent: intent::TaskRef { task },
        };
        let lapse = bus
            .transact(move |journal| match board::lapse::lapse(journal, request) {
                Ok(lapsed) => {
                    let first = lapsed.first();
                    let last = lapsed.last();
                    JournalTransaction::committed(Ok(lapsed), first, last)
                }
                Err(error) => JournalTransaction::settled(Err(error)),
            })
            .map_err(|error| store_fault("bus::timer-lapse-failed", &error.to_string()))?;
        match lapse {
            Ok(_) | Err(board::lapse::LapseError::Refused(_)) => Ok(Fired::ClaimLapse),
            Err(error) => Err(store_fault("bus::timer-lapse-failed", &error.to_string())),
        }
    }

    fn consume_renewal(&mut self, bus: &Bus, event: TimerEvent) -> CommandResult<Fired> {
        let Some(task) = self
            .renewals
            .iter()
            .find_map(|(task, armed)| (armed.id == event.id()).then_some(task.clone()))
        else {
            return Ok(Fired::Stale);
        };
        let armed = self
            .renewals
            .remove(&task)
            .expect("event task came from the renewal map");
        if event.owner() != armed.authority.task.to_string()
            || event.deadline() != armed.authority.deadline
        {
            return Err(environment_fault(
                "bus::timer-authority-mismatch",
                "fired timer identity disagreed with its captured renewal authority",
            ));
        }
        let at = Instant::new(jiff::Timestamp::now())
            .map_err(|error| environment_fault("bus::timer-clock-invalid", &error.to_string()))?;
        if at < armed.authority.deadline || at >= armed.authority.lease.body().expiry {
            return Ok(Fired::Stale);
        }
        let rolling = shifted(
            at,
            std::time::Duration::from_secs(crate::board_command::AUTOMATIC_ROLLING_SECONDS as u64),
        )?;
        let until = std::cmp::max(rolling, armed.authority.lease.body().expiry);
        let maximum = std::cmp::max(until, armed.authority.lease.body().maximum);
        let authority = armed.authority;
        let origin = resident_origin(authority.epoch)?;
        let mut proposal = JournalWrite::new(Actor::Spice, origin, at, Producer::BusTimer);
        let guarded = authority.clone();
        proposal.guard(JournalGuard::new(move |read| {
            renewal_guard(read, &guarded, at)
        }));
        let renewed = proposal
            .fact(
                wire::body::ClaimRenewed {
                    task: authority.stamp.clone(),
                    epoch: authority.epoch,
                    until,
                    maximum,
                },
                FactCause::Root,
            )
            .map_err(|error| store_fault("claim::renewal-invalid", &error.to_string()))?;
        let mut lease = authority.lease.body().clone();
        lease.renewal = at;
        lease.expiry = until;
        lease.maximum = maximum;
        proposal.lease_head(authority.stamp, renewed, move |_| lease);
        match bus
            .commit(proposal)
            .map_err(|error| store_fault("bus::commit-failed", &error.to_string()))?
        {
            CommitOutcome::Committed(_) => Ok(Fired::ClaimRenew),
            CommitOutcome::Refused(refusal) if refusal.reason().starts_with("claim::renewal-") => {
                Ok(Fired::Stale)
            }
            CommitOutcome::Refused(refusal) => Err(store_fault(
                "claim::renewal-commit-refused",
                refusal.reason(),
            )),
            CommitOutcome::Fault(fault) => {
                Err(store_fault("journal::commit-failed", fault.detail()))
            }
        }
    }

    fn consume_warning(&mut self, bus: &Bus, event: TimerEvent) -> CommandResult<Fired> {
        let Some(task) = self
            .warnings
            .iter()
            .find_map(|(task, armed)| (armed.id == event.id()).then_some(task.clone()))
        else {
            return Ok(Fired::Stale);
        };
        let armed = self
            .warnings
            .remove(&task)
            .expect("event task came from the warning map");
        if event.owner() != armed.authority.task.to_string()
            || event.deadline() != armed.authority.deadline
        {
            return Err(environment_fault(
                "bus::timer-authority-mismatch",
                "fired timer identity disagreed with its captured warning authority",
            ));
        }
        let at = Instant::new(jiff::Timestamp::now())
            .map_err(|error| environment_fault("bus::timer-clock-invalid", &error.to_string()))?;
        if at < armed.authority.deadline || at >= armed.authority.expiry {
            return Ok(Fired::Stale);
        }
        let remaining = armed
            .authority
            .expiry
            .get()
            .duration_since(at.get())
            .unsigned_abs()
            .as_secs();
        let origin = resident_origin(armed.authority.epoch)?;
        let authority = armed.authority;
        for attempt in 0..WARNING_COMMIT_RETRIES {
            let minted_millis = bus
                .read(|read| read.steering_minted_millis())
                .map_err(|error| store_fault("steer::index-unreadable", &error.to_string()))?;
            let key = mint_warning_key(minted_millis, at)?;
            let body = wire::body::ClaimLeaseWarned {
                task: authority.stamp.clone(),
                claim: authority.claim.position(),
                lease: authority.lease.position(),
                lane: authority.lane,
                occupant: authority.occupant,
                epoch: authority.epoch,
                expiry: authority.expiry,
                directive: new_uuid(),
                key: key.to_string(),
                text: format!(
                    "CLAIM LEASE URGENT: {} has {remaining}s remaining; run spice task reclaim {}",
                    authority.task, authority.task
                ),
            };
            let guarded = authority.clone();
            let head_origin = origin.clone();
            let mut proposal =
                JournalWrite::new(Actor::Spice, origin.clone(), at, Producer::BusTimer);
            proposal.guard(JournalGuard::new(move |read| {
                warning_guard(read, &guarded, at)
            }));
            let pending = proposal
                .fact(body.clone(), FactCause::Root)
                .map_err(|error| store_fault("claim::warning-invalid", &error.to_string()))?;
            proposal.steering_head(pending, move |seq| {
                steer::Ledger::claim_warned_head(&body, head_origin, at, seq)
                    .expect("the admitted claim warning builds its authority head")
            });
            match bus
                .commit(proposal)
                .map_err(|error| store_fault("bus::commit-failed", &error.to_string()))?
            {
                CommitOutcome::Committed(_) => {
                    return Ok(Fired::ClaimWarning(WarningDelivery {
                        stamp: authority.stamp,
                        claim: authority.claim.position(),
                        lease: authority.lease.position(),
                        lane: authority.lane,
                        occupant: authority.occupant,
                        epoch: authority.epoch,
                        expiry: authority.expiry,
                    }));
                }
                CommitOutcome::Refused(refusal)
                    if refusal.reason().starts_with("claim::warning-") =>
                {
                    return Ok(Fired::Stale);
                }
                CommitOutcome::Refused(_) if attempt + 1 < WARNING_COMMIT_RETRIES => {}
                CommitOutcome::Refused(refusal) => {
                    return Err(store_fault(
                        "claim::warning-commit-refused",
                        refusal.reason(),
                    ));
                }
                CommitOutcome::Fault(fault) => {
                    return Err(store_fault("journal::commit-failed", fault.detail()));
                }
            }
        }
        unreachable!("the final warning commit attempt returns instead of retrying")
    }

    fn consume_steering(&mut self, bus: &Bus, event: TimerEvent) -> CommandResult<()> {
        let Some(directive) = self
            .directives
            .iter()
            .find_map(|(directive, armed)| (armed.id == event.id()).then_some(*directive))
        else {
            return Ok(());
        };
        let armed = self
            .directives
            .remove(&directive)
            .expect("event directive came from the armed map");
        let expected_kind = match armed.authority.body.transition {
            steering::DeadlineTransition::Escalate { .. } => TimerKind::SteerEscalate,
            steering::DeadlineTransition::Lapse => TimerKind::SteerLapse,
        };
        if event.owner() != directive.to_string()
            || event.deadline() != armed.authority.deadline
            || event.kind() != expected_kind
        {
            return Err(environment_fault(
                "bus::timer-authority-mismatch",
                "fired timer identity disagreed with its captured steering authority",
            ));
        }
        let at = Instant::new(jiff::Timestamp::now())
            .map_err(|error| environment_fault("bus::timer-clock-invalid", &error.to_string()))?;
        steering::apply_deadline(bus, &armed.authority.body, at).map(drop)
    }

    fn worker(&self) -> CommandResult<&TimerWorker> {
        self.worker.as_ref().ok_or_else(|| {
            environment_fault(
                "bus::timer-worker-closed",
                "the production timer worker ended before resident shutdown",
            )
        })
    }
}

fn timer_snapshot(bus: &Bus) -> CommandResult<TimerSnapshot> {
    bus.read(|read| {
        Ok(TimerSnapshot {
            tasks: read.task_heads()?,
            residency: read.residency_head()?,
            steering: read.pending_steering(None)?,
            facts: read.fact_snapshot().map_err(|error| {
                GuardError::Invalid(format!("timer fact snapshot is invalid: {error}"))
            })?,
        })
    })
    .map_err(|error| store_fault("bus::timer-snapshot-failed", &error.to_string()))
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.shutdown();
        }
        if let Some(forwarder) = self.forwarder.take() {
            let _ = forwarder.join();
        }
    }
}

fn desired_claims(
    tasks: &[journal::TaskHeads],
    residency: Option<&journal::JournalHead<wire::ResidencyHead>>,
) -> CommandResult<BTreeMap<TaskHandle, ClaimAuthority>> {
    let Some(residency) = residency else {
        return Ok(BTreeMap::new());
    };
    if residency.body().state != ResidentState::Seated {
        return Ok(BTreeMap::new());
    }
    let mut desired = BTreeMap::new();
    for heads in tasks {
        let Some(claim) = heads.claim().map(journal::JournalHead::body) else {
            continue;
        };
        // A restart may move residency before carrying live authority. Keep the
        // old exact heads armed so an expired claim cannot remain active forever.
        if claim.disposition != ClaimDisposition::Active {
            continue;
        }
        let lease = coherent_lease(heads, claim)?;
        let task = wire::TaskHandle::for_project(
            &heads.statement().body().project,
            &heads.statement().body().born,
        );
        desired.insert(
            task.clone(),
            ClaimAuthority {
                task,
                occupant: claim.occupant,
                epoch: claim.epoch,
                expiry: lease.body().expiry,
            },
        );
    }
    Ok(desired)
}

fn desired_warnings(
    tasks: &[journal::TaskHeads],
    residency: Option<&journal::JournalHead<wire::ResidencyHead>>,
    facts: &[wire::Envelope<wire::AnyFact>],
) -> CommandResult<BTreeMap<TaskHandle, WarningAuthority>> {
    let Some(residency) = residency else {
        return Ok(BTreeMap::new());
    };
    if residency.body().state != ResidentState::Seated {
        return Ok(BTreeMap::new());
    }
    let mut desired = BTreeMap::new();
    for heads in tasks {
        let Some(claim) = heads.claim() else {
            continue;
        };
        if claim.body().disposition != ClaimDisposition::Active
            || claim.body().epoch != residency.body().epoch
            || !current_claim_occupant(residency.body(), claim.body())
        {
            continue;
        }
        if crate::board_command::is_automatic_claim(claim.body(), facts) {
            continue;
        }
        let lease = coherent_lease(heads, claim.body())?;
        let task = TaskHandle::for_project(
            &heads.statement().body().project,
            &heads.statement().body().born,
        );
        let authority = WarningAuthority {
            task: task.clone(),
            stamp: heads.statement().body().born.clone(),
            claim: claim.clone(),
            lease: lease.clone(),
            lane: claim.body().lane,
            occupant: claim.body().occupant,
            epoch: claim.body().epoch,
            deadline: warning_deadline(lease.body().expiry)?,
            expiry: lease.body().expiry,
        };
        if !warning_recorded(facts, &authority) {
            desired.insert(task, authority);
        }
    }
    Ok(desired)
}

fn desired_renewals(
    tasks: &[journal::TaskHeads],
    residency: Option<&journal::JournalHead<wire::ResidencyHead>>,
    facts: &[wire::Envelope<wire::AnyFact>],
) -> CommandResult<BTreeMap<TaskHandle, RenewalAuthority>> {
    let Some(residency) = residency else {
        return Ok(BTreeMap::new());
    };
    if residency.body().state != ResidentState::Seated {
        return Ok(BTreeMap::new());
    }
    let now = Instant::new(jiff::Timestamp::now())
        .map_err(|error| environment_fault("bus::timer-clock-invalid", &error.to_string()))?;
    let mut desired = BTreeMap::new();
    for heads in tasks {
        let Some(claim) = heads.claim() else { continue };
        if claim.body().disposition != ClaimDisposition::Active
            || claim.body().epoch != residency.body().epoch
            || !current_claim_occupant(residency.body(), claim.body())
            || !crate::board_command::is_automatic_claim(claim.body(), facts)
        {
            continue;
        }
        let lease = coherent_lease(heads, claim.body())?;
        if lease.body().expiry <= now {
            continue;
        }
        let deadline = shifted(
            lease.body().renewal,
            std::time::Duration::from_secs(
                crate::board_command::AUTOMATIC_HEARTBEAT_SECONDS as u64,
            ),
        )?;
        let task = TaskHandle::for_project(
            &heads.statement().body().project,
            &heads.statement().body().born,
        );
        desired.insert(
            task.clone(),
            RenewalAuthority {
                task,
                stamp: heads.statement().body().born.clone(),
                claim: claim.clone(),
                lease: lease.clone(),
                lane: claim.body().lane,
                occupant: claim.body().occupant,
                epoch: claim.body().epoch,
                deadline,
            },
        );
    }
    Ok(desired)
}

fn renewal_guard(
    read: &journal::JournalRead<'_>,
    expected: &RenewalAuthority,
    at: Instant,
) -> Result<(), GuardError> {
    if at < expected.deadline || at >= expected.lease.body().expiry {
        return Err(GuardError::refused("claim::renewal-outside-horizon"));
    }
    let Some(heads) = read.task_heads_for(&expected.stamp)? else {
        return Err(GuardError::refused("claim::renewal-task-moved"));
    };
    if heads.claim() != Some(&expected.claim) || heads.lease() != Some(&expected.lease) {
        return Err(GuardError::refused("claim::renewal-authority-moved"));
    }
    let Some(residency) = read.residency_head()? else {
        return Err(GuardError::refused("claim::renewal-residency-missing"));
    };
    if residency.body().state != ResidentState::Seated
        || residency.body().epoch != expected.epoch
        || !current_claim_occupant(residency.body(), expected.claim.body())
    {
        return Err(GuardError::refused("claim::renewal-residency-moved"));
    }
    Ok(())
}

fn coherent_lease<'a>(
    heads: &'a journal::TaskHeads,
    claim: &wire::ClaimHead,
) -> CommandResult<&'a journal::JournalHead<wire::LeaseHead>> {
    let task = TaskHandle::for_project(
        &heads.statement().body().project,
        &heads.statement().body().born,
    );
    let lease = heads.lease().ok_or_else(|| {
        environment_fault(
            "bus::timer-lease-missing",
            &format!("active task {task} has no durable lease head"),
        )
    })?;
    if lease.body().epoch != claim.epoch {
        return Err(environment_fault(
            "bus::timer-lease-epoch",
            &format!("task {task} claim and lease epochs disagree"),
        ));
    }
    if !(lease.body().start <= lease.body().renewal
        && lease.body().renewal <= lease.body().expiry
        && lease.body().expiry <= lease.body().maximum)
    {
        return Err(environment_fault(
            "bus::timer-lease-incoherent",
            &format!("task {task} lease does not satisfy start <= renewal <= expiry <= maximum"),
        ));
    }
    Ok(lease)
}

fn current_claim_occupant(residency: &wire::ResidencyHead, claim: &wire::ClaimHead) -> bool {
    residency
        .lanes
        .iter()
        .any(|lane| lane.lane == claim.lane && lane.occupant == Some(claim.occupant))
}

#[derive(Clone)]
struct StaleWarning {
    head: journal::JournalHead<wire::SteeringHead>,
    body: wire::body::ClaimLeaseWarned,
}

fn cancel_stale_warnings(
    bus: &Bus,
    snapshot: &TimerSnapshot,
) -> CommandResult<(BTreeSet<CanonicalUuid>, bool)> {
    let at = Instant::new(jiff::Timestamp::now())
        .map_err(|error| environment_fault("claim::warning-clock-invalid", &error.to_string()))?;
    let stale = stale_warnings(
        &snapshot.tasks,
        snapshot.residency.as_ref(),
        &snapshot.steering,
        snapshot.facts.facts(),
        at,
    )?;
    let mut refresh = BTreeSet::new();
    let mut changed = false;
    for stale in stale {
        let directive = stale.head.body().directive;
        let lane = stale.head.body().lane;
        let expected = stale.head.clone();
        let generation = WarningGeneration::from(&stale.body);
        let mut proposal = JournalWrite::new(
            Actor::Spice,
            stale.head.body().origin.clone(),
            at,
            Producer::CommandDoor,
        );
        proposal.guard(JournalGuard::new(move |read| {
            if read.steering_head(directive)?.as_ref() != Some(&expected) {
                return Err(GuardError::refused("claim::warning-standing-moved"));
            }
            if generation.current_residency(read, at)?.is_some() {
                return Err(GuardError::refused("claim::warning-authority-current"));
            }
            Ok(())
        }));
        let canceled = wire::body::SteerCanceled {
            key: stale.head.body().key.clone(),
            lane,
        };
        let pending = proposal
            .fact(
                canceled.clone(),
                FactCause::Existing(stale.head.body().created),
            )
            .map_err(|error| store_fault("claim::warning-cancel-invalid", &error.to_string()))?;
        let next = steer::Ledger::transition_head(
            stale.head.body(),
            steer::Steering::Canceled(&canceled),
            at,
        )
        .map_err(|error| store_fault("claim::warning-index-invalid", &error.to_string()))?;
        proposal.steering_head(pending, move |_| next);
        match bus
            .commit(proposal)
            .map_err(|error| store_fault("bus::commit-failed", &error.to_string()))?
        {
            CommitOutcome::Committed(_) => {
                refresh.insert(lane);
                changed = true;
            }
            CommitOutcome::Refused(refusal) if refusal.reason().starts_with("claim::warning-") => {}
            CommitOutcome::Refused(refusal) => {
                return Err(store_fault(
                    "claim::warning-cancel-refused",
                    refusal.reason(),
                ));
            }
            CommitOutcome::Fault(fault) => {
                return Err(store_fault("journal::commit-failed", fault.detail()));
            }
        }
    }
    Ok((refresh, changed))
}

fn stale_warnings(
    tasks: &[journal::TaskHeads],
    residency: Option<&journal::JournalHead<wire::ResidencyHead>>,
    steering: &[journal::JournalHead<wire::SteeringHead>],
    facts: &[wire::Envelope<wire::AnyFact>],
    at: Instant,
) -> CommandResult<Vec<StaleWarning>> {
    let mut stale = Vec::new();
    for head in steering {
        let created = head.body().created;
        let Ok(index) = facts.binary_search_by_key(&created, |envelope| envelope.seq) else {
            return Err(store_fault(
                "claim::warning-cause-missing",
                &format!(
                    "steering directive {} has no creating fact",
                    head.body().directive
                ),
            ));
        };
        let Some(body) = facts[index].fact.read::<wire::body::ClaimLeaseWarned>() else {
            continue;
        };
        if body.directive != head.body().directive
            || body.key != head.body().key
            || body.lane != head.body().lane
        {
            return Err(store_fault(
                "claim::warning-cause-moved",
                &format!(
                    "claim warning cause disagrees with steering directive {}",
                    head.body().directive
                ),
            ));
        }
        let generation = WarningGeneration::from(&body);
        let task = tasks
            .iter()
            .find(|heads| heads.statement().body().born == generation.stamp);
        if !generation.current(task, residency, at) {
            stale.push(StaleWarning {
                head: head.clone(),
                body,
            });
        }
    }
    Ok(stale)
}

fn warning_recorded(facts: &[wire::Envelope<wire::AnyFact>], authority: &WarningAuthority) -> bool {
    facts.iter().any(|envelope| {
        envelope
            .fact
            .read::<wire::body::ClaimLeaseWarned>()
            .is_some_and(|body| {
                body.task == authority.stamp
                    && body.claim == authority.claim.position()
                    && body.lease == authority.lease.position()
                    && body.expiry == authority.expiry
            })
    })
}

fn warning_guard(
    read: &journal::JournalRead<'_>,
    expected: &WarningAuthority,
    at: Instant,
) -> Result<(), GuardError> {
    if at < expected.deadline || at >= expected.expiry {
        return Err(GuardError::refused("claim::warning-outside-horizon"));
    }
    let Some(heads) = read.task_heads_for(&expected.stamp)? else {
        return Err(GuardError::refused("claim::warning-task-moved"));
    };
    if heads.claim() != Some(&expected.claim) || heads.lease() != Some(&expected.lease) {
        return Err(GuardError::refused("claim::warning-authority-moved"));
    }
    let Some(residency) = read.residency_head()? else {
        return Err(GuardError::refused("claim::warning-residency-missing"));
    };
    if residency.body().state != ResidentState::Seated
        || residency.body().epoch != expected.epoch
        || !current_claim_occupant(residency.body(), expected.claim.body())
    {
        return Err(GuardError::refused("claim::warning-residency-moved"));
    }
    let facts = read
        .fact_snapshot()
        .map_err(|error| GuardError::Invalid(format!("warning snapshot is invalid: {error}")))?;
    if warning_recorded(facts.facts(), expected) {
        return Err(GuardError::refused("claim::warning-already-recorded"));
    }
    Ok(())
}

fn warning_deadline(expiry: Instant) -> CommandResult<Instant> {
    let deadline = expiry
        .get()
        .checked_add(jiff::SignedDuration::from_secs(-CLAIM_WARNING_SECONDS))
        .map_err(|error| environment_fault("claim::warning-horizon-invalid", &error.to_string()))?;
    Instant::new(deadline)
        .map_err(|error| environment_fault("claim::warning-horizon-invalid", &error.to_string()))
}

fn mint_warning_key(previous: Option<i64>, at: Instant) -> CommandResult<steer::Key> {
    let now = at.get().as_millisecond();
    let millis = previous.map_or(Ok(now), |previous| {
        previous
            .checked_add(1)
            .map(|next| next.max(now))
            .ok_or_else(|| {
                environment_fault(
                    "claim::warning-key-invalid",
                    "epoch millisecond overflowed while advancing the indexed key clock",
                )
            })
    })?;
    steer::Key::for_millis(millis)
        .map_err(|error| environment_fault("claim::warning-key-invalid", &error.to_string()))
}

fn resident_origin(epoch: CanonicalUuid) -> CommandResult<wire::Origin> {
    format!("resident:{epoch}")
        .parse::<wire::Origin>()
        .map_err(|error| store_fault("resident::origin-invalid", &error.to_string()))
}

fn desired_directives(
    heads: &[journal::JournalHead<wire::SteeringHead>],
    config: &steer::Config,
) -> CommandResult<BTreeMap<CanonicalUuid, DirectiveAuthority>> {
    let horizons = config.stage_horizons();
    let mut desired = BTreeMap::new();
    for directive in heads {
        let directive = directive.body();
        let published = directive.published;
        let next = directive.stage.next();
        let stage_deadline = next
            .map(|stage| {
                let index = match stage {
                    wire::body::SteeringStage::Reminded => 0,
                    wire::body::SteeringStage::Escalated => 1,
                    wire::body::SteeringStage::Overdue => 2,
                    wire::body::SteeringStage::Published => {
                        unreachable!("published is not a transition target")
                    }
                };
                shifted(published, horizons[index])
            })
            .transpose()?;
        let (deadline, transition) = match (next, stage_deadline) {
            (Some(to), Some(deadline)) if deadline < directive.expires => (
                deadline,
                steering::DeadlineTransition::Escalate {
                    from: directive.stage,
                    to,
                },
            ),
            _ => (directive.expires, steering::DeadlineTransition::Lapse),
        };
        let key = directive
            .key
            .parse::<steer::Key>()
            .map_err(|error| store_fault("steer::index-invalid", &error.to_string()))?;
        desired.insert(
            directive.directive,
            DirectiveAuthority {
                deadline,
                body: steering::SteeringDeadline {
                    directive: directive.directive,
                    key,
                    lane: directive.lane,
                    expires: directive.expires,
                    transition,
                },
            },
        );
    }
    Ok(desired)
}

fn shifted(at: Instant, duration: std::time::Duration) -> CommandResult<Instant> {
    let duration = jiff::SignedDuration::try_from(duration)
        .map_err(|error| environment_fault("steer::horizon-invalid", &error.to_string()))?;
    let shifted = at
        .get()
        .checked_add(duration)
        .map_err(|error| environment_fault("steer::horizon-invalid", &error.to_string()))?;
    Instant::new(shifted)
        .map_err(|error| environment_fault("steer::horizon-invalid", &error.to_string()))
}

#[cfg(test)]
mod tests;
