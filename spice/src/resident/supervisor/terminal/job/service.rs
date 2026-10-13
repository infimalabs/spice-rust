//! Long-lived service claim authority.

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
}

/// One initially admitted claim, retained until its durable request is
/// published and the same authority can validate it again.
pub(in crate::resident) struct ServiceAdmission<'a> {
    pub(super) claim: ServiceClaim,
    authority: &'a ServiceAuthority,
}

impl ServiceAdmission<'_> {
    pub(super) fn validate(&self) -> io::Result<()> {
        self.authority
            .owns(&self.claim)?
            .then_some(())
            .ok_or_else(|| {
                io::Error::new(
                    ErrorKind::PermissionDenied,
                    "the service task claim ended during launch",
                )
            })
    }
}

/// Read-only task authority available to a detached occupant's terminal.
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

    pub(in crate::resident) fn admit(&self) -> io::Result<ServiceAdmission<'_>> {
        Ok(ServiceAdmission {
            claim: self.claim()?,
            authority: self,
        })
    }

    fn claim(&self) -> io::Result<ServiceClaim> {
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
                        "the service caller is not the lane's current occupant",
                    ));
                }
                let heads = read
                    .active_claim_for_actor(&Actor::Lane(self.lane))
                    .map_err(io::Error::other)?
                    .ok_or_else(|| {
                        io::Error::new(
                            ErrorKind::PermissionDenied,
                            "service launch requires an active task claim",
                        )
                    })?;
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
                Ok(ServiceClaim {
                    task: heads.statement().body().born.clone(),
                    claimed: claim.claimed,
                    phase: heads.workflow().body().phase.to_string(),
                    lane: self.lane,
                    occupant: self.occupant,
                })
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
