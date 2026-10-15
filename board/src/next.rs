use std::collections::{BTreeMap, BTreeSet};

use wire::{Actor, ClaimCell, ClaimDisposition, Instant, TaskIntent, TaskStamp, TeamSubscription};

use crate::task::Entry;
use crate::urgency::{Urgency, declared_priority};

/// Readiness facts every allocation surface shares.
///
/// Ordinary dependencies require settlement: each settled task records whether
/// it ended positively. Controlled outputs also require that their controller
/// has not settled negatively, and an output of a Plan further requires that
/// Plan's positive completion. An output whose immediate preparation is a
/// Design instead requires that Design's exact selection, which never
/// concludes it, so each known preparation parent records whether it is a
/// Design and whether the working-group partition holds it. Each Design that
/// still has a live output awaiting that selection is recorded too, so it
/// waits on the operator rather than a lane, as is each controller whose
/// released work is still in progress, so its Design waits on that work.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResolvedTasks {
    settled: BTreeMap<TaskStamp, bool>,
    designs: BTreeMap<TaskStamp, bool>,
    active_designs: BTreeSet<TaskStamp>,
    unselected: BTreeSet<TaskStamp>,
    released: BTreeSet<TaskStamp>,
    working: BTreeSet<TaskStamp>,
}

impl ResolvedTasks {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `task` settled, positively or not.
    pub fn settle(&mut self, task: TaskStamp, positive: bool) {
        self.settled.insert(task, positive);
    }

    /// Record whether the known task `task` is a Design.
    pub fn know(&mut self, task: TaskStamp, intent: Option<TaskIntent>) {
        self.designs
            .insert(task, intent == Some(TaskIntent::Design));
    }

    /// Record that the working-group partition holds `task`. Only a known
    /// Design reads as active; gather tasks under it run while it stays open.
    pub fn group(&mut self, task: TaskStamp) {
        self.active_designs.insert(task);
    }

    /// Record one live task; an output still lacking its selection marks the
    /// preparation that constructed it.
    pub fn observe(&mut self, statement: &wire::TaskHead, workflow: &wire::WorkflowHead) {
        if let Some(parent) = unselected_output_of(statement, workflow) {
            self.unselected.insert(parent.clone());
        }
    }

    /// Record each controller whose released work is still in progress among
    /// `entries`: a filed, pending, undeferred task, never a gather task,
    /// whose control allocation already treats as satisfied. Call it once
    /// every readiness fact those entries read has been recorded.
    pub fn hold_released_work<'a>(&mut self, entries: impl IntoIterator<Item = &'a Entry>) {
        let working = entries
            .into_iter()
            .filter(|entry| {
                let workflow = entry.workflow();
                entry.is_filed()
                    && workflow.status == "pending"
                    && workflow.completion.is_none()
                    && workflow.deferral.is_none()
                    && !gather_task(entry.statement(), workflow)
                    && control_satisfied(entry.statement(), false, self)
            })
            .filter_map(|entry| entry.statement().preparation.as_ref())
            .map(|preparation| preparation.controller.clone())
            .collect::<Vec<_>>();
        self.working.extend(working);
    }

    /// Whether this task state is an open Design waiting for one of the
    /// observed outputs to receive the operator's selection.
    #[must_use]
    pub fn awaits_selection(
        &self,
        statement: &wire::TaskHead,
        workflow: &wire::WorkflowHead,
    ) -> bool {
        wire::intent::is_open_design(statement.intent, workflow)
            && self.unselected.contains(&statement.born)
    }

    /// Whether this task state is an open Design whose released work is still
    /// in progress: a filed, pending, undeferred task under its control.
    #[must_use]
    pub fn awaits_authorized_work(
        &self,
        statement: &wire::TaskHead,
        workflow: &wire::WorkflowHead,
    ) -> bool {
        statement.intent == Some(TaskIntent::Design)
            && workflow.status == "pending"
            && workflow.completion.is_none()
            && self.working.contains(&statement.born)
    }

    fn settled(&self, task: &TaskStamp) -> bool {
        self.settled.contains_key(task)
    }

    fn positive(&self, task: &TaskStamp) -> bool {
        self.settled.get(task) == Some(&true)
    }

    fn released(&self, task: &TaskStamp) -> bool {
        self.released.contains(task)
    }

    /// Whether `task` is known here and has not settled negatively: it is
    /// still open, or it concluded positively.
    fn standing(&self, task: &TaskStamp) -> bool {
        self.designs.contains_key(task) && self.settled.get(task) != Some(&false)
    }

    /// Whether `task` is a Design; `None` when its intent is unknown here.
    fn design(&self, task: &TaskStamp) -> Option<bool> {
        self.designs.get(task).copied()
    }

    /// Whether `task` is a known Design that is still open and that the
    /// working-group partition holds.
    fn open_active_design(&self, task: &TaskStamp) -> bool {
        self.design(task) == Some(true) && !self.settled(task) && self.active_designs.contains(task)
    }

    /// Whether this row remains Design-side speculative work rather than
    /// execution the allocator may expose in an agent-facing summary.
    ///
    /// A Design row itself stays on its operator-facing planning surface only
    /// after it reaches the open `design` phase. Work controlled by a Design
    /// stays there until the same control predicate used by allocation releases
    /// it. A gather task is Design-side evidence work that never carries an
    /// authorization, so it always stays here.
    #[must_use]
    pub fn speculative_design_work(
        &self,
        statement: &wire::TaskHead,
        workflow: &wire::WorkflowHead,
    ) -> bool {
        if wire::intent::is_open_design(statement.intent, workflow) {
            return true;
        }
        statement.preparation.as_ref().is_some_and(|preparation| {
            self.design(&preparation.controller) == Some(true)
                && !control_satisfied(statement, false, self)
        })
    }
}

/// Reusable task-graph evidence for repeated allocation decisions over one
/// exact entry generation.
///
/// Build this once when task heads change. Actor, subscription, environment,
/// and time remain inputs to each selection, while dependency resolution,
/// effective priority, and downstream reachability are shared.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectionIndex {
    resolved: ResolvedTasks,
    scores: RankingScores,
}

impl SelectionIndex {
    /// Derive the allocation evidence owned by `entries`.
    #[must_use]
    pub fn new(entries: &[Entry]) -> Self {
        Self {
            resolved: resolved_tasks(entries),
            scores: RankingScores::new(entries),
        }
    }

    /// Current dependency, preparation, and settlement evidence.
    #[must_use]
    pub const fn resolved(&self) -> &ResolvedTasks {
        &self.resolved
    }
}

/// The phase a gather task walks: evidence-only work under an open, active
/// Design.
const GATHER_PHASE: &str = "gather";

/// Marks a plan phase the board inserted at the head of a flow to hold a task
/// for taste review, as opposed to one the author's route named. Only a marked
/// phase leaves with its hold.
pub const TASTE_PLAN_TAG: &str = "taste-review:plan-prepended";

/// Whether an authored route with `intent` and `flow` is a gather route: no
/// declared intent and the one `gather` phase. It holds no commit authority,
/// lands nothing, and is never a preparation output awaiting selection; any
/// other route under a Design is an ordinary output.
#[must_use]
pub fn gather_route(intent: Option<TaskIntent>, flow: &[wire::Phase]) -> bool {
    intent.is_none() && matches!(flow, [phase] if phase.as_str() == GATHER_PHASE)
}

/// Whether a task is a gather task: its authored route, read past a plan
/// phase a taste hold inserted, is a gather route. A hold therefore never
/// changes what a task is, only which phase it waits in.
#[must_use]
pub fn gather_task(statement: &wire::TaskHead, workflow: &wire::WorkflowHead) -> bool {
    let held = statement.tags.iter().any(|tag| tag == TASTE_PLAN_TAG);
    let route = match workflow.flow.as_slice() {
        [first, authored @ ..] if held && first.as_str() == "plan" => authored,
        authored => authored,
    };
    gather_route(statement.intent, route)
}

/// The preparation whose selection one live output still lacks: an output its
/// preparation constructed directly that is pending without an execution
/// authorization. A gather task is evidence, never an output.
///
/// A Design cannot be concluded done while it owns such an output; only the
/// operator's selection, or closing or deleting the output, discharges it.
#[must_use]
pub fn unselected_output_of<'a>(
    statement: &'a wire::TaskHead,
    workflow: &wire::WorkflowHead,
) -> Option<&'a TaskStamp> {
    let preparation = statement.preparation.as_ref()?;
    (workflow.status == "pending"
        && workflow.completion.is_none()
        && statement.authorization.is_none()
        && preparation.construction == statement.born
        && !gather_task(statement, workflow))
    .then_some(&preparation.parent)
}

/// Whether an open Design waits on the operator's selection: an output it
/// constructed still lacks selection. No lane can discharge that, so the
/// Design is ineligible until the operator selects or settles its outputs.
/// A Plan never waits here: its outputs follow its own positive review.
#[must_use]
pub fn awaits_selection(entry: &Entry, satisfied: &ResolvedTasks) -> bool {
    satisfied.awaits_selection(entry.statement(), entry.workflow())
}

/// Whether an open Design waits on work it already released: until that
/// settles a lane has nothing new to judge, so the Design follows normal
/// workflow rather than being handed out. Settling the last such task makes it
/// assignable again for follow-up.
#[must_use]
pub fn awaits_authorized_work(entry: &Entry, satisfied: &ResolvedTasks) -> bool {
    satisfied.awaits_authorized_work(entry.statement(), entry.workflow())
}
/// Why one open task's allocation gate is closed.
///
/// Possession, deferral, environmental holds, passive Designs, hidden filing,
/// and stale claims are separate standings and therefore do not appear here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkWait {
    /// An unresolved gate has no route through current open work.
    Blocked,
    /// The task follows normal Plan/Design control or an advancing prerequisite.
    Workflow,
    /// An open Design waits for the operator to select one of its outputs.
    Selection,
}

/// Classify every open allocator gate from one exact task generation.
///
/// This is presentation evidence only: allocation continues to use
/// [`dependencies_satisfied`]. A Plan-constructed dependency sequence is a
/// normal workflow wait only when every unresolved edge reaches active, ready,
/// review, selection, or another normal wait. Ordinary dependency gates,
/// missing authority, cycles, and distinct parked states terminate the chain
/// as blocked execution trouble.
#[must_use]
pub fn work_waits<'a>(
    entries: &'a [Entry],
    environment: &wire::Environment,
) -> BTreeMap<&'a TaskStamp, WorkWait> {
    let resolved = resolved_tasks(entries);
    let index = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.born(), index))
        .collect::<BTreeMap<_, _>>();
    let mut states = vec![Progress::Unknown; entries.len()];
    for task in 0..entries.len() {
        progress(task, entries, &index, &resolved, environment, &mut states);
    }
    entries
        .iter()
        .zip(states)
        .filter_map(|(entry, state)| {
            let wait = match state {
                Progress::Neutral => WorkWait::Workflow,
                Progress::Blocked => WorkWait::Blocked,
                Progress::Selection => WorkWait::Selection,
                Progress::Unknown | Progress::Visiting | Progress::Moving | Progress::Distinct => {
                    return None;
                }
            };
            Some((entry.born(), wait))
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Progress {
    Unknown,
    Visiting,
    Moving,
    Neutral,
    Blocked,
    Selection,
    Distinct,
}

fn progress(
    task: usize,
    entries: &[Entry],
    index: &BTreeMap<&TaskStamp, usize>,
    resolved: &ResolvedTasks,
    environment: &wire::Environment,
    states: &mut [Progress],
) -> Progress {
    match states[task] {
        Progress::Unknown => {}
        known => return known,
    }
    let entry = &entries[task];
    let workflow = entry.workflow();
    if workflow.status != "pending" || workflow.completion.is_some() {
        states[task] = Progress::Distinct;
        return Progress::Distinct;
    }
    if !environment_satisfied(entry, environment) {
        states[task] = Progress::Distinct;
        return Progress::Distinct;
    }
    if entry
        .claim()
        .is_some_and(|claim| claim.disposition == ClaimDisposition::Active)
    {
        states[task] = Progress::Moving;
        return Progress::Moving;
    }
    if entry.is_oops()
        || !entry.is_filed()
        || entry.project().visibility().is_hidden()
        || workflow.deferral.is_some()
        || passive_design(entry)
        || entry
            .claim()
            .is_some_and(|claim| claim.disposition == ClaimDisposition::Lapsed)
    {
        states[task] = Progress::Distinct;
        return Progress::Distinct;
    }
    if awaits_selection(entry, resolved) {
        states[task] = Progress::Selection;
        return Progress::Selection;
    }
    if awaits_authorized_work(entry, resolved) {
        states[task] = Progress::Neutral;
        return Progress::Neutral;
    }
    if dependencies_satisfied(entry, resolved) {
        states[task] = Progress::Moving;
        return Progress::Moving;
    }

    states[task] = Progress::Visiting;
    let control_wait = !control_satisfied(
        entry.statement(),
        gather_task(entry.statement(), entry.workflow()),
        resolved,
    );
    let normal_control = control_wait && normal_control_wait(entry, entries, index);
    if control_wait && !normal_control {
        states[task] = Progress::Blocked;
        return Progress::Blocked;
    }
    let unresolved_dependencies = entry
        .statement()
        .dependencies
        .iter()
        .filter(|dependency| !resolved.settled(dependency));
    if !entry.statement().dependencies.is_empty()
        && !normal_control
        && !plan_constructed(entry, entries, index)
    {
        states[task] = Progress::Blocked;
        return Progress::Blocked;
    }
    for dependency in unresolved_dependencies {
        let Some(dependency) = index.get(dependency).copied() else {
            states[task] = Progress::Blocked;
            return Progress::Blocked;
        };
        if !matches!(
            progress(dependency, entries, index, resolved, environment, states),
            Progress::Moving | Progress::Neutral | Progress::Selection
        ) {
            states[task] = Progress::Blocked;
            return Progress::Blocked;
        }
    }
    states[task] = Progress::Neutral;
    Progress::Neutral
}

fn plan_constructed(entry: &Entry, entries: &[Entry], index: &BTreeMap<&TaskStamp, usize>) -> bool {
    let Some(preparation) = &entry.statement().preparation else {
        return false;
    };
    preparation.construction == entry.statement().born
        && index
            .get(&preparation.parent)
            .is_some_and(|parent| entries[*parent].statement().intent == Some(TaskIntent::Plan))
}

fn normal_control_wait(
    entry: &Entry,
    entries: &[Entry],
    index: &BTreeMap<&TaskStamp, usize>,
) -> bool {
    let Some(preparation) = &entry.statement().preparation else {
        return false;
    };
    if preparation.construction != entry.statement().born
        || gather_task(entry.statement(), entry.workflow())
    {
        return false;
    }
    let planning_open = |task: &TaskStamp| {
        index.get(task).is_some_and(|index| {
            let task = &entries[*index];
            match task.statement().intent {
                Some(TaskIntent::Plan) => {
                    task.workflow().status == "pending" && task.workflow().completion.is_none()
                }
                Some(TaskIntent::Design) => {
                    wire::intent::is_open_design(task.statement().intent, task.workflow())
                }
                None => false,
            }
        })
    };
    let parent_open = planning_open(&preparation.parent);
    let parent_is_plan = index
        .get(&preparation.parent)
        .is_some_and(|parent| entries[*parent].statement().intent == Some(TaskIntent::Plan));
    parent_open && (parent_is_plan || planning_open(&preparation.controller))
}

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

    /// Whether the selection is in the actor's last-resort class: an
    /// independent handoff back to its prior owner, or a row the actor freed.
    #[must_use]
    pub const fn last_resort(self) -> bool {
        self.last_resort
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
        &resolved_tasks(entries),
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
        &resolved_tasks(entries),
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
    satisfied: &ResolvedTasks,
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
    satisfied: &ResolvedTasks,
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

/// [`select_with_satisfied_at`] using graph evidence already derived for this
/// exact entry generation.
#[must_use]
pub fn select_with_index_at<'a>(
    entries: &'a [Entry],
    actor: &Actor,
    subscription: &TeamSubscription,
    band: u64,
    recent_cell: Option<&ClaimCell>,
    environment: &wire::Environment,
    index: &SelectionIndex,
    now: Instant,
) -> Option<Selection<'a>> {
    let candidates = indexed_candidates(
        entries,
        actor,
        subscription,
        recent_cell,
        environment,
        index.resolved(),
        &index.scores,
        Some(now),
    );
    choose(&candidates, band).map(|candidate| candidates[candidate])
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
    let completed = resolved_tasks(entries);
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
    completed: &ResolvedTasks,
    now: Option<Instant>,
) -> Vec<Selection<'a>> {
    let scores = RankingScores::new(entries);
    indexed_candidates(
        entries,
        actor,
        subscription,
        recent_cell,
        environment,
        completed,
        &scores,
        now,
    )
}

fn indexed_candidates<'a>(
    entries: &'a [Entry],
    actor: &Actor,
    subscription: &TeamSubscription,
    recent_cell: Option<&ClaimCell>,
    environment: &wire::Environment,
    completed: &ResolvedTasks,
    scores: &RankingScores,
    now: Option<Instant>,
) -> Vec<Selection<'a>> {
    debug_assert_eq!(entries.len(), scores.effective_priorities.len());
    debug_assert_eq!(entries.len(), scores.downstream_weights.len());
    entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| selectable(entry, actor, subscription, completed, now))
        .filter(|(_, entry)| environment_satisfied(entry, environment))
        .map(|(entry_index, entry)| Selection {
            entry,
            entry_index,
            urgency: Urgency::from_heads(entry),
            effective_priority: scores.effective_priorities[entry_index],
            downstream_weight: scores.downstream_weights[entry_index],
            preferred: is_preferred_continuation(entry, actor),
            last_resort: is_independent_handoff(entry, actor) || is_released_by(entry, actor),
            peer_occupied: peer_occupies(entries, entry, actor),
            movement: recent_cell.map_or(0, |recent| movement_cost(recent, entry)),
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct RankingScores {
    effective_priorities: Vec<i64>,
    downstream_weights: Vec<usize>,
}

impl RankingScores {
    fn new(entries: &[Entry]) -> Self {
        let graph = RankingGraph::new(entries);
        let priorities = effective_priorities(entries, &graph);
        let downstream = downstream_weights(&graph);
        Self {
            effective_priorities: graph
                .entry_nodes
                .iter()
                .map(|node| priorities[*node])
                .collect(),
            downstream_weights: graph
                .entry_nodes
                .iter()
                .map(|node| downstream[*node])
                .collect(),
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
        && entry.is_filed()
        && workflow.status == "pending"
        && workflow.completion.is_none()
}

/// Whether this lane may be *handed* the entry.
///
/// Allocation is lifetime scoped: Steer admits none, Drive admits work created
/// for the same immutable team, and Drain admits every otherwise workable
/// public task.
///
/// Reading is deliberately wider: every public row, whatever its root, plus
/// rows on the reader's own origin thread -- see `actor_route_matches`, which
/// decides what `spice task list` renders. The two scopes must not be unified.
/// Only a lane that can *see* a row can correct it; if reading followed Steer's
/// empty automatic allocation, such a row would be unreachable by construction
/// and the board would gain an absorbing state, wrong once and wrong forever.
fn selectable(
    entry: &Entry,
    actor: &Actor,
    subscription: &TeamSubscription,
    completed: &ResolvedTasks,
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

fn eligible(entry: &Entry, actor: &Actor, completed: &ResolvedTasks) -> bool {
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
fn workable(entry: &Entry, actor: &Actor, completed: &ResolvedTasks) -> bool {
    !entry.project().visibility().is_hidden()
        && entry.is_filed()
        && operational(entry, actor, completed)
}

fn operational(entry: &Entry, actor: &Actor, completed: &ResolvedTasks) -> bool {
    let workflow = entry.workflow();
    workflow.status == "pending"
        && workflow.completion.is_none()
        && workflow.deferral.is_none()
        && workflow
            .assignment
            .as_ref()
            .is_none_or(|assignment| assignment == actor || assignment_lapsed(entry, assignment))
        && dependencies_satisfied(entry, completed)
        && !awaits_selection(entry, completed)
        && !awaits_authorized_work(entry, completed)
        && !passive_design(entry)
}

/// Whether an open Design stands outside every working group.
///
/// A Design-intent row in plan or review intake remains ordinary allocatable
/// work until its current head reaches `design`.
#[must_use]
pub fn passive_design(entry: &Entry) -> bool {
    wire::intent::is_open_design(entry.statement().intent, entry.workflow()) && !entry.grouped()
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

fn operational_without_assignment(entry: &Entry, completed: &ResolvedTasks) -> bool {
    let workflow = entry.workflow();
    workflow.status == "pending"
        && workflow.completion.is_none()
        && workflow.deferral.is_none()
        && dependencies_satisfied(entry, completed)
        && !awaits_selection(entry, completed)
        && !awaits_authorized_work(entry, completed)
        && !passive_design(entry)
}

/// Ordinary prerequisites may resolve negatively; planning boundaries may not.
#[must_use]
pub fn dependencies_satisfied(entry: &Entry, completed: &ResolvedTasks) -> bool {
    entry
        .statement()
        .dependencies
        .iter()
        .all(|dependency| completed.settled(dependency))
        && control_satisfied(
            entry.statement(),
            gather_task(entry.statement(), entry.workflow()),
            completed,
        )
}

/// A constructed output waits for its controller to stand: known, and not
/// settled negatively by collapse, closure, or deletion. When its preparation
/// is a Design, the output, Plan or task, also waits for that Design's exact
/// execution authorization, which permanently releases that output even when
/// the Design later concludes negatively. Otherwise it waits for its
/// preparation to conclude positively; nothing else, including operator
/// closure, releases it. Retained pre-cutover group markers are unsupported
/// evidence and never participate in eligibility. A preparation a Plan
/// constructs runs alongside it. An unknown preparation leaves the output
/// gated. A gather task needs no authorization: it runs exactly while the
/// Design that constructed it is open and grouped.
fn control_satisfied(statement: &wire::TaskHead, gather: bool, completed: &ResolvedTasks) -> bool {
    let Some(preparation) = &statement.preparation else {
        return !gather;
    };
    if gather {
        return completed.open_active_design(&preparation.parent);
    }
    if completed.released(&statement.born) {
        return true;
    }
    if preparation.construction != statement.born {
        return true;
    }
    let controller = completed.standing(&preparation.controller);
    match completed.design(&preparation.parent) {
        None => false,
        Some(true) => statement
            .authorization
            .as_ref()
            .is_some_and(|decision| decision.controller == preparation.controller),
        Some(false) => {
            statement.intent.is_some() || (controller && completed.positive(&preparation.parent))
        }
    }
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
    satisfied: &ResolvedTasks,
    entry: &Entry,
    actor: &Actor,
    environment: &wire::Environment,
) -> bool {
    workable(entry, actor, satisfied) && environment_satisfied(entry, environment) && unheld(entry)
}

/// Whether one exact task may be captured, including typed OOPS triage.
///
/// Hidden rows remain unavailable to allocator selection. An explicitly named
/// OOPS row may be captured while it remains parked, but filing is required
/// before dependencies or completion move.
#[must_use]
pub fn capturable_entry(
    satisfied: &ResolvedTasks,
    entry: &Entry,
    actor: &Actor,
    environment: &wire::Environment,
) -> bool {
    let public = workable(entry, actor, satisfied);
    let oops = entry.is_oops() && operational(entry, actor, satisfied);
    (public || oops) && environment_satisfied(entry, environment) && unheld(entry)
}

/// Whether one named entry is eligible through the team's current task scope.
#[must_use]
pub fn selectable_entry(
    satisfied: &ResolvedTasks,
    entry: &Entry,
    actor: &Actor,
    subscription: &TeamSubscription,
    environment: &wire::Environment,
) -> bool {
    selectable(entry, actor, subscription, satisfied, None)
        && environment_satisfied(entry, environment)
}

/// [`selectable_entry`] at an explicit allocation instant.
#[must_use]
pub fn selectable_entry_at(
    satisfied: &ResolvedTasks,
    entry: &Entry,
    actor: &Actor,
    subscription: &TeamSubscription,
    environment: &wire::Environment,
    now: Instant,
) -> bool {
    selectable(entry, actor, subscription, satisfied, Some(now))
        && environment_satisfied(entry, environment)
}

/// Earliest future phase-handoff instant that currently blocks `actor`.
///
/// Ordinary route, workflow, dependency, environment, assignment, and
/// possession predicates must already admit the row. This lets lifecycle arm
/// the exact durable handoff deadline when the actor has no candidate yet.
#[must_use]
pub fn blocked_handoff_until(
    entries: &[Entry],
    satisfied: &ResolvedTasks,
    actor: &Actor,
    subscription: &TeamSubscription,
    environment: &wire::Environment,
    now: Instant,
) -> Option<Instant> {
    entries
        .iter()
        .filter(|entry| selectable(entry, actor, subscription, satisfied, None))
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
    satisfied: &ResolvedTasks,
    entry: &Entry,
    _actor: &Actor,
    environment: &wire::Environment,
) -> bool {
    !entry.project().visibility().is_hidden()
        && entry.is_filed()
        && operational_without_assignment(entry, satisfied)
        && environment_satisfied(entry, environment)
}

/// Each settled task with whether its conclusion is positive, whether each
/// entry is a Design, which preparations still own an unselected output, and
/// which controllers' released work is still in progress.
///
/// Closing or deleting a task resolves ordinary prerequisites. Neither approves
/// execution controlled by that task. All readiness surfaces use these facts.
#[must_use]
pub fn resolved_tasks(entries: &[Entry]) -> ResolvedTasks {
    let mut resolved = ResolvedTasks::new();
    for candidate in entries {
        let status = candidate.workflow().status.as_str();
        if status == "completed" || status == "deleted" {
            resolved.settle(
                candidate.born().clone(),
                candidate.workflow().concluded_positively(),
            );
        }
        resolved.know(candidate.born().clone(), candidate.statement().intent);
        if candidate.grouped() {
            resolved.group(candidate.born().clone());
        }
        resolved.observe(candidate.statement(), candidate.workflow());
    }
    resolved.released = crate::control::released_tasks(entries);
    resolved.hold_released_work(entries);
    resolved
}
