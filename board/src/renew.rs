//! Journal-backed renewal of task possession.

use std::error::Error;
use std::fmt;

use journal::{
    BuildError, CommitError, FactCause, GuardError, Journal, JournalGuard, JournalHead,
    JournalWrite, ReadError,
};
use wire::{
    CanonicalUuid, Command, CommandKind, Instant, Producer, Seq, TaskHandle, WireVersion, Witness,
    body, intent,
};

use crate::{Active, ClaimRefusal, Entry, ProjectPolicy};

const CHANGED: &str = "renewal inputs changed before extension could commit";

/// Durable timer inputs for one claim renewal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    /// Captured project vocabulary used by both renewal observations.
    pub projects: ProjectPolicy,
    /// Identity stored with the canonical command receipt.
    pub command: CanonicalUuid,
    /// Occupant whose renewal timer fired.
    pub occupant: CanonicalUuid,
    /// Resident generation permitted to renew the lease.
    pub epoch: CanonicalUuid,
    /// Current instant stored as the new renewal instant.
    pub at: Instant,
    /// Proposed strict extension of the current expiry.
    pub until: Instant,
    /// Exact task named by the renewal command.
    pub intent: intent::ClaimRenew,
}

/// The renewed task and its one-fact committed range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Renewed {
    task: TaskHandle,
    first: Seq,
    last: Seq,
}

impl Renewed {
    /// Task whose live possession was extended.
    #[must_use]
    pub const fn task(&self) -> &TaskHandle {
        &self.task
    }

    /// First fact position assigned to the renewal transaction.
    #[must_use]
    pub const fn first(&self) -> Seq {
        self.first
    }

    /// Last fact position assigned to the renewal transaction.
    #[must_use]
    pub const fn last(&self) -> Seq {
        self.last
    }
}

/// Expected reasons a timer cannot renew possession.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Refusal {
    MissingTask(TaskHandle),
    MissingClaim,
    MissingLease,
    Claim(ClaimRefusal),
    ClaimEpoch {
        holding: CanonicalUuid,
        named: CanonicalUuid,
    },
    ClaimOccupant {
        holding: CanonicalUuid,
        named: CanonicalUuid,
    },
    NonExtending {
        stored: Instant,
        proposed: Instant,
    },
    BeyondMaximum {
        proposed: Instant,
        maximum: Instant,
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
            Self::MissingClaim => formatter.write_str("renewal requires a claim head"),
            Self::MissingLease => formatter.write_str("renewal requires a lease head"),
            Self::Claim(error) => write!(formatter, "claim refuses renewal: {error}"),
            Self::ClaimEpoch { holding, named } => {
                write!(formatter, "claim epoch is {holding}, not {named}")
            }
            Self::ClaimOccupant { holding, named } => {
                write!(formatter, "claim occupant is {holding}, not {named}")
            }
            Self::NonExtending { stored, proposed } => write!(
                formatter,
                "proposed expiry {proposed} does not extend stored expiry {stored}"
            ),
            Self::BeyondMaximum { proposed, maximum } => {
                write!(
                    formatter,
                    "proposed expiry {proposed} exceeds maximum {maximum}"
                )
            }
            Self::MissingResidency => formatter.write_str("renewal requires a residency head"),
            Self::ResidentState(state) => {
                write!(formatter, "resident state {state:?} refuses renewal")
            }
            Self::StaleResidentEpoch { seated, named } => {
                write!(formatter, "resident epoch is {seated}, not {named}")
            }
            Self::MissingLane(lane) => write!(formatter, "resident has no lane {lane}"),
            Self::LaneDisposition { lane, disposition } => write!(
                formatter,
                "lane {lane} disposition {disposition:?} refuses renewal"
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

impl Error for Refusal {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Claim(error) => Some(error),
            _ => None,
        }
    }
}

/// A typed refusal or fault from lease renewal.
#[derive(Debug)]
pub enum RenewError {
    Refused(Refusal),
    Read(GuardError),
    Build(BuildError),
    Commit(CommitError),
}

impl fmt::Display for RenewError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(error) => write!(formatter, "claim renewal refused: {error}"),
            Self::Read(error) => write!(formatter, "claim renewal read fault: {error}"),
            Self::Build(error) => write!(formatter, "claim renewal build fault: {error}"),
            Self::Commit(error) => write!(formatter, "claim renewal commit fault: {error}"),
        }
    }
}

impl Error for RenewError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Refused(error) => Some(error),
            Self::Read(error) => Some(error),
            Self::Build(error) => Some(error),
            Self::Commit(error) => Some(error),
        }
    }
}

impl From<Refusal> for RenewError {
    fn from(error: Refusal) -> Self {
        Self::Refused(error)
    }
}

/// Re-prove and atomically extend one live active claim.
pub fn renew(journal: &mut Journal, request: Request) -> Result<Renewed, RenewError> {
    renew_after_observation(journal, request, |_| Ok(()))
}

fn renew_after_observation<F>(
    journal: &mut Journal,
    request: Request,
    after: F,
) -> Result<Renewed, RenewError>
where
    F: FnOnce(&mut Journal) -> Result<(), RenewError>,
{
    let observed = read_observation(journal, &request)?;
    after(journal)?;
    let task = request.intent.task.clone();
    let write = build_write(request, observed)?;
    let applied = journal.apply(write).map_err(map_commit_error)?;
    Ok(Renewed {
        task,
        first: applied.first(),
        last: applied.last(),
    })
}

fn read_observation(journal: &Journal, request: &Request) -> Result<Observation, RenewError> {
    journal
        .read_with(|read| observe(read, request))
        .map_err(|error| match error {
            ReadError::Journal(error) | ReadError::Answer(ObserveError::Fault(error)) => {
                RenewError::Read(error)
            }
            ReadError::Answer(ObserveError::Refused(error)) => RenewError::Refused(error),
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
    let entry = Entry::new(
        statement.body().clone(),
        workflow.body().clone(),
        Some(claim.body().clone()),
        Some(lease.body().clone()),
        &request.projects,
    )
    .map_err(|error| GuardError::Invalid(format!("task {task} is invalid: {error}")))?;
    Active::admit(&entry, &claim.body().actor, request.at).map_err(Refusal::Claim)?;
    admit_extension(claim.body(), lease.body(), request)?;
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

fn admit_extension(
    claim: &wire::ClaimHead,
    lease: &wire::LeaseHead,
    request: &Request,
) -> Result<(), Refusal> {
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
    if request.until <= lease.expiry {
        return Err(Refusal::NonExtending {
            stored: lease.expiry,
            proposed: request.until,
        });
    }
    if request.until > lease.maximum {
        return Err(Refusal::BeyondMaximum {
            proposed: request.until,
            maximum: lease.maximum,
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

fn build_write(request: Request, observed: Observation) -> Result<JournalWrite, RenewError> {
    let task = request.intent.task.clone();
    let holder = observed.claim.body().actor.clone();
    let lane = observed.claim.body().lane;
    let maximum = observed.lease.body().maximum;
    let origin = format!("task:{task}")
        .parse()
        .expect("task handle forms an origin");
    let mut write = JournalWrite::new(holder.clone(), origin, request.at, Producer::BusTimer);
    write.guard(change_guard(request.clone(), observed.clone()));
    let fact = write
        .fact(
            body::ClaimRenewed {
                task: task.clone(),
                epoch: request.epoch,
                until: request.until,
                maximum,
            },
            FactCause::Root,
        )
        .map_err(RenewError::Build)?;
    let mut next = observed.lease.body().clone();
    next.renewal = request.at;
    next.expiry = request.until;
    write.lease_head(task.clone(), fact, move |_| next);
    write
        .command_result(
            Command {
                v: WireVersion::CURRENT,
                id: request.command,
                epoch: request.epoch,
                kind: CommandKind::ClaimRenew,
                expected: witnesses(&request, &observed, lane),
                body: request.intent,
            },
            request.epoch,
            intent::ClaimAnswer {
                task,
                holder,
                warnings: Vec::new(),
                next: None,
            },
        )
        .map_err(RenewError::Build)?;
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

fn map_commit_error(error: CommitError) -> RenewError {
    match error {
        CommitError::Refused(refusal) if refusal.reason().starts_with(CHANGED) => {
            let detail = refusal
                .reason()
                .strip_prefix(CHANGED)
                .expect("prefix matched")
                .trim_start_matches(": ")
                .to_owned();
            RenewError::Refused(Refusal::Changed(detail))
        }
        error => RenewError::Commit(error),
    }
}

#[cfg(test)]
mod tests {
    use super::{Refusal, RenewError, Request, renew, renew_after_observation};
    use crate::allocate::{Request as WinRequest, win};
    use journal::{FactCause, Journal, JournalWrite};
    use wire::{
        Actor, CanonicalUuid, Instant, LaneResidency, Origin, Phase, Producer, ResidencyHead,
        ResidentState, TaskHandle, TaskHead, WorkflowHead, body, intent,
    };

    fn uuid(last: u8) -> CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131165{last:02}")
            .parse()
            .expect("uuid")
    }

    fn instant(minute: u8) -> Instant {
        format!("2026-08-01T08:{minute:02}:00.000Z")
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

    fn renew_request(command: u8) -> Request {
        Request {
            projects: projects(),
            command: uuid(command),
            occupant: uuid(21),
            epoch: uuid(22),
            at: instant(10),
            until: instant(25),
            intent: intent::ClaimRenew {
                task: task(),
                lease_seconds: 15,
            },
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
                    title: "Renew".to_owned(),
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
            title: "Renew".to_owned(),
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
            subscriptions: vec![wire::LineageSubscription {
                lineage_id: uuid(20),
                policy: wire::intent::LineageLife::Drain,
                manual_projects: Vec::new(),
                auto_projects: Vec::new(),
            }],
        });
        journal.apply(write).expect("seed");
        win(
            &mut journal,
            WinRequest {
                band: 2,
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
                source: body::SourceBoundary::Proven { heads: Vec::new() },
            },
        )
        .expect("win");
        journal
    }

    #[test]
    fn renewal_changed_after_observation_refuses_before_a_second_fact() {
        let mut journal = claimed();
        let outer = renew_after_observation(&mut journal, renew_request(32), |journal| {
            renew(journal, renew_request(33))?;
            Ok(())
        });
        assert!(matches!(
            outer,
            Err(RenewError::Refused(Refusal::Changed(_)))
        ));
        assert_eq!(journal.fact_snapshot().expect("facts").facts().len(), 3);
        assert_eq!(journal.verify().expect("verify").receipts, 2);
    }
}
