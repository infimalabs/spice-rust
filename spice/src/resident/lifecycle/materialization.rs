//! Incremental lifecycle projection retained across durable commits.

use std::collections::{BTreeMap, BTreeSet};

use board::Entry;
use journal::TaskHeads;
use wire::{Actor, CanonicalUuid, ClaimDisposition, Instant, Seq, TaskStamp};

use super::{CommandResult, store_fault};

#[cfg_attr(test, derive(Clone, Debug, PartialEq, Eq))]
pub(super) struct Projection {
    cursor: Option<Seq>,
    ledger: steer::Ledger,
    operator_directives: BTreeSet<CanonicalUuid>,
    entries: Vec<Entry>,
    heads: BTreeMap<TaskStamp, TaskHeads>,
    fact_instants: BTreeMap<Seq, Instant>,
    ready_at: BTreeMap<TaskStamp, Instant>,
    dependents: BTreeMap<TaskStamp, BTreeSet<TaskStamp>>,
    creator_lineages: BTreeMap<TaskStamp, Option<CanonicalUuid>>,
}

struct EntryUpdate {
    heads: TaskHeads,
    entry: Entry,
}

struct AdvancePlan {
    cursor: Seq,
    operator_directives: Vec<CanonicalUuid>,
    fact_instants: BTreeMap<Seq, Instant>,
    entries: BTreeMap<TaskStamp, EntryUpdate>,
    creator_lineages: BTreeMap<TaskStamp, Option<CanonicalUuid>>,
    ready_at: BTreeMap<TaskStamp, Instant>,
}

type FactDelta<'a> = Vec<&'a wire::Envelope<wire::AnyFact>>;

impl Projection {
    pub(super) fn new() -> Self {
        Self {
            cursor: None,
            ledger: steer::Ledger::new(),
            operator_directives: BTreeSet::new(),
            entries: Vec::new(),
            heads: BTreeMap::new(),
            fact_instants: BTreeMap::new(),
            ready_at: BTreeMap::new(),
            dependents: BTreeMap::new(),
            creator_lineages: BTreeMap::new(),
        }
    }

    pub(super) const fn cursor(&self) -> Option<Seq> {
        self.cursor
    }

    pub(super) fn entries(&self) -> &[Entry] {
        &self.entries
    }

    #[cfg(test)]
    pub(super) const fn ledger(&self) -> &steer::Ledger {
        &self.ledger
    }

    pub(super) fn has_operator_steering(&self, lane: CanonicalUuid) -> bool {
        self.ledger
            .pending(lane)
            .iter()
            .any(|directive| self.operator_directives.contains(&directive.directive()))
    }

    pub(super) const fn ready_at(&self) -> &BTreeMap<TaskStamp, Instant> {
        &self.ready_at
    }

    pub(super) fn advance(
        &mut self,
        snapshot: &bus::TaskSnapshot,
        projects: &board::ProjectPolicy,
    ) -> CommandResult<()> {
        let Some((cursor, delta)) = self.committed_delta(snapshot)? else {
            return Ok(());
        };
        let plan = self.prepare(snapshot, projects, cursor, &delta)?;

        // This is the final fallible operation. `Ledger::advance` rolls its
        // touched keys back on refusal, and applying the prepared plan below is
        // infallible, so no partial projection can escape to a retry.
        self.ledger
            .advance(delta.iter().copied())
            .map_err(|error| store_fault("lifecycle::steering-invalid", &error.to_string()))?;
        self.apply(plan);
        Ok(())
    }

    fn committed_delta<'a>(
        &self,
        snapshot: &'a bus::TaskSnapshot,
    ) -> CommandResult<Option<(Seq, FactDelta<'a>)>> {
        let cursor = snapshot.cursor().ok_or_else(|| {
            store_fault(
                "lifecycle::snapshot-empty",
                "a running resident has no durable lifecycle snapshot cursor",
            )
        })?;
        if self.cursor.is_some_and(|admitted| cursor < admitted) {
            return Err(store_fault(
                "lifecycle::snapshot-retreated",
                &format!(
                    "lifecycle snapshot cursor {cursor} precedes admitted cursor {:?}",
                    self.cursor
                ),
            ));
        }
        if self.cursor == Some(cursor) {
            return Ok(None);
        }

        let delta = snapshot.facts().after(self.cursor).collect::<Vec<_>>();
        if delta.iter().any(|fact| fact.seq > cursor) {
            return Err(store_fault(
                "lifecycle::snapshot-gap",
                &format!(
                    "lifecycle delta after {:?} crosses cursor {cursor} at {:?}",
                    self.cursor,
                    delta
                        .iter()
                        .find(|fact| fact.seq > cursor)
                        .map(|fact| fact.seq)
                ),
            ));
        }
        Ok(Some((cursor, delta)))
    }

    fn prepare(
        &self,
        snapshot: &bus::TaskSnapshot,
        projects: &board::ProjectPolicy,
        cursor: Seq,
        delta: &[&wire::Envelope<wire::AnyFact>],
    ) -> CommandResult<AdvancePlan> {
        let changed = self.changed_tasks(snapshot, delta)?;
        let creator_lineages = creator_lineage_events(delta)?;
        let entries = self.entry_updates(snapshot, projects, &changed, &creator_lineages)?;
        let affected = self.affected_tasks(&changed, &entries);
        let fact_instants = delta
            .iter()
            .map(|fact| (fact.seq, fact.at))
            .collect::<BTreeMap<_, _>>();
        let operator_directives = delta
            .iter()
            .filter(|envelope| envelope.actor == Actor::Operator)
            .filter_map(|envelope| {
                envelope
                    .fact
                    .read::<wire::body::SteerSent>()
                    .map(|body| body.directive)
            })
            .collect();
        let ready_at = affected
            .into_iter()
            .map(|task| {
                self.projected_ready_instant(&task, &entries, &fact_instants)
                    .map(|ready| (task, ready))
            })
            .collect::<CommandResult<BTreeMap<_, _>>>()?;
        Ok(AdvancePlan {
            cursor,
            operator_directives,
            fact_instants,
            entries,
            creator_lineages,
            ready_at,
        })
    }

    fn changed_tasks(
        &self,
        snapshot: &bus::TaskSnapshot,
        delta: &[&wire::Envelope<wire::AnyFact>],
    ) -> CommandResult<BTreeSet<TaskStamp>> {
        let changed = if self.cursor.is_none() {
            snapshot
                .tasks()
                .iter()
                .map(|heads| heads.statement().body().born.clone())
                .collect::<BTreeSet<_>>()
        } else {
            delta
                .iter()
                .filter_map(|fact| {
                    changed_task(fact).map(|task| {
                        task.map_err(|error| {
                            store_fault(
                                "lifecycle::task-fact-invalid",
                                &format!(
                                    "task-bearing fact {} at {} has an invalid task: {error}",
                                    fact.fact.kind(),
                                    fact.seq
                                ),
                            )
                        })
                    })
                })
                .collect::<CommandResult<BTreeSet<_>>>()?
        };
        Ok(changed)
    }

    fn entry_updates(
        &self,
        snapshot: &bus::TaskSnapshot,
        projects: &board::ProjectPolicy,
        changed: &BTreeSet<TaskStamp>,
        creator_lineages: &BTreeMap<TaskStamp, Option<CanonicalUuid>>,
    ) -> CommandResult<BTreeMap<TaskStamp, EntryUpdate>> {
        let mut entries = BTreeMap::new();
        for task in changed {
            let heads = snapshot
                .tasks()
                .binary_search_by(|heads| heads.statement().body().born.cmp(task))
                .ok()
                .map(|index| snapshot.tasks()[index].clone())
                .ok_or_else(|| {
                    store_fault(
                        "lifecycle::task-head-missing",
                        &format!("changed task {task} has no current storage heads"),
                    )
                })?;
            let creator_lineage = creator_lineages
                .get(task)
                .or_else(|| self.creator_lineages.get(task))
                .copied()
                .flatten();
            let entry = entry(&heads, projects)?.with_creator_lineage(creator_lineage);
            entries.insert(task.clone(), EntryUpdate { heads, entry });
        }
        Ok(entries)
    }

    fn affected_tasks(
        &self,
        changed: &BTreeSet<TaskStamp>,
        entries: &BTreeMap<TaskStamp, EntryUpdate>,
    ) -> BTreeSet<TaskStamp> {
        let mut affected = changed.clone();
        for dependency in changed {
            if let Some(rows) = self.dependents.get(dependency) {
                affected.extend(
                    rows.iter()
                        .filter(|task| {
                            self.projected_entry(entries, task).is_some_and(|entry| {
                                entry.statement().dependencies.contains(dependency)
                            })
                        })
                        .cloned(),
                );
            }
            for (task, update) in entries {
                if update.entry.statement().dependencies.contains(dependency) {
                    affected.insert(task.clone());
                }
            }
        }
        if self.cursor.is_none() {
            affected.extend(entries.keys().cloned());
        }
        affected
    }

    fn apply(&mut self, plan: AdvancePlan) {
        self.operator_directives.extend(plan.operator_directives);
        self.fact_instants.extend(plan.fact_instants);
        for (task, update) in plan.entries {
            self.apply_entry(task, update);
        }
        self.creator_lineages.extend(plan.creator_lineages);
        self.ready_at.extend(plan.ready_at);
        self.cursor = Some(plan.cursor);
    }

    fn apply_entry(&mut self, task: TaskStamp, update: EntryUpdate) {
        if let Ok(index) = self.entries.binary_search_by(|row| row.born().cmp(&task)) {
            for dependency in &self.entries[index].statement().dependencies {
                if let Some(rows) = self.dependents.get_mut(dependency) {
                    rows.remove(&task);
                    if rows.is_empty() {
                        self.dependents.remove(dependency);
                    }
                }
            }
            self.entries[index] = update.entry;
        } else {
            let index = self
                .entries
                .binary_search_by(|row| row.born().cmp(&task))
                .expect_err("the changed task is not present");
            self.entries.insert(index, update.entry);
        }
        let entry = self
            .entries
            .binary_search_by(|row| row.born().cmp(&task))
            .ok()
            .map(|index| &self.entries[index])
            .expect("the changed entry was inserted");
        for dependency in &entry.statement().dependencies {
            self.dependents
                .entry(dependency.clone())
                .or_default()
                .insert(task.clone());
        }
        self.heads.insert(task, update.heads);
    }

    fn projected_entry<'a>(
        &'a self,
        entries: &'a BTreeMap<TaskStamp, EntryUpdate>,
        task: &TaskStamp,
    ) -> Option<&'a Entry> {
        entries.get(task).map(|update| &update.entry).or_else(|| {
            self.entries
                .binary_search_by(|row| row.born().cmp(task))
                .ok()
                .map(|index| &self.entries[index])
        })
    }

    fn projected_heads<'a>(
        &'a self,
        entries: &'a BTreeMap<TaskStamp, EntryUpdate>,
        task: &TaskStamp,
    ) -> Option<&'a TaskHeads> {
        entries
            .get(task)
            .map(|update| &update.heads)
            .or_else(|| self.heads.get(task))
    }

    fn projected_ready_instant(
        &self,
        task: &TaskStamp,
        entries: &BTreeMap<TaskStamp, EntryUpdate>,
        fact_instants: &BTreeMap<Seq, Instant>,
    ) -> CommandResult<Instant> {
        let entry = self.projected_entry(entries, task).ok_or_else(|| {
            store_fault(
                "lifecycle::task-head-missing",
                &format!("validated task {task} has no current entry"),
            )
        })?;
        let heads = self.projected_heads(entries, task).ok_or_else(|| {
            store_fault(
                "lifecycle::task-head-missing",
                &format!("validated task {task} has no storage heads"),
            )
        })?;
        let mut transition = heads.workflow().position();
        if entry
            .claim()
            .is_some_and(|claim| claim.disposition != ClaimDisposition::Active)
            && let Some(claim) = heads.claim()
        {
            transition = transition.max(claim.position());
        }
        for dependency in &entry.statement().dependencies {
            let dependency_heads = self.projected_heads(entries, dependency).ok_or_else(|| {
                store_fault(
                    "lifecycle::dependency-head-missing",
                    &format!("task {task} dependency {dependency} has no storage heads"),
                )
            })?;
            transition = transition.max(dependency_heads.workflow().position());
        }
        fact_instants
            .get(&transition)
            .or_else(|| self.fact_instants.get(&transition))
            .copied()
            .ok_or_else(|| {
                store_fault(
                    "lifecycle::ready-fact-missing",
                    &format!(
                        "task {task} READY transition head {transition} has no retained fact instant"
                    ),
                )
            })
    }
}

fn entry(heads: &TaskHeads, projects: &board::ProjectPolicy) -> CommandResult<Entry> {
    Entry::new(
        heads.statement().body().clone(),
        heads.workflow().body().clone(),
        heads.claim().map(|head| head.body().clone()),
        heads.lease().map(|head| head.body().clone()),
        projects,
    )
    .map_err(|error| {
        store_fault(
            "lifecycle::task-head-invalid",
            &format!(
                "task {} current heads are invalid: {error}",
                heads.statement().body().born
            ),
        )
    })
}

fn changed_task(
    envelope: &wire::Envelope<wire::AnyFact>,
) -> Option<Result<TaskStamp, wire::TaskStampError>> {
    if !matches!(
        envelope.fact.kind(),
        wire::FactKind::TaskAdded
            | wire::FactKind::TaskPhased
            | wire::FactKind::TaskDone
            | wire::FactKind::TaskReviewed
            | wire::FactKind::TaskAssigned
            | wire::FactKind::TaskNoted
            | wire::FactKind::TaskDeferred
            | wire::FactKind::TaskWoken
            | wire::FactKind::TaskEdited
            | wire::FactKind::TaskDeleted
            | wire::FactKind::ClaimWon
            | wire::FactKind::ClaimCarried
            | wire::FactKind::ClaimRebound
            | wire::FactKind::ClaimFreed
            | wire::FactKind::ClaimLapsed
            | wire::FactKind::ClaimRenewed
    ) {
        return None;
    }
    Some(
        envelope
            .fact
            .body()
            .get("task")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .parse(),
    )
}

fn creator_lineage_events(
    delta: &[&wire::Envelope<wire::AnyFact>],
) -> CommandResult<BTreeMap<TaskStamp, Option<CanonicalUuid>>> {
    let mut creators = BTreeMap::new();
    for envelope in delta {
        let Some(added) = envelope.fact.read::<wire::body::TaskAdded>() else {
            continue;
        };
        let creator = match envelope.actor {
            Actor::Lane(_) => added.creation.creator_lineage(),
            Actor::Operator | Actor::Spice if added.creation.creator_lineage().is_none() => None,
            Actor::Operator | Actor::Spice => {
                return Err(store_fault(
                    "lifecycle::task-creator-lineage-invalid",
                    "a non-lane task creation names a creator lineage",
                ));
            }
        };
        if creators.insert(added.task.clone(), creator).is_some() {
            return Err(store_fault(
                "lifecycle::task-creator-duplicate",
                &format!("task {} has more than one creation fact", added.task),
            ));
        }
    }
    Ok(creators)
}
