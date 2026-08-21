//! Incremental lifecycle projection retained across durable commits.

use std::collections::{BTreeMap, BTreeSet};

use board::Entry;
use journal::TaskHeads;
use wire::{Actor, CanonicalUuid, ClaimDisposition, Instant, Seq, TaskStamp};

use super::{CommandResult, store_fault};

pub(super) struct Projection {
    cursor: Option<Seq>,
    ledger: steer::Ledger,
    entries: Vec<Entry>,
    heads: BTreeMap<TaskStamp, TaskHeads>,
    fact_instants: BTreeMap<Seq, Instant>,
    ready_at: BTreeMap<TaskStamp, Instant>,
    dependents: BTreeMap<TaskStamp, BTreeSet<TaskStamp>>,
    subscriptions: Subscriptions,
}

#[derive(Default)]
struct Subscriptions {
    lanes_by_task: BTreeMap<TaskStamp, BTreeSet<CanonicalUuid>>,
    project_by_task: BTreeMap<TaskStamp, Option<String>>,
    projects_by_lane: BTreeMap<CanonicalUuid, BTreeMap<String, usize>>,
}

impl Projection {
    pub(super) fn new() -> Self {
        Self {
            cursor: None,
            ledger: steer::Ledger::new(),
            entries: Vec::new(),
            heads: BTreeMap::new(),
            fact_instants: BTreeMap::new(),
            ready_at: BTreeMap::new(),
            dependents: BTreeMap::new(),
            subscriptions: Subscriptions::default(),
        }
    }

    pub(super) const fn cursor(&self) -> Option<Seq> {
        self.cursor
    }

    pub(super) fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub(super) const fn ledger(&self) -> &steer::Ledger {
        &self.ledger
    }

    pub(super) const fn ready_at(&self) -> &BTreeMap<TaskStamp, Instant> {
        &self.ready_at
    }

    pub(super) fn advance(
        &mut self,
        snapshot: &bus::TaskSnapshot,
        projects: &board::ProjectPolicy,
    ) -> CommandResult<()> {
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
            return Ok(());
        }

        let delta = snapshot.facts().after(self.cursor).collect::<Vec<_>>();
        if delta.last().map(|fact| fact.seq) != Some(cursor) {
            return Err(store_fault(
                "lifecycle::snapshot-gap",
                &format!(
                    "lifecycle delta after {:?} ends at {:?}, expected {cursor}",
                    self.cursor,
                    delta.last().map(|fact| fact.seq)
                ),
            ));
        }
        for fact in &delta {
            self.fact_instants.insert(fact.seq, fact.at);
        }
        self.ledger
            .advance(delta.iter().copied())
            .map_err(|error| store_fault("lifecycle::steering-invalid", &error.to_string()))?;

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
        for task in &changed {
            self.update_entry(snapshot, task, projects)?;
        }
        self.subscriptions
            .advance(delta.iter().copied(), &self.entries)?;

        let mut affected = changed.clone();
        for task in &changed {
            if let Some(rows) = self.dependents.get(task) {
                affected.extend(rows.iter().cloned());
            }
        }
        if self.cursor.is_none() {
            affected.extend(self.entries.iter().map(|entry| entry.born().clone()));
        }
        for task in affected {
            let ready = self.ready_instant(&task)?;
            self.ready_at.insert(task, ready);
        }
        self.cursor = Some(cursor);
        Ok(())
    }

    pub(super) fn project_residency(
        &self,
        residency: &wire::ResidencyHead,
    ) -> Result<wire::ResidencyHead, String> {
        self.subscriptions.project(residency)
    }

    fn update_entry(
        &mut self,
        snapshot: &bus::TaskSnapshot,
        task: &TaskStamp,
        projects: &board::ProjectPolicy,
    ) -> CommandResult<()> {
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
        let entry = entry(&heads, projects)?;
        if let Ok(index) = self.entries.binary_search_by(|row| row.born().cmp(task)) {
            for dependency in &self.entries[index].statement().dependencies {
                if let Some(rows) = self.dependents.get_mut(dependency) {
                    rows.remove(task);
                    if rows.is_empty() {
                        self.dependents.remove(dependency);
                    }
                }
            }
            self.entries[index] = entry;
        } else {
            let index = self
                .entries
                .binary_search_by(|row| row.born().cmp(task))
                .expect_err("the changed task is not present");
            self.entries.insert(index, entry);
        }
        let entry = self
            .entries
            .binary_search_by(|row| row.born().cmp(task))
            .ok()
            .map(|index| &self.entries[index])
            .expect("the changed entry was inserted");
        for dependency in &entry.statement().dependencies {
            self.dependents
                .entry(dependency.clone())
                .or_default()
                .insert(task.clone());
        }
        let project = (!entry.project().visibility().is_hidden())
            .then(|| entry.project().as_str().to_owned());
        self.subscriptions.update_project(task, project);
        self.heads.insert(task.clone(), heads);
        Ok(())
    }

    fn ready_instant(&self, task: &TaskStamp) -> CommandResult<Instant> {
        let entry = self
            .entries
            .binary_search_by(|row| row.born().cmp(task))
            .ok()
            .map(|index| &self.entries[index])
            .ok_or_else(|| {
                store_fault(
                    "lifecycle::task-head-missing",
                    &format!("validated task {task} has no current entry"),
                )
            })?;
        let heads = self.heads.get(task).ok_or_else(|| {
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
            let dependency = self.heads.get(dependency).ok_or_else(|| {
                store_fault(
                    "lifecycle::dependency-head-missing",
                    &format!("task {task} dependency {dependency} has no storage heads"),
                )
            })?;
            transition = transition.max(dependency.workflow().position());
        }
        self.fact_instants.get(&transition).copied().ok_or_else(|| {
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

impl Subscriptions {
    fn update_project(&mut self, task: &TaskStamp, project: Option<String>) {
        let previous = self.project_by_task.insert(task.clone(), project.clone());
        if previous.as_ref() == Some(&project) {
            return;
        }
        let lanes = self.lanes_by_task.get(task).cloned().unwrap_or_default();
        for lane in lanes {
            if let Some(Some(previous)) = &previous {
                self.remove(lane, previous);
            }
            if let Some(project) = &project {
                self.add(lane, project);
            }
        }
    }

    fn advance<'a>(
        &mut self,
        facts: impl IntoIterator<Item = &'a wire::Envelope<wire::AnyFact>>,
        entries: &[Entry],
    ) -> CommandResult<()> {
        for envelope in facts {
            let event = match envelope.fact.kind() {
                wire::FactKind::TaskAdded => match envelope.actor {
                    Actor::Lane(lane) => envelope
                        .fact
                        .read::<wire::body::TaskAdded>()
                        .map(|body| (body.task, lane)),
                    Actor::Operator | Actor::Spice => None,
                },
                wire::FactKind::ClaimWon => envelope
                    .fact
                    .read::<wire::body::ClaimWon>()
                    .map(|body| (body.task, body.lane)),
                _ => None,
            };
            let Some((task, lane)) = event else {
                continue;
            };
            if entries
                .binary_search_by(|entry| entry.born().cmp(&task))
                .is_err()
            {
                return Err(store_fault(
                    "lifecycle::subscription-history-invalid",
                    &format!("subscription history task {task} has no current heads"),
                ));
            }
            if self
                .lanes_by_task
                .entry(task.clone())
                .or_default()
                .insert(lane)
                && let Some(Some(project)) = self.project_by_task.get(&task)
            {
                let project = project.clone();
                self.add(lane, &project);
            }
        }
        Ok(())
    }

    fn project(&self, residency: &wire::ResidencyHead) -> Result<wire::ResidencyHead, String> {
        let mut projected = residency.clone();
        for subscription in &mut projected.subscriptions {
            subscription.auto_projects.clear();
        }
        for (lane, projects) in &self.projects_by_lane {
            let Some(lineage) = projected
                .lanes
                .iter()
                .find(|seat| seat.lane == *lane)
                .map(|seat| seat.lineage_id)
            else {
                continue;
            };
            let mut subscriptions = projected
                .subscriptions
                .iter_mut()
                .filter(|subscription| subscription.lineage_id == lineage);
            let subscription = subscriptions.next().ok_or_else(|| {
                format!("lineage {lineage} has no durable task-subscription policy")
            })?;
            if subscriptions.next().is_some() {
                return Err(format!(
                    "lineage {lineage} has more than one durable task-subscription policy"
                ));
            }
            for (project, count) in projects {
                if *count > 0 {
                    subscription.learn_project(project);
                }
            }
        }
        Ok(projected)
    }

    fn add(&mut self, lane: CanonicalUuid, project: &str) {
        *self
            .projects_by_lane
            .entry(lane)
            .or_default()
            .entry(project.to_owned())
            .or_default() += 1;
    }

    fn remove(&mut self, lane: CanonicalUuid, project: &str) {
        let Some(projects) = self.projects_by_lane.get_mut(&lane) else {
            return;
        };
        if let Some(count) = projects.get_mut(project) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                projects.remove(project);
            }
        }
        if projects.is_empty() {
            self.projects_by_lane.remove(&lane);
        }
    }
}
