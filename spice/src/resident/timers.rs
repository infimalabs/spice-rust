//! Resident-owned fold of durable deadlines into process-local waits.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::thread::{self, JoinHandle};

use bus::timer::{TimerEvent, TimerId, TimerKind, TimerWorker};
use bus::{Bus, CommitOutcome, JournalTransaction};
use journal::{FactCause, GuardError, JournalGuard, JournalWrite};
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, Instant, Producer, ResidentState, TaskHandle, intent,
};

use super::supervisor::command::ClaimLapseNotice;
use super::{CommandResult, environment_fault, new_uuid, steering, store_fault};

mod rooms;

use rooms::RoomAuthority;
pub(super) use rooms::RoomDue;

/// Process-local waits mirror durable claim, steering, lifecycle, and room
/// authority.
/// Each fired event re-proves its captured durable state before appending, so
/// restart catch-up and stale races use the same guarded transition path.
pub(super) struct Runtime {
    worker: Option<TimerWorker>,
    events: tokio::sync::mpsc::UnboundedReceiver<TimerEvent>,
    forwarder: Option<JoinHandle<()>>,
    steering: steer::Config,
    /// Root of per-lane supervisor records whose kept jobs extend a vacated
    /// lane's claim.
    supervisors: PathBuf,
    deferrals: BTreeMap<TaskHandle, ArmedDeferral>,
    claims: BTreeMap<TaskHandle, ArmedClaim>,
    renewals: BTreeMap<TaskHandle, ArmedRenewal>,
    directives: BTreeMap<CanonicalUuid, ArmedDirective>,
    wake: Option<ArmedWake>,
    rooms: BTreeMap<RoomDue, ArmedRoom>,
    horizon: Option<wire::Seq>,
}

struct TimerSnapshot {
    authority: bus::TaskSnapshot,
    steering: Vec<journal::JournalHead<wire::SteeringHead>>,
    items: Vec<(
        wire::rooms::RoomReceiptRecord,
        journal::JournalHead<wire::rooms::RoomReceiptHead>,
    )>,
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
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    epoch: CanonicalUuid,
    expiry: Instant,
}

/// One claim this resident just lapsed, with the lane that held it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct LapsedClaim {
    lane: CanonicalUuid,
    task: TaskHandle,
    expiry: Instant,
}

impl LapsedClaim {
    /// Address the lapse to the holding lane's current occupant, which a
    /// restart may have re-seated since the claim was won; a vacant lane
    /// hears nothing.
    pub(super) fn notice(
        &self,
        residency: &wire::ResidencyHead,
    ) -> Option<(CanonicalUuid, ClaimLapseNotice)> {
        let occupant = residency
            .lanes
            .iter()
            .find(|durable| durable.lane == self.lane)?
            .occupant?;
        Some((
            self.lane,
            ClaimLapseNotice {
                occupant,
                task: self.task.to_string(),
                expiry: self.expiry,
            },
        ))
    }
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
    /// Furthest expiry this renewal may grant, present only while the claim's
    /// lane is vacant and an unfinished deadline-bounded job of the lane runs.
    bound: Option<Instant>,
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

struct ArmedRoom {
    id: TimerId,
    authority: RoomAuthority,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Fired {
    Stale,
    DeferredTaskWoken(TaskHandle),
    /// A claim lapse fired; `Some` names the claim it committed, `None` a
    /// lapse whose captured heads had already moved.
    ClaimLapse(Option<LapsedClaim>),
    ClaimRenew,
    Steering,
    TaskWake(CanonicalUuid),
    LaneResume(CanonicalUuid),
    /// An operator item lapsed.
    Room,
}

impl Runtime {
    pub(super) fn start(steering: steer::Config, supervisors: PathBuf) -> CommandResult<Self> {
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
            steering,
            supervisors,
            deferrals: BTreeMap::new(),
            claims: BTreeMap::new(),
            renewals: BTreeMap::new(),
            directives: BTreeMap::new(),
            wake: None,
            rooms: BTreeMap::new(),
            horizon: None,
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
        self.horizon = snapshot.horizon;
        let tasks = snapshot.authority.tasks();
        let residency = snapshot.authority.residency();
        let desired_deferrals = desired_deferrals(tasks);
        let desired = desired_claims(tasks, residency)?;
        let supervisors = &self.supervisors;
        let desired_renewals =
            desired_renewals(tasks, residency, |lane| vacated_bound(supervisors, lane))?;
        let desired_directives = desired_directives(&snapshot.steering, &self.steering)?;
        let desired_rooms = rooms::desired(&snapshot.items, residency);
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
        self.synchronize_rooms(desired_rooms)?;
        Ok(())
    }

    fn synchronize_rooms(
        &mut self,
        desired: BTreeMap<RoomDue, RoomAuthority>,
    ) -> CommandResult<()> {
        let stale = self
            .rooms
            .iter()
            .filter_map(|(due, armed)| {
                (desired.get(due) != Some(&armed.authority)).then_some(due.clone())
            })
            .collect::<Vec<_>>();
        for due in stale {
            let armed = self
                .rooms
                .remove(&due)
                .expect("stale room deadline came from the armed map");
            self.worker()?
                .cancel(armed.id)
                .map_err(|error| store_fault("bus::timer-cancel-failed", &error.to_string()))?;
        }
        for (due, authority) in desired {
            if self.rooms.contains_key(&due) {
                continue;
            }
            self.arm_room(authority)?;
        }
        Ok(())
    }

    fn arm_room(&mut self, authority: RoomAuthority) -> CommandResult<()> {
        let id = self
            .worker()?
            .arm(
                authority.due.owner(),
                authority.due.kind(),
                authority.deadline,
            )
            .map_err(|error| store_fault("bus::timer-arm-failed", &error.to_string()))?;
        self.rooms
            .insert(authority.due.clone(), ArmedRoom { id, authority });
        Ok(())
    }

    /// Whether a queued notification is already represented by the last fold.
    ///
    /// Synchronization reads through the writer's current cursor, which can
    /// cover several notifications already waiting in the broadcast channel.
    /// Replaying each of those notifications would fold the same authority
    /// again without observing anything newer.
    pub(super) fn covers(&self, commit: bus::JournalCommit) -> bool {
        self.horizon
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
            TimerKind::RoomTimeout | TimerKind::RoomLapse => self.consume_room(bus, event),
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
            command: new_uuid(),
            occupant: armed.authority.occupant,
            epoch: armed.authority.epoch,
            expiry: armed.authority.expiry,
            at,
            intent: intent::TaskRef { task: task.clone() },
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
            Ok(_) => Ok(Fired::ClaimLapse(Some(LapsedClaim {
                lane: armed.authority.lane,
                task,
                expiry: armed.authority.expiry,
            }))),
            Err(board::lapse::LapseError::Refused(_)) => Ok(Fired::ClaimLapse(None)),
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
        self.renew(bus, armed.authority, at)
    }

    /// Consume the captured renewal at its own deadline without waiting on wall time.
    #[cfg(test)]
    pub(super) fn consume_renewal_at_deadline(
        &mut self,
        bus: &Bus,
        stamp: &wire::TaskStamp,
    ) -> CommandResult<Fired> {
        let Some(task) = self
            .renewals
            .keys()
            .find(|task| task.born().as_ref().is_ok_and(|born| born == stamp))
            .cloned()
        else {
            return Ok(Fired::Stale);
        };
        let armed = self
            .renewals
            .remove(&task)
            .expect("selected renewal remains armed");
        let at = armed.authority.deadline;
        self.renew(bus, armed.authority, at)
    }

    fn renew(&self, bus: &Bus, authority: RenewalAuthority, at: Instant) -> CommandResult<Fired> {
        let supervisors = &self.supervisors;
        renew_claim(bus, authority, at, |lane| vacated_bound(supervisors, lane))
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

    fn consume_room(&mut self, bus: &Bus, event: TimerEvent) -> CommandResult<Fired> {
        let Some(due) = self
            .rooms
            .iter()
            .find_map(|(due, armed)| (armed.id == event.id()).then(|| due.clone()))
        else {
            return Ok(Fired::Stale);
        };
        let armed = &self.rooms[&due];
        if event.owner() != due.owner()
            || event.kind() != due.kind()
            || event.deadline() != armed.authority.deadline
        {
            return Err(environment_fault(
                "bus::timer-authority-mismatch",
                "fired timer identity disagreed with its captured room authority",
            ));
        }
        let at = Instant::new(jiff::Timestamp::now())
            .map_err(|error| environment_fault("bus::timer-clock-invalid", &error.to_string()))?;
        self.fire_room(bus, &due, at)
    }

    /// Settle one armed room deadline at `at`. A wait that fires before its
    /// wall-clock deadline re-arms for it rather than dropping it.
    fn fire_room(&mut self, bus: &Bus, due: &RoomDue, at: Instant) -> CommandResult<Fired> {
        let Some(armed) = self.rooms.remove(due) else {
            return Ok(Fired::Stale);
        };
        if at < armed.authority.deadline {
            self.arm_room(armed.authority)?;
            return Ok(Fired::Stale);
        }
        if rooms::settle(bus, &armed.authority, at)? {
            Ok(Fired::Room)
        } else {
            Ok(Fired::Stale)
        }
    }

    /// Fire the armed room deadline of `due` as if the wall clock read `at`.
    #[cfg(test)]
    pub(super) fn fire_room_at(
        &mut self,
        bus: &Bus,
        due: &RoomDue,
        at: Instant,
    ) -> CommandResult<Fired> {
        self.fire_room(bus, due, at)
    }

    /// The deadline armed for `due`, absent when nothing is armed for it.
    #[cfg(test)]
    pub(super) fn room_deadline(&self, due: &RoomDue) -> Option<Instant> {
        self.rooms.get(due).map(|armed| armed.authority.deadline)
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

fn renew_claim(
    bus: &Bus,
    authority: RenewalAuthority,
    at: Instant,
    vacated_bound: impl Fn(CanonicalUuid) -> Option<Instant>,
) -> CommandResult<Fired> {
    if at < authority.deadline || at >= authority.lease.body().expiry {
        return Ok(Fired::Stale);
    }
    let rolling = shifted(
        at,
        std::time::Duration::from_secs(crate::board_command::AUTOMATIC_ROLLING_SECONDS as u64),
    )?;
    let mut until = std::cmp::max(rolling, authority.lease.body().expiry);
    if authority.bound.is_some() {
        // A vacated lane's claim rolls only on the job still running now,
        // never past one rolling window beyond that job's delivered ending.
        match vacated_bound(authority.lane) {
            Some(bound) if bound > authority.lease.body().expiry => {
                until = std::cmp::min(until, bound);
            }
            _ => return Ok(Fired::Stale),
        }
    }
    let reclaim_horizon = shifted(
        at,
        std::time::Duration::from_secs(crate::board_command::AUTOMATIC_MAXIMUM_SECONDS as u64),
    )?;
    let maximum = std::cmp::max(reclaim_horizon, authority.lease.body().maximum);
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
        CommitOutcome::Fault(fault) => Err(store_fault("journal::commit-failed", fault.detail())),
    }
}

fn timer_snapshot(bus: &Bus) -> CommandResult<TimerSnapshot> {
    loop {
        let authority = bus
            .task_snapshot()
            .map_err(|error| store_fault("bus::timer-snapshot-failed", &error.to_string()))?;
        let (steering, items, horizon) = bus
            .read(|read| {
                Ok((
                    crate::steering_read::pending(read, None)?,
                    read.open_room_items()?,
                    read.cursor()?,
                ))
            })
            .map_err(|error| store_fault("bus::timer-snapshot-failed", &error.to_string()))?;
        if horizon != authority.cursor() {
            continue;
        }
        return Ok(TimerSnapshot {
            authority,
            steering,
            items,
            horizon,
        });
    }
}

/// Renew one vacated claim against the delivered job horizon supplied by an
/// in-process test, exercising the production bound, authority, guard, and
/// durable transition.
#[cfg(test)]
pub(in crate::resident) fn renew_vacated_claim_at(
    bus: &Bus,
    stamp: &wire::TaskStamp,
    at: Instant,
    delivered: Instant,
) -> CommandResult<Fired> {
    let bound = vacated_reclaim_bound(delivered)?;
    let snapshot = timer_snapshot(bus)?;
    let authority = desired_renewals_with_clock(
        snapshot.authority.tasks(),
        snapshot.authority.residency(),
        |_| Some(bound),
        || Ok(at),
    )?
    .into_values()
    .find(|authority| &authority.stamp == stamp)
    .ok_or_else(|| {
        store_fault(
            "claim::renewal-test-unavailable",
            "the fixture claim did not produce vacated renewal authority",
        )
    })?;
    renew_claim(bus, authority, at, |_| Some(bound))
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

fn desired_deferrals(tasks: &[journal::TaskHeads]) -> BTreeMap<TaskHandle, DeferralAuthority> {
    let mut desired = BTreeMap::new();
    for heads in tasks {
        let workflow = heads.workflow();
        let Some(deadline) = workflow.body().deferral else {
            continue;
        };
        if !workflow.body().status.is_pending() {
            continue;
        }
        let statement = heads.statement().body();
        if statement.filing.is_draft() {
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
    desired
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
                lane: claim.lane,
                occupant: claim.occupant,
                epoch: claim.epoch,
                expiry: lease.body().expiry,
            },
        );
    }
    Ok(desired)
}

/// Roll every live lease whose claim is held by a lane's current occupant, or
/// by the last occupant of a lane left vacant while one of its
/// deadline-bounded jobs still runs.
///
/// Liveness, not the way the claim was minted, decides renewal: a claim won
/// through allocation rolls exactly like a resident-minted automatic one while
/// its occupant remains seated, so a lapse follows the occupant leaving rather
/// than one long foreground command outliving a fixed lease. An occupant that
/// ends its turn on a kept job leaves the lease rolling as `vacated_bound`
/// bounds it; a service never qualifies.
fn desired_renewals(
    tasks: &[journal::TaskHeads],
    residency: Option<&journal::JournalHead<wire::ResidencyHead>>,
    vacated_bound: impl Fn(CanonicalUuid) -> Option<Instant>,
) -> CommandResult<BTreeMap<TaskHandle, RenewalAuthority>> {
    desired_renewals_with_clock(tasks, residency, vacated_bound, || {
        Instant::new(jiff::Timestamp::now())
            .map_err(|error| environment_fault("bus::timer-clock-invalid", &error.to_string()))
    })
}

fn desired_renewals_with_clock(
    tasks: &[journal::TaskHeads],
    residency: Option<&journal::JournalHead<wire::ResidencyHead>>,
    vacated_bound: impl Fn(CanonicalUuid) -> Option<Instant>,
    clock: impl FnOnce() -> CommandResult<Instant>,
) -> CommandResult<BTreeMap<TaskHandle, RenewalAuthority>> {
    let Some(residency) = residency else {
        return Ok(BTreeMap::new());
    };
    if residency.body().state != ResidentState::Seated {
        return Ok(BTreeMap::new());
    }
    let now = clock()?;
    let mut desired = BTreeMap::new();
    for heads in tasks {
        let Some(claim) = heads.claim() else { continue };
        if claim.body().disposition != ClaimDisposition::Active
            || claim.body().epoch != residency.body().epoch
        {
            continue;
        }
        let Some(seat) = renewal_seat(residency.body(), claim.body()) else {
            continue;
        };
        let lease = coherent_lease(heads, claim.body())?;
        if spice::claim_lease::is_indefinite(lease.body()) || lease.body().expiry <= now {
            continue;
        }
        let bound = match seat {
            RenewalSeat::Occupied => None,
            RenewalSeat::Vacated => match vacated_bound(claim.body().lane) {
                Some(bound) if bound > lease.body().expiry => Some(bound),
                _ => continue,
            },
        };
        let deadline = renewal_deadline(lease.body())?;
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
                bound,
            },
        );
    }
    Ok(desired)
}

/// Deadline at which one live lease is due for its next rolling renewal.
///
/// An automatic lease sits exactly one heartbeat inside its rolling horizon, so
/// the horizon-relative term reproduces its current heartbeat; a longer
/// lane-held lease waits until it enters that same last rolling window instead
/// of committing heartbeats that would extend nothing.
fn renewal_deadline(lease: &wire::LeaseHead) -> CommandResult<Instant> {
    let entering = shifted_signed(
        lease.expiry,
        jiff::SignedDuration::from_secs(-crate::board_command::AUTOMATIC_ROLLING_SECONDS),
    )?;
    shifted_signed(
        std::cmp::max(lease.renewal, entering),
        jiff::SignedDuration::from_secs(crate::board_command::AUTOMATIC_HEARTBEAT_SECONDS),
    )
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
    let expected_seat = if expected.bound.is_some() {
        RenewalSeat::Vacated
    } else {
        RenewalSeat::Occupied
    };
    if residency.body().state != ResidentState::Seated
        || residency.body().epoch != expected.epoch
        || renewal_seat(residency.body(), expected.claim.body()) != Some(expected_seat)
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

/// How a claim's lane relates to the occupant holding it, for renewal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RenewalSeat {
    /// The lane seats the claim's occupant.
    Occupied,
    /// The lane is vacant and its last occupant holds the claim.
    Vacated,
}

/// The claim's lane seat, absent when another occupant holds the lane or the
/// lane is gone.
fn renewal_seat(residency: &wire::ResidencyHead, claim: &wire::ClaimHead) -> Option<RenewalSeat> {
    let lane = residency
        .lanes
        .iter()
        .find(|lane| lane.lane == claim.lane)?;
    match lane.occupant {
        Some(occupant) => (occupant == claim.occupant).then_some(RenewalSeat::Occupied),
        None => (lane.lineage.last() == Some(&claim.occupant)).then_some(RenewalSeat::Vacated),
    }
}

/// The furthest a vacated lane's lease may roll: one rolling window past the
/// instant its unfinished deadline-bounded jobs have all ended and the keeper
/// wakes the lane to deliver them, so the successor that wake seats still
/// finds the claim live and promotes it. Unreadable job records extend
/// nothing, so the claim lapses on the lease it already carries.
fn vacated_bound(supervisors: &std::path::Path, lane: CanonicalUuid) -> Option<Instant> {
    let delivered =
        super::supervisor::terminal::Jobs::beneath(supervisors, lane).finite_horizon()?;
    vacated_reclaim_bound(delivered).ok()
}

fn vacated_reclaim_bound(delivered: Instant) -> CommandResult<Instant> {
    shifted(
        delivered,
        std::time::Duration::from_secs(crate::board_command::AUTOMATIC_ROLLING_SECONDS as u64),
    )
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
    shifted_signed(at, duration)
}

fn shifted_signed(at: Instant, duration: jiff::SignedDuration) -> CommandResult<Instant> {
    let shifted = at
        .get()
        .checked_add(duration)
        .map_err(|error| environment_fault("steer::horizon-invalid", &error.to_string()))?;
    Instant::new(shifted)
        .map_err(|error| environment_fault("steer::horizon-invalid", &error.to_string()))
}

#[cfg(test)]
mod tests;
