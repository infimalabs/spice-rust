use std::collections::BTreeSet;

use wire::{Actor, ClaimCell, ClaimDisposition, Instant, TaskStamp, TeamSubscription};

use crate::task::Entry;
use crate::urgency::{Urgency, declared_priority};

/// Deterministic answer from the board's sole allocation query.
#[derive(Clone, Copy, Debug)]
pub struct Selection<'a> {
    entry: &'a Entry,
    entry_index: usize,
    urgency: Urgency,
    effective_priority: i64,
    downstream_weight: usize,
    last_resort: bool,
    preferred: bool,
    peer_occupied: bool,
    movement: u8,
}

impl<'a> Selection<'a> {
    /// Selected current head bundle.
    #[must_use]
    pub const fn entry(self) -> &'a Entry {
        self.entry
    }

    /// Position of the selected entry in the supplied frontier.
    #[must_use]
    pub const fn entry_index(self) -> usize {
        self.entry_index
    }

    /// Scalar that ranked the selection.
    #[must_use]
    pub const fn urgency(self) -> Urgency {
        self.urgency
    }

    /// Unsettled public entries transitively downstream of the selection.
    #[must_use]
    pub const fn downstream_weight(self) -> usize {
        self.downstream_weight
    }
}

/// Select the highest-ranked subscribed and eligible task for `actor`.
///
/// This is the only allocation policy in `board`. It derives completed
/// dependencies from the same supplied head set, applies ordinary task and
/// environment filters, then ranks a prior claimant's continuation first.
/// Independent handoffs back to their prior claimant, self-authored reviews,
/// and explicitly freed rows form the last-resort class. Effective priority and
/// downstream weight follow. Queue ages within `band` of that group's oldest
/// row admit locality: an active peer in the same project/phase cell repels,
/// then movement from `recent_cell` costs one each for changing project and
/// phase. Rows outside the band cannot be rescued by locality. Identical
/// evidence always returns the same task regardless of slice order.
#[must_use]
pub fn select<'a>(
    entries: &'a [Entry],
    actor: &Actor,
    subscription: &TeamSubscription,
    band: u64,
    recent_cell: Option<&ClaimCell>,
    environment: &wire::Environment,
) -> Option<Selection<'a>> {
    select_with_satisfied(
        entries,
        actor,
        subscription,
        band,
        recent_cell,
        environment,
        &satisfied_tasks(entries),
    )
}

/// [`select`] at an explicit allocation instant, including an open phase
/// handoff window.
#[must_use]
pub fn select_at<'a>(
    entries: &'a [Entry],
    actor: &Actor,
    subscription: &TeamSubscription,
    band: u64,
    recent_cell: Option<&ClaimCell>,
    environment: &wire::Environment,
    now: Instant,
) -> Option<Selection<'a>> {
    select_with_satisfied_at(
        entries,
        actor,
        subscription,
        band,
        recent_cell,
        environment,
        &satisfied_tasks(entries),
        now,
    )
}

/// Select from a live frontier using explicitly indexed settled dependencies.
///
/// A dependency absent from `satisfied` remains blocked. Callers may therefore
/// omit terminal task heads while preserving the distinction between known
/// completion or deletion and a stamp missing from authority.
#[must_use]
pub fn select_with_satisfied<'a>(
    entries: &'a [Entry],
    actor: &Actor,
    subscription: &TeamSubscription,
    band: u64,
    recent_cell: Option<&ClaimCell>,
    environment: &wire::Environment,
    satisfied: &BTreeSet<TaskStamp>,
) -> Option<Selection<'a>> {
    let candidates = candidates(
        entries,
        actor,
        subscription,
        recent_cell,
        environment,
        satisfied,
        None,
    );
    choose(&candidates, band).map(|index| candidates[index])
}

/// [`select_with_satisfied`] at an explicit allocation instant.
#[must_use]
pub fn select_with_satisfied_at<'a>(
    entries: &'a [Entry],
    actor: &Actor,
    subscription: &TeamSubscription,
    band: u64,
    recent_cell: Option<&ClaimCell>,
    environment: &wire::Environment,
    satisfied: &BTreeSet<TaskStamp>,
    now: Instant,
) -> Option<Selection<'a>> {
    let candidates = candidates(
        entries,
        actor,
        subscription,
        recent_cell,
        environment,
        satisfied,
        Some(now),
    );
    choose(&candidates, band).map(|index| candidates[index])
}

/// Every task [`select`] would hand `actor`, in the order it would hand them out.
///
/// Each step is exactly `select`'s choice among the tasks not yet offered, so
/// the head is always `select`'s answer. Standing is held fixed between steps:
/// the order answers what the allocator ranks next on this evidence, not what
/// it would rank after the earlier offers were claimed.
#[must_use]
pub fn offer_order<'a>(
    entries: &'a [Entry],
    actor: &Actor,
    subscription: &TeamSubscription,
    band: u64,
    recent_cell: Option<&ClaimCell>,
    environment: &wire::Environment,
) -> Vec<Selection<'a>> {
    offer_order_at_inner(
        entries,
        actor,
        subscription,
        band,
        recent_cell,
        environment,
        None,
    )
}

/// [`offer_order`] at an explicit allocation instant.
#[must_use]
pub fn offer_order_at<'a>(
    entries: &'a [Entry],
    actor: &Actor,
    subscription: &TeamSubscription,
    band: u64,
    recent_cell: Option<&ClaimCell>,
    environment: &wire::Environment,
    now: Instant,
) -> Vec<Selection<'a>> {
    offer_order_at_inner(
        entries,
        actor,
        subscription,
        band,
        recent_cell,
        environment,
        Some(now),
    )
}

fn offer_order_at_inner<'a>(
    entries: &'a [Entry],
    actor: &Actor,
    subscription: &TeamSubscription,
    band: u64,
    recent_cell: Option<&ClaimCell>,
    environment: &wire::Environment,
    now: Option<Instant>,
) -> Vec<Selection<'a>> {
    let completed = satisfied_tasks(entries);
    let mut remaining = candidates(
        entries,
        actor,
        subscription,
        recent_cell,
        environment,
        &completed,
        now,
    );
    let mut order = Vec::with_capacity(remaining.len());
    while let Some(index) = choose(&remaining, band) {
        order.push(remaining.swap_remove(index));
    }
    order
}

/// Every subscribed, eligible, environment-satisfied entry, scored once.
fn candidates<'a>(
    entries: &'a [Entry],
    actor: &Actor,
    subscription: &TeamSubscription,
    recent_cell: Option<&ClaimCell>,
    environment: &wire::Environment,
    completed: &BTreeSet<TaskStamp>,
    now: Option<Instant>,
) -> Vec<Selection<'a>> {
    let graph = RankingGraph::new(entries);
    let priorities = effective_priorities(entries, &graph);
    let downstream = downstream_weights(&graph);
    entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| selectable(entry, actor, subscription, completed, now))
        .filter(|(_, entry)| environment_satisfied(entry, environment))
        .map(|(entry_index, entry)| {
            let node = graph.entry_nodes[entry_index];
            Selection {
                entry,
                entry_index,
                urgency: Urgency::from_heads(entry),
                effective_priority: priorities[node],
                downstream_weight: downstream[node],
                preferred: is_preferred_continuation(entry, actor),
                last_resort: is_independent_handoff(entry, actor) || is_released_by(entry, actor),
                peer_occupied: peer_occupies(entries, entry, actor),
                movement: recent_cell.map_or(0, |recent| movement_cost(recent, entry)),
            }
        })
        .collect()
}

/// Index of the one candidate the allocator hands out first.
///
/// A prior claimant's continuation outranks other work. Within that class,
/// ordinary work outranks last-resort rows, then effective priority and
/// downstream weight. Within `band` of that group's greatest urgency, peer
/// occupancy repels, then locality movement costs, then the handle decides.
fn choose(candidates: &[Selection<'_>], band: u64) -> Option<usize> {
    let preferred = candidates.iter().any(|candidate| candidate.preferred);
    let peer_class = candidates
        .iter()
        .filter(|candidate| candidate.preferred == preferred)
        .any(|candidate| !candidate.last_resort);
    let effective_priority = candidates
        .iter()
        .filter(|candidate| candidate.preferred == preferred && candidate.last_resort != peer_class)
        .map(|candidate| candidate.effective_priority)
        .max()?;
    let downstream_weight = candidates
        .iter()
        .filter(|candidate| {
            candidate.preferred == preferred
                && candidate.last_resort != peer_class
                && candidate.effective_priority == effective_priority
        })
        .map(|candidate| candidate.downstream_weight)
        .max()?;
    let urgency = candidates
        .iter()
        .filter(|candidate| {
            candidate.preferred == preferred
                && candidate.last_resort != peer_class
                && candidate.effective_priority == effective_priority
                && candidate.downstream_weight == downstream_weight
        })
        .map(|candidate| candidate.urgency)
        .max()?;
    let in_band = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| {
            candidate.preferred == preferred
                && candidate.last_resort != peer_class
                && candidate.effective_priority == effective_priority
                && candidate.downstream_weight == downstream_weight
                && urgency.value().abs_diff(candidate.urgency.value()) <= band
        })
        .collect::<Vec<_>>();
    let peer_occupied = in_band
        .iter()
        .map(|(_, candidate)| candidate.peer_occupied)
        .min()?;
    let movement = in_band
        .iter()
        .filter(|(_, candidate)| candidate.peer_occupied == peer_occupied)
        .map(|(_, candidate)| candidate.movement)
        .min()?;
    in_band
        .into_iter()
        .filter(|(_, candidate)| {
            candidate.peer_occupied == peer_occupied && candidate.movement == movement
        })
        .min_by_key(|(_, candidate)| candidate.entry.handle())
        .map(|(index, _)| index)
}

fn peer_occupies(entries: &[Entry], candidate: &Entry, actor: &Actor) -> bool {
    entries.iter().any(|entry| {
        entry.claim().is_some_and(|claim| {
            claim.disposition == ClaimDisposition::Active
                && &claim.actor != actor
                && claim.cell.project == candidate.project().as_str()
                && claim.cell.phase == candidate.workflow().phase
        })
    })
}

fn movement_cost(recent: &ClaimCell, candidate: &Entry) -> u8 {
    u8::from(recent.project != candidate.project().as_str())
        + u8::from(recent.phase != candidate.workflow().phase)
}

/// Principal's most recently claimed cell from durable claim history.
///
/// `latest_win` is the position and claim-time cell of the newest claim the
/// principal won, which the snapshot indexes so no history is decoded here.
/// Schema-4 facts carry the exact claim-time cell. For the one legacy latest
/// win whose fact predates that field, the migrated current claim head
/// supplies its cell when it still names the same win. Older superseded
/// legacy wins have no invented locality and therefore remain neutral.
#[must_use]
pub fn recent_claim_cell(
    entries: &[Entry],
    latest_win: Option<(wire::Seq, Option<ClaimCell>)>,
    actor: &Actor,
) -> Option<ClaimCell> {
    let (claimed, cell) = latest_win?;
    cell.or_else(|| {
        entries.iter().find_map(|entry| {
            entry.claim().and_then(|claim| {
                (&claim.actor == actor && claim.claimed == claimed).then(|| claim.cell.clone())
            })
        })
    })
}

/// Whether `entry` is an independent handoff back to its prior claimant.
///
/// The allocator ranks this class last so peer work wins, without coupling the
/// ownership rule to any phase name.
#[must_use]
pub fn is_independent_handoff(entry: &Entry, actor: &Actor) -> bool {
    entry.phase_handoff().is_some_and(|handoff| {
        handoff.kind() == crate::PhaseHandoffKind::Independent && handoff.prior() == actor
    })
}

fn is_preferred_continuation(entry: &Entry, actor: &Actor) -> bool {
    entry.phase_handoff().is_some_and(|handoff| {
        handoff.kind() == crate::PhaseHandoffKind::Continue && handoff.prior() == actor
    })
}

/// Dense live-graph indexes used by every repeated ranking traversal.
///
/// Stamp resolution and adjacency construction happen once. Traversals use
/// integer positions only, so no ordered lookup sits inside a fixed-point or
/// per-root graph walk.
struct RankingGraph {
    entry_nodes: Vec<usize>,
    priority_edges: Vec<(usize, usize)>,
    dependents: Vec<Vec<usize>>,
}

impl RankingGraph {
    fn new(entries: &[Entry]) -> Self {
        let mut by_stamp = (0..entries.len()).collect::<Vec<_>>();
        by_stamp.sort_unstable_by(|left, right| {
            entries[*left]
                .born()
                .cmp(entries[*right].born())
                .then_with(|| left.cmp(right))
        });
        let mut entry_nodes = vec![0; entries.len()];
        let mut nodes = 0;
        for (position, entry_index) in by_stamp.iter().copied().enumerate() {
            if position == 0
                || entries[by_stamp[position - 1]].born() != entries[entry_index].born()
            {
                nodes += 1;
            }
            entry_nodes[entry_index] = nodes - 1;
        }

        let mut priority_edges = Vec::new();
        let mut dependents = vec![Vec::new(); nodes];
        for (entry_index, entry) in entries.iter().enumerate() {
            if !unsettled_public(entry) {
                continue;
            }
            let dependent = entry_nodes[entry_index];
            for prerequisite in &entry.statement().dependencies {
                let Ok(position) = by_stamp
                    .binary_search_by(|candidate| entries[*candidate].born().cmp(prerequisite))
                else {
                    continue;
                };
                let prerequisite = entry_nodes[by_stamp[position]];
                priority_edges.push((dependent, prerequisite));
                dependents[prerequisite].push(dependent);
            }
        }
        Self {
            entry_nodes,
            priority_edges,
            dependents,
        }
    }
}

/// Effective priority for every task after unsettled public dependents flow upstream.
///
/// Propagation walks dependency edges from dependent to prerequisite until a
/// fixed point. Scores only rise through paths among the finite live vertices,
/// so at most \(V\) edge passes are required even for a malformed cyclic graph.
/// `max` is commutative, which keeps the result independent of slice order.
fn effective_priorities(entries: &[Entry], graph: &RankingGraph) -> Vec<i64> {
    let mut priorities = vec![i64::MIN; graph.dependents.len()];
    for (entry_index, entry) in entries.iter().enumerate() {
        let priority = &mut priorities[graph.entry_nodes[entry_index]];
        *priority = (*priority).max(declared_priority(entry));
    }

    loop {
        let mut changed = false;
        for &(dependent, prerequisite) in &graph.priority_edges {
            let inherited = priorities[dependent];
            if inherited > priorities[prerequisite] {
                priorities[prerequisite] = inherited;
                changed = true;
            }
        }
        if !changed {
            return priorities;
        }
    }
}

/// Count distinct unsettled public dependents reachable from every live task.
///
/// Reused dense visited and pending storage makes each root walk \(O(V+E)\).
/// A visited bit both counts converging paths once and makes malformed cycles
/// finite.
fn downstream_weights(graph: &RankingGraph) -> Vec<usize> {
    let mut weights = vec![0; graph.dependents.len()];
    let mut visited = vec![false; graph.dependents.len()];
    let mut pending = Vec::with_capacity(graph.dependents.len());
    for (root, weight) in weights.iter_mut().enumerate() {
        visited.fill(false);
        pending.clear();
        pending.extend(graph.dependents[root].iter().copied());
        while let Some(dependent) = pending.pop() {
            if dependent == root || std::mem::replace(&mut visited[dependent], true) {
                continue;
            }
            *weight += 1;
            pending.extend(graph.dependents[dependent].iter().copied());
        }
    }
    weights
}

fn unsettled_public(entry: &Entry) -> bool {
    let workflow = entry.workflow();
    !entry.project().visibility().is_hidden()
        && workflow.status == "pending"
        && workflow.completion.is_none()
}

/// Whether this lane may be *handed* the entry.
///
/// Allocation is lifetime scoped: Steer admits none, Drive admits work created
/// for the same immutable team, and Drain admits every otherwise workable
/// public task.
///
/// Reading is deliberately wider, and stem scoped rather than lifetime scoped
/// -- see `actor_route_matches`, which decides what `spice task list` renders.
/// The two scopes must not be unified. Only a lane that can *see* a row can
/// correct it; if reading followed Steer's empty automatic allocation, such a
/// row would be unreachable by construction and the board would gain an
/// absorbing state, wrong once and wrong forever.
///
/// Reading and allocation remain intentionally independent. Project stems can
/// widen reading without becoming lifetime predicates again.
fn selectable(
    entry: &Entry,
    actor: &Actor,
    subscription: &TeamSubscription,
    completed: &BTreeSet<TaskStamp>,
    now: Option<Instant>,
) -> bool {
    subscription.admits_creator_team(entry.creator_team())
        && handoff_admits(entry, actor, now)
        && eligible(entry, actor, completed)
}

fn handoff_admits(entry: &Entry, actor: &Actor, now: Option<Instant>) -> bool {
    let Some(now) = now else {
        return true;
    };
    let Some(handoff) = entry.phase_handoff() else {
        return true;
    };
    if now >= handoff.until() {
        return true;
    }
    match handoff.kind() {
        crate::PhaseHandoffKind::Continue => handoff.prior() == actor,
        crate::PhaseHandoffKind::Independent => handoff.prior() != actor,
    }
}

fn eligible(entry: &Entry, actor: &Actor, completed: &BTreeSet<TaskStamp>) -> bool {
    workable(entry, actor, completed) && unheld(entry)
}

/// Whether `actor` explicitly freed `entry` and nobody has claimed it since.
///
/// Such a row ranks last for that actor rather than being excluded, so a lane
/// that frees a task drains its other work first yet still gets the row back
/// on a quiet board.
#[must_use]
pub fn is_released_by(entry: &Entry, actor: &Actor) -> bool {
    entry.explicit_release()
        && entry.claim().is_some_and(|claim| {
            claim.disposition == ClaimDisposition::Freed && &claim.actor == actor
        })
}

/// Every eligibility clause except possession.
///
/// Allocation and stealing agree on all of this and disagree on possession
/// alone, so possession is the only thing either caller states for itself.
fn workable(entry: &Entry, actor: &Actor, completed: &BTreeSet<TaskStamp>) -> bool {
    !entry.project().visibility().is_hidden() && operational(entry, actor, completed)
}

fn operational(entry: &Entry, actor: &Actor, completed: &BTreeSet<TaskStamp>) -> bool {
    let workflow = entry.workflow();
    workflow.status == "pending"
        && workflow.completion.is_none()
        && workflow.deferral.is_none()
        && workflow
            .assignment
            .as_ref()
            .is_none_or(|assignment| assignment == actor || assignment_lapsed(entry, assignment))
        && entry
            .statement()
            .dependencies
            .iter()
            .all(|dependency| completed.contains(dependency))
}

/// Whether a review assignment died with the claim that carried it.
///
/// A review assignment pins the row to one reviewer. When that reviewer's own
/// claim has lapsed, the pin outlives the possession it came with: nobody
/// holds the row, yet every other actor is filtered out and the assignee is the
/// one lane known to have left. Such an assignment is treated as none, so the
/// next drain-mode lane takes the review over and mints its own assignment. A
/// freed claim is not a lapse: the reviewer let go on purpose and the release
/// already cleared the pin, so a pin that survives a free is somebody else's.
fn assignment_lapsed(entry: &Entry, assignment: &Actor) -> bool {
    entry.claim().is_some_and(|claim| {
        claim.disposition == ClaimDisposition::Lapsed && &claim.actor == assignment
    })
}

fn operational_without_assignment(entry: &Entry, completed: &BTreeSet<TaskStamp>) -> bool {
    let workflow = entry.workflow();
    workflow.status == "pending"
        && workflow.completion.is_none()
        && workflow.deferral.is_none()
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

/// Whether every typed environmental requirement on one task is satisfied.
///
/// This is deliberately separate from dependency, workflow, assignment, and
/// possession eligibility. Every selection surface first applies its ordinary
/// task rules, then this single derived filter before ranking or admission.
#[must_use]
pub fn environment_satisfied(entry: &Entry, environment: &wire::Environment) -> bool {
    entry.statement().requirements.are_satisfied_by(environment)
}

/// Whether one named entry satisfies the same eligibility law as allocation.
#[must_use]
pub fn eligible_entry(
    entries: &[Entry],
    entry: &Entry,
    actor: &Actor,
    environment: &wire::Environment,
) -> bool {
    workable(entry, actor, &satisfied_tasks(entries))
        && environment_satisfied(entry, environment)
        && unheld(entry)
}

/// Whether one exact task may be captured, including parked `.oops` triage.
///
/// Hidden rows remain unavailable to allocator selection. The one exception is
/// an explicitly named `.oops` row: a lane may hold it while investigating,
/// but promotion is still required before dependencies or completion move.
#[must_use]
pub fn capturable_entry(
    entries: &[Entry],
    entry: &Entry,
    actor: &Actor,
    environment: &wire::Environment,
) -> bool {
    let completed = satisfied_tasks(entries);
    let public = workable(entry, actor, &completed);
    let oops = entry.project().visibility().hidden_stem() == Some("oops")
        && operational(entry, actor, &completed);
    (public || oops) && environment_satisfied(entry, environment) && unheld(entry)
}

/// Whether one named entry is eligible through the team's current task scope.
#[must_use]
pub fn selectable_entry(
    entries: &[Entry],
    entry: &Entry,
    actor: &Actor,
    subscription: &TeamSubscription,
    environment: &wire::Environment,
) -> bool {
    selectable(entry, actor, subscription, &satisfied_tasks(entries), None)
        && environment_satisfied(entry, environment)
}

/// [`selectable_entry`] at an explicit allocation instant.
#[must_use]
pub fn selectable_entry_at(
    entries: &[Entry],
    entry: &Entry,
    actor: &Actor,
    subscription: &TeamSubscription,
    environment: &wire::Environment,
    now: Instant,
) -> bool {
    selectable(
        entry,
        actor,
        subscription,
        &satisfied_tasks(entries),
        Some(now),
    ) && environment_satisfied(entry, environment)
}

/// Earliest future phase-handoff instant that currently blocks `actor`.
///
/// Ordinary route, workflow, dependency, environment, assignment, and
/// possession predicates must already admit the row. This lets lifecycle arm
/// the exact durable handoff deadline when the actor has no candidate yet.
#[must_use]
pub fn blocked_handoff_until(
    entries: &[Entry],
    actor: &Actor,
    subscription: &TeamSubscription,
    environment: &wire::Environment,
    now: Instant,
) -> Option<Instant> {
    let completed = satisfied_tasks(entries);
    entries
        .iter()
        .filter(|entry| selectable(entry, actor, subscription, &completed, None))
        .filter(|entry| environment_satisfied(entry, environment))
        .filter(|entry| !handoff_admits(entry, actor, Some(now)))
        .filter_map(|entry| entry.phase_handoff().map(|handoff| handoff.until()))
        .min()
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
pub fn stealable_entry(
    entries: &[Entry],
    entry: &Entry,
    _actor: &Actor,
    environment: &wire::Environment,
) -> bool {
    !entry.project().visibility().is_hidden()
        && operational_without_assignment(entry, &satisfied_tasks(entries))
        && environment_satisfied(entry, environment)
}

/// Birth stamps that no longer block a dependent, stated once for every caller.
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
pub fn satisfied_tasks(entries: &[Entry]) -> BTreeSet<TaskStamp> {
    entries
        .iter()
        .filter(|candidate| {
            let status = candidate.workflow().status.as_str();
            status == "completed" || status == "deleted"
        })
        .map(|candidate| candidate.born().clone())
        .collect()
}
