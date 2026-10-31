//! Claim-lifetime possession of configured resource-pool shards.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use board::Entry;
use sill::{HeldShard, LockAttempt, LockAvailability, LockTopology};
use wire::intent::CommandAnswer;
use wire::{Actor, ClaimDisposition, Environment, Outcome, TaskStamp};

use super::{environment_fault, refusal};

/// Resident-owned shards retained for as long as their task claim is active.
pub(crate) struct ResourceClaims {
    topology: LockTopology,
    held: Mutex<BTreeMap<TaskStamp, Vec<HeldShard>>>,
}

impl ResourceClaims {
    pub(crate) fn new(topology: LockTopology) -> Self {
        Self {
            topology,
            held: Mutex::new(BTreeMap::new()),
        }
    }

    /// Reconstruct claim-held shards before the command surface accepts work.
    pub(crate) fn restore(&self, bus: &bus::Bus) -> Result<(), Outcome<CommandAnswer>> {
        let snapshot = super::read_guard::validated_snapshot(bus, Actor::Operator)?;
        self.reconcile(&snapshot.entries)
    }

    /// Reconcile retained possessions, then add current availability to the
    /// environment used by one allocation decision.
    pub(crate) fn observe(
        &self,
        base: Environment,
        entries: &[Entry],
    ) -> Result<Environment, Outcome<CommandAnswer>> {
        self.reconcile(entries)?;
        let report = self.topology.report().map_err(resource_fault)?;
        let required = entries
            .iter()
            .flat_map(|entry| entry.statement().resources.iter())
            .map(|(name, _)| name.clone())
            .collect::<BTreeSet<_>>();
        let mut environment = base;
        for name in required {
            let available = report
                .pools
                .iter()
                .find(|pool| pool.name == name.as_str())
                .map_or(0, |pool| {
                    pool.entries
                        .iter()
                        .filter(|entry| entry.availability == LockAvailability::Available)
                        .count() as u32
                });
            environment = environment.with_resource_pool(name, available);
        }
        Ok(environment)
    }

    /// Reserve the selected task's requirement before its claim is published.
    pub(crate) fn reserve<'a>(
        &'a self,
        entry: &Entry,
        actor: &Actor,
        entries: &[Entry],
    ) -> Result<ResourceReservation<'a>, Outcome<CommandAnswer>> {
        self.reconcile(entries)?;
        if entry.statement().resources.is_empty() {
            return Ok(ResourceReservation::empty(self));
        }
        let task = entry.born().clone();
        let context = owner_context(actor, &task)?;
        let retained = self.held.lock().unwrap_or_else(|error| error.into_inner());
        if retained.contains_key(&task) {
            return Ok(ResourceReservation::transfer(self, task, context));
        }
        let shards = acquire_all(
            &self.topology,
            &entry.statement().resources,
            context.as_deref(),
        )?;
        Ok(ResourceReservation::acquired(self, task, shards))
    }

    fn reconcile(&self, entries: &[Entry]) -> Result<(), Outcome<CommandAnswer>> {
        let active = entries
            .iter()
            .filter_map(|entry| {
                let claim = entry.claim()?;
                (claim.disposition == ClaimDisposition::Active).then_some((entry, &claim.actor))
            })
            .collect::<Vec<_>>();
        let active_tasks = active
            .iter()
            .map(|(entry, _)| entry.born().clone())
            .collect::<BTreeSet<_>>();
        let mut retained = self.held.lock().unwrap_or_else(|error| error.into_inner());
        retained.retain(|task, _| active_tasks.contains(task));
        for (entry, actor) in active {
            let task = entry.born();
            if retained.contains_key(task) {
                continue;
            }
            if entry.statement().resources.is_empty() {
                continue;
            }
            let context = owner_context(actor, task)?;
            match acquire_all(
                &self.topology,
                &entry.statement().resources,
                context.as_deref(),
            ) {
                Ok(shards) => {
                    retained.insert(task.clone(), shards);
                }
                Err(Outcome::Refusal(_)) => {
                    // Another process still protects the resource. Its kernel
                    // possession already makes this pool unavailable.
                }
                Err(outcome) => return Err(outcome),
            }
        }
        Ok(())
    }
}

fn acquire_all(
    topology: &LockTopology,
    resources: &wire::ResourceRequirements,
    context: Option<&str>,
) -> Result<Vec<HeldShard>, Outcome<CommandAnswer>> {
    let mut shards = Vec::new();
    for (name, count) in resources.iter() {
        shards.extend(acquire(topology, name.as_str(), count, context)?);
    }
    Ok(shards)
}

fn acquire(
    topology: &LockTopology,
    name: &str,
    count: u32,
    context: Option<&str>,
) -> Result<Vec<HeldShard>, Outcome<CommandAnswer>> {
    let mut shards = Vec::with_capacity(count as usize);
    for _ in 0..count {
        match topology.try_acquire(name, None).map_err(resource_fault)? {
            LockAttempt::Acquired(mut shard) => {
                shard
                    .set_context(context.map(ToOwned::to_owned))
                    .map_err(|error| {
                        environment_fault("board::resource-owner-unwritable", &error.to_string())
                    })?;
                shards.push(shard);
            }
            LockAttempt::Contended { .. } => {
                return Err(refusal(
                    "board::resource-unavailable",
                    Some(name.to_owned()),
                    "another claim or process holds the required resource pool",
                ));
            }
        }
    }
    Ok(shards)
}

fn owner_context(
    actor: &Actor,
    task: &TaskStamp,
) -> Result<Option<String>, Outcome<CommandAnswer>> {
    let Actor::Lane(lane) = actor else {
        return Ok(None);
    };
    serde_json::to_string(&wire::ResourceClaimOwner {
        lane: *lane,
        task: task.clone(),
    })
    .map(Some)
    .map_err(|error| environment_fault("board::resource-owner-invalid", &error.to_string()))
}

fn resource_fault(error: sill::LockSurfaceError) -> Outcome<CommandAnswer> {
    environment_fault("board::resource-pool-unavailable", &error.to_string())
}

/// Shards acquired before the claim commit, transferred to resident retention
/// only after the journal accepts that claim.
pub(crate) struct ResourceReservation<'a> {
    owner: &'a ResourceClaims,
    task: Option<TaskStamp>,
    acquired: Vec<HeldShard>,
    transfer_context: Option<String>,
}

impl<'a> ResourceReservation<'a> {
    fn empty(owner: &'a ResourceClaims) -> Self {
        Self {
            owner,
            task: None,
            acquired: Vec::new(),
            transfer_context: None,
        }
    }

    fn acquired(owner: &'a ResourceClaims, task: TaskStamp, acquired: Vec<HeldShard>) -> Self {
        Self {
            owner,
            task: Some(task),
            acquired,
            transfer_context: None,
        }
    }

    fn transfer(owner: &'a ResourceClaims, task: TaskStamp, context: Option<String>) -> Self {
        Self {
            owner,
            task: Some(task),
            acquired: Vec::new(),
            transfer_context: context,
        }
    }

    pub(crate) fn commit(mut self) -> Result<(), Outcome<CommandAnswer>> {
        let Some(task) = self.task.take() else {
            return Ok(());
        };
        let mut retained = self
            .owner
            .held
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(context) = self.transfer_context.take()
            && let Some(shards) = retained.get_mut(&task)
        {
            for shard in shards {
                shard.set_context(Some(context.clone())).map_err(|error| {
                    environment_fault("board::resource-owner-unwritable", &error.to_string())
                })?;
            }
            return Ok(());
        }
        retained.insert(task, std::mem::take(&mut self.acquired));
        Ok(())
    }
}
