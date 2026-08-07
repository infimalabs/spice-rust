use std::collections::{BTreeMap, BTreeSet};

use wire::{Actor, ClaimDisposition, LineageSubscription, TaskHandle};

use crate::task::Entry;
use crate::urgency::{Urgency, declared_priority};

/// Deterministic answer from the board's sole allocation query.
#[derive(Clone, Copy, Debug)]
pub struct Selection<'a> {
    entry: &'a Entry,
    urgency: Urgency,
    effective_priority: i64,
    self_review: bool,
}

impl<'a> Selection<'a> {
    /// Selected current head bundle.
    #[must_use]
    pub const fn entry(self) -> &'a Entry {
        self.entry
    }

    /// Scalar that ranked the selection.
    #[must_use]
    pub const fn urgency(self) -> Urgency {
        self.urgency
    }
}

/// Project server-managed create/claim subscriptions from durable history.
///
/// The residency head owns the policy and manual pins. Auto projects remain
/// facts: task creation and claim transactions already record the lane and
/// task atomically, so replaying those facts avoids a second mutable authority
/// while surviving restart exactly.
pub fn project_subscription_history(
    residency: &mut wire::ResidencyHead,
    entries: &[Entry],
    facts: &[wire::Envelope<wire::AnyFact>],
) -> Result<(), String> {
    for subscription in &mut residency.subscriptions {
        subscription.auto_projects.clear();
    }
    for envelope in facts {
        let lane = match envelope.fact.kind() {
            wire::FactKind::TaskAdded => match &envelope.actor {
                Actor::Lane(lane) => *lane,
                Actor::Operator | Actor::Spice => continue,
            },
            wire::FactKind::ClaimWon => envelope
                .fact
                .body()
                .get("lane")
                .and_then(|value| value.as_str())
                .ok_or_else(|| "claim::won fact has no lane".to_owned())?
                .parse()
                .map_err(|error| format!("claim::won fact lane is invalid: {error}"))?,
            _ => continue,
        };
        let task = envelope
            .fact
            .body()
            .get("task")
            .and_then(|value| value.as_str())
            .ok_or_else(|| format!("{} fact has no task", envelope.fact.kind()))?;
        let entry = entries
            .iter()
            .find(|entry| entry.statement().handle.as_str() == task)
            .ok_or_else(|| format!("subscription history task {task} has no current heads"))?;
        if entry.project().visibility().is_hidden() {
            continue;
        }
        let Some(lineage) = residency
            .lanes
            .iter()
            .find(|seat| seat.lane == lane)
            .map(|seat| seat.lineage_id)
        else {
            continue;
        };
        let mut subscriptions = residency
            .subscriptions
            .iter_mut()
            .filter(|subscription| subscription.lineage_id == lineage);
        let subscription = subscriptions
            .next()
            .ok_or_else(|| format!("lineage {lineage} has no durable task-subscription policy"))?;
        if subscriptions.next().is_some() {
            return Err(format!(
                "lineage {lineage} has more than one durable task-subscription policy"
            ));
        }
        subscription.learn_project(entry.project().as_str());
    }
    Ok(())
}

/// Select the highest-ranked subscribed and eligible task for `actor`.
///
/// This is the only allocation path in `board`. It derives completed
/// dependencies from the same supplied head set, ranks peer work before the
/// actor's own review, then effective priority, then urgency. Scores inside
/// `band` are equivalent and break by ascending handle spelling. Identical
/// heads, actor, and band always return the same task regardless of slice order.
#[must_use]
pub fn select<'a>(
    entries: &'a [Entry],
    actor: &Actor,
    subscription: &LineageSubscription,
    band: u64,
) -> Option<Selection<'a>> {
    let completed = satisfied_handles(entries);
    let priorities = effective_priorities(entries);

    let candidates = entries
        .iter()
        .filter(|entry| selectable(entry, actor, subscription, &completed))
        .map(|entry| {
            let effective_priority = priorities
                .get(&entry.statement().handle)
                .copied()
                .unwrap_or_else(|| declared_priority(entry));
            Selection {
                entry,
                urgency: Urgency::from_heads_with_effective_priority(
                    entry,
                    &completed,
                    effective_priority,
                ),
                effective_priority,
                self_review: is_self_review(entry, actor),
            }
        })
        .collect::<Vec<_>>();
    let peer_class = candidates.iter().any(|candidate| !candidate.self_review);
    let effective_priority = candidates
        .iter()
        .filter(|candidate| candidate.self_review != peer_class)
        .map(|candidate| candidate.effective_priority)
        .max()?;
    let urgency = candidates
        .iter()
        .filter(|candidate| {
            candidate.self_review != peer_class
                && candidate.effective_priority == effective_priority
        })
        .map(|candidate| candidate.urgency)
        .max()?;
    candidates
        .into_iter()
        .filter(|candidate| {
            candidate.self_review != peer_class
                && candidate.effective_priority == effective_priority
                && urgency.value().abs_diff(candidate.urgency.value()) <= band
        })
        .min_by_key(|candidate| &candidate.entry.statement().handle)
}

fn is_self_review(entry: &Entry, actor: &Actor) -> bool {
    let workflow = entry.workflow();
    workflow.phase.as_str() == "review"
        && workflow.review == "unassigned"
        && workflow.assignment.is_none()
        && entry.claim().is_some_and(|claim| {
            claim.disposition == ClaimDisposition::Freed && &claim.actor == actor
        })
}

/// Effective priority for every entry after unsettled public dependents flow upstream.
///
/// Propagation walks dependency edges from dependent to prerequisite until a
/// fixed point. Scores only rise through a finite priority set, so even a
/// malformed cyclic graph terminates. `max` is commutative, which keeps the
/// result independent of the supplied slice order.
fn effective_priorities(entries: &[Entry]) -> BTreeMap<TaskHandle, i64> {
    let mut priorities: BTreeMap<TaskHandle, i64> = BTreeMap::new();
    for entry in entries {
        let declared = declared_priority(entry);
        priorities
            .entry(entry.statement().handle.clone())
            .and_modify(|current| *current = (*current).max(declared))
            .or_insert(declared);
    }

    loop {
        let mut changed = false;
        for dependent in entries.iter().filter(|entry| {
            let workflow = entry.workflow();
            !entry.project().visibility().is_hidden()
                && workflow.status == "pending"
                && workflow.completion.is_none()
        }) {
            let inherited = priorities
                .get(&dependent.statement().handle)
                .copied()
                .unwrap_or_else(|| declared_priority(dependent));
            for prerequisite in &dependent.statement().dependencies {
                let Some(priority) = priorities.get_mut(prerequisite) else {
                    continue;
                };
                if inherited > *priority {
                    *priority = inherited;
                    changed = true;
                }
            }
        }
        if !changed {
            return priorities;
        }
    }
}

/// Whether this lane may be *handed* the entry.
///
/// Allocation is subscription scoped: a lane is offered only work its
/// subscription admits, because the subscription is what declares that lane
/// responsible for a project.
///
/// Reading is deliberately wider, and stem scoped rather than subscription
/// scoped -- see `actor_route_matches`, which decides what `spice task list`
/// renders. The two scopes must not be unified. A task can be filed, or
/// corrected, into a project outside every subscription that could act on it,
/// and only a lane that can *see* a row can move it back; if reading followed
/// allocation, such a row would be unreachable by construction and the board
/// would gain an absorbing state, wrong once and wrong forever.
///
/// The invariant is therefore directional rather than an equality. Widening
/// reading or narrowing allocation both preserve it; narrowing reading to the
/// subscription, or widening allocation to the public stems, breaks it.
fn selectable(
    entry: &Entry,
    actor: &Actor,
    subscription: &LineageSubscription,
    completed: &BTreeSet<TaskHandle>,
) -> bool {
    subscription.admits_project(entry.project().as_str()) && eligible(entry, actor, completed)
}

fn eligible(entry: &Entry, actor: &Actor, completed: &BTreeSet<TaskHandle>) -> bool {
    workable(entry, actor, completed) && unheld(entry)
}

/// Every eligibility clause except possession.
///
/// Allocation and stealing agree on all of this and disagree on possession
/// alone, so possession is the only thing either caller states for itself.
fn workable(entry: &Entry, actor: &Actor, completed: &BTreeSet<TaskHandle>) -> bool {
    let workflow = entry.workflow();
    !entry.project().visibility().is_hidden()
        && workflow.status == "pending"
        && workflow.completion.is_none()
        && workflow.deferral.is_none()
        && workflow
            .assignment
            .as_ref()
            .is_none_or(|assignment| assignment == actor)
        && entry
            .statement()
            .dependencies
            .iter()
            .all(|dependency| completed.contains(dependency))
}

/// Whether no claim currently possesses the entry.
fn unheld(entry: &Entry) -> bool {
    !entry
        .claim()
        .is_some_and(|claim| claim.disposition == ClaimDisposition::Active)
}

/// Whether one named entry satisfies the same eligibility law as allocation.
#[must_use]
pub fn eligible_entry(entries: &[Entry], entry: &Entry, actor: &Actor) -> bool {
    workable(entry, actor, &satisfied_handles(entries)) && unheld(entry)
}

/// Whether one named entry is eligible through the lineage's current task scope.
#[must_use]
pub fn selectable_entry(
    entries: &[Entry],
    entry: &Entry,
    actor: &Actor,
    subscription: &LineageSubscription,
) -> bool {
    selectable(entry, actor, subscription, &satisfied_handles(entries))
}

/// Whether one named entry may be taken from whoever currently holds it.
///
/// [`eligible_entry`] without the possession clause, and without any clause put
/// in its place: a steal takes the task at once rather than waiting out a lease
/// or proving the holder gone. Taking immediately is safe rather than reckless
/// because the displaced holder is told: its next renewal refuses as
/// `claim::ownership-refused` and names whoever holds the claim now.
///
/// Every other clause still applies, so a steal can no more take a hidden,
/// completed, deferred or dependency-blocked task than an allocation can.
#[must_use]
pub fn stealable_entry(entries: &[Entry], entry: &Entry, actor: &Actor) -> bool {
    workable(entry, actor, &satisfied_handles(entries))
}

/// Handles that no longer block a dependent, stated once for every caller.
///
/// A withdrawn prerequisite belongs here beside a completed one. It will never
/// complete, so leaving it out would let one deletion block every dependent
/// permanently, with no move left that could clear the edge: the dependent's
/// blocker is settled, and the operator who withdrew it is not told that
/// anything is now stuck.
///
/// This is public because allocation is not the only reader. Whatever reports
/// to the operator which tasks are blocked has to decide the same question, and
/// deciding it separately is how one row comes to be counted ready and blocked
/// at once: the allocator hands over work the report still calls stuck.
#[must_use]
pub fn satisfied_handles(entries: &[Entry]) -> BTreeSet<TaskHandle> {
    entries
        .iter()
        .filter(|candidate| {
            let status = candidate.workflow().status.as_str();
            status == "completed" || status == "deleted"
        })
        .map(|candidate| candidate.statement().handle.clone())
        .collect()
}
