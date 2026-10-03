//! Pure task projection shared by every app Work surface.
//!
//! The admitted board snapshot and current lane topology are the sole inputs.
//! This module owns lifecycle precedence, stable Work scope identity,
//! full-project grouping, relationship summaries, counts, board arrangements
//! over the resident's offer order, and owner naming so consumers cannot drift.

mod relationship;
pub use relationship::{
    WorkRelationshipDirection, WorkRelationshipEdge, WorkRelationshipError,
    WorkRelationshipExpansion, WorkRelationshipFrontier, WorkRelationshipGraph,
    WorkRelationshipKind, WorkRelationshipNode, WorkRelationshipOptions,
    WorkRelationshipOriginNode, WorkRelationshipTaskNode,
};
#[cfg(test)]
mod team_scope_tests;

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Display, Formatter};

use wire::head::ClaimDisposition;
use wire::intent::{BoardTask, ProjectVisibility};
use wire::{Actor, TaskStamp};

use crate::runtime::{BoardView, LaneMember, LaneView};
use crate::watchband;

/// One mutually exclusive task lifecycle in the Work surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkAvailability {
    /// A current active claim possesses this open task.
    InFlight,
    /// Open, unclaimed, and immediately workable.
    Ready,
    /// Open and unclaimed, but requirements or dependencies do not hold.
    Blocked,
    /// Open and unclaimed, with an explicit wakeable deferral.
    Deferred,
    /// No longer open, regardless of stale possession evidence.
    Terminal,
}

impl WorkAvailability {
    /// Stable presentation key shared by every Work lifecycle surface.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::InFlight => "in-flight",
            Self::Ready => "ready",
            Self::Blocked => "blocked",
            Self::Deferred => "deferred",
            Self::Terminal => "terminal",
        }
    }
}

/// Exact public or hidden stem identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct WorkStemIdentity {
    visibility: WorkVisibility,
    stem: String,
}

/// Visibility is part of identity: `.oops.*` and `oops.*` never collapse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkVisibility {
    Public,
    Hidden,
}
/// Durable-in-render selection for the global Work surface.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum WorkScope {
    /// Every open task across every visibility-qualified stem.
    #[default]
    AllOpen,
    /// Open tasks under one exact public or hidden stem.
    Stem(WorkStemIdentity),
    /// Open tasks created by one current team or last claimed by one of its
    /// current member lanes, optionally narrowed to one exact stem.
    Team {
        team: wire::CanonicalUuid,
        stem: Option<WorkStemIdentity>,
    },
}

/// The pill strip an open task rides.
///
/// A task created by a team in the loaded roster rides that team's rail; every
/// other task, including one whose creator team has since dissolved, rides the
/// global strip. This mirrors drive admission, which offers a task only to its
/// creator team.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkRail {
    Global,
    Team(wire::CanonicalUuid),
}

impl WorkStemIdentity {
    #[must_use]
    pub const fn new(visibility: WorkVisibility, stem: String) -> Self {
        Self { visibility, stem }
    }

    #[must_use]
    pub const fn visibility(&self) -> WorkVisibility {
        self.visibility
    }

    #[must_use]
    pub fn stem(&self) -> &str {
        &self.stem
    }

    #[must_use]
    pub const fn hidden(&self) -> bool {
        matches!(self.visibility, WorkVisibility::Hidden)
    }
}
impl Display for WorkStemIdentity {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        if self.hidden() {
            formatter.write_str(".")?;
        }
        write!(formatter, "{}.*", self.stem)
    }
}

impl WorkScope {
    /// Every open task associated with one team.
    #[must_use]
    pub const fn team_wide(team: wire::CanonicalUuid) -> Self {
        Self::Team { team, stem: None }
    }

    /// The exact stem this scope narrows to, alone or within a team.
    #[must_use]
    pub const fn stem(&self) -> Option<&WorkStemIdentity> {
        match self {
            Self::Stem(identity)
            | Self::Team {
                stem: Some(identity),
                ..
            } => Some(identity),
            Self::AllOpen | Self::Team { stem: None, .. } => None,
        }
    }

    #[must_use]
    pub const fn team(&self) -> Option<wire::CanonicalUuid> {
        match self {
            Self::Team { team, .. } => Some(*team),
            Self::AllOpen | Self::Stem(_) => None,
        }
    }
}

/// The inline order of cards inside each shared board lane.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorkOrder {
    /// The resident allocator's offer order: the task offered next leads.
    /// Work the allocator does not offer follows in queue order.
    #[default]
    Offer,
    /// Newest creation first, by durable birth stamp.
    Newest,
}

/// The attribute that clusters cards inside each shared board lane.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorkClustering {
    /// One unbroken row per lane.
    #[default]
    None,
    /// One cluster per full dotted project.
    Project,
    /// One cluster per workflow phase.
    Phase,
}

/// How cards sit inside the one shared set of board lanes.
///
/// Every arrangement renders the same cards in the same lanes; only their
/// inline order and separation differ, so switching never loses, duplicates,
/// or reorders durable data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkArrangement {
    pub order: WorkOrder,
    pub clustering: WorkClustering,
}

/// The shared attribute of one cluster of cards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkClusterKey<'a> {
    /// The whole lane, when no clustering is chosen.
    Lane,
    Project(&'a str),
    Phase(&'a str),
}

/// Consecutive cards sharing one cluster attribute, in the chosen order.
#[derive(Debug)]
pub struct WorkCluster<'p, 'a> {
    pub key: WorkClusterKey<'a>,
    pub tasks: Vec<&'p WorkTask<'a>>,
}

/// Exclusive lifecycle totals for one scope.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkCounts {
    in_flight: usize,
    ready: usize,
    blocked: usize,
    deferred: usize,
    terminal: usize,
}

impl WorkCounts {
    fn record(&mut self, availability: WorkAvailability) {
        match availability {
            WorkAvailability::InFlight => self.in_flight += 1,
            WorkAvailability::Ready => self.ready += 1,
            WorkAvailability::Blocked => self.blocked += 1,
            WorkAvailability::Deferred => self.deferred += 1,
            WorkAvailability::Terminal => self.terminal += 1,
        }
    }
    #[must_use]
    pub const fn in_flight(self) -> usize {
        self.in_flight
    }

    #[must_use]
    pub const fn ready(self) -> usize {
        self.ready
    }

    #[must_use]
    pub const fn blocked(self) -> usize {
        self.blocked
    }

    #[must_use]
    pub const fn deferred(self) -> usize {
        self.deferred
    }

    #[must_use]
    pub const fn terminal(self) -> usize {
        self.terminal
    }

    #[must_use]
    pub const fn unavailable(self) -> usize {
        self.blocked + self.deferred
    }
    #[must_use]
    pub const fn open(self) -> usize {
        self.in_flight + self.ready + self.blocked + self.deferred
    }

    #[must_use]
    pub const fn has_open(self) -> bool {
        self.open() > 0
    }
}

/// One dependency edge as resolved against this admitted board snapshot.
#[derive(Debug)]
pub struct WorkDependency<'a> {
    /// Stable dependency identity even when its row is unavailable.
    born: &'a TaskStamp,
    /// Resolved task index in [`WorkProjection::tasks`].
    task: Option<usize>,
    /// Whether the dependency has a completion head.
    satisfied: bool,
}

impl<'a> WorkDependency<'a> {
    #[must_use]
    pub const fn born(&self) -> &'a TaskStamp {
        self.born
    }

    #[must_use]
    pub const fn task(&self) -> Option<usize> {
        self.task
    }

    #[must_use]
    pub const fn satisfied(&self) -> bool {
        self.satisfied
    }
}

/// One reverse edge to a task that names this task as a dependency.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkDependent {
    /// Dependent task index in [`WorkProjection::tasks`].
    task: usize,
}

impl WorkDependent {
    #[must_use]
    pub const fn task(self) -> usize {
        self.task
    }
}

/// One task projected for Work consumers.
#[derive(Debug)]
pub struct WorkTask<'a> {
    row: &'a BoardTask,
    availability: WorkAvailability,
    /// Full-project bucket in [`WorkProjection::projects`].
    project: usize,
    /// An unsatisfied environment is one explicit blocker.
    environment_blocked: bool,
    /// Position in the resident's offer order, when the allocator offers it.
    offer_rank: Option<usize>,
    dependencies: Vec<WorkDependency<'a>>,
    dependents: Vec<WorkDependent>,
    /// Current roster teams that associate this task by immutable creator or
    /// latest winning claim lane.
    teams: Vec<wire::CanonicalUuid>,
    /// The pill strip this task rides; `None` while a team-created task waits
    /// for the roster that decides whether its creator team still stands.
    rail: Option<WorkRail>,
}

impl<'a> WorkTask<'a> {
    #[must_use]
    pub const fn row(&self) -> &'a BoardTask {
        self.row
    }

    #[must_use]
    pub const fn availability(&self) -> WorkAvailability {
        self.availability
    }

    #[must_use]
    pub const fn project(&self) -> usize {
        self.project
    }

    #[must_use]
    pub const fn environment_blocked(&self) -> bool {
        self.environment_blocked
    }

    /// This task's position in the resident's offer order: `Some(0)` is the
    /// task offered next, and `None` is work the allocator does not offer.
    #[must_use]
    pub const fn offer_rank(&self) -> Option<usize> {
        self.offer_rank
    }

    #[must_use]
    pub fn dependencies(&self) -> &[WorkDependency<'a>] {
        &self.dependencies
    }

    #[must_use]
    pub fn dependents(&self) -> &[WorkDependent] {
        &self.dependents
    }

    pub fn blocked_dependencies(&self) -> impl Iterator<Item = &WorkDependency<'_>> {
        self.dependencies
            .iter()
            .filter(|dependency| !dependency.satisfied)
    }

    /// The actor currently working this open task.
    ///
    /// Terminal rows can retain stale active-claim evidence, so lifecycle must
    /// agree before a claim is presented as live possession.
    #[must_use]
    pub fn working_claimant(&self) -> Option<&'a Actor> {
        if self.availability != WorkAvailability::InFlight {
            return None;
        }
        self.row
            .claim
            .as_ref()
            .filter(|claim| claim.body.disposition == ClaimDisposition::Active)
            .map(|claim| &claim.body.actor)
    }

    /// The actor that owns this task: its active claimant, else its assignee.
    #[must_use]
    pub fn owner(&self) -> Option<&'a Actor> {
        self.row
            .claim
            .as_ref()
            .filter(|claim| claim.body.disposition == ClaimDisposition::Active)
            .map(|claim| &claim.body.actor)
            .or(self.row.workflow.body.assignment.as_ref())
    }

    /// The attribute this task shares with its cluster.
    #[must_use]
    pub fn cluster_key(&self, clustering: WorkClustering) -> WorkClusterKey<'a> {
        match clustering {
            WorkClustering::None => WorkClusterKey::Lane,
            WorkClustering::Project => WorkClusterKey::Project(&self.row.statement.body.project),
            WorkClustering::Phase => WorkClusterKey::Phase(self.row.workflow.body.phase.as_str()),
        }
    }

    #[must_use]
    pub fn belongs_to_team(&self, team: wire::CanonicalUuid) -> bool {
        self.teams.contains(&team)
    }
}

/// One full dotted project in the stem-ordered project grouping.
#[derive(Debug)]
pub struct WorkProject<'a> {
    project: &'a str,
    /// Stem bucket in [`WorkProjection::stems`].
    stem: usize,
    counts: WorkCounts,
    /// Deterministically ranked indexes into [`WorkProjection::tasks`].
    tasks: Vec<usize>,
}

impl<'a> WorkProject<'a> {
    #[must_use]
    pub const fn project(&self) -> &'a str {
        self.project
    }

    #[must_use]
    pub const fn stem(&self) -> usize {
        self.stem
    }

    #[must_use]
    pub const fn counts(&self) -> WorkCounts {
        self.counts
    }

    #[must_use]
    pub fn tasks(&self) -> &[usize] {
        &self.tasks
    }
}

/// One visibility-qualified stem scope shared with the chrome pills.
#[derive(Debug)]
pub struct WorkStem {
    identity: WorkStemIdentity,
    counts: WorkCounts,
    /// Lexically ordered indexes into [`WorkProjection::projects`].
    projects: Vec<usize>,
}

impl WorkStem {
    #[must_use]
    pub const fn identity(&self) -> &WorkStemIdentity {
        &self.identity
    }

    #[must_use]
    pub const fn counts(&self) -> WorkCounts {
        self.counts
    }

    #[must_use]
    pub fn projects(&self) -> &[usize] {
        &self.projects
    }
}

/// Complete client-side Work model for one admitted board snapshot.
#[derive(Debug)]
pub struct WorkProjection<'a> {
    counts: WorkCounts,
    tasks: Vec<WorkTask<'a>>,
    projects: Vec<WorkProject<'a>>,
    stems: Vec<WorkStem>,
    /// Every task index in the resident's offer order, then every task the
    /// allocator does not offer in lifecycle and queue order.
    offered: Vec<usize>,
    /// Every task index, newest birth stamp first.
    newest: Vec<usize>,
    /// Current roster team identities, absent until the lane topology loads.
    /// A team route settles unavailable only when a loaded roster lacks it.
    teams: Option<BTreeSet<wire::CanonicalUuid>>,
}

impl<'a> WorkProjection<'a> {
    #[must_use]
    pub fn tasks(&self) -> &[WorkTask<'a>] {
        &self.tasks
    }

    #[must_use]
    pub fn projects(&self) -> &[WorkProject<'a>] {
        &self.projects
    }

    #[must_use]
    pub fn stems(&self) -> &[WorkStem] {
        &self.stems
    }
    #[must_use]
    pub const fn counts(&self) -> WorkCounts {
        self.counts
    }

    #[must_use]
    pub fn stem(&self, identity: &WorkStemIdentity) -> Option<&WorkStem> {
        self.stems.iter().find(|stem| stem.identity == *identity)
    }

    /// Whether the projection holds `scope`; `None` while a team scope waits
    /// for the lane roster that decides it.
    #[must_use]
    pub fn contains_scope(&self, scope: &WorkScope) -> Option<bool> {
        match scope {
            WorkScope::AllOpen => Some(true),
            WorkScope::Stem(identity) => Some(self.stem(identity).is_some()),
            WorkScope::Team { team, stem } => self.teams.as_ref().map(|teams| {
                teams.contains(team) && stem.as_ref().is_none_or(|stem| self.stem(stem).is_some())
            }),
        }
    }

    /// Exact visibility-qualified stem scope containing one admitted project.
    #[must_use]
    pub fn scope_for_project(&self, project: &str) -> Option<WorkScope> {
        let project = self
            .projects
            .iter()
            .find(|candidate| candidate.project == project)?;
        Some(WorkScope::Stem(self.stems[project.stem].identity.clone()))
    }

    #[must_use]
    pub fn scope_counts(&self, scope: &WorkScope) -> WorkCounts {
        match scope {
            WorkScope::AllOpen => self.counts,
            WorkScope::Stem(identity) => self.stem(identity).map_or(self.counts, WorkStem::counts),
            WorkScope::Team { .. } => {
                let mut counts = WorkCounts::default();
                for task in self.tasks.iter().filter(|task| self.in_scope(task, scope)) {
                    counts.record(task.availability);
                }
                counts
            }
        }
    }

    /// Whether one task belongs to the selected Work scope.
    #[must_use]
    pub fn in_scope(&self, task: &WorkTask<'_>, scope: &WorkScope) -> bool {
        match scope {
            WorkScope::AllOpen => true,
            WorkScope::Stem(identity) => self.task_stem(task) == identity,
            WorkScope::Team { team, stem } => {
                task.belongs_to_team(*team)
                    && stem
                        .as_ref()
                        .is_none_or(|stem| self.task_stem(task) == stem)
            }
        }
    }

    fn task_stem(&self, task: &WorkTask<'_>) -> &WorkStemIdentity {
        &self.stems[self.projects[task.project].stem].identity
    }

    /// Aggregate open counts for the tasks riding one pill strip.
    #[must_use]
    pub fn rail_counts(&self, rail: WorkRail) -> WorkCounts {
        let mut counts = WorkCounts::default();
        for task in self.tasks.iter().filter(|task| task.rail == Some(rail)) {
            counts.record(task.availability);
        }
        counts
    }

    /// Each stem with open work riding one pill strip, in stem order, with
    /// the counts of only that strip's tasks.
    #[must_use]
    pub fn rail_stems(&self, rail: WorkRail) -> Vec<(&WorkStemIdentity, WorkCounts)> {
        let mut counts = vec![WorkCounts::default(); self.stems.len()];
        for task in self.tasks.iter().filter(|task| task.rail == Some(rail)) {
            counts[self.projects[task.project].stem].record(task.availability);
        }
        self.stems
            .iter()
            .zip(counts)
            .filter(|(_, counts)| counts.has_open())
            .map(|(stem, counts)| (&stem.identity, counts))
            .collect()
    }

    /// The scope to open `task` under: `scope` itself when it already shows
    /// the task, otherwise every open task, so a filter never hides the task
    /// the operator asked to see.
    #[must_use]
    pub fn scope_showing(&self, task: &TaskStamp, scope: &WorkScope) -> WorkScope {
        let shown = self
            .tasks
            .iter()
            .find(|candidate| candidate.row.statement.body.born == *task)
            .is_some_and(|candidate| self.in_scope(candidate, scope));
        if shown {
            scope.clone()
        } else {
            WorkScope::AllOpen
        }
    }

    /// The in-scope tasks of one shared board lane, top to bottom, clustered.
    ///
    /// Cards follow the chosen order. Clustering gathers each attribute's
    /// cards into one cluster that keeps that order, and clusters follow the
    /// order of their first card, so the lane's leading card still leads.
    /// Every arrangement places a task in exactly the one lane its
    /// availability names.
    #[must_use]
    pub fn lane<'p>(
        &'p self,
        availability: WorkAvailability,
        scope: &WorkScope,
        arrangement: WorkArrangement,
    ) -> Vec<WorkCluster<'p, 'a>> {
        let order = match arrangement.order {
            WorkOrder::Offer => &self.offered,
            WorkOrder::Newest => &self.newest,
        };
        let mut clusters = Vec::<WorkCluster<'p, 'a>>::new();
        for task in order.iter().map(|&index| &self.tasks[index]) {
            if task.availability != availability || !self.in_scope(task, scope) {
                continue;
            }
            let key = task.cluster_key(arrangement.clustering);
            match clusters.iter_mut().find(|cluster| cluster.key == key) {
                Some(cluster) => cluster.tasks.push(task),
                None => clusters.push(WorkCluster {
                    key,
                    tasks: vec![task],
                }),
            }
        }
        clusters
    }

    pub fn from_board(board: &'a BoardView) -> Self {
        Self::from_board_and_lanes(board, None)
    }

    #[must_use]
    pub fn from_board_and_lanes(board: &'a BoardView, lanes: Option<&LaneView>) -> Self {
        let Some(lanes) = lanes else {
            return Self::from_board_with_team_memberships(board, None, &BTreeMap::new());
        };
        let mut teams = BTreeSet::new();
        let mut member_teams = BTreeMap::new();
        for team in lanes.lanes.iter().filter(|team| team.teamed()) {
            teams.insert(team.lane);
            for member in &team.members {
                member_teams.insert(member.lane, team.lane);
            }
        }
        Self::from_board_with_team_memberships(board, Some(teams), &member_teams)
    }

    fn from_board_with_team_memberships(
        board: &'a BoardView,
        teams: Option<BTreeSet<wire::CanonicalUuid>>,
        member_teams: &BTreeMap<wire::CanonicalUuid, wire::CanonicalUuid>,
    ) -> Self {
        let by_born = board
            .tasks
            .iter()
            .enumerate()
            .map(|(index, task)| (&task.statement.body.born, index))
            .collect::<BTreeMap<_, _>>();
        let environment_blocked = board
            .tasks
            .iter()
            .map(|task| !board.environment_satisfies(task))
            .collect::<Vec<_>>();
        let availability = board
            .tasks
            .iter()
            .enumerate()
            .map(|(index, task)| classify(board, task, &by_born, environment_blocked[index]))
            .collect::<Vec<_>>();

        let dependent_indexes = dependent_indexes(board, &by_born, &availability);
        let offer_ranks = offer_ranks(board, &by_born);

        let mut counts = WorkCounts::default();
        let mut stem_builders = BTreeMap::<StemKey<'a>, StemBuilder<'a>>::new();
        for (index, task) in board.tasks.iter().enumerate() {
            counts.record(availability[index]);
            let key = stem_key(task);
            let stem = stem_builders.entry(key).or_default();
            stem.counts.record(availability[index]);
            let project = stem
                .projects
                .entry(task.statement.body.project.as_str())
                .or_default();
            project.counts.record(availability[index]);
            project.tasks.push(index);
        }

        let mut projects = Vec::new();
        let mut stems = Vec::with_capacity(stem_builders.len());
        let mut project_by_task = vec![0; board.tasks.len()];
        for (key, stem_builder) in stem_builders {
            let stem_index = stems.len();
            let mut project_indexes = Vec::with_capacity(stem_builder.projects.len());
            for (project, mut builder) in stem_builder.projects {
                builder.tasks.sort_by(|left, right| {
                    compare_tasks(&board.tasks, &availability, *left, *right)
                });
                let project_index = projects.len();
                for &task in &builder.tasks {
                    project_by_task[task] = project_index;
                }
                project_indexes.push(project_index);
                projects.push(WorkProject {
                    project,
                    stem: stem_index,
                    counts: builder.counts,
                    tasks: builder.tasks,
                });
            }
            stems.push(WorkStem {
                identity: WorkStemIdentity {
                    visibility: key.visibility,
                    stem: key.stem.to_owned(),
                },
                counts: stem_builder.counts,
                projects: project_indexes,
            });
        }

        let tasks = board
            .tasks
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let dependencies = work_dependencies(row, board, &by_born);
                let task_teams = associated_teams(row, teams.as_ref(), member_teams);
                let dependents = dependent_indexes[index]
                    .iter()
                    .copied()
                    .map(|task| WorkDependent { task })
                    .collect();
                WorkTask {
                    row,
                    availability: availability[index],
                    project: project_by_task[index],
                    environment_blocked: environment_blocked[index],
                    offer_rank: offer_ranks[index],
                    dependencies,
                    dependents,
                    teams: task_teams,
                    rail: task_rail(row, teams.as_ref()),
                }
            })
            .collect();

        let (offered, newest) = board_orders(board, &offer_ranks, &availability);

        Self {
            counts,
            tasks,
            projects,
            stems,
            offered,
            teams,
            newest,
        }
    }
}

fn task_rail(task: &BoardTask, teams: Option<&BTreeSet<wire::CanonicalUuid>>) -> Option<WorkRail> {
    let Some(creator) = task.creator_team else {
        return Some(WorkRail::Global);
    };
    teams.map(|teams| {
        if teams.contains(&creator) {
            WorkRail::Team(creator)
        } else {
            WorkRail::Global
        }
    })
}

fn associated_teams(
    task: &BoardTask,
    teams: Option<&BTreeSet<wire::CanonicalUuid>>,
    member_teams: &BTreeMap<wire::CanonicalUuid, wire::CanonicalUuid>,
) -> Vec<wire::CanonicalUuid> {
    let mut associated = Vec::with_capacity(2);
    if let Some(team) = task
        .creator_team
        .filter(|team| teams.is_some_and(|teams| teams.contains(team)))
    {
        associated.push(team);
    }
    if let Some(team) = task
        .claim
        .as_ref()
        .and_then(|claim| member_teams.get(&claim.body.lane))
        .copied()
        && !associated.contains(&team)
    {
        associated.push(team);
    }
    associated
}

fn work_dependencies<'a>(
    task: &'a BoardTask,
    board: &BoardView,
    by_born: &BTreeMap<&TaskStamp, usize>,
) -> Vec<WorkDependency<'a>> {
    task.statement
        .body
        .dependencies
        .iter()
        .map(|born| {
            let task = by_born.get(born).copied();
            WorkDependency {
                born,
                task,
                satisfied: task.is_some_and(|dependency| {
                    board.tasks[dependency].workflow.body.completion.is_some()
                }),
            }
        })
        .collect()
}

/// Each task's position in the resident's offer order, by task index. A stamp
/// the resident repeats keeps its first, leading position.
fn offer_ranks(board: &BoardView, by_born: &BTreeMap<&TaskStamp, usize>) -> Vec<Option<usize>> {
    let mut ranks = vec![None; board.tasks.len()];
    for (rank, born) in board.offer.iter().enumerate() {
        if let Some(&index) = by_born.get(born) {
            ranks[index].get_or_insert(rank);
        }
    }
    ranks
}

/// Every task index in the resident's offer order, and newest first.
///
/// The resident ranks only work it would offer; that rank leads, and the rest
/// keeps lifecycle and queue order beneath it.
fn board_orders(
    board: &BoardView,
    offer_ranks: &[Option<usize>],
    availability: &[WorkAvailability],
) -> (Vec<usize>, Vec<usize>) {
    let rank = |index: &usize| offer_ranks[*index].unwrap_or(usize::MAX);
    let mut offered = (0..board.tasks.len()).collect::<Vec<_>>();
    offered.sort_by(|left, right| {
        rank(left)
            .cmp(&rank(right))
            .then_with(|| compare_tasks(&board.tasks, availability, *left, *right))
    });
    let mut newest = (0..board.tasks.len()).collect::<Vec<_>>();
    newest.sort_by(|left, right| {
        board.tasks[*right]
            .statement
            .body
            .born
            .cmp(&board.tasks[*left].statement.body.born)
    });
    (offered, newest)
}

/// The name an operator reads for one task owner.
///
/// A seated lane reads as its agent's configured name or worktree, exactly as
/// its message cards sign themselves. A lane absent from the current topology
/// keeps a short identity prefix, since no name for it is known. The full
/// actor spelling remains the caller's to expose for disambiguation.
#[must_use]
pub fn owner_label(actor: &Actor, member: Option<&LaneMember>) -> String {
    match actor {
        Actor::Operator => "Operator".to_owned(),
        Actor::Spice => "Spice".to_owned(),
        Actor::Lane(lane) => member.filter(|member| member.lane == *lane).map_or_else(
            || format!("lane {}", watchband::short(*lane)),
            |member| {
                watchband::agent_message_label(&member.worktree, member.name.as_deref()).to_owned()
            },
        ),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct StemKey<'a> {
    visibility: WorkVisibility,
    stem: &'a str,
}

#[derive(Default)]
struct StemBuilder<'a> {
    counts: WorkCounts,
    projects: BTreeMap<&'a str, ProjectBuilder>,
}

#[derive(Default)]
struct ProjectBuilder {
    counts: WorkCounts,
    tasks: Vec<usize>,
}

fn dependent_indexes(
    board: &BoardView,
    by_born: &BTreeMap<&TaskStamp, usize>,
    availability: &[WorkAvailability],
) -> Vec<Vec<usize>> {
    let mut dependents = vec![Vec::new(); board.tasks.len()];
    for (dependent, task) in board.tasks.iter().enumerate() {
        for dependency in &task.statement.body.dependencies {
            if let Some(&index) = by_born.get(dependency) {
                dependents[index].push(dependent);
            }
        }
    }
    for indexes in &mut dependents {
        indexes.sort_by(|left, right| compare_tasks(&board.tasks, availability, *left, *right));
    }
    dependents
}

fn stem_key(task: &BoardTask) -> StemKey<'_> {
    match &task.visibility {
        ProjectVisibility::Public => StemKey {
            visibility: WorkVisibility::Public,
            stem: task
                .statement
                .body
                .project
                .trim_start_matches('.')
                .split('.')
                .next()
                .filter(|stem| !stem.is_empty())
                .unwrap_or("task"),
        },
        ProjectVisibility::Hidden { stem } => StemKey {
            visibility: WorkVisibility::Hidden,
            stem,
        },
    }
}

fn classify(
    board: &BoardView,
    task: &BoardTask,
    by_born: &BTreeMap<&TaskStamp, usize>,
    environment_blocked: bool,
) -> WorkAvailability {
    let workflow = &task.workflow.body;
    if workflow.status != "pending" || workflow.completion.is_some() {
        return WorkAvailability::Terminal;
    }
    if task
        .claim
        .as_ref()
        .is_some_and(|claim| claim.body.disposition == ClaimDisposition::Active)
    {
        return WorkAvailability::InFlight;
    }
    if workflow.deferral.is_some() {
        return WorkAvailability::Deferred;
    }
    let dependency_blocked = task.statement.body.dependencies.iter().any(|dependency| {
        by_born
            .get(dependency)
            .is_none_or(|&index| board.tasks[index].workflow.body.completion.is_none())
    });
    if environment_blocked || dependency_blocked {
        WorkAvailability::Blocked
    } else {
        WorkAvailability::Ready
    }
}

fn compare_tasks(
    tasks: &[BoardTask],
    availability: &[WorkAvailability],
    left: usize,
    right: usize,
) -> Ordering {
    availability[left]
        .cmp(&availability[right])
        .then_with(|| {
            tasks[left]
                .statement
                .body
                .queue_origin
                .cmp(&tasks[right].statement.body.queue_origin)
        })
        .then_with(|| {
            tasks[left]
                .statement
                .body
                .born
                .cmp(&tasks[right].statement.body.born)
        })
}

#[cfg(test)]
pub(crate) mod tests;
