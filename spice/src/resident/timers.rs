//! Resident-owned projection of durable deadlines into process-local waits.

use std::collections::BTreeMap;
use std::thread::{self, JoinHandle};

use board::ProjectPolicy;
use bus::timer::{TimerEvent, TimerId, TimerKind, TimerWorker};
use bus::{Bus, JournalTransaction};
use wire::{CanonicalUuid, ClaimDisposition, Instant, ResidentState, TaskHandle, intent};

use super::{CommandResult, environment_fault, new_uuid, store_fault};

/// The only currently armed production kind is `ClaimLapse`: it has a complete
/// guarded `BusTimer` transition. The other declared kinds remain unarmed
/// until their own durable transition contracts exist; they are never routed
/// through a user command as a fallback.
pub(super) struct Runtime {
    worker: Option<TimerWorker>,
    events: tokio::sync::mpsc::UnboundedReceiver<TimerEvent>,
    forwarder: Option<JoinHandle<()>>,
    projects: ProjectPolicy,
    claims: BTreeMap<TaskHandle, ArmedClaim>,
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

impl Runtime {
    pub(super) fn start(projects: ProjectPolicy) -> CommandResult<Self> {
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
            claims: BTreeMap::new(),
        })
    }

    pub(super) async fn recv(&mut self) -> Option<TimerEvent> {
        self.events.recv().await
    }

    /// Continuously reconcile durable commits and consume fired claim timers.
    pub(super) async fn drive(
        mut self,
        bus: &Bus,
        mut commits: bus::BroadcastSubscriber,
    ) -> CommandResult<()> {
        self.synchronize(bus)?;
        loop {
            tokio::select! {
                timer = self.recv() => {
                    let timer = timer.ok_or_else(|| environment_fault(
                        "bus::timer-events-closed",
                        "the production timer event stream closed while the resident was live",
                    ))?;
                    self.consume(bus, timer)?;
                    self.synchronize(bus)?;
                }
                committed = commits.recv() => match committed {
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        self.synchronize(bus)?;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return Err(environment_fault(
                            "bus::timer-commits-closed",
                            "the durable commit stream closed while the resident was live",
                        ));
                    }
                }
            }
        }
    }

    /// Make process-local waits exactly match current lapse-capable claim heads.
    pub(super) fn synchronize(&mut self, bus: &Bus) -> CommandResult<()> {
        let snapshot = bus
            .task_snapshot()
            .map_err(|error| store_fault("bus::timer-snapshot-failed", &error.to_string()))?;
        let desired = desired_claims(&snapshot)?;
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
        Ok(())
    }

    /// Consume one fired event using only authority captured when it was armed.
    pub(super) fn consume(&mut self, bus: &Bus, event: TimerEvent) -> CommandResult<()> {
        if event.kind() != TimerKind::ClaimLapse {
            return Err(environment_fault(
                "bus::timer-kind-unhandled",
                &format!("production received unarmed timer kind {:?}", event.kind()),
            ));
        }
        let Some(task) = self
            .claims
            .iter()
            .find_map(|(task, armed)| (armed.id == event.id()).then_some(task.clone()))
        else {
            // A cancellation can race an already-emitted event. The durable
            // re-proof made its authority stale before this consumer saw it.
            return Ok(());
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
            Ok(_) | Err(board::lapse::LapseError::Refused(_)) => Ok(()),
            Err(error) => Err(store_fault("bus::timer-lapse-failed", &error.to_string())),
        }
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
    snapshot: &bus::TaskSnapshot,
) -> CommandResult<BTreeMap<TaskHandle, ClaimAuthority>> {
    let Some(residency) = snapshot.residency() else {
        return Ok(BTreeMap::new());
    };
    if residency.body().state != ResidentState::Seated {
        return Ok(BTreeMap::new());
    }
    let mut desired = BTreeMap::new();
    for heads in snapshot.tasks() {
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
                    heads.statement().body().handle
                ),
            )
        })?;
        if lease.body().epoch != claim.epoch {
            return Err(environment_fault(
                "bus::timer-lease-epoch",
                &format!(
                    "task {} claim and lease epochs disagree",
                    heads.statement().body().handle
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
                    heads.statement().body().handle
                ),
            ));
        }
        let task = heads.statement().body().handle.clone();
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

#[cfg(test)]
mod tests {
    use board::Entry;
    use journal::{FactCause, Journal, JournalWrite, PendingFact, TaskHeads};
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

    #[test]
    fn expired_commit_bearing_claim_lapses_and_returns_to_allocation() {
        let fixture = fixture();
        let commits = fixture
            .bus
            .subscribe()
            .expect("lapse commit subscription registers");
        let mut timers = Runtime::start(fixture.projects.clone()).expect("timer runtime starts");
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
                    task: authority.task.clone(),
                    uuid: uuid(1),
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
                    task: authority.task.clone(),
                    actor: Actor::Lane(authority.lane),
                    lane: authority.lane,
                    occupant: authority.occupant,
                    epoch: authority.epoch,
                    until: authority.expiry,
                    git: Some(GitBoundary {
                        head: "local-commit".to_owned(),
                        integrated: "local-commit".to_owned(),
                        upstream: Some("origin/main".to_owned()),
                        upstream_head: Some("baseline".to_owned()),
                        local_commits: 1,
                    }),
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
        let statement_task = authority.task.clone();
        let statement_phase = phase.clone();
        write.task_head(added, move |position| TaskHead {
            handle: statement_task,
            uuid: uuid(1),
            title: "Capture commit after lapse".to_owned(),
            description: None,
            acceptance: Vec::new(),
            project: "timer.claim".to_owned(),
            tags: Vec::new(),
            dependencies: Vec::new(),
            provenance: statement_origin,
            revision: position,
        });
        let workflow_task = authority.task.clone();
        write.workflow_head(workflow_task, added, move |_| WorkflowHead {
            flow: vec![statement_phase.clone()],
            phase: statement_phase,
            status: "pending".to_owned(),
            review: "unassigned".to_owned(),
            assignment: None,
            deferral: None,
            completion: None,
        });
        let claim_task = authority.task.clone();
        let lane = authority.lane;
        let occupant = authority.occupant;
        let epoch = authority.epoch;
        write.claim_head(claim_task, won, move |position| ClaimHead {
            actor: Actor::Lane(lane),
            fact: position,
            lane,
            occupant,
            epoch,
            disposition: ClaimDisposition::Active,
        });
        let lease_task = authority.task.clone();
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
            lanes: vec![LaneResidency {
                lane,
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
        });
    }

    fn lapsed_heads(fixture: &Fixture) -> TaskHeads {
        fixture
            .bus
            .task_snapshot()
            .expect("lapsed heads read")
            .tasks()
            .iter()
            .find(|heads| heads.statement().body().handle == fixture.authority.task)
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
                    task: fixture.authority.task.clone(),
                    head: heads.statement().position(),
                },
                Witness::Workflow {
                    task: fixture.authority.task.clone(),
                    head: heads.workflow().position(),
                    phase: heads.workflow().body().phase.clone(),
                },
                Witness::Claim {
                    task: fixture.authority.task.clone(),
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
        "TIMER-claim01".parse().expect("fixture task")
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
