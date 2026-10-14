//! Long-lived service task and turn authority.

use std::io::{self, ErrorKind};
use std::path::PathBuf;

use journal::JournalReader;
use serde::{Deserialize, Serialize};
use wire::{Actor, CanonicalUuid, ClaimDisposition, Seq, TaskStamp};
/// The exact task possession that owns one unbounded command.
///
/// A new phase or possession mints another win, so neither can inherit the
/// service. Occupant succession inside the same lane and claim does not.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(in crate::resident) struct ServiceClaim {
    task: TaskStamp,
    claimed: Seq,
    phase: String,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
}

impl ServiceClaim {
    pub(super) fn matches(&self, heads: &journal::TaskHeads) -> bool {
        heads.statement().body().born == self.task
            && heads.workflow().body().phase.as_str() == self.phase
            && heads.claim().is_some_and(|head| {
                let claim = head.body();
                claim.disposition == ClaimDisposition::Active
                    && claim.actor == Actor::Lane(self.lane)
                    && claim.claimed == self.claimed
                    && claim.lane == self.lane
            })
    }

    #[cfg(test)]
    pub(super) fn for_tests(task: TaskStamp, lane: CanonicalUuid, occupant: CanonicalUuid) -> Self {
        Self {
            task,
            claimed: Seq::new(1).expect("fixture claim position"),
            phase: "implementation".to_owned(),
            lane,
            occupant,
        }
    }
}

/// The lifetime owner of one unbounded command.
pub(super) enum ServiceOwner {
    /// The exact active claim survives occupant succession.
    Task(ServiceClaim),
    /// An unclaimed launch survives only its current occupant turn.
    Turn(CanonicalUuid),
}

/// One initially admitted owner, retained until its durable request is
/// published and the same authority can validate it again.
pub(in crate::resident) struct ServiceAdmission<'a> {
    pub(super) owner: ServiceOwner,
    authority: &'a ServiceAuthority,
}

impl ServiceAdmission<'_> {
    pub(super) fn validate(&self) -> io::Result<()> {
        let owned = match &self.owner {
            ServiceOwner::Task(claim) => self.authority.owns(claim)?,
            ServiceOwner::Turn(turn) => {
                *turn == self.authority.occupant && self.authority.seated()?
            }
        };
        owned.then_some(()).ok_or_else(|| {
            io::Error::new(
                ErrorKind::PermissionDenied,
                "the service owner ended during launch",
            )
        })
    }
}

/// Read-only task and turn authority available to a detached occupant's terminal.
#[derive(Clone, Debug)]
pub(in crate::resident) struct ServiceAuthority {
    journal: PathBuf,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
}

impl ServiceAuthority {
    pub(in crate::resident) fn new(
        journal: PathBuf,
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
    ) -> Self {
        Self {
            journal,
            lane,
            occupant,
        }
    }

    /// Capture one job's ambient task and, for a service, its lifetime owner
    /// at the same authority read.
    pub(in crate::resident) fn admit_job(
        &self,
        service: bool,
    ) -> io::Result<(Option<TaskStamp>, Option<ServiceAdmission<'_>>)> {
        let (task, claim) = self.claim(service)?;
        let admission = service.then(|| ServiceAdmission {
            owner: claim.map_or(ServiceOwner::Turn(self.occupant), ServiceOwner::Task),
            authority: self,
        });
        Ok((task, admission))
    }

    /// The current occupant's active task and, only when needed for a service,
    /// its exact claim. Being seated is required even when there is no claim:
    /// only a live turn can launch.
    fn claim(&self, service: bool) -> io::Result<(Option<TaskStamp>, Option<ServiceClaim>)> {
        let reader = JournalReader::open(&self.journal).map_err(io::Error::other)?;
        reader
            .read_with(|read| {
                let residency = read
                    .residency_head()
                    .map_err(io::Error::other)?
                    .ok_or_else(|| {
                        io::Error::new(ErrorKind::PermissionDenied, "resident authority is absent")
                    })?;
                let seated = residency
                    .body()
                    .lanes
                    .iter()
                    .any(|row| row.lane == self.lane && row.occupant == Some(self.occupant));
                if !seated {
                    return Err(io::Error::new(
                        ErrorKind::PermissionDenied,
                        "the job caller is not the lane's current occupant",
                    ));
                }
                let Some(heads) = read
                    .active_claim_for_actor(&Actor::Lane(self.lane))
                    .map_err(io::Error::other)?
                else {
                    return Ok((None, None));
                };
                let claim = heads
                    .claim()
                    .expect("an active claim lookup returns a claim head")
                    .body();
                if claim.lane != self.lane || claim.occupant != self.occupant {
                    return Err(io::Error::new(
                        ErrorKind::PermissionDenied,
                        "the active task claim belongs to another lane occupant",
                    ));
                }
                let task = heads.statement().body().born.clone();
                let service_claim = service.then(|| ServiceClaim {
                    task: task.clone(),
                    claimed: claim.claimed,
                    phase: heads.workflow().body().phase.to_string(),
                    lane: self.lane,
                    occupant: self.occupant,
                });
                Ok((Some(task), service_claim))
            })
            .map_err(io::Error::other)
    }

    fn seated(&self) -> io::Result<bool> {
        let reader = JournalReader::open(&self.journal).map_err(io::Error::other)?;
        reader
            .read_with(|read| {
                Ok::<bool, io::Error>(
                    read.residency_head()
                        .map_err(io::Error::other)?
                        .is_some_and(|head| {
                            head.body().lanes.iter().any(|row| {
                                row.lane == self.lane && row.occupant == Some(self.occupant)
                            })
                        }),
                )
            })
            .map_err(io::Error::other)
    }

    fn owns(&self, claim: &ServiceClaim) -> io::Result<bool> {
        let reader = JournalReader::open(&self.journal).map_err(io::Error::other)?;
        reader
            .read_with(|read| {
                let heads = read.task_heads_for(&claim.task).map_err(io::Error::other)?;
                Ok::<bool, io::Error>(heads.as_ref().is_some_and(|heads| claim.matches(heads)))
            })
            .map_err(io::Error::other)
    }
}
