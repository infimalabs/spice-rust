//! Journal-backed expiry of task possession.

use std::error::Error;
use std::fmt;

use journal::{
    BuildError, CommitError, FactCause, GuardError, Journal, JournalGuard, JournalHead,
    JournalWrite, ReadError,
};
use wire::{
    CanonicalUuid, ClaimDisposition, Command, CommandKind, Instant, Producer, Seq, TaskHandle,
    WireVersion, Witness, body, intent,
};

use crate::{Entry, ProjectPolicy};

const CHANGED: &str = "lapse inputs changed before expiry could commit";

/// Durable timer inputs for one claim expiry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    /// Captured project vocabulary used by both expiry observations.
    pub projects: ProjectPolicy,
    /// Identity stored with the canonical command receipt.
    pub command: CanonicalUuid,
    /// Occupant whose lease timer fired.
    pub occupant: CanonicalUuid,
    /// Resident generation under which the timer was armed.
    pub epoch: CanonicalUuid,
    /// Exact durable expiry carried by the fired timer.
    pub expiry: Instant,
    /// Current instant at which expiry is evaluated.
    pub at: Instant,
    /// Exact task named by the lapse command.
    pub intent: intent::TaskRef,
}

/// The lapsed task and its one-fact committed range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Lapsed {
    task: TaskHandle,
    first: Seq,
    last: Seq,
}

impl Lapsed {
    /// Task made eligible for allocation again.
    #[must_use]
    pub const fn task(&self) -> &TaskHandle {
        &self.task
    }

    /// First fact position assigned to the lapse transaction.
    #[must_use]
    pub const fn first(&self) -> Seq {
        self.first
    }

    /// Last fact position assigned to the lapse transaction.
    #[must_use]
    pub const fn last(&self) -> Seq {
        self.last
    }
}

/// Expected reasons a timer cannot lapse possession.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Refusal {
    MissingTask(TaskHandle),
    MissingClaim,
    MissingLease,
    Inactive(ClaimDisposition),
    ClaimLeaseEpoch {
        claim: CanonicalUuid,
        lease: CanonicalUuid,
    },
    ClaimEpoch {
        holding: CanonicalUuid,
        named: CanonicalUuid,
    },
    ClaimOccupant {
        holding: CanonicalUuid,
        named: CanonicalUuid,
    },
    IncoherentLease,
    StaleExpiry {
        stored: Instant,
        fired: Instant,
    },
    Early {
        at: Instant,
        expiry: Instant,
    },
    MissingResidency,
    ResidentState(wire::ResidentState),
    StaleResidentEpoch {
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
    Changed(String),
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingTask(task) => write!(formatter, "task {task} has no current heads"),
            Self::MissingClaim => formatter.write_str("lapse requires a claim head"),
            Self::MissingLease => formatter.write_str("lapse requires a lease head"),
            Self::Inactive(disposition) => {
                write!(formatter, "claim is not active: {disposition:?}")
            }
            Self::ClaimLeaseEpoch { claim, lease } => {
                write!(
                    formatter,
                    "claim epoch {claim} does not match lease epoch {lease}"
                )
            }
            Self::ClaimEpoch { holding, named } => {
                write!(formatter, "claim epoch is {holding}, not {named}")
            }
            Self::ClaimOccupant { holding, named } => {
                write!(formatter, "claim occupant is {holding}, not {named}")
            }
            Self::IncoherentLease => {
                formatter.write_str("lease must satisfy start <= renewal <= expiry <= maximum")
            }
            Self::StaleExpiry { stored, fired } => {
                write!(
                    formatter,
                    "lease expires at {stored}, not fired expiry {fired}"
                )
            }
            Self::Early { at, expiry } => {
                write!(formatter, "lapse instant {at} precedes expiry {expiry}")
            }
            Self::MissingResidency => formatter.write_str("lapse requires a residency head"),
            Self::ResidentState(state) => {
                write!(formatter, "resident state {state:?} refuses lapse")
            }
            Self::StaleResidentEpoch { seated, named } => {
                write!(formatter, "resident epoch is {seated}, not {named}")
            }
            Self::MissingLane(lane) => write!(formatter, "resident has no lane {lane}"),
            Self::LaneDisposition { lane, disposition } => write!(
                formatter,
                "lane {lane} disposition {disposition:?} refuses lapse"
            ),
            Self::Displaced {
                lane,
                seated,
                named,
            } => write!(formatter, "lane {lane} holds {seated:?}, not {named}"),
            Self::Changed(detail) => write!(formatter, "{CHANGED}: {detail}"),
        }
    }
}

impl Error for Refusal {}

/// A typed refusal or fault from claim expiry.
#[derive(Debug)]
pub enum LapseError {
    Refused(Refusal),
    Read(GuardError),
    Build(BuildError),
    Commit(CommitError),
}

impl fmt::Display for LapseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(error) => write!(formatter, "claim lapse refused: {error}"),
            Self::Read(error) => write!(formatter, "claim lapse read fault: {error}"),
            Self::Build(error) => write!(formatter, "claim lapse build fault: {error}"),
            Self::Commit(error) => write!(formatter, "claim lapse commit fault: {error}"),
        }
    }
}

impl Error for LapseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Refused(error) => Some(error),
            Self::Read(error) => Some(error),
            Self::Build(error) => Some(error),
            Self::Commit(error) => Some(error),
        }
    }
}

impl From<Refusal> for LapseError {
    fn from(error: Refusal) -> Self {
        Self::Refused(error)
    }
}

/// Re-prove and atomically lapse one expired active claim.
pub fn lapse(journal: &mut Journal, request: Request) -> Result<Lapsed, LapseError> {
    lapse_after_observation(journal, request, |_| Ok(()))
}

fn lapse_after_observation<F>(
    journal: &mut Journal,
    request: Request,
    after: F,
) -> Result<Lapsed, LapseError>
where
    F: FnOnce(&mut Journal) -> Result<(), LapseError>,
{
    let observed = read_observation(journal, &request)?;
    after(journal)?;
    let task = request.intent.task.clone();
    let write = build_write(request, observed)?;
    let applied = journal.apply(write).map_err(map_commit_error)?;
    Ok(Lapsed {
        task,
        first: applied.first(),
        last: applied.last(),
    })
}

fn read_observation(journal: &Journal, request: &Request) -> Result<Observation, LapseError> {
    journal
        .read_with(|read| observe(read, request))
        .map_err(|error| match error {
            ReadError::Journal(error) | ReadError::Answer(ObserveError::Fault(error)) => {
                LapseError::Read(error)
            }
            ReadError::Answer(ObserveError::Refused(error)) => LapseError::Refused(error),
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
    Refused(Refusal),
    Fault(GuardError),
}

impl From<Refusal> for ObserveError {
    fn from(error: Refusal) -> Self {
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
    request: &Request,
) -> Result<Observation, ObserveError> {
    let task = &request.intent.task;
    let statement = read
        .task_head(task)?
        .ok_or_else(|| Refusal::MissingTask(task.clone()))?;
    let workflow = read
        .workflow_head(task)?
        .ok_or_else(|| GuardError::Invalid(format!("task {task} has no workflow head")))?;
    let claim = read.claim_head(task)?.ok_or(Refusal::MissingClaim)?;
    let lease = read.lease_head(task)?.ok_or(Refusal::MissingLease)?;
    Entry::new(
        statement.body().clone(),
        workflow.body().clone(),
        Some(claim.body().clone()),
        Some(lease.body().clone()),
        &request.projects,
    )
    .map_err(|error| GuardError::Invalid(format!("task {task} is invalid: {error}")))?;
    admit_expiry(claim.body(), lease.body(), request)?;
    let residency = read.residency_head()?.ok_or(Refusal::MissingResidency)?;
    admit_residency(residency.body(), claim.body().lane, request)?;
    Ok(Observation {
        statement,
        workflow,
        claim,
        lease,
        residency,
    })
}

fn admit_expiry(
    claim: &wire::ClaimHead,
    lease: &wire::LeaseHead,
    request: &Request,
) -> Result<(), Refusal> {
    if claim.disposition != ClaimDisposition::Active {
        return Err(Refusal::Inactive(claim.disposition));
    }
    if claim.epoch != lease.epoch {
        return Err(Refusal::ClaimLeaseEpoch {
            claim: claim.epoch,
            lease: lease.epoch,
        });
    }
    if claim.epoch != request.epoch {
        return Err(Refusal::ClaimEpoch {
            holding: claim.epoch,
            named: request.epoch,
        });
    }
    if claim.occupant != request.occupant {
        return Err(Refusal::ClaimOccupant {
            holding: claim.occupant,
            named: request.occupant,
        });
    }
    if !(lease.start <= lease.renewal
        && lease.renewal <= lease.expiry
        && lease.expiry <= lease.maximum)
    {
        return Err(Refusal::IncoherentLease);
    }
    if lease.expiry != request.expiry {
        return Err(Refusal::StaleExpiry {
            stored: lease.expiry,
            fired: request.expiry,
        });
    }
    if request.at < lease.expiry {
        return Err(Refusal::Early {
            at: request.at,
            expiry: lease.expiry,
        });
    }
    Ok(())
}

fn admit_residency(
    head: &wire::ResidencyHead,
    lane: CanonicalUuid,
    request: &Request,
) -> Result<(), Refusal> {
    if head.state != wire::ResidentState::Seated {
        return Err(Refusal::ResidentState(head.state));
    }
    if head.epoch != request.epoch {
        return Err(Refusal::StaleResidentEpoch {
            seated: head.epoch,
            named: request.epoch,
        });
    }
    let seat = head
        .lanes
        .iter()
        .find(|seat| seat.lane == lane)
        .ok_or(Refusal::MissingLane(lane))?;
    if seat.disposition != "occupied" {
        return Err(Refusal::LaneDisposition {
            lane,
            disposition: seat.disposition.clone(),
        });
    }
    if seat.occupant != Some(request.occupant) {
        return Err(Refusal::Displaced {
            lane,
            seated: seat.occupant,
            named: request.occupant,
        });
    }
    Ok(())
}

fn build_write(request: Request, observed: Observation) -> Result<JournalWrite, LapseError> {
    let task = request.intent.task.clone();
    let holder = observed.claim.body().actor.clone();
    let lane = observed.claim.body().lane;
    let origin = format!("task:{task}")
        .parse()
        .expect("task handle forms an origin");
    let mut write = JournalWrite::new(holder.clone(), origin, request.at, Producer::BusTimer);
    write.guard(change_guard(request.clone(), observed.clone()));
    let fact = write
        .fact(
            body::ClaimLapsed {
                task: task.clone(),
                occupant: request.occupant,
                expiry: request.expiry,
            },
            FactCause::Root,
        )
        .map_err(LapseError::Build)?;
    let mut next = observed.claim.body().clone();
    next.disposition = ClaimDisposition::Lapsed;
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
                kind: CommandKind::ClaimLapse,
                expected: witnesses(&request, &observed, lane),
                body: request.intent,
            },
            request.epoch,
            intent::ClaimAnswer { task, holder },
        )
        .map_err(LapseError::Build)?;
    Ok(write)
}

fn witnesses(request: &Request, observed: &Observation, lane: CanonicalUuid) -> Vec<Witness> {
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
            lane,
            occupant: request.occupant,
            epoch: request.epoch,
        },
    ]
}

fn change_guard(request: Request, expected: Observation) -> JournalGuard {
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

fn map_commit_error(error: CommitError) -> LapseError {
    match error {
        CommitError::Refused(refusal) if refusal.reason().starts_with(CHANGED) => {
            let detail = refusal
                .reason()
                .strip_prefix(CHANGED)
                .expect("prefix matched")
                .trim_start_matches(": ")
                .to_owned();
            LapseError::Refused(Refusal::Changed(detail))
        }
        error => LapseError::Commit(error),
    }
}

#[cfg(test)]
mod tests {
    use super::{LapseError, Refusal, Request, lapse, lapse_after_observation};
    use crate::allocate::{Request as WinRequest, win};
    use journal::{FactCause, Journal, JournalWrite};
    use wire::{
        Actor, CanonicalUuid, Instant, LaneResidency, Origin, Phase, Producer, ResidencyHead,
        ResidentState, TaskHandle, TaskHead, WorkflowHead, body, intent,
    };

    fn uuid(last: u8) -> CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131164{last:02}")
            .parse()
            .expect("uuid")
    }

    fn instant(minute: u8) -> Instant {
        format!("2026-08-01T07:{minute:02}:00.000Z")
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

    fn lapse_request(command: u8) -> Request {
        Request {
            projects: projects(),
            command: uuid(command),
            occupant: uuid(21),
            epoch: uuid(22),
            expiry: instant(20),
            at: instant(20),
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
                    title: "Lapse".to_owned(),
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
            title: "Lapse".to_owned(),
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
                lineage_id: uuid(20),
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
    fn expiry_changed_after_observation_refuses_before_a_second_fact() {
        let mut journal = claimed();
        let outer = lapse_after_observation(&mut journal, lapse_request(32), |journal| {
            lapse(journal, lapse_request(33))?;
            Ok(())
        });
        assert!(matches!(
            outer,
            Err(LapseError::Refused(Refusal::Changed(_)))
        ));
        assert_eq!(journal.fact_snapshot().expect("facts").facts().len(), 3);
        assert_eq!(journal.verify().expect("verify").receipts, 2);
    }
}
