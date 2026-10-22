//! Pure task projection shared by every app task control and overview.
//!
//! The admitted board snapshot and current lane topology are the sole inputs.
//! This module owns lifecycle precedence, stable selection scope identity,
//! full-project grouping, relationship summaries, counts, board arrangements
//! over the resident's offer order, and owner naming so consumers cannot drift.

#[cfg(any(target_arch = "wasm32", test))]
pub(crate) mod lens;
mod operator;
mod overview;
mod relationship;
mod settlement;
mod wall;
pub use operator::{OperatorQueue, OperatorWait};
pub use overview::{
    TaskOverviewBase, TaskOverviewContext, TaskOverviewEdge, TaskOverviewGraph, TaskOverviewNode,
    TaskOverviewNodeKind, TaskOverviewSelection,
};
pub use relationship::{
    WorkRelationshipDirection, WorkRelationshipEdge, WorkRelationshipError,
    WorkRelationshipExpansion, WorkRelationshipFrontier, WorkRelationshipGraph,
    WorkRelationshipKind, WorkRelationshipNode, WorkRelationshipOptions,
    WorkRelationshipOriginNode, WorkRelationshipTaskNode,
};
pub use settlement::{SettlementAct, SettlementRecord};
pub use wall::DependencyWall;
#[cfg(test)]
mod team_scope_tests;

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Display, Formatter};

use wire::head::ClaimDisposition;
use wire::intent::{BoardTask, ProjectVisibility};
use wire::{Actor, TaskStamp};

use crate::runtime::{BoardView, LaneMember, LaneView};
use crate::view::{WorkRoute, WorkRouteStem};
use crate::watchband;

/// One mutually exclusive task lifecycle in the current board projection,
/// following task status's standing precedence for open work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkAvailability {
    /// A current active claim possesses this open task.
    InFlight,
    /// Open, unclaimed, and immediately workable.
    Ready,
    /// Open and unclaimed in its review phase, awaiting a reviewer.
    Review,
    /// Open, with a lapsed claim awaiting takeover.
    Stale,
    /// Open and unclaimed, held at a dependency or controller gate only a
    /// human can repair.
    Blocked,
    /// Open and unclaimed, following normal Plan/Design control or a
    /// prerequisite chain that still reaches advancing work.
    Waiting,
    /// Open and unclaimed, a Design outside every working group: no lane can
    /// acquire it until the operator moves it into one.
    Passive,
    /// Open and unclaimed, past the gate, a Design whose remaining conclusion
    /// waits on the operator's selection of an output it constructed.
    Selection,
    /// Open and unclaimed, past the gate but needing an environment
    /// requirement only the operator can satisfy.
    Held,
    /// Open and unclaimed, with an explicit wakeable deferral.
    Deferred,
    /// Open and unclaimed internal row outside public allocation scope.
    Parked,
    /// Editable prospective work that is not filed yet. It is not work: no
    /// strip, count, or allocator ever includes it.
    Draft,
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
            Self::Review => "review",
            Self::Stale => "stale",
            Self::Blocked => "blocked",
            Self::Waiting => "waiting",
            Self::Passive => "passive",
            Self::Selection => "selection",
            Self::Held => "held",
            Self::Deferred => "deferred",
            Self::Parked => "parked",
            Self::Draft => "draft",
            Self::Terminal => "terminal",
        }
    }

    /// Whether an agent can take this open work up without the operator.
    #[must_use]
    pub const fn moving(self) -> bool {
        matches!(
            self,
            Self::InFlight | Self::Ready | Self::Review | Self::Stale
        )
    }
}

/// Exact public or hidden stem identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct WorkStemIdentity {
    visibility: WorkVisibility,
    stem: String,
}

/// Visibility is part of identity: hidden and public stems never collapse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkVisibility {
    Public,
    Hidden,
}

/// Current UI-placement home, separate from immutable creator provenance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkOwner {
    /// Current live team home.
    Team(wire::CanonicalUuid),
    /// The Design-planning namespace. A task can depend on more than one open
    /// Design, so no single Design stamp is an owner identity.
    Design,
}

/// Browser-local policy selecting a complete set for the unified overview.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum WorkScope {
    /// Every open execution task across every visibility-qualified stem.
    #[default]
    AllOpen,
    /// Open execution tasks under one exact public or hidden stem.
    ///
    /// Work gated by an open Design uses [`Self::Planning`] even when it shares
    /// the same project stem.
    Stem(WorkStemIdentity),
    /// Open Designs and every task transitively gated by an open Design.
    Planning { stem: Option<WorkStemIdentity> },
    /// Open execution tasks currently worked by one team's members.
    Team { team: wire::CanonicalUuid },
    /// Open tasks currently worked within any Design of one working group, as
    /// its partition held them when the scope was chosen.
    Group {
        group: wire::rooms::RoomAuthorityStamp,
        designs: Vec<wire::TaskStamp>,
    },
}
/// Resolution of one durable Work identity against hydrated mutable authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WorkRouteResolution {
    /// The board, team topology, or working-group partition has not settled.
    Pending,
    /// Hydrated authority proves that this identity is absent.
    Missing,
    /// The current authority resolves the identity to this selection scope.
    Resolved(WorkScope),
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
    /// The exact stem this scope narrows to, alone or within the
    /// Design-planning rail.
    #[must_use]
    pub const fn stem(&self) -> Option<&WorkStemIdentity> {
        match self {
            Self::Stem(identity)
            | Self::Planning {
                stem: Some(identity),
            } => Some(identity),
            Self::AllOpen
            | Self::Planning { stem: None }
            | Self::Team { .. }
            | Self::Group { .. } => None,
        }
    }

    /// Every open Design-owned planning task.
    #[must_use]
    pub const fn planning() -> Self {
        Self::Planning { stem: None }
    }

    #[must_use]
    pub const fn team(&self) -> Option<wire::CanonicalUuid> {
        match self {
            Self::Team { team } => Some(*team),
            Self::AllOpen | Self::Stem(_) | Self::Planning { .. } | Self::Group { .. } => None,
        }
    }
}

impl WorkScope {
    /// Stable durable identity for this scope. Current group membership is not
    /// serialized into the route.
    #[must_use]
    pub(crate) fn route(&self) -> WorkRoute {
        match self {
            Self::AllOpen => WorkRoute::AllOpen,
            Self::Stem(stem) => WorkRoute::Stem(stem.route_stem()),
            Self::Planning { stem } => WorkRoute::Planning {
                stem: stem.as_ref().map(WorkStemIdentity::route_stem),
            },
            Self::Team { team } => WorkRoute::Team(*team),
            Self::Group { group, .. } => WorkRoute::Group(group.clone()),
        }
    }
}

impl WorkStemIdentity {
    fn route_stem(&self) -> WorkRouteStem {
        WorkRouteStem::new(self.hidden(), self.stem.clone())
            .expect("an admitted board project has a canonical stem")
    }
}

/// Resolve a durable Work identity through current board, topology, and group
/// authority. All-open and all-planning exist even before either is populated.
#[must_use]
pub(crate) fn resolve_work_route(
    route: &WorkRoute,
    work: Option<&WorkProjection<'_>>,
    groups: Option<&wire::rooms::WorkingGroupPartition>,
) -> WorkRouteResolution {
    let stem = |stem: &WorkRouteStem| {
        WorkStemIdentity::new(
            if stem.hidden() {
                WorkVisibility::Hidden
            } else {
                WorkVisibility::Public
            },
            stem.stem().to_owned(),
        )
    };
    match route {
        WorkRoute::AllOpen => WorkRouteResolution::Resolved(WorkScope::AllOpen),
        WorkRoute::Planning { stem: None } => WorkRouteResolution::Resolved(WorkScope::planning()),
        WorkRoute::Stem(route_stem)
        | WorkRoute::Planning {
            stem: Some(route_stem),
        } => {
            let Some(work) = work else {
                return WorkRouteResolution::Pending;
            };
            let identity = stem(route_stem);
            let scope = if matches!(route, WorkRoute::Stem(_)) {
                WorkScope::Stem(identity)
            } else {
                WorkScope::Planning {
                    stem: Some(identity),
                }
            };
            if work.contains_scope(&scope) == Some(true) {
                WorkRouteResolution::Resolved(scope)
            } else {
                WorkRouteResolution::Missing
            }
        }
        WorkRoute::Team(team) => {
            let Some(work) = work else {
                return WorkRouteResolution::Pending;
            };
            let scope = WorkScope::Team { team: *team };
            match work.contains_scope(&scope) {
                Some(true) => WorkRouteResolution::Resolved(scope),
                Some(false) => WorkRouteResolution::Missing,
                None => WorkRouteResolution::Pending,
            }
        }
        WorkRoute::Group(group) => {
            let Some(groups) = groups else {
                return WorkRouteResolution::Pending;
            };
            groups
                .groups
                .iter()
                .find(|candidate| candidate.group == *group)
                .map_or(WorkRouteResolution::Missing, |candidate| {
                    WorkRouteResolution::Resolved(WorkScope::Group {
                        group: candidate.group.clone(),
                        designs: candidate.anchors.clone(),
                    })
                })
        }
    }
}

/// One visibility-qualified stem and the scoped tasks grouped beneath it.
///
/// Groups follow their first task in lifecycle and offer order, matching the
/// task-strip grouping rule. Tasks inside a group retain that same order.
#[derive(Debug)]
pub struct StemTaskGroup<'p, 'a> {
    stem: &'p WorkStemIdentity,
    tasks: Vec<&'p WorkTask<'a>>,
}

impl<'p, 'a> StemTaskGroup<'p, 'a> {
    #[must_use]
    pub const fn stem(&self) -> &WorkStemIdentity {
        self.stem
    }

    #[must_use]
    pub fn tasks(&self) -> &[&'p WorkTask<'a>] {
        &self.tasks
    }

    #[must_use]
    pub fn counts(&self) -> WorkCounts {
        self.tasks.iter().copied().collect()
    }
}

/// The most actionable state one Work partition holds, which marks its tab.
/// Variants run from most to least actionable; the first one present wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkMark {
    /// Work only the operator can unblock: a dependency or controller wall
    /// with no advancing path, or an environment hold such as a trust grant.
    Blocked,
    /// A Design holding a proposal the operator has not decided.
    Decision,
    /// A steeped or nudged Design no lane holds, ready for a new proposal.
    Proposal,
}

impl WorkMark {
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::Decision => "decision",
            Self::Proposal => "proposal",
        }
    }
}

/// Exclusive lifecycle totals for one scope, plus the unpossessed Designs
/// among them whose pass cycle is ready for a proposal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkCounts {
    in_flight: usize,
    ready: usize,
    review: usize,
    stale: usize,
    blocked: usize,
    workflow_waiting: usize,
    passive: usize,
    selection: usize,
    held: usize,
    deferred: usize,
    parked: usize,
    terminal: usize,
    /// Overlaps `ready` and `passive`; never part of [`Self::open`].
    proposal_ready: usize,
}

impl WorkCounts {
    fn record(&mut self, availability: WorkAvailability, proposal_ready: bool) {
        self.proposal_ready += usize::from(proposal_ready);
        match availability {
            WorkAvailability::InFlight => self.in_flight += 1,
            WorkAvailability::Ready => self.ready += 1,
            WorkAvailability::Review => self.review += 1,
            WorkAvailability::Stale => self.stale += 1,
            WorkAvailability::Blocked => self.blocked += 1,
            WorkAvailability::Waiting => self.workflow_waiting += 1,
            WorkAvailability::Passive => self.passive += 1,
            WorkAvailability::Selection => self.selection += 1,
            WorkAvailability::Held => self.held += 1,
            WorkAvailability::Deferred => self.deferred += 1,
            WorkAvailability::Parked => self.parked += 1,
            WorkAvailability::Terminal => self.terminal += 1,
            WorkAvailability::Draft => {}
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
    pub const fn review(self) -> usize {
        self.review
    }

    #[must_use]
    pub const fn stale(self) -> usize {
        self.stale
    }

    #[must_use]
    pub const fn blocked(self) -> usize {
        self.blocked
    }
    #[must_use]
    pub const fn workflow_waiting(self) -> usize {
        self.workflow_waiting
    }

    /// Open Designs outside every working group.
    #[must_use]
    pub const fn passive(self) -> usize {
        self.passive
    }

    #[must_use]
    pub const fn deferred(self) -> usize {
        self.deferred
    }

    #[must_use]
    pub const fn terminal(self) -> usize {
        self.terminal
    }

    /// Open work no agent is offered until its workflow or the operator moves:
    /// neutral Plan/Design sequencing, environment holds, a passive Design,
    /// selection, deferral, or hidden parked work.
    #[must_use]
    pub const fn waiting(self) -> usize {
        self.workflow_waiting
            + self.held
            + self.passive
            + self.selection
            + self.deferred
            + self.parked
    }

    /// The partition's tab mark and how many tasks carry it: work only the
    /// operator can unblock, then a Design awaiting the operator's decision,
    /// then an unpossessed Design ready for a proposal. Deferral is an
    /// intentional set-aside; neutral workflow waiting, parked work, and a
    /// passive Design with no ready pass need nothing and mark nothing.
    #[must_use]
    pub const fn mark(self) -> Option<(WorkMark, usize)> {
        let blocked = self.blocked + self.held;
        if blocked > 0 {
            Some((WorkMark::Blocked, blocked))
        } else if self.selection > 0 {
            Some((WorkMark::Decision, self.selection))
        } else if self.proposal_ready > 0 {
            Some((WorkMark::Proposal, self.proposal_ready))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn open(self) -> usize {
        self.in_flight + self.ready + self.review + self.stale + self.blocked + self.waiting()
    }

    #[must_use]
    pub const fn has_open(self) -> bool {
        self.open() > 0
    }
}

impl<'p, 'a> FromIterator<&'p WorkTask<'a>> for WorkCounts {
    fn from_iter<I: IntoIterator<Item = &'p WorkTask<'a>>>(tasks: I) -> Self {
        let mut counts = Self::default();
        for task in tasks {
            counts.record(task.availability, task.proposal_ready);
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
    /// A Design held for the operator's selection of an output it constructed,
    /// whether or not a lane still holds its claim.
    awaiting_selection: bool,
    /// An unpossessed Design, offered or passive, whose pass cycle is ready
    /// for a proposal.
    proposal_ready: bool,
    /// Position in the resident's offer order, when the allocator offers it.
    offer_rank: Option<usize>,
    dependencies: Vec<WorkDependency<'a>>,
    dependents: Vec<WorkDependent>,
    /// Current accountable placement owner; immutable creator-team provenance
    /// remains on the board row.
    owner: Option<WorkOwner>,
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

    #[must_use]
    pub const fn awaiting_selection(&self) -> bool {
        self.awaiting_selection
    }

    /// An unpossessed Design, offered or passive, whose pass cycle is ready
    /// for a proposal: the state its tab marks.
    #[must_use]
    pub const fn proposal_ready(&self) -> bool {
        self.proposal_ready
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

    /// The independently steerable lane bound to this task's current claim.
    ///
    /// Terminal rows can retain stale active-claim evidence, so lifecycle must
    /// agree before the claim's lane is presented as current work.
    #[must_use]
    pub fn working_lane(&self) -> Option<wire::CanonicalUuid> {
        if self.availability != WorkAvailability::InFlight {
            return None;
        }
        self.row
            .claim
            .as_ref()
            .filter(|claim| claim.body.disposition == ClaimDisposition::Active)
            .map(|claim| claim.body.lane)
    }

    /// The canonical member accent for the lane bound to this task's current claim.
    ///
    /// Lanes outside the admitted roster have no member accent.
    /// [`Self::working_lane`] excludes stale and terminal claim evidence.
    #[must_use]
    pub fn working_claimant_accent(&self, lanes: Option<&LaneView>) -> Option<usize> {
        watchband::lane_accent(&lanes?.lanes, self.working_lane()?)
    }

    /// The actor that owns this task: its active claimant, else its assignee.
    #[must_use]
    pub fn owner(&self) -> Option<&'a Actor> {
        self.row
            .claim
            .as_ref()
            .filter(|claim| claim.body.disposition == ClaimDisposition::Active)
            .map(|claim| &claim.body.actor)
            .or_else(|| self.row.workflow.body.status.reviewer())
    }

    #[must_use]
    pub const fn accountable_owner(&self) -> Option<&WorkOwner> {
        self.owner.as_ref()
    }

    #[must_use]
    pub fn belongs_to_team(&self, team: wire::CanonicalUuid) -> bool {
        self.owner == Some(WorkOwner::Team(team))
    }

    #[must_use]
    pub fn belongs_to_design(&self) -> bool {
        matches!(self.owner, Some(WorkOwner::Design))
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
    /// Current independently steerable member to team placement.
    member_teams: BTreeMap<wire::CanonicalUuid, wire::CanonicalUuid>,
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
            WorkScope::AllOpen | WorkScope::Planning { stem: None } => Some(true),
            WorkScope::Stem(identity)
            | WorkScope::Planning {
                stem: Some(identity),
            } => Some(self.stem(identity).is_some()),
            WorkScope::Team { team } => self.teams.as_ref().map(|teams| teams.contains(team)),
            WorkScope::Group { .. } => Some(true),
        }
    }

    #[must_use]
    pub fn scope_counts(&self, scope: &WorkScope) -> WorkCounts {
        let mut counts = WorkCounts::default();
        let mut matched = false;
        for task in &self.tasks {
            if self.in_scope(task, scope) {
                matched = true;
                counts.record(task.availability, task.proposal_ready);
            }
        }
        if matched {
            return counts;
        }
        match scope {
            WorkScope::Stem(_) => self.scope_counts(&WorkScope::AllOpen),
            WorkScope::Planning { stem: Some(_) } => self.scope_counts(&WorkScope::planning()),
            _ => WorkCounts::default(),
        }
    }

    /// Whether one task belongs to the selected overview scope.
    ///
    /// A team holds exactly what its member lanes actively claim, whatever the
    /// task's accountable owner, so a member-claimed Design shows in both its
    /// team and Planning while staying out of All open and its stem.
    #[must_use]
    pub fn in_scope(&self, task: &WorkTask<'_>, scope: &WorkScope) -> bool {
        match scope {
            WorkScope::AllOpen => !task.belongs_to_design(),
            WorkScope::Stem(identity) => {
                !task.belongs_to_design() && self.task_stem(task) == identity
            }
            WorkScope::Planning { stem } => {
                task.belongs_to_design()
                    && stem
                        .as_ref()
                        .is_none_or(|stem| self.task_stem(task) == stem)
            }
            WorkScope::Team { team } => task
                .working_lane()
                .is_some_and(|lane| self.member_teams.get(&lane) == Some(team)),
            WorkScope::Group { designs, .. } => {
                let statement = &task.row.statement.body;
                task.working_claimant().is_some()
                    && designs
                        .iter()
                        .any(|design| statement.is_associated_with_design(design))
            }
        }
    }

    /// The exact stem holding one task.
    #[must_use]
    pub fn task_stem(&self, task: &WorkTask<'_>) -> &WorkStemIdentity {
        &self.stems[self.projects[task.project].stem].identity
    }

    /// One scope's open tasks in strip order: in-flight, ready, review,
    /// stale, blocked, neutral workflow waiting, passive, selection, held,
    /// deferred, then parked, with each availability retaining resident offer
    /// order.
    #[must_use]
    pub fn scope_strip<'p>(&'p self, scope: &WorkScope) -> Vec<&'p WorkTask<'a>> {
        use WorkAvailability::{
            Blocked, Deferred, Held, InFlight, Parked, Passive, Ready, Review, Selection, Stale,
            Waiting,
        };

        let mut strip = Vec::new();
        for availability in [
            InFlight, Ready, Review, Stale, Blocked, Waiting, Passive, Selection, Held, Deferred,
            Parked,
        ] {
            strip.extend(
                self.offered
                    .iter()
                    .map(|&index| &self.tasks[index])
                    .filter(|task| task.availability == availability && self.in_scope(task, scope)),
            );
        }
        strip
    }

    /// Every active task count and one tab per lane working in a group scope.
    ///
    /// Tasks retain strip order. A malformed snapshot that binds several
    /// active tasks to one lane contributes every task to the total while
    /// presenting that lane once, at its first task's position.
    #[must_use]
    pub fn group_worker_strip<'p>(
        &'p self,
        scope: &WorkScope,
    ) -> (WorkCounts, Vec<(wire::CanonicalUuid, &'p WorkTask<'a>)>) {
        let mut counts = WorkCounts::default();
        let mut workers = Vec::new();
        for task in self.scope_strip(scope) {
            counts.record(task.availability, task.proposal_ready);
            let Some(lane) = task.working_lane() else {
                continue;
            };
            if !workers.iter().any(|(existing, _)| *existing == lane) {
                workers.push((lane, task));
            }
        }
        (counts, workers)
    }

    /// One scope's open task strip, grouped by visibility-qualified stem.
    ///
    /// Groups follow their first task in [`Self::scope_strip`] order and
    /// gather every later match without changing the tasks' relative order.
    #[must_use]
    pub fn stem_task_groups<'p>(&'p self, scope: &WorkScope) -> Vec<StemTaskGroup<'p, 'a>> {
        let mut groups = Vec::<StemTaskGroup<'p, 'a>>::new();
        for task in self.scope_strip(scope) {
            let stem = self.task_stem(task);
            if let Some(group) = groups.iter_mut().find(|group| group.stem == stem) {
                group.tasks.push(task);
            } else {
                groups.push(StemTaskGroup {
                    stem,
                    tasks: vec![task],
                });
            }
        }
        groups
    }

    pub fn from_board(board: &'a BoardView) -> Self {
        Self::from_board_and_lanes(board, None)
    }

    #[must_use]
    pub fn from_board_and_lanes(board: &'a BoardView, lanes: Option<&LaneView>) -> Self {
        let Some(lanes) = lanes else {
            return Self::from_board_with_team_memberships(board, None, BTreeMap::new());
        };
        let mut teams = BTreeSet::new();
        let mut member_teams = BTreeMap::new();
        for team in lanes.lanes.iter().filter(|team| team.teamed()) {
            teams.insert(team.lane);
            for member in &team.members {
                member_teams.insert(member.lane, team.lane);
            }
        }
        Self::from_board_with_team_memberships(board, Some(teams), member_teams)
    }

    fn from_board_with_team_memberships(
        board: &'a BoardView,
        teams: Option<BTreeSet<wire::CanonicalUuid>>,
        member_teams: BTreeMap<wire::CanonicalUuid, wire::CanonicalUuid>,
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

        let proposal_ready = proposal_ready(board, &availability);
        let dependent_indexes = dependent_indexes(board, &by_born, &availability);
        let design_owned = design_owned_indexes(board, &dependent_indexes);
        let offer_ranks = offer_ranks(board, &by_born);

        let mut counts = WorkCounts::default();
        let mut stem_builders = BTreeMap::<StemKey<'a>, StemBuilder<'a>>::new();
        for (index, task) in board.tasks.iter().enumerate() {
            counts.record(availability[index], proposal_ready[index]);
            let key = stem_key(task);
            let stem = stem_builders.entry(key).or_default();
            stem.counts
                .record(availability[index], proposal_ready[index]);
            let project = stem
                .projects
                .entry(task.statement.body.project.as_str())
                .or_default();
            project
                .counts
                .record(availability[index], proposal_ready[index]);
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
                let owner =
                    accountable_owner(row, design_owned[index], teams.as_ref(), &member_teams);
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
                    awaiting_selection: board.awaiting_selection(&row.statement.body.born),
                    proposal_ready: proposal_ready[index],
                    offer_rank: offer_ranks[index],
                    dependencies,
                    dependents,
                    owner,
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
            member_teams,
        }
    }
}

fn accountable_owner(
    task: &BoardTask,
    design_owned: bool,
    teams: Option<&BTreeSet<wire::CanonicalUuid>>,
    member_teams: &BTreeMap<wire::CanonicalUuid, wire::CanonicalUuid>,
) -> Option<WorkOwner> {
    // Dependency reachability is the complete current Design gate. Preparation
    // is immutable provenance and cannot decide where work is placed: adding
    // an open Design path docks its whole dependent subtree here, and selection
    // of the final such path moves the subtree back to execution.
    if design_owned {
        return Some(WorkOwner::Design);
    }
    home_team(task, teams, member_teams).map(WorkOwner::Team)
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
                    !board.tasks[dependency].workflow.body.status.is_pending()
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

/// Whether each task, by index, is an unpossessed Design, offered or passive,
/// whose pass cycle is ready for a proposal.
fn proposal_ready(board: &BoardView, availability: &[WorkAvailability]) -> Vec<bool> {
    board
        .tasks
        .iter()
        .zip(availability)
        .map(|(task, availability)| {
            matches!(
                availability,
                WorkAvailability::Ready | WorkAvailability::Passive
            ) && board.design_cycles.get(&task.statement.body.born)
                == Some(&wire::rooms::DesignCycle::ReadyForProposal)
        })
        .collect()
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
/// keeps a short identity prefix, since no name for it is known. A born human
/// and the historical operator share [`wire::principal::NamedActor`]'s
/// principal-directory presentation: its short name (the recorded profile's
/// initials) alone, as an agent reads, or the code alone when the profile
/// yields none. An unresolved human keeps its full actor spelling; an
/// unresolved operator keeps its role label. The full actor spelling remains
/// the caller's hover text for every owner.
#[must_use]
pub fn owner_label(
    actor: &Actor,
    member: Option<&LaneMember>,
    principals: Option<&wire::principal::PrincipalDirectory>,
) -> String {
    match actor {
        Actor::Operator => principals
            .and_then(|principals| {
                principals
                    .principal(actor)
                    .map(|_| principals.named(actor).to_string())
            })
            .unwrap_or_else(|| "Operator".to_owned()),
        Actor::Human(_) => principals.map_or_else(
            || actor.to_string(),
            |principals| principals.named(actor).to_string(),
        ),
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

/// Mark every task reachable through dependency direction from an open Design.
///
/// The visited set both bounds the walk to one pass over the graph and makes
/// manually assembled dependency cycles harmless.
fn design_owned_indexes(board: &BoardView, dependents: &[Vec<usize>]) -> Vec<bool> {
    let mut owned = vec![false; board.tasks.len()];
    let mut frontier = Vec::with_capacity(board.tasks.len());
    for (index, task) in board.tasks.iter().enumerate() {
        if task
            .design_standing()
            .is_some_and(|standing| standing.is_open())
        {
            owned[index] = true;
            frontier.push(index);
        }
    }
    while let Some(index) = frontier.pop() {
        for &dependent in &dependents[index] {
            if !owned[dependent] {
                owned[dependent] = true;
                frontier.push(dependent);
            }
        }
    }
    owned
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

/// Task status's standing precedence: possession and distinct set-aside
/// states, then the resident-owned selection/workflow/blocker partition, then
/// review and environment readiness. The resident judges closed gates over
/// terminal evidence the open board does not carry; this surface only consumes
/// that answer, and the allocator never offers a hidden row.
fn classify(board: &BoardView, task: &BoardTask, environment_blocked: bool) -> WorkAvailability {
    let workflow = &task.workflow.body;
    if !workflow.status.is_pending() {
        return WorkAvailability::Terminal;
    }
    if task.statement.body.filing.is_draft() {
        return WorkAvailability::Draft;
    }
    match task.claim.as_ref().map(|claim| claim.body.disposition) {
        Some(ClaimDisposition::Active) => return WorkAvailability::InFlight,
        _ if task.statement.body.filing.is_draft()
            || matches!(task.visibility, ProjectVisibility::Hidden { .. }) =>
        {
            return WorkAvailability::Parked;
        }
        // A passive Design still holding an output for the operator's
        // selection waits on that decision, not on grouping.
        _ if task.design_standing() == Some(wire::intent::DesignStanding::Passive) => {
            return if board.awaiting_selection(&task.statement.body.born) {
                WorkAvailability::Selection
            } else {
                WorkAvailability::Passive
            };
        }
        Some(ClaimDisposition::Lapsed) => return WorkAvailability::Stale,
        _ => {}
    }
    if workflow.deferral.is_some() {
        WorkAvailability::Deferred
    } else if board.awaiting_selection(&task.statement.body.born) {
        WorkAvailability::Selection
    } else if board.workflow_waiting.contains(&task.statement.body.born) {
        WorkAvailability::Waiting
    } else if board.blocked.contains(&task.statement.body.born) {
        WorkAvailability::Blocked
    } else if workflow.phase.as_str() == "review" {
        WorkAvailability::Review
    } else if environment_blocked {
        WorkAvailability::Held
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
