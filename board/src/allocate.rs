//! The sole journal-backed allocator transition.

use std::error::Error;
use std::fmt;

use journal::{
    BuildError, CommitError, FactCause, GuardError, Journal, JournalGuard, JournalHead,
    JournalWrite, ReadError, TaskHeads,
};
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, ClaimHead, Command, CommandKind, Instant, LeaseHead,
    Name, Producer, Seq, TaskHandle, WireVersion, Witness, body, intent,
};

use crate::{Entry, ProjectPolicy, select};

const CURRENT_RULE: &str = "next";
const CHANGED: &str = "allocator inputs changed before the claim could commit";

/// Authenticated inputs for one handle-free allocation request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    /// Captured project vocabulary used by both allocation observations.
    pub projects: ProjectPolicy,
    /// Identity stored with the canonical command receipt.
    pub command: CanonicalUuid,
    /// Principal derived from the authenticated resident capability.
    pub actor: Actor,
    /// Lane whose live occupant will hold the claim.
    pub lane: CanonicalUuid,
    /// Current occupant authenticated for that lane.
    pub occupant: CanonicalUuid,
    /// Resident generation minting the claim.
    pub epoch: CanonicalUuid,
    /// Current instant used as the lease start and first renewal.
    pub at: Instant,
    /// Initial lease expiry.
    pub expiry: Instant,
    /// Maximum instant through which this lease may be renewed.
    pub maximum: Instant,
    /// Handle-free eligibility rule; release one admits only `next`.
    pub rule: Name,
}

/// The task and journal range won by an allocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Won {
    task: TaskHandle,
    first: Seq,
    last: Seq,
}

impl Won {
    /// Task selected by the one allocation query and possessed by this commit.
    #[must_use]
    pub const fn task(&self) -> &TaskHandle {
        &self.task
    }

    /// First fact position assigned to the claim transaction.
    #[must_use]
    pub const fn first(&self) -> Seq {
        self.first
    }

    /// Last fact position assigned to the claim transaction.
    #[must_use]
    pub const fn last(&self) -> Seq {
        self.last
    }
}

/// Expected reasons an allocation does not mint possession.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Refusal {
    Rule(Name),
    Deadline {
        at: Instant,
        expiry: Instant,
        maximum: Instant,
    },
    ActorLane {
        actor: CanonicalUuid,
        named: CanonicalUuid,
    },
    MissingResidency,
    ResidentState(wire::ResidentState),
    StaleEpoch {
        seated: CanonicalUuid,
        named: CanonicalUuid,
    },
    MissingLane(CanonicalUuid),
    LaneDisposition {
        lane: CanonicalUuid,
        disposition: String,
    },
    Displaced {
        lane: CanonicalUuid,
        seated: Option<CanonicalUuid>,
        named: CanonicalUuid,
    },
    Empty,
    Changed(String),
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rule(rule) => write!(
                formatter,
                "allocator rule {rule} is not the sole current rule {CURRENT_RULE}"
            ),
            Self::Deadline {
                at,
                expiry,
                maximum,
            } => write!(
                formatter,
                "claim lease must satisfy start {at} < expiry {expiry} <= maximum {maximum}"
            ),
            Self::ActorLane { actor, named } => {
                write!(
                    formatter,
                    "lane actor {actor} cannot claim through lane {named}"
                )
            }
            Self::MissingResidency => {
                formatter.write_str("allocator requires a current residency head")
            }
            Self::ResidentState(state) => {
                write!(
                    formatter,
                    "resident state {state:?} does not admit allocation"
                )
            }
            Self::StaleEpoch { seated, named } => {
                write!(
                    formatter,
                    "resident epoch is {seated}, not named epoch {named}"
                )
            }
            Self::MissingLane(lane) => write!(formatter, "resident has no lane {lane}"),
            Self::LaneDisposition { lane, disposition } => write!(
                formatter,
                "lane {lane} disposition {disposition:?} does not admit allocation"
            ),
            Self::Displaced {
                lane,
                seated,
                named,
            } => write!(
                formatter,
                "lane {lane} holds occupant {seated:?}, not named occupant {named}"
            ),
            Self::Empty => formatter.write_str("allocator found no eligible task"),
            Self::Changed(detail) => write!(formatter, "{CHANGED}: {detail}"),
        }
    }
}

impl Error for Refusal {}

/// A refusal or fault produced while winning allocation authority.
#[derive(Debug)]
pub enum WinError {
    Refused(Refusal),
    Read(GuardError),
    Build(BuildError),
    Commit(CommitError),
}

impl fmt::Display for WinError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(refusal) => write!(formatter, "allocation refused: {refusal}"),
            Self::Read(error) => write!(formatter, "allocation read fault: {error}"),
            Self::Build(error) => write!(formatter, "allocation build fault: {error}"),
            Self::Commit(error) => write!(formatter, "allocation commit fault: {error}"),
        }
    }
}

impl Error for WinError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Refused(refusal) => Some(refusal),
            Self::Read(error) => Some(error),
            Self::Build(error) => Some(error),
            Self::Commit(error) => Some(error),
        }
    }
}

impl From<Refusal> for WinError {
    fn from(refusal: Refusal) -> Self {
        Self::Refused(refusal)
    }
}

/// Run the sole allocation query and atomically mint its winning claim.
///
/// The preliminary read supplies the proposal that must be built before
/// `Journal::apply`. The immediate transaction then reconstructs and compares
/// the complete head set and residency before any fact is inserted, so the
/// selection that commits is the same selection evaluated inside the write.
pub fn win(journal: &mut Journal, request: Request) -> Result<Won, WinError> {
    win_after_observation(journal, request, |_| Ok(()))
}

fn win_after_observation<F>(
    journal: &mut Journal,
    request: Request,
    after: F,
) -> Result<Won, WinError>
where
    F: FnOnce(&mut Journal) -> Result<(), WinError>,
{
    admit_request(&request)?;
    let observed = read_observation(journal, &request)?;
    after(journal)?;
    let (winner, write) = build_write(request, observed)?;
    let applied = journal.apply(write).map_err(map_commit_error)?;
    Ok(Won {
        task: winner,
        first: applied.first(),
        last: applied.last(),
    })
}

fn read_observation(journal: &Journal, request: &Request) -> Result<Observation, WinError> {
    journal
        .read_with(|read| observe(read, request))
        .map_err(|error| match error {
            ReadError::Journal(error) | ReadError::Answer(ObservationError::Fault(error)) => {
                WinError::Read(error)
            }
            ReadError::Answer(ObservationError::Refused(refusal)) => WinError::Refused(refusal),
        })
}

fn build_write(
    request: Request,
    observed: Observation,
) -> Result<(TaskHandle, JournalWrite), WinError> {
    let winner = observed.winner().clone();
    let expected_witnesses = witnesses(&request, observed.selected());

    let origin = format!("task:{winner}")
        .parse()
        .expect("a validated task handle always forms a task origin");
    let mut write = JournalWrite::new(
        request.actor.clone(),
        origin,
        request.at,
        Producer::CommandDoor,
    );
    write.guard(change_guard(request.clone(), observed));

    let fact = write
        .fact(
            body::ClaimWon {
                task: winner.clone(),
                actor: request.actor.clone(),
                lane: request.lane,
                occupant: request.occupant,
                epoch: request.epoch,
                until: request.expiry,
                git: None,
            },
            FactCause::Root,
        )
        .map_err(WinError::Build)?;
    let claim_actor = request.actor.clone();
    let lane = request.lane;
    let occupant = request.occupant;
    let epoch = request.epoch;
    write.claim_head(winner.clone(), fact, move |position| ClaimHead {
        actor: claim_actor,
        fact: position,
        lane,
        occupant,
        epoch,
        disposition: ClaimDisposition::Active,
    });
    let start = request.at;
    let expiry = request.expiry;
    let maximum = request.maximum;
    write.lease_head(winner.clone(), fact, move |_| LeaseHead {
        start,
        renewal: start,
        expiry,
        maximum,
        epoch,
    });

    write
        .command_result(
            Command {
                v: WireVersion::CURRENT,
                id: request.command,
                epoch: request.epoch,
                kind: CommandKind::ClaimWin,
                expected: expected_witnesses,
                body: intent::ClaimWin {
                    rule: request.rule.clone(),
                },
            },
            request.epoch,
            intent::ClaimAnswer {
                task: winner.clone(),
                holder: request.actor,
                warnings: Vec::new(),
            },
        )
        .map_err(WinError::Build)?;
    Ok((winner, write))
}

fn change_guard(request: Request, expected: Observation) -> JournalGuard {
    JournalGuard::new(move |read| match observe(read, &request) {
        Ok(current) if current == expected => Ok(()),
        Ok(current) if current.winner != expected.winner => Err(GuardError::refused(format!(
            "{CHANGED}: winner moved from {} to {}",
            expected.winner, current.winner
        ))),
        Ok(current) if current.residency != expected.residency => {
            Err(GuardError::refused(format!("{CHANGED}: residency changed")))
        }
        Ok(_) => Err(GuardError::refused(format!(
            "{CHANGED}: authoritative task heads changed"
        ))),
        Err(ObservationError::Refused(refusal)) => {
            Err(GuardError::refused(format!("{CHANGED}: {refusal}")))
        }
        Err(ObservationError::Fault(error)) => Err(error),
    })
}

fn map_commit_error(error: CommitError) -> WinError {
    match error {
        CommitError::Refused(refusal) if refusal.reason().starts_with(CHANGED) => {
            let detail = refusal
                .reason()
                .strip_prefix(CHANGED)
                .expect("matched changed prefix")
                .trim_start_matches(": ")
                .to_owned();
            WinError::Refused(Refusal::Changed(detail))
        }
        fault => WinError::Commit(fault),
    }
}

fn admit_request(request: &Request) -> Result<(), Refusal> {
    if request.rule.as_str() != CURRENT_RULE {
        return Err(Refusal::Rule(request.rule.clone()));
    }
    if !(request.at < request.expiry && request.expiry <= request.maximum) {
        return Err(Refusal::Deadline {
            at: request.at,
            expiry: request.expiry,
            maximum: request.maximum,
        });
    }
    if let Actor::Lane(actor) = &request.actor
        && actor != &request.lane
    {
        return Err(Refusal::ActorLane {
            actor: *actor,
            named: request.lane,
        });
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Observation {
    heads: Vec<TaskHeads>,
    residency: JournalHead<wire::ResidencyHead>,
    winner: TaskHandle,
}

impl Observation {
    fn winner(&self) -> &TaskHandle {
        &self.winner
    }

    fn selected(&self) -> &TaskHeads {
        self.heads
            .iter()
            .find(|heads| heads.statement().body().handle == self.winner)
            .expect("the selected entry came from this complete head set")
    }
}

enum ObservationError {
    Refused(Refusal),
    Fault(GuardError),
}

impl From<GuardError> for ObservationError {
    fn from(error: GuardError) -> Self {
        Self::Fault(error)
    }
}

impl From<Refusal> for ObservationError {
    fn from(refusal: Refusal) -> Self {
        Self::Refused(refusal)
    }
}

fn observe(
    read: &journal::JournalRead<'_>,
    request: &Request,
) -> Result<Observation, ObservationError> {
    let residency = read.residency_head()?.ok_or(Refusal::MissingResidency)?;
    admit_residency(residency.body(), request)?;
    let heads = read.task_heads()?;
    let entries = entries(&heads, &request.projects)?;
    let winner = select(&entries, &request.actor)
        .ok_or(Refusal::Empty)?
        .entry()
        .statement()
        .handle
        .clone();
    Ok(Observation {
        heads,
        residency,
        winner,
    })
}

fn admit_residency(residency: &wire::ResidencyHead, request: &Request) -> Result<(), Refusal> {
    if residency.state != wire::ResidentState::Seated {
        return Err(Refusal::ResidentState(residency.state));
    }
    if residency.epoch != request.epoch {
        return Err(Refusal::StaleEpoch {
            seated: residency.epoch,
            named: request.epoch,
        });
    }
    let seat = residency
        .lanes
        .iter()
        .find(|seat| seat.lane == request.lane)
        .ok_or(Refusal::MissingLane(request.lane))?;
    if seat.disposition != "occupied" {
        return Err(Refusal::LaneDisposition {
            lane: request.lane,
            disposition: seat.disposition.clone(),
        });
    }
    if seat.occupant != Some(request.occupant) {
        return Err(Refusal::Displaced {
            lane: request.lane,
            seated: seat.occupant,
            named: request.occupant,
        });
    }
    Ok(())
}

fn entries(heads: &[TaskHeads], projects: &ProjectPolicy) -> Result<Vec<Entry>, ObservationError> {
    heads
        .iter()
        .cloned()
        .map(|heads| {
            admit_possession(&heads)?;
            let (statement, workflow, claim, lease) = heads.into_bodies();
            let handle = statement.handle.clone();
            Entry::new(statement, workflow, claim, lease, projects).map_err(|source| {
                ObservationError::Fault(GuardError::Invalid(format!(
                    "allocator task {handle} is invalid: {source}"
                )))
            })
        })
        .collect()
}

fn admit_possession(heads: &TaskHeads) -> Result<(), GuardError> {
    let task = &heads.statement().body().handle;
    let (Some(claim), Some(lease)) = (heads.claim(), heads.lease()) else {
        if heads.claim().is_some() || heads.lease().is_some() {
            return Err(GuardError::Invalid(format!(
                "allocator task {task} has only one of claim and lease heads"
            )));
        }
        return Ok(());
    };
    if claim.body().epoch != lease.body().epoch {
        return Err(GuardError::Invalid(format!(
            "allocator task {task} claim and lease epochs disagree"
        )));
    }
    let lease = lease.body();
    if !(lease.start <= lease.renewal
        && lease.renewal <= lease.expiry
        && lease.expiry <= lease.maximum)
    {
        return Err(GuardError::Invalid(format!(
            "allocator task {task} lease life is incoherent"
        )));
    }
    Ok(())
}

fn witnesses(request: &Request, selected: &TaskHeads) -> Vec<Witness> {
    let task = selected.statement().body().handle.clone();
    vec![
        Witness::Task {
            task: task.clone(),
            head: selected.statement().position(),
        },
        Witness::Workflow {
            task: task.clone(),
            head: selected.workflow().position(),
            phase: selected.workflow().body().phase.clone(),
        },
        Witness::Claim {
            task,
            head: selected.claim().map(JournalHead::position),
        },
        Witness::Eligibility {
            rule: request.rule.clone(),
        },
        Witness::Residency {
            lane: request.lane,
            occupant: request.occupant,
            epoch: request.epoch,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::{Refusal, Request, WinError, win, win_after_observation};
    use journal::{FactCause, Journal, JournalWrite};
    use wire::{
        Actor, CanonicalUuid, Instant, LaneResidency, Origin, Phase, Producer, ResidencyHead,
        ResidentState, TaskHandle, TaskHead, WorkflowHead, body,
    };

    fn uuid(last: u8) -> CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
            .parse()
            .expect("fixture uuid")
    }

    fn instant(minute: u8) -> Instant {
        format!("2026-08-01T06:{minute:02}:00.000Z")
            .parse()
            .expect("fixture instant")
    }

    fn request(command: u8) -> Request {
        let lane = uuid(20);
        Request {
            projects: crate::ProjectPolicy::new(
                ["board".to_owned()],
                Vec::new(),
                dialect::PROJECT_MIN_DEPTH as u64,
                dialect::PROJECT_MAX_DEPTH as u64,
            )
            .expect("fixture project policy"),
            command: uuid(command),
            actor: Actor::Lane(lane),
            lane,
            occupant: uuid(21),
            epoch: uuid(22),
            at: instant(5),
            expiry: instant(20),
            maximum: instant(35),
            rule: "next".parse().expect("rule"),
        }
    }

    fn journal() -> Journal {
        let task = "BOARD-a1".parse::<TaskHandle>().expect("task");
        let phase = "todo".parse::<Phase>().expect("phase");
        let origin = "task:BOARD-root".parse::<Origin>().expect("origin");
        let mut journal = Journal::in_memory().expect("journal");
        let mut write = JournalWrite::new(
            Actor::Spice,
            origin.clone(),
            instant(0),
            Producer::CommandDoor,
        );
        let fact = write
            .fact(
                body::TaskAdded {
                    task: task.clone(),
                    uuid: uuid(1),
                    title: "Allocate one task".to_owned(),
                    description: None,
                    acceptance: Vec::new(),
                    project: "board.allocate".to_owned(),
                    tags: vec!["priority:high".to_owned()],
                    depends: Vec::new(),
                    flow: vec![phase.clone()],
                    phase: phase.clone(),
                    revision: 1,
                },
                FactCause::Root,
            )
            .expect("task fact");
        let statement_task = task.clone();
        write.task_head(fact, move |position| TaskHead {
            handle: statement_task,
            uuid: uuid(1),
            title: "Allocate one task".to_owned(),
            description: None,
            acceptance: Vec::new(),
            project: "board.allocate".to_owned(),
            tags: vec!["priority:high".to_owned()],
            dependencies: Vec::new(),
            provenance: origin,
            revision: position,
        });
        write.workflow_head(task, fact, move |_| WorkflowHead {
            flow: vec![phase.clone()],
            phase,
            status: "pending".to_owned(),
            review: "unassigned".to_owned(),
            assignment: None,
            deferral: None,
            completion: None,
        });
        write.residency_head(fact, |_| ResidencyHead {
            epoch: uuid(22),
            state: ResidentState::Seated,
            lanes: vec![LaneResidency {
                lane: uuid(20),
                lineage_id: uuid(20),
                name: None,
                worktree: "/worktrees/lane-a".to_owned(),
                life: "drive".to_owned(),
                vendor: "claude".parse().expect("fixture vendor"),
                occupant: Some(uuid(21)),
                lineage: Vec::new(),
                capabilities: Vec::new(),
                session: None,
                disposition: "occupied".to_owned(),
            }],
            subscriptions: Vec::new(),
        });
        journal.apply(write).expect("seed authority");
        journal
    }

    #[test]
    fn a_winner_changed_after_observation_refuses_before_a_second_claim() {
        let mut journal = journal();
        let outer = win_after_observation(&mut journal, request(31), |journal| {
            let raced = win(journal, request(32))?;
            assert_eq!(raced.task().as_str(), "BOARD-a1");
            Ok(())
        });

        assert!(matches!(outer, Err(WinError::Refused(Refusal::Changed(_)))));
        let facts = journal.fact_snapshot().expect("fact snapshot");
        assert_eq!(facts.facts().len(), 2);
        assert_eq!(journal.verify().expect("authority verifies").receipts, 1);
    }
}
