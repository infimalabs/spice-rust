//! Journal-backed possession transitions.

use std::error::Error;
use std::fmt;

use journal::{
    BuildError, CommitError, FactCause, GuardError, Journal, JournalGuard, JournalHead,
    JournalWrite, ReadError,
};
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, Command, CommandKind, Instant, Producer, Seq,
    TaskHandle, WireVersion, Witness, body, intent,
};

use crate::{Active, ClaimRefusal, Entry, ProjectPolicy};

const CHANGED: &str = "possession inputs changed before the release could commit";

/// Authenticated inputs for one claim release.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FreeRequest {
    /// Captured project vocabulary used by both release observations.
    pub projects: ProjectPolicy,
    /// Identity stored with the canonical command receipt.
    pub command: CanonicalUuid,
    /// Principal derived from the authenticated resident capability.
    pub actor: Actor,
    /// Lane whose live occupant currently holds the claim.
    pub lane: CanonicalUuid,
    /// Current occupant authenticated for that lane.
    pub occupant: CanonicalUuid,
    /// Resident generation that minted the claim.
    pub epoch: CanonicalUuid,
    /// Current instant at which the live lease is evaluated.
    pub at: Instant,
    /// Exact task named by the release command.
    pub intent: intent::TaskRef,
}

/// The released task and its one-fact committed range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Freed {
    task: TaskHandle,
    first: Seq,
    last: Seq,
}

impl Freed {
    /// Task made eligible for allocation again.
    #[must_use]
    pub const fn task(&self) -> &TaskHandle {
        &self.task
    }

    /// First fact position assigned to the release transaction.
    #[must_use]
    pub const fn first(&self) -> Seq {
        self.first
    }

    /// Last fact position assigned to the release transaction.
    #[must_use]
    pub const fn last(&self) -> Seq {
        self.last
    }
}

/// Expected reasons possession cannot be released.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FreeRefusal {
    MissingTask(TaskHandle),
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
    Claim(ClaimRefusal),
    ClaimLane {
        holding: CanonicalUuid,
        named: CanonicalUuid,
    },
    ClaimOccupant {
        holding: CanonicalUuid,
        named: CanonicalUuid,
    },
    ClaimEpoch {
        holding: CanonicalUuid,
        named: CanonicalUuid,
    },
    Changed(String),
}

impl fmt::Display for FreeRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingTask(task) => write!(formatter, "task {task} has no current heads"),
            Self::MissingResidency => formatter.write_str("release requires a residency head"),
            Self::ResidentState(state) => {
                write!(formatter, "resident state {state:?} refuses release")
            }
            Self::StaleEpoch { seated, named } => {
                write!(formatter, "resident epoch is {seated}, not {named}")
            }
            Self::MissingLane(lane) => write!(formatter, "resident has no lane {lane}"),
            Self::LaneDisposition { lane, disposition } => write!(
                formatter,
                "lane {lane} disposition {disposition:?} refuses release"
            ),
            Self::Displaced {
                lane,
                seated,
                named,
            } => write!(formatter, "lane {lane} holds {seated:?}, not {named}"),
            Self::Claim(refusal) => write!(formatter, "claim refuses release: {refusal}"),
            Self::ClaimLane { holding, named } => {
                write!(formatter, "claim lane is {holding}, not {named}")
            }
            Self::ClaimOccupant { holding, named } => {
                write!(formatter, "claim occupant is {holding}, not {named}")
            }
            Self::ClaimEpoch { holding, named } => {
                write!(formatter, "claim epoch is {holding}, not {named}")
            }
            Self::Changed(detail) => write!(formatter, "{CHANGED}: {detail}"),
        }
    }
}

impl Error for FreeRefusal {}

/// A typed refusal or fault from claim release.
#[derive(Debug)]
pub enum FreeError {
    Refused(FreeRefusal),
    Read(GuardError),
    Build(BuildError),
    Commit(CommitError),
}

impl fmt::Display for FreeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(error) => write!(formatter, "claim release refused: {error}"),
            Self::Read(error) => write!(formatter, "claim release read fault: {error}"),
            Self::Build(error) => write!(formatter, "claim release build fault: {error}"),
            Self::Commit(error) => write!(formatter, "claim release commit fault: {error}"),
        }
    }
}

impl Error for FreeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Refused(error) => Some(error),
            Self::Read(error) => Some(error),
            Self::Build(error) => Some(error),
            Self::Commit(error) => Some(error),
        }
    }
}

impl From<FreeRefusal> for FreeError {
    fn from(error: FreeRefusal) -> Self {
        Self::Refused(error)
    }
}

/// Re-prove and atomically release one active claim.
pub fn free(journal: &mut Journal, request: FreeRequest) -> Result<Freed, FreeError> {
    free_after_observation(journal, request, |_| Ok(()))
}

fn free_after_observation<F>(
    journal: &mut Journal,
    request: FreeRequest,
    after: F,
) -> Result<Freed, FreeError>
where
    F: FnOnce(&mut Journal) -> Result<(), FreeError>,
{
    let observed = read_observation(journal, &request)?;
    after(journal)?;
    let task = request.intent.task.clone();
    let write = build_write(request, observed)?;
    let applied = journal.apply(write).map_err(map_commit_error)?;
    Ok(Freed {
        task,
        first: applied.first(),
        last: applied.last(),
    })
}

fn read_observation(journal: &Journal, request: &FreeRequest) -> Result<Observation, FreeError> {
    journal
        .read_with(|read| observe(read, request))
        .map_err(|error| match error {
            ReadError::Journal(error) | ReadError::Answer(ObserveError::Fault(error)) => {
                FreeError::Read(error)
            }
            ReadError::Answer(ObserveError::Refused(error)) => FreeError::Refused(error),
        })
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Observation {
    statement: JournalHead<wire::TaskHead>,
    workflow: JournalHead<wire::WorkflowHead>,
    claim: JournalHead<wire::ClaimHead>,
    lease: JournalHead<wire::LeaseHead>,
    residency: JournalHead<wire::ResidencyHead>,
}

enum ObserveError {
    Refused(FreeRefusal),
    Fault(GuardError),
}

impl From<FreeRefusal> for ObserveError {
    fn from(error: FreeRefusal) -> Self {
        Self::Refused(error)
    }
}

impl From<GuardError> for ObserveError {
    fn from(error: GuardError) -> Self {
        Self::Fault(error)
    }
}

fn observe(
    read: &journal::JournalRead<'_>,
    request: &FreeRequest,
) -> Result<Observation, ObserveError> {
    let residency = read
        .residency_head()?
        .ok_or(FreeRefusal::MissingResidency)?;
    admit_residency(residency.body(), request)?;
    let task = &request.intent.task;
    let statement = read
        .task_head(task)?
        .ok_or_else(|| FreeRefusal::MissingTask(task.clone()))?;
    let workflow = read
        .workflow_head(task)?
        .ok_or_else(|| GuardError::Invalid(format!("task {task} has no workflow head")))?;
    let claim = read
        .claim_head(task)?
        .ok_or(FreeRefusal::Claim(ClaimRefusal::MissingClaim))?;
    let lease = read
        .lease_head(task)?
        .ok_or(FreeRefusal::Claim(ClaimRefusal::MissingLease))?;
    let entry = Entry::new(
        statement.body().clone(),
        workflow.body().clone(),
        Some(claim.body().clone()),
        Some(lease.body().clone()),
        &request.projects,
    )
    .map_err(|error| GuardError::Invalid(format!("task {task} is invalid: {error}")))?;
    let active = Active::admit(&entry, &request.actor, request.at).map_err(FreeRefusal::Claim)?;
    admit_claim(&active, request)?;
    Ok(Observation {
        statement,
        workflow,
        claim,
        lease,
        residency,
    })
}

fn admit_claim(active: &Active<'_>, request: &FreeRequest) -> Result<(), FreeRefusal> {
    if active.lane() != request.lane {
        return Err(FreeRefusal::ClaimLane {
            holding: active.lane(),
            named: request.lane,
        });
    }
    if active.occupant() != request.occupant {
        return Err(FreeRefusal::ClaimOccupant {
            holding: active.occupant(),
            named: request.occupant,
        });
    }
    if active.epoch() != request.epoch {
        return Err(FreeRefusal::ClaimEpoch {
            holding: active.epoch(),
            named: request.epoch,
        });
    }
    Ok(())
}

fn admit_residency(head: &wire::ResidencyHead, request: &FreeRequest) -> Result<(), FreeRefusal> {
    if head.state != wire::ResidentState::Seated {
        return Err(FreeRefusal::ResidentState(head.state));
    }
    if head.epoch != request.epoch {
        return Err(FreeRefusal::StaleEpoch {
            seated: head.epoch,
            named: request.epoch,
        });
    }
    let seat = head
        .lanes
        .iter()
        .find(|seat| seat.lane == request.lane)
        .ok_or(FreeRefusal::MissingLane(request.lane))?;
    if seat.disposition != "occupied" {
        return Err(FreeRefusal::LaneDisposition {
            lane: request.lane,
            disposition: seat.disposition.clone(),
        });
    }
    if seat.occupant != Some(request.occupant) {
        return Err(FreeRefusal::Displaced {
            lane: request.lane,
            seated: seat.occupant,
            named: request.occupant,
        });
    }
    Ok(())
}

fn build_write(request: FreeRequest, observed: Observation) -> Result<JournalWrite, FreeError> {
    let task = request.intent.task.clone();
    let origin = format!("task:{task}")
        .parse()
        .expect("task handle forms an origin");
    let mut write = JournalWrite::new(
        request.actor.clone(),
        origin,
        request.at,
        Producer::CommandDoor,
    );
    write.guard(change_guard(request.clone(), observed.clone()));
    let fact = write
        .fact(
            body::ClaimFreed {
                task: task.clone(),
                actor: request.actor.clone(),
            },
            FactCause::Root,
        )
        .map_err(FreeError::Build)?;
    let mut next = observed.claim.body().clone();
    next.disposition = ClaimDisposition::Freed;
    write.claim_head(task.clone(), fact, move |position| {
        next.fact = position;
        next
    });
    write
        .command_result(
            Command {
                v: WireVersion::CURRENT,
                id: request.command,
                epoch: request.epoch,
                kind: CommandKind::ClaimFree,
                expected: witnesses(&request, &observed),
                body: request.intent,
            },
            request.epoch,
            intent::ClaimAnswer {
                task,
                holder: request.actor,
            },
        )
        .map_err(FreeError::Build)?;
    Ok(write)
}

fn witnesses(request: &FreeRequest, observed: &Observation) -> Vec<Witness> {
    let task = request.intent.task.clone();
    vec![
        Witness::Claim {
            task: task.clone(),
            head: Some(observed.claim.position()),
        },
        Witness::Lease {
            task,
            head: observed.lease.position(),
        },
        Witness::Residency {
            lane: request.lane,
            occupant: request.occupant,
            epoch: request.epoch,
        },
    ]
}

fn change_guard(request: FreeRequest, expected: Observation) -> JournalGuard {
    JournalGuard::new(move |read| match observe(read, &request) {
        Ok(current) if current == expected => Ok(()),
        Ok(_) => Err(GuardError::refused(format!(
            "{CHANGED}: authoritative heads changed"
        ))),
        Err(ObserveError::Refused(error)) => {
            Err(GuardError::refused(format!("{CHANGED}: {error}")))
        }
        Err(ObserveError::Fault(error)) => Err(error),
    })
}

fn map_commit_error(error: CommitError) -> FreeError {
    match error {
        CommitError::Refused(refusal) if refusal.reason().starts_with(CHANGED) => {
            let detail = refusal
                .reason()
                .strip_prefix(CHANGED)
                .expect("prefix matched")
                .trim_start_matches(": ")
                .to_owned();
            FreeError::Refused(FreeRefusal::Changed(detail))
        }
        error => FreeError::Commit(error),
    }
}

#[cfg(test)]
mod tests {
    use super::{FreeError, FreeRefusal, FreeRequest, free, free_after_observation};
    use crate::allocate::{Request as WinRequest, win};
    use journal::{FactCause, Journal, JournalWrite};
    use wire::{
        Actor, CanonicalUuid, Instant, LaneResidency, Origin, Phase, Producer, ResidencyHead,
        ResidentState, TaskHandle, TaskHead, WorkflowHead, body, intent,
    };

    fn uuid(last: u8) -> CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
            .parse()
            .expect("uuid")
    }

    fn instant(minute: u8) -> Instant {
        format!("2026-08-01T06:{minute:02}:00.000Z")
            .parse()
            .expect("instant")
    }

    fn task() -> TaskHandle {
        "BOARD-a1".parse().expect("task")
    }

    fn projects() -> crate::ProjectPolicy {
        crate::ProjectPolicy::new(
            ["board".to_owned()],
            Vec::new(),
            dialect::PROJECT_MIN_DEPTH as u64,
            dialect::PROJECT_MAX_DEPTH as u64,
        )
        .expect("fixture project policy")
    }

    fn free_request(command: u8) -> FreeRequest {
        FreeRequest {
            projects: projects(),
            command: uuid(command),
            actor: Actor::Lane(uuid(20)),
            lane: uuid(20),
            occupant: uuid(21),
            epoch: uuid(22),
            at: instant(10),
            intent: intent::TaskRef { task: task() },
        }
    }

    fn claimed() -> Journal {
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
                    task: task(),
                    uuid: uuid(1),
                    title: "Release".to_owned(),
                    description: None,
                    acceptance: Vec::new(),
                    project: "board.possession".to_owned(),
                    tags: Vec::new(),
                    depends: Vec::new(),
                    flow: vec![phase.clone()],
                    phase: phase.clone(),
                    revision: 1,
                },
                FactCause::Root,
            )
            .expect("task fact");
        write.task_head(fact, move |position| TaskHead {
            handle: task(),
            uuid: uuid(1),
            title: "Release".to_owned(),
            description: None,
            acceptance: Vec::new(),
            project: "board.possession".to_owned(),
            tags: Vec::new(),
            dependencies: Vec::new(),
            provenance: origin,
            revision: position,
        });
        write.workflow_head(task(), fact, move |_| WorkflowHead {
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
                team: uuid(20),
                worktree: "/worktrees/lane-a".to_owned(),
                life: "drive".to_owned(),
                vendor: "claude".parse().expect("fixture vendor"),
                occupant: Some(uuid(21)),
                lineage: Vec::new(),
                capabilities: Vec::new(),
                session: None,
                disposition: "occupied".to_owned(),
            }],
        });
        journal.apply(write).expect("seed");
        win(
            &mut journal,
            WinRequest {
                projects: projects(),
                command: uuid(31),
                actor: Actor::Lane(uuid(20)),
                lane: uuid(20),
                occupant: uuid(21),
                epoch: uuid(22),
                at: instant(5),
                expiry: instant(20),
                maximum: instant(35),
                rule: "next".parse().expect("rule"),
            },
        )
        .expect("win");
        journal
    }

    #[test]
    fn a_release_changed_after_observation_refuses_before_a_second_fact() {
        let mut journal = claimed();
        let outer = free_after_observation(&mut journal, free_request(32), |journal| {
            free(journal, free_request(33))?;
            Ok(())
        });
        assert!(matches!(
            outer,
            Err(FreeError::Refused(FreeRefusal::Changed(_)))
        ));
        assert_eq!(journal.fact_snapshot().expect("facts").facts().len(), 3);
        assert_eq!(journal.verify().expect("verify").receipts, 2);
    }
}
