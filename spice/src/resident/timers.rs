//! Resident-owned fold of durable deadlines into process-local waits.

use std::collections::BTreeMap;
use std::thread::{self, JoinHandle};

use board::ProjectPolicy;
use bus::timer::{TimerEvent, TimerId, TimerKind, TimerWorker};
use bus::{Bus, CommitOutcome, JournalTransaction};
use journal::{FactCause, GuardError, JournalGuard, JournalWrite};
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, Instant, Producer, ResidentState, TaskHandle, intent,
};

use super::{CommandResult, environment_fault, new_uuid, steering, store_fault};

/// Process-local waits mirror durable claim, steering, and lifecycle authority.
/// Each fired event re-proves its captured durable state before appending, so
/// restart catch-up and stale races use the same guarded transition path.
pub(super) struct Runtime {
    worker: Option<TimerWorker>,
    events: tokio::sync::mpsc::UnboundedReceiver<TimerEvent>,
    forwarder: Option<JoinHandle<()>>,
    projects: ProjectPolicy,
    steering: steer::Config,
    deferrals: BTreeMap<TaskHandle, ArmedDeferral>,
    claims: BTreeMap<TaskHandle, ArmedClaim>,
    renewals: BTreeMap<TaskHandle, ArmedRenewal>,
    directives: BTreeMap<CanonicalUuid, ArmedDirective>,
    wake: Option<ArmedWake>,
    facts: TimerFacts,
}

struct TimerSnapshot {
    tasks: Vec<journal::TaskHeads>,
    residency: Option<journal::JournalHead<wire::ResidencyHead>>,
    steering: Vec<journal::JournalHead<wire::SteeringHead>>,
    horizon: Option<wire::Seq>,
}

#[derive(Default)]
struct TimerFacts {
    rows: Vec<wire::Envelope<wire::AnyFact>>,
    horizon: Option<wire::Seq>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DeferralAuthority {
    task: TaskHandle,
    stamp: wire::TaskStamp,
    workflow: journal::JournalHead<wire::WorkflowHead>,
    deadline: Instant,
}

struct ArmedDeferral {
    id: TimerId,
    authority: DeferralAuthority,
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
    DeferredTaskWoken(TaskHandle),
    ClaimLapse,
    ClaimRenew,
    Steering,
    TaskWake(CanonicalUuid),
    LaneResume(CanonicalUuid),
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
            deferrals: BTreeMap::new(),
            claims: BTreeMap::new(),
            renewals: BTreeMap::new(),
            directives: BTreeMap::new(),
            wake: None,
            facts: TimerFacts::default(),
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
    pub(super) fn synchronize(&mut self, bus: &Bus) -> CommandResult<()> {
        let snapshot = timer_snapshot(bus)?;
        self.facts.synchronize(bus, snapshot.horizon)?;
        let desired_deferrals = desired_deferrals(&snapshot.tasks, &self.projects)?;
        let desired = desired_claims(&snapshot.tasks, snapshot.residency.as_ref())?;
        let desired_renewals = desired_renewals(
            &snapshot.tasks,
            snapshot.residency.as_ref(),
            &self.facts.rows,
        )?;
        let desired_directives = desired_directives(&snapshot.steering, &self.steering)?;
        self.synchronize_deferrals(desired_deferrals)?;
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
        Ok(())
    }

    /// Whether a queued notification is already represented by the last fold.
    ///
    /// Synchronization reads through the writer's current cursor, which can
    /// cover several notifications already waiting in the broadcast channel.
    /// Replaying each of those notifications would decode the complete fact
    /// stream again without observing any newer authority.
    pub(super) fn covers(&self, commit: bus::JournalCommit) -> bool {
        self.facts
            .horizon
            .is_some_and(|horizon| horizon.get() >= commit.last())
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

    fn synchronize_deferrals(
        &mut self,
        desired: BTreeMap<TaskHandle, DeferralAuthority>,
    ) -> CommandResult<()> {
        let stale = self
            .deferrals
            .iter()
            .filter_map(|(task, armed)| {
                (desired.get(task) != Some(&armed.authority)).then_some(task.clone())
            })
            .collect::<Vec<_>>();
        for task in stale {
            let armed = self
                .deferrals
                .remove(&task)
                .expect("stale task came from the deferral map");
            self.worker()?
                .cancel(armed.id)
                .map_err(|error| store_fault("bus::timer-cancel-failed", &error.to_string()))?;
        }
        for (task, authority) in desired {
            if self.deferrals.contains_key(&task) {
                continue;
            }
            let id = self
                .worker()?
                .arm(task.to_string(), TimerKind::TaskWake, authority.deadline)
                .map_err(|error| store_fault("bus::timer-arm-failed", &error.to_string()))?;
            self.deferrals.insert(task, ArmedDeferral { id, authority });
        }
        Ok(())
    }

    /// Consume one fired event using only authority captured when it was armed.
    pub(super) fn consume(&mut self, bus: &Bus, event: TimerEvent) -> CommandResult<Fired> {
        match event.kind() {
            TimerKind::TaskWake
                if self
                    .wake
                    .as_ref()
                    .is_some_and(|armed| armed.id == event.id()) =>
            {
                self.consume_lifecycle(event)
            }
            TimerKind::TaskWake => self.consume_deferral(bus, event),
            TimerKind::LaneResume => self.consume_lifecycle(event),
            TimerKind::SteerEscalate | TimerKind::SteerLapse => {
                self.consume_steering(bus, event).map(|()| Fired::Steering)
            }
            TimerKind::ClaimLapse => self.consume_claim(bus, event),
            TimerKind::ClaimRenew => self.consume_renewal(bus, event),
            kind => Err(environment_fault(
                "bus::timer-kind-unhandled",
                &format!("production received unarmed timer kind {kind:?}"),
            )),
        }
    }

    /// Commit the board's canonical wake transition as the Spice actor.
    ///
    /// `TaskWoken` belongs to the command-door fact vocabulary whether a user
    /// or the resident clears the deferral. The Spice actor and exact captured
    /// workflow guard distinguish this deadline-driven transition from a
    /// manual wake while preserving one task history vocabulary.
    fn consume_deferral(&mut self, bus: &Bus, event: TimerEvent) -> CommandResult<Fired> {
        let Some(task) = self
            .deferrals
            .iter()
            .find_map(|(task, armed)| (armed.id == event.id()).then_some(task.clone()))
        else {
            return Ok(Fired::Stale);
        };
        let armed = self
            .deferrals
            .remove(&task)
            .expect("event task came from the deferral map");
        if event.owner() != armed.authority.task.to_string()
            || event.deadline() != armed.authority.deadline
        {
            return Err(environment_fault(
                "bus::timer-authority-mismatch",
                "fired timer identity disagreed with its captured deferral authority",
            ));
        }
        let at = Instant::new(jiff::Timestamp::now())
            .map_err(|error| environment_fault("bus::timer-clock-invalid", &error.to_string()))?;
        if at < armed.authority.deadline {
            return Ok(Fired::Stale);
        }
        let authority = armed.authority;
        let origin = format!("task:{}", authority.stamp)
            .parse::<wire::Origin>()
            .map_err(|error| store_fault("task::origin-invalid", &error.to_string()))?;
        let mut proposal = JournalWrite::new(Actor::Spice, origin, at, Producer::CommandDoor);
        let guarded = authority.clone();
        proposal.guard(JournalGuard::new(move |read| {
            deferral_guard(read, &guarded, at)
        }));
        let woken = proposal
            .fact(
                wire::body::TaskWoken {
                    task: authority.stamp.clone(),
                },
                FactCause::Root,
            )
            .map_err(|error| store_fault("task::deferral-wake-invalid", &error.to_string()))?;
        let mut workflow = authority.workflow.body().clone();
        workflow.deferral = None;
        proposal.workflow_head(authority.stamp, woken, move |_| workflow);
        match bus
            .commit(proposal)
            .map_err(|error| store_fault("bus::commit-failed", &error.to_string()))?
        {
            CommitOutcome::Committed(_) => Ok(Fired::DeferredTaskWoken(task)),
            CommitOutcome::Refused(refusal) if refusal.reason().starts_with("task::deferral-") => {
                Ok(Fired::Stale)
            }
            CommitOutcome::Refused(refusal) => Err(store_fault(
                "task::deferral-wake-commit-refused",
                refusal.reason(),
            )),
            CommitOutcome::Fault(fault) => {
                Err(store_fault("journal::commit-failed", fault.detail()))
            }
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
        let reclaim_horizon = shifted(
            at,
            std::time::Duration::from_secs(crate::board_command::AUTOMATIC_MAXIMUM_SECONDS as u64),
        )?;
        let maximum = std::cmp::max(reclaim_horizon, armed.authority.lease.body().maximum);
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
    loop {
        let authority = bus
            .task_snapshot()
            .map_err(|error| store_fault("bus::timer-snapshot-failed", &error.to_string()))?;
        let (steering, horizon) = bus
            .read(|read| Ok((crate::steering_read::pending(read, None)?, read.cursor()?)))
            .map_err(|error| store_fault("bus::timer-snapshot-failed", &error.to_string()))?;
        if horizon != authority.cursor() {
            continue;
        }
        return Ok(TimerSnapshot {
            tasks: authority.tasks().to_vec(),
            residency: authority.residency().cloned(),
            steering,
            horizon,
        });
    }
}

impl TimerFacts {
    /// Extend the timer's immutable authority stream only through one captured
    /// cursor. Full history is decoded once at startup; every later fold reads
    /// only facts appended since the last synchronized cursor.
    fn synchronize(&mut self, bus: &Bus, target: Option<wire::Seq>) -> CommandResult<()> {
        let Some(target) = target else {
            return Ok(());
        };
        if self
            .horizon
            .is_some_and(|horizon| horizon.get() >= target.get())
        {
            return Ok(());
        }
        let reader = bus.reader();
        while self.horizon.is_none_or(|horizon| horizon < target) {
            let page = reader
                .canvas_facts(
                    journal::FactPageStart::After(self.horizon),
                    target,
                    journal::FactReadBudget::RETAINED,
                )
                .map_err(|error| store_fault("bus::timer-facts-failed", &error.to_string()))?;
            let Some(last) = page.rows.last().map(|fact| fact.seq) else {
                return Err(store_fault(
                    "bus::timer-facts-incomplete",
                    &format!("fact page ended before captured timer horizon {target}"),
                ));
            };
            self.rows.extend(page.rows);
            self.horizon = Some(last);
        }
        Ok(())
    }
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

fn desired_deferrals(
    tasks: &[journal::TaskHeads],
    projects: &ProjectPolicy,
) -> CommandResult<BTreeMap<TaskHandle, DeferralAuthority>> {
    let mut desired = BTreeMap::new();
    for heads in tasks {
        let workflow = heads.workflow();
        let Some(deadline) = workflow.body().deferral else {
            continue;
        };
        if workflow.body().completion.is_some() {
            continue;
        }
        let statement = heads.statement().body();
        let project = projects.admit(&statement.project).map_err(|error| {
            store_fault(
                "bus::timer-project-invalid",
                &format!("task {} has invalid project: {error}", statement.born),
            )
        })?;
        if project.visibility().is_hidden() {
            continue;
        }
        let task = TaskHandle::for_project(&statement.project, &statement.born);
        desired.insert(
            task.clone(),
            DeferralAuthority {
                task,
                stamp: statement.born.clone(),
                workflow: workflow.clone(),
                deadline,
            },
        );
    }
    Ok(desired)
}

fn deferral_guard(
    read: &journal::JournalRead<'_>,
    expected: &DeferralAuthority,
    at: Instant,
) -> Result<(), GuardError> {
    if at < expected.deadline {
        return Err(GuardError::refused("task::deferral-before-deadline"));
    }
    let Some(heads) = read.task_heads_for(&expected.stamp)? else {
        return Err(GuardError::refused("task::deferral-task-moved"));
    };
    if heads.workflow() != &expected.workflow {
        return Err(GuardError::refused("task::deferral-authority-moved"));
    }
    Ok(())
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
        if spice::claim_lease::is_indefinite(lease.body()) || lease.body().expiry <= now {
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
