//! Resident-owned projection of durable deadlines into process-local waits.

use std::collections::BTreeMap;
use std::thread::{self, JoinHandle};

use board::ProjectPolicy;
use bus::timer::{TimerEvent, TimerId, TimerKind, TimerWorker};
use bus::{Bus, JournalTransaction};
use wire::{CanonicalUuid, ClaimDisposition, Instant, ResidentState, TaskHandle, intent};

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
    claims: BTreeMap<TaskHandle, ArmedClaim>,
    directives: BTreeMap<CanonicalUuid, ArmedDirective>,
    wake: Option<ArmedWake>,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Fired {
    Stale,
    ClaimLapse,
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
            claims: BTreeMap::new(),
            directives: BTreeMap::new(),
            wake: None,
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
        let snapshot = bus
            .read(|read| {
                Ok((
                    read.task_heads()?,
                    read.residency_head()?,
                    read.pending_steering(None)?,
                ))
            })
            .map_err(|error| store_fault("bus::timer-snapshot-failed", &error.to_string()))?;
        let desired = desired_claims(&snapshot.0, snapshot.1.as_ref())?;
        let desired_directives = desired_directives(&snapshot.2, &self.steering)?;
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

    /// Consume one fired event using only authority captured when it was armed.
    pub(super) fn consume(&mut self, bus: &Bus, event: TimerEvent) -> CommandResult<Fired> {
        match event.kind() {
            TimerKind::TaskWake | TimerKind::LaneResume => self.consume_lifecycle(event),
            TimerKind::SteerEscalate | TimerKind::SteerLapse => {
                self.consume_steering(bus, event).map(|()| Fired::Steering)
            }
            TimerKind::ClaimLapse => self.consume_claim(bus, event),
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
        if claim.disposition != ClaimDisposition::Active || claim.epoch != residency.body().epoch {
            continue;
        }
        if !residency.body().lanes.iter().any(|seat| {
            seat.lane == claim.lane
                && seat.disposition == "occupied"
                && seat.occupant == Some(claim.occupant)
        }) {
            continue;
        }
        let lease = heads.lease().ok_or_else(|| {
            environment_fault(
                "bus::timer-lease-missing",
                &format!(
                    "active task {} has no durable lease head",
                    wire::TaskHandle::for_project(
                        &heads.statement().body().project,
                        &heads.statement().body().incepted,
                    )
                ),
            )
        })?;
        if lease.body().epoch != claim.epoch {
            return Err(environment_fault(
                "bus::timer-lease-epoch",
                &format!(
                    "task {} claim and lease epochs disagree",
                    wire::TaskHandle::for_project(
                        &heads.statement().body().project,
                        &heads.statement().body().incepted,
                    )
                ),
            ));
        }
        if !(lease.body().start <= lease.body().renewal
            && lease.body().renewal <= lease.body().expiry
            && lease.body().expiry <= lease.body().maximum)
        {
            return Err(environment_fault(
                "bus::timer-lease-incoherent",
                &format!(
                    "task {} lease does not satisfy start <= renewal <= expiry <= maximum",
                    wire::TaskHandle::for_project(
                        &heads.statement().body().project,
                        &heads.statement().body().incepted,
                    )
                ),
            ));
        }
        let task = wire::TaskHandle::for_project(
            &heads.statement().body().project,
            &heads.statement().body().incepted,
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
mod tests {
    use board::Entry;
    use journal::{FactCause, Journal, JournalWrite, PendingFact, TaskHeads};
    use steer::Standing as _;
    use wire::body::GitBoundary;
    use wire::{
        Accepted, Actor, ClaimHead, Command, CommandKind, LaneResidency, LeaseHead, Origin,
        Outcome, Phase, Producer, Reply, ReplyPosition, ResidencyHead, TaskHead, WireVersion,
        Witness, WorkflowHead, body,
    };

    use super::*;

    struct Fixture {
        bus: Bus,
        projects: ProjectPolicy,
        authority: Authority,
    }

    struct Authority {
        task: TaskHandle,
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
        epoch: CanonicalUuid,
        start: Instant,
        expiry: Instant,
        maximum: Instant,
    }

    impl Authority {
        fn incepted(&self) -> wire::TaskStamp {
            self.task.incepted().expect("fixture task carries a stamp")
        }
    }

    #[test]
    fn expired_commit_bearing_claim_lapses_and_returns_to_allocation() {
        let fixture = fixture();
        let commits = fixture
            .bus
            .subscribe()
            .expect("lapse commit subscription registers");
        let mut timers = Runtime::start(fixture.projects.clone(), steer::Config::default())
            .expect("timer runtime starts");
        timers
            .synchronize(&fixture.bus)
            .expect("expired claim arms");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("fixture async runtime builds");
        let fired = runtime
            .block_on(timers.recv())
            .expect("expired claim timer fires");
        timers
            .consume(&fixture.bus, fired)
            .expect("expired claim lapses");
        timers
            .synchronize(&fixture.bus)
            .expect("lapsed claim disarms");

        let committed = commits.recv().expect("lapse broadcasts after durability");
        assert_eq!(committed.first(), committed.last());
        let heads = lapsed_heads(&fixture);
        assert_lapsed_and_eligible(&fixture, &heads);
        capture_lapsed_task(&fixture, &heads);
        assert_commit_history(&fixture.bus);
        drop(timers);
        fixture.bus.shutdown().expect("fixture bus stops");
    }

    #[test]
    fn task_wake_is_an_explicit_timer_event_for_the_selected_lane() {
        let bus = Bus::start(Journal::in_memory().expect("fixture journal opens"))
            .expect("fixture bus starts");
        let mut timers =
            Runtime::start(projects(), steer::Config::default()).expect("timer runtime starts");
        let now = Instant::new(jiff::Timestamp::now()).expect("fixture clock is canonical");
        let lane = uuid(41);
        timers
            .synchronize_wake(Some(super::super::lifecycle::WakeDeadline {
                lane,
                at: shifted(now, -1),
                kind: super::super::lifecycle::WakeKind::Task,
            }))
            .expect("past task wake arms");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("fixture async runtime builds");
        let event = runtime
            .block_on(timers.recv())
            .expect("task wake timer fires");
        assert_eq!(event.kind(), TimerKind::TaskWake);
        assert_eq!(event.owner(), lane.to_string());
        assert_eq!(
            timers.consume(&bus, event).expect("task wake consumes"),
            Fired::TaskWake(lane)
        );
        drop(timers);
        bus.shutdown().expect("fixture bus stops");
    }

    #[test]
    fn lane_resume_is_an_explicit_timer_event_for_the_held_lane() {
        let bus = Bus::start(Journal::in_memory().expect("fixture journal opens"))
            .expect("fixture bus starts");
        let mut timers =
            Runtime::start(projects(), steer::Config::default()).expect("timer runtime starts");
        let now = Instant::new(jiff::Timestamp::now()).expect("fixture clock is canonical");
        let lane = uuid(42);
        timers
            .synchronize_wake(Some(super::super::lifecycle::WakeDeadline {
                lane,
                at: shifted(now, -1),
                kind: super::super::lifecycle::WakeKind::Hold,
            }))
            .expect("past lane resume arms");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("fixture async runtime builds");
        let event = runtime
            .block_on(timers.recv())
            .expect("lane resume timer fires");
        assert_eq!(event.kind(), TimerKind::LaneResume);
        assert_eq!(
            timers.consume(&bus, event).expect("lane resume consumes"),
            Fired::LaneResume(lane)
        );
        drop(timers);
        bus.shutdown().expect("fixture bus stops");
    }

    #[test]
    fn an_expired_silence_horizon_moves_one_stable_directive_and_rearms_the_next() {
        let (bus, directive, key) = steering_fixture();
        let mut timers =
            Runtime::start(projects(), steer::Config::default()).expect("timer runtime starts");
        timers.synchronize(&bus).expect("steering horizon arms");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("fixture async runtime builds");
        let fired = runtime
            .block_on(timers.recv())
            .expect("past reminder horizon fires");
        assert_eq!(fired.kind(), TimerKind::SteerEscalate);
        timers
            .consume(&bus, fired)
            .expect("reminder transition commits");
        timers.synchronize(&bus).expect("next horizon re-arms");

        let snapshot = bus.fact_snapshot().expect("steering facts read");
        let ledger = steer::Ledger::from_snapshot(&snapshot).expect("steering replays");
        let standing = ledger
            .directive(directive)
            .expect("stable directive remains");
        assert_eq!(standing.key(), &key);
        assert_eq!(standing.stage(), wire::body::SteeringStage::Reminded);
        assert_eq!(standing.escalations().len(), 1);
        assert_eq!(ledger.directives().len(), 1);
        let armed = timers
            .directives
            .get(&directive)
            .expect("escalated horizon is armed next");
        assert!(matches!(
            armed.authority.body.transition,
            steering::DeadlineTransition::Escalate {
                from: wire::body::SteeringStage::Reminded,
                to: wire::body::SteeringStage::Escalated,
            }
        ));
        drop(timers);
        bus.shutdown().expect("fixture bus stops");
    }

    #[test]
    fn retirement_winning_the_race_makes_a_fired_escalation_append_nothing() {
        let (bus, directive, key) = steering_fixture();
        let mut timers =
            Runtime::start(projects(), steer::Config::default()).expect("timer runtime starts");
        timers.synchronize(&bus).expect("steering horizon arms");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("fixture async runtime builds");
        let fired = runtime
            .block_on(timers.recv())
            .expect("past reminder horizon fires");

        let at = Instant::new(jiff::Timestamp::now()).expect("fixture clock");
        let current = bus
            .read(move |read| read.steering_head(directive))
            .expect("indexed directive reads")
            .expect("indexed directive exists");
        let mut retirement = JournalWrite::new(
            Actor::Lane(lane_uuid()),
            format!("resident:{}", epoch_uuid())
                .parse()
                .expect("fixture origin"),
            at,
            Producer::AckScanner,
        );
        let body = body::SteerAcked {
            directive,
            key: key.to_string(),
            lane: lane_uuid(),
            epoch: epoch_uuid(),
            locus: body::TranscriptLocus {
                source: None,
                member: 1,
                offset: 2,
                length: 3,
                event: 0,
                line: 1,
                column: 1,
            },
        };
        let pending = retirement
            .fact(body.clone(), FactCause::Root)
            .expect("retirement fact builds");
        let next =
            steer::Ledger::transition_head(current.body(), steer::Steering::Acked(&body), at)
                .expect("retirement head builds");
        retirement.steering_head(pending, move |_| next);
        assert!(matches!(
            bus.commit(retirement).expect("retirement submits"),
            bus::CommitOutcome::Committed(_)
        ));

        timers
            .consume(&bus, fired)
            .expect("stale fired transition settles harmlessly");
        timers.synchronize(&bus).expect("retired timer disarms");
        let snapshot = bus.fact_snapshot().expect("steering facts read");
        assert_eq!(
            snapshot
                .facts()
                .iter()
                .filter(|fact| fact.fact.kind() == wire::FactKind::SteerEscalated)
                .count(),
            0
        );
        let ledger = steer::Ledger::from_snapshot(&snapshot).expect("steering replays");
        assert_eq!(ledger.disposition(&key), Some(wire::Disposition::Acked));
        assert!(!timers.directives.contains_key(&directive));
        drop(timers);
        bus.shutdown().expect("fixture bus stops");
    }

    #[test]
    fn overdue_startup_catches_up_all_missing_stages_in_order_on_one_identity() {
        let (bus, directive, key) = steering_fixture_with_age(600);
        let mut timers =
            Runtime::start(projects(), steer::Config::default()).expect("timer runtime starts");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("fixture async runtime builds");
        for expected in [
            wire::body::SteeringStage::Reminded,
            wire::body::SteeringStage::Escalated,
            wire::body::SteeringStage::Overdue,
        ] {
            timers.synchronize(&bus).expect("next overdue horizon arms");
            let fired = runtime
                .block_on(timers.recv())
                .expect("overdue horizon fires");
            timers
                .consume(&bus, fired)
                .expect("ordered transition commits");
            let snapshot = bus.fact_snapshot().expect("steering facts read");
            let ledger = steer::Ledger::from_snapshot(&snapshot).expect("steering replays");
            let standing = ledger
                .directive(directive)
                .expect("stable directive remains");
            assert_eq!(standing.key(), &key);
            assert_eq!(standing.stage(), expected);
        }
        timers
            .synchronize(&bus)
            .expect("expiry re-arms after overdue");
        let snapshot = bus.fact_snapshot().expect("steering facts read");
        let ledger = steer::Ledger::from_snapshot(&snapshot).expect("steering replays");
        let standing = ledger
            .directive(directive)
            .expect("stable directive remains");
        assert_eq!(standing.escalations().len(), 3);
        assert_eq!(ledger.directives().len(), 1);
        assert!(matches!(
            timers
                .directives
                .get(&directive)
                .expect("expiry is armed")
                .authority
                .body
                .transition,
            steering::DeadlineTransition::Lapse
        ));
        drop(timers);
        bus.shutdown().expect("fixture bus stops");
    }

    fn steering_fixture() -> (Bus, CanonicalUuid, steer::Key) {
        steering_fixture_with_age(30)
    }

    fn steering_fixture_with_age(age_seconds: i64) -> (Bus, CanonicalUuid, steer::Key) {
        let now = Instant::new(jiff::Timestamp::now()).expect("fixture clock");
        let published = shifted(now, -age_seconds);
        let expires = shifted(now, 3600);
        let directive = uuid(80);
        let key: steer::Key = "1kJK6N3R".parse().expect("fixture key");
        let origin: wire::Origin = format!("resident:{}", epoch_uuid())
            .parse()
            .expect("fixture origin");
        let mut write = JournalWrite::new(
            Actor::Operator,
            origin.clone(),
            published,
            Producer::CommandDoor,
        );
        let body = body::SteerSent {
            directive,
            key: key.to_string(),
            lane: lane_uuid(),
            text: "hold the stable line".to_owned(),
            control: None,
            attachments: Vec::new(),
            expires,
        };
        let pending = write
            .fact(body.clone(), FactCause::Root)
            .expect("steering publication builds");
        write.steering_head(pending, move |seq| {
            steer::Ledger::sent_head(&body, origin, published, seq).expect("steering head builds")
        });
        let mut journal = Journal::in_memory().expect("fixture journal opens");
        journal.apply(write).expect("steering publication seeds");
        (
            Bus::start(journal).expect("fixture bus starts"),
            directive,
            key,
        )
    }

    fn lane_uuid() -> CanonicalUuid {
        uuid(81)
    }

    fn epoch_uuid() -> CanonicalUuid {
        uuid(82)
    }

    fn fixture() -> Fixture {
        let now = Instant::new(jiff::Timestamp::now()).expect("fixture clock is canonical");
        let authority = Authority {
            task: task(),
            lane: uuid(20),
            occupant: uuid(21),
            epoch: uuid(22),
            start: shifted(now, -2),
            expiry: shifted(now, -1),
            maximum: shifted(now, 60),
        };
        let mut journal = Journal::in_memory().expect("fixture journal opens");
        journal
            .apply(seed_write(&authority))
            .expect("fixture authority seeds");
        Fixture {
            bus: Bus::start(journal).expect("fixture bus starts"),
            projects: projects(),
            authority,
        }
    }

    fn seed_write(authority: &Authority) -> JournalWrite {
        let phase = "todo".parse::<Phase>().expect("fixture phase");
        let origin = format!("task:{}", authority.task)
            .parse::<Origin>()
            .expect("fixture origin");
        let mut write = JournalWrite::new(
            Actor::Lane(authority.lane),
            origin.clone(),
            authority.start,
            Producer::CommandDoor,
        );
        let added = write
            .fact(
                body::TaskAdded {
                    task: authority.incepted(),
                    title: "Capture commit after lapse".to_owned(),
                    description: None,
                    acceptance: Vec::new(),
                    project: "timer.claim".to_owned(),
                    tags: Vec::new(),
                    depends: Vec::new(),
                    flow: vec![phase.clone()],
                    phase: phase.clone(),
                    revision: 1,
                },
                FactCause::Root,
            )
            .expect("task fact builds");
        let won = write
            .fact(
                body::ClaimWon {
                    task: authority.incepted(),
                    actor: Actor::Lane(authority.lane),
                    lane: authority.lane,
                    occupant: authority.occupant,
                    epoch: authority.epoch,
                    cell: Some(wire::ClaimCell {
                        project: "timer.claim".to_owned(),
                        phase: phase.clone(),
                    }),
                    until: authority.expiry,
                    git: Some(GitBoundary {
                        head: "local-commit".to_owned(),
                        integrated: "local-commit".to_owned(),
                        upstream: Some("origin/main".to_owned()),
                        upstream_head: Some("baseline".to_owned()),
                        local_commits: 1,
                    }),
                    source: body::SourceBoundary::Proven { heads: Vec::new() },
                },
                FactCause::Pending(added),
            )
            .expect("claim fact builds");
        let seated = write
            .fact(
                body::UpSeated {
                    epoch: authority.epoch,
                },
                FactCause::Pending(won),
            )
            .expect("resident fact builds");
        attach_heads(&mut write, authority, phase, origin, added, won, seated);
        write
    }

    fn attach_heads(
        write: &mut JournalWrite,
        authority: &Authority,
        phase: Phase,
        origin: Origin,
        added: PendingFact,
        won: PendingFact,
        seated: PendingFact,
    ) {
        let statement_origin = origin;
        let statement_task = authority.incepted();
        let statement_phase = phase.clone();
        write.task_head(added, move |position| TaskHead {
            incepted: statement_task,
            queue_origin: position,
            title: "Capture commit after lapse".to_owned(),
            description: None,
            acceptance: Vec::new(),
            project: "timer.claim".to_owned(),
            tags: Vec::new(),
            dependencies: Vec::new(),
            provenance: statement_origin,
            revision: position,
        });
        let workflow_task = authority.incepted();
        write.workflow_head(workflow_task, added, move |_| WorkflowHead {
            flow: vec![statement_phase.clone()],
            phase: statement_phase,
            status: "pending".to_owned(),
            review: "unassigned".to_owned(),
            assignment: None,
            deferral: None,
            completion: None,
        });
        let claim_task = authority.incepted();
        let lane = authority.lane;
        let occupant = authority.occupant;
        let epoch = authority.epoch;
        write.claim_head(claim_task, won, move |position| ClaimHead {
            actor: Actor::Lane(lane),
            fact: position,
            claimed: position,
            cell: wire::ClaimCell {
                project: "timer.claim".to_owned(),
                phase,
            },
            lane,
            occupant,
            epoch,
            disposition: ClaimDisposition::Active,
        });
        let lease_task = authority.incepted();
        let start = authority.start;
        let expiry = authority.expiry;
        let maximum = authority.maximum;
        write.lease_head(lease_task, won, move |_| LeaseHead {
            start,
            renewal: start,
            expiry,
            maximum,
            epoch,
        });
        write.residency_head(seated, move |_| ResidencyHead {
            epoch,
            state: ResidentState::Seated,
            topology_revision: 1,
            lanes: vec![LaneResidency {
                lane,
                lineage_id: lane,
                name: None,
                worktree: "/fixture".to_owned(),
                life: "drive".to_owned(),
                vendor: "claude".parse().expect("fixture vendor"),
                occupant: Some(occupant),
                lineage: vec![occupant],
                capabilities: Vec::new(),
                session: None,
                disposition: "occupied".to_owned(),
            }],
            subscriptions: vec![wire::LineageSubscription {
                lineage_id: lane,
                policy: wire::intent::LineageLife::Drive,
                manual_projects: Vec::new(),
                auto_projects: Vec::new(),
            }],
        });
    }

    fn lapsed_heads(fixture: &Fixture) -> TaskHeads {
        fixture
            .bus
            .task_snapshot()
            .expect("lapsed heads read")
            .tasks()
            .iter()
            .find(|heads| heads.statement().body().incepted == fixture.authority.incepted())
            .expect("fixture task remains")
            .clone()
    }

    fn assert_lapsed_and_eligible(fixture: &Fixture, heads: &TaskHeads) {
        assert_eq!(
            heads
                .claim()
                .expect("claim head remains")
                .body()
                .disposition,
            ClaimDisposition::Lapsed
        );
        let (statement, workflow, claim, lease) = heads.clone().into_bodies();
        let entry = Entry::new(statement, workflow, claim, lease, &fixture.projects)
            .expect("lapsed task remains valid");
        assert!(
            board::eligible_entry(
                std::slice::from_ref(&entry),
                &entry,
                &Actor::Lane(fixture.authority.lane),
            ),
            "the commit-bearing task is eligible for stale-claim adoption"
        );
    }

    fn capture_lapsed_task(fixture: &Fixture, heads: &TaskHeads) {
        let capture = "capture".parse::<wire::Name>().expect("capture rule");
        let capture_command = Command {
            v: WireVersion::CURRENT,
            id: uuid(31),
            epoch: fixture.authority.epoch,
            kind: CommandKind::ClaimWin,
            expected: vec![
                Witness::Task {
                    task: fixture.authority.incepted(),
                    head: heads.statement().position(),
                },
                Witness::Workflow {
                    task: fixture.authority.incepted(),
                    head: heads.workflow().position(),
                    phase: heads.workflow().body().phase.clone(),
                },
                Witness::Claim {
                    task: fixture.authority.incepted(),
                    head: Some(heads.claim().expect("lapsed claim head").position()),
                },
                Witness::Eligibility {
                    rule: capture.clone(),
                },
                Witness::Residency {
                    lane: fixture.authority.lane,
                    occupant: fixture.authority.occupant,
                    epoch: fixture.authority.epoch,
                },
            ],
            body: serde_json::to_value(wire::intent::ClaimWin { rule: capture })
                .expect("capture intent encodes"),
        }
        .admit(&Accepted {
            wire: WireVersion::CURRENT,
            actor: Actor::Lane(fixture.authority.lane),
            epoch: fixture.authority.epoch,
        })
        .expect("capture command admits");
        let capture = crate::board_command::dispatch_with_repository(
            &fixture.bus,
            &capture_command,
            None,
            &fixture.projects,
            &board::NextConfig::default(),
        )
        .expect("board owns capture command");
        assert!(
            matches!(
                capture,
                Outcome::Reply(Reply {
                    position: ReplyPosition::Committed(_),
                    ..
                })
            ),
            "commit capture did not acquire the lapsed task: {capture:?}"
        );
    }

    fn assert_commit_history(bus: &Bus) {
        let facts = bus.fact_snapshot().expect("lapse facts read");
        assert!(
            facts
                .facts()
                .iter()
                .any(|fact| fact.fact.kind() == wire::FactKind::ClaimLapsed)
        );
        assert_eq!(
            facts
                .facts()
                .iter()
                .find(|fact| fact.fact.kind() == wire::FactKind::ClaimWon)
                .expect("winning claim fact remains")
                .fact
                .body()["git"]["local_commits"],
            1
        );
        assert_eq!(
            facts
                .facts()
                .iter()
                .filter(|fact| fact.fact.kind() == wire::FactKind::ClaimWon)
                .count(),
            2,
            "capture writes a new claim after the timer-owned lapse"
        );
    }

    fn projects() -> ProjectPolicy {
        ProjectPolicy::new(
            ["timer".to_owned()],
            Vec::new(),
            dialect::PROJECT_MIN_DEPTH as u64,
            dialect::PROJECT_MAX_DEPTH as u64,
        )
        .expect("fixture project policy")
    }

    fn task() -> TaskHandle {
        TaskHandle::for_project(
            "timer.claim",
            &"1kKRRsDP".parse().expect("fixture task stamp"),
        )
    }

    fn uuid(last: u8) -> CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131164{last:02}")
            .parse()
            .expect("fixture UUID")
    }

    fn shifted(at: Instant, seconds: i64) -> Instant {
        Instant::new(
            at.get()
                .checked_add(jiff::SignedDuration::from_secs(seconds))
                .expect("fixture instant is in range"),
        )
        .expect("shifted fixture instant is canonical")
    }
}
