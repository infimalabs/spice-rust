//! Pure task projection shared by every app task control and overview.
//!
//! The admitted board snapshot and current lane topology are the sole inputs.
//! This module owns lifecycle precedence, stable selection scope identity,
//! full-project grouping, relationship summaries, counts, board arrangements
//! over the resident's offer order, and owner naming so consumers cannot drift.

#[cfg(any(target_arch = "wasm32", test))]
pub(crate) mod lens;
mod overview;
mod relationship;
mod wall;
pub use overview::{
    TaskOverviewContext, TaskOverviewEdge, TaskOverviewGraph, TaskOverviewNode,
    TaskOverviewNodeKind, TaskOverviewSelection,
};
pub use relationship::{
    WorkRelationshipDirection, WorkRelationshipEdge, WorkRelationshipError,
    WorkRelationshipExpansion, WorkRelationshipFrontier, WorkRelationshipGraph,
    WorkRelationshipKind, WorkRelationshipNode, WorkRelationshipOptions,
    WorkRelationshipOriginNode, WorkRelationshipTaskNode,
};
pub use wall::{DependencyWall, DependencyWalls};
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

/// One mutually exclusive task lifecycle in the current board projection.
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
    /// Open and unclaimed in a hidden project the allocator never offers,
    /// such as a filed OOPS report awaiting promotion.
    Parked,
    /// No longer open, regardless of stale possession evidence.
    Terminal,
}

impl WorkAvailability {
    /// Stable presentation key shared by every task lifecycle surface.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::InFlight => "in-flight",
            Self::Ready => "ready",
            Self::Blocked => "blocked",
            Self::Deferred => "deferred",
            Self::Parked => "parked",
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
/// Browser-local policy selecting a complete set for the unified overview.
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

/// One visibility-qualified stem and the team-local tasks grouped beneath it.
///
/// Groups follow their first task in lifecycle and offer order, matching the
/// task-strip grouping rule. Tasks inside a group retain that same order.
#[derive(Debug)]
pub struct TeamTaskGroup<'p, 'a> {
    stem: &'p WorkStemIdentity,
    tasks: Vec<&'p WorkTask<'a>>,
}

impl<'p, 'a> TeamTaskGroup<'p, 'a> {
    #[must_use]
    pub const fn stem(&self) -> &WorkStemIdentity {
        self.stem
    }

    #[must_use]
    pub fn tasks(&self) -> &[&'p WorkTask<'a>] {
        &self.tasks
    }
}

/// One lane working a working group, with the group's associated in-flight
/// tasks it actively claims.
///
/// Lanes follow their first task in offer order, the team rail's order, and
/// tasks inside a lane retain that same order.
#[derive(Debug)]
pub struct GroupLane<'p, 'a> {
    lane: wire::CanonicalUuid,
    tasks: Vec<&'p WorkTask<'a>>,
}

impl<'p, 'a> GroupLane<'p, 'a> {
    #[must_use]
    pub const fn lane(&self) -> wire::CanonicalUuid {
        self.lane
    }

    #[must_use]
    pub fn tasks(&self) -> &[&'p WorkTask<'a>] {
        &self.tasks
    }
}

/// Exclusive lifecycle totals for one scope.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkCounts {
    in_flight: usize,
    ready: usize,
    blocked: usize,
    deferred: usize,
    parked: usize,
    terminal: usize,
}

impl WorkCounts {
    fn record(&mut self, availability: WorkAvailability) {
        match availability {
            WorkAvailability::InFlight => self.in_flight += 1,
            WorkAvailability::Ready => self.ready += 1,
            WorkAvailability::Blocked => self.blocked += 1,
            WorkAvailability::Deferred => self.deferred += 1,
            WorkAvailability::Parked => self.parked += 1,
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
        self.blocked + self.deferred + self.parked
    }
    #[must_use]
    pub const fn open(self) -> usize {
        self.in_flight + self.ready + self.blocked + self.deferred + self.parked
    }

    #[must_use]
    pub const fn has_open(self) -> bool {
        self.open() > 0
    }
}

impl FromIterator<WorkAvailability> for WorkCounts {
    fn from_iter<I: IntoIterator<Item = WorkAvailability>>(availabilities: I) -> Self {
        let mut counts = Self::default();
        for availability in availabilities {
            counts.record(availability);
        }
        counts
    }
}

/// One dependency edge as resolved against this admitted board snapshot.
#[derive(Debug)]
pub struct WorkDependency<'a> {
    /// Stable dependency identity even when its row is unavailable.
    born: &'a TaskStamp,
    /// Resolved task index in [`WorkProjection::tasks`].
    task: Option<usize>,
    /// Whether the dependency is settled: its row has a completion head, or the
    /// open board omits it, which it does only for terminal prerequisites.
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
    /// The one current roster team that homes this task, by strength: the
    /// team of the lane actively claiming it (none when that lane is
    /// unteamed), then its creator team, then its last claimant's team.
    team: Option<wire::CanonicalUuid>,
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

    /// The canonical member accent for the lane currently working this task.
    ///
    /// Operator claims and lanes outside the admitted roster have no member
    /// accent. `working_claimant` excludes stale and terminal claim evidence.
    #[must_use]
    pub fn working_claimant_accent(&self, lanes: Option<&LaneView>) -> Option<usize> {
        let Actor::Lane(lane) = self.working_claimant()? else {
            return None;
        };
        watchband::lane_accent(&lanes?.lanes, *lane)
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

    #[must_use]
    pub fn belongs_to_team(&self, team: wire::CanonicalUuid) -> bool {
        self.team == Some(team)
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

    #[must_use]
    pub fn scope_counts(&self, scope: &WorkScope) -> WorkCounts {
        match scope {
            WorkScope::AllOpen => self.counts,
            WorkScope::Stem(identity) => self.stem(identity).map_or(self.counts, WorkStem::counts),
            WorkScope::Team { .. } => self
                .tasks
                .iter()
                .filter(|task| self.in_scope(task, scope))
                .map(|task| task.availability)
                .collect(),
        }
    }

    /// Whether one task belongs to the selected overview scope.
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

    /// The exact stem holding one task.
    #[must_use]
    pub fn task_stem(&self, task: &WorkTask<'_>) -> &WorkStemIdentity {
        &self.stems[self.projects[task.project].stem].identity
    }

    /// One team's open task rail, grouped by visibility-qualified stem.
    ///
    /// The source order is in-flight, ready, blocked, then deferred, with each
    /// lifecycle retaining resident offer order. As in the global task strip,
    /// groups follow their first task and gather every later match without
    /// changing the tasks' relative order.
    #[must_use]
    pub fn team_task_groups<'p>(&'p self, team: wire::CanonicalUuid) -> Vec<TeamTaskGroup<'p, 'a>> {
        use WorkAvailability::{Blocked, Deferred, InFlight, Parked, Ready};

        let mut groups = Vec::<TeamTaskGroup<'p, 'a>>::new();
        for availability in [InFlight, Ready, Blocked, Deferred, Parked] {
            for task in self.offered.iter().map(|&index| &self.tasks[index]) {
                if task.availability != availability || !task.belongs_to_team(team) {
                    continue;
                }
                let stem = self.task_stem(task);
                if let Some(group) = groups.iter_mut().find(|group| group.stem == stem) {
                    group.tasks.push(task);
                } else {
                    groups.push(TeamTaskGroup {
                        stem,
                        tasks: vec![task],
                    });
                }
            }
        }
        groups
    }

    /// The lanes currently working one working group: exactly the lanes
    /// actively claiming an in-flight task associated with any of its designs.
    #[must_use]
    pub fn group_lanes<'p>(&'p self, designs: &[wire::TaskStamp]) -> Vec<GroupLane<'p, 'a>> {
        let mut lanes = Vec::<GroupLane<'p, 'a>>::new();
        for task in self.offered.iter().map(|&index| &self.tasks[index]) {
            let Some(Actor::Lane(lane)) = task.working_claimant() else {
                continue;
            };
            let statement = &task.row.statement.body;
            if !designs
                .iter()
                .any(|design| crate::runtime::associated(statement, design))
            {
                continue;
            }
            if let Some(group) = lanes.iter_mut().find(|group| group.lane == *lane) {
                group.tasks.push(task);
            } else {
                lanes.push(GroupLane {
                    lane: *lane,
                    tasks: vec![task],
                });
            }
        }
        lanes
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
            .map(|(index, task)| classify(board, task, environment_blocked[index]))
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
                let team = home_team(row, teams.as_ref(), member_teams);
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
                    team,
                }
            })
            .collect();

        let offered = board_order(board, &offer_ranks, &availability);

        Self {
            counts,
            tasks,
            projects,
            stems,
            offered,
            teams,
        }
    }
}

fn home_team(
    task: &BoardTask,
    teams: Option<&BTreeSet<wire::CanonicalUuid>>,
    member_teams: &BTreeMap<wire::CanonicalUuid, wire::CanonicalUuid>,
) -> Option<wire::CanonicalUuid> {
    let claimant_team = |claim: &wire::head::ClaimHead| member_teams.get(&claim.lane).copied();
    if let Some(claim) = task
        .claim
        .as_ref()
        .filter(|claim| claim.body.disposition == wire::head::ClaimDisposition::Active)
    {
        return claimant_team(&claim.body);
    }
    task.creator_team
        .filter(|team| teams.is_some_and(|teams| teams.contains(team)))
        .or_else(|| {
            task.claim
                .as_ref()
                .and_then(|claim| claimant_team(&claim.body))
        })
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
                satisfied: task.is_none_or(|dependency| {
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

/// Every task index in the resident's offer order.
///
/// The resident ranks only work it would offer; that rank leads, and the rest
/// keeps lifecycle and queue order beneath it.
fn board_order(
    board: &BoardView,
    offer_ranks: &[Option<usize>],
    availability: &[WorkAvailability],
) -> Vec<usize> {
    let rank = |index: &usize| offer_ranks[*index].unwrap_or(usize::MAX);
    let mut offered = (0..board.tasks.len()).collect::<Vec<_>>();
    offered.sort_by(|left, right| {
        rank(left)
            .cmp(&rank(right))
            .then_with(|| compare_tasks(&board.tasks, availability, *left, *right))
    });
    offered
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

/// The allocator never offers a hidden row, and its own prerequisite and
/// controller gate decides Blocked; the resident judges that gate over
/// terminal evidence the open board does not carry.
fn classify(board: &BoardView, task: &BoardTask, environment_blocked: bool) -> WorkAvailability {
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
    if matches!(task.visibility, ProjectVisibility::Hidden { .. }) {
        return WorkAvailability::Parked;
    }
    if workflow.deferral.is_some() {
        return WorkAvailability::Deferred;
    }
    if environment_blocked || board.blocked.contains(&task.statement.body.born) {
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
