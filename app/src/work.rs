//! Pure task projection shared by every Canvas Work surface.
//!
//! The admitted board snapshot is the sole input. This module owns lifecycle
//! precedence, stable stem identity, full-project grouping, relationship
//! summaries, counts, the board arrangements over the resident's offer
//! order, and owner naming so chrome and Work views cannot drift.

mod relationship;
pub use relationship::{
    WorkRelationshipDirection, WorkRelationshipEdge, WorkRelationshipError,
    WorkRelationshipExpansion, WorkRelationshipFrontier, WorkRelationshipGraph,
    WorkRelationshipKind, WorkRelationshipNode, WorkRelationshipOptions,
    WorkRelationshipOriginNode, WorkRelationshipTaskNode,
};

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt::{self, Display, Formatter};

use wire::head::ClaimDisposition;
use wire::intent::{BoardTask, ProjectVisibility};
use wire::{Actor, TaskStamp};

use crate::runtime::{BoardView, LaneMember};
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
}

impl WorkStemIdentity {
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
    #[must_use]
    pub const fn stem(&self) -> Option<&WorkStemIdentity> {
        match self {
            Self::AllOpen => None,
            Self::Stem(identity) => Some(identity),
        }
    }
}

/// The vertical order of cards inside each shared board lane.
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
    /// One unbroken column per lane.
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
/// vertical order and separation differ, so switching never loses,
/// duplicates, or reorders durable data.
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
    dependencies: Vec<WorkDependency<'a>>,
    dependents: Vec<WorkDependent>,
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

    #[must_use]
    pub fn contains_scope(&self, scope: &WorkScope) -> bool {
        scope
            .stem()
            .is_none_or(|identity| self.stem(identity).is_some())
    }

    #[must_use]
    pub fn scope_counts(&self, scope: &WorkScope) -> WorkCounts {
        scope
            .stem()
            .and_then(|identity| self.stem(identity))
            .map_or(self.counts, WorkStem::counts)
    }

    /// Whether one task's project belongs to the selected scope.
    #[must_use]
    pub fn in_scope(&self, task: &WorkTask<'_>, scope: &WorkScope) -> bool {
        scope.stem().is_none_or(|identity| {
            self.stems[self.projects[task.project].stem].identity == *identity
        })
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

    #[must_use]
    pub fn from_board(board: &'a BoardView) -> Self {
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
                let dependencies = row
                    .statement
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
                    .collect();
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
                    dependencies,
                    dependents,
                }
            })
            .collect();

        let (offered, newest) = board_orders(board, &by_born, &availability);

        Self {
            counts,
            tasks,
            projects,
            stems,
            offered,
            newest,
        }
    }
}

/// Every task index in the resident's offer order, and newest first.
///
/// The resident ranks only work it would offer; that rank leads, and the rest
/// keeps lifecycle and queue order beneath it.
fn board_orders(
    board: &BoardView,
    by_born: &BTreeMap<&TaskStamp, usize>,
    availability: &[WorkAvailability],
) -> (Vec<usize>, Vec<usize>) {
    let offer_rank = board
        .offer
        .iter()
        .enumerate()
        .filter_map(|(rank, born)| by_born.get(born).map(|&index| (index, rank)))
        .collect::<BTreeMap<_, _>>();
    let rank = |index: &usize| offer_rank.get(index).copied().unwrap_or(usize::MAX);
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
mod tests {
    use super::*;
    use wire::intent::{BoardStatement, Positioned};
    use wire::{Actor, CanonicalUuid, ClaimCell, ClaimHead, Environment, EnvironmentRequirement};

    fn seq(value: u64) -> wire::Seq {
        wire::Seq::new(value).expect("fixture sequence is positive")
    }

    fn stamp(value: i64) -> TaskStamp {
        TaskStamp::for_micros(value).expect("fixture stamp is representable")
    }

    fn task(value: i64, project: &str) -> BoardTask {
        let born = stamp(value);
        BoardTask {
            visibility: if project.starts_with('.') {
                ProjectVisibility::Hidden {
                    stem: project
                        .trim_start_matches('.')
                        .split('.')
                        .next()
                        .expect("hidden fixture has a stem")
                        .to_owned(),
                }
            } else {
                ProjectVisibility::Public
            },
            origin_thread: "operator".to_owned(),
            creator_team: None,
            presentation_title: format!("task {value}"),
            statement: Positioned {
                position: seq(1),
                body: BoardStatement {
                    handle: wire::TaskHandle::for_project(project, &born),
                    born: born.clone(),
                    queue_origin: seq(value.unsigned_abs() + 1),
                    description: None,
                    acceptance: Vec::new(),
                    requirements: wire::EnvironmentRequirements::EMPTY,
                    project: project.to_owned(),
                    tags: Vec::new(),
                    dependencies: Vec::new(),
                    provenance: format!("task:{born}").parse().expect("fixture origin"),
                    creation: wire::body::TaskCreationContext::unavailable(
                        wire::body::TaskCreationUnavailable::OperatorActor,
                    ),
                    revision: seq(1),
                },
            },
            workflow: Positioned {
                position: seq(2),
                body: wire::WorkflowHead {
                    flow: vec!["todo".parse().expect("fixture phase")],
                    phase: "todo".parse().expect("fixture phase"),
                    status: "pending".to_owned(),
                    review: "unassigned".to_owned(),
                    assignment: None,
                    deferral: None,
                    completion: None,
                },
            },
            claim: None,
            lease: None,
            claim_context: None,
            git_context: None,
        }
    }

    fn claim(task: &BoardTask, disposition: ClaimDisposition) -> Positioned<ClaimHead> {
        Positioned {
            position: seq(3),
            body: ClaimHead {
                actor: Actor::Operator,
                fact: seq(3),
                claimed: seq(3),
                cell: ClaimCell {
                    project: task.statement.body.project.clone(),
                    phase: task.workflow.body.phase.clone(),
                },
                lane: "11111111-1111-4111-8111-111111111111"
                    .parse::<CanonicalUuid>()
                    .expect("fixture lane"),
                occupant: "22222222-2222-4222-8222-222222222222"
                    .parse::<CanonicalUuid>()
                    .expect("fixture occupant"),
                epoch: "33333333-3333-4333-8333-333333333333"
                    .parse::<CanonicalUuid>()
                    .expect("fixture epoch"),
                recovery: None,
                disposition,
            },
        }
    }

    fn board(tasks: Vec<BoardTask>) -> BoardView {
        BoardView {
            schema: 1,
            cursor: seq(100),
            tasks,
            offer: Vec::new(),
            resident: wire::ResidentState::Seated,
            environment: Environment::journal_schema(1).expect("fixture schema"),
        }
    }

    #[test]
    fn lifecycle_precedence_is_terminal_then_claim_then_deferral_then_blocker() {
        let mut terminal = task(1, "alpha.lifecycle");
        terminal.workflow.body.completion = Some(
            "2026-09-23T06:00:00.000Z"
                .parse()
                .expect("fixture completion"),
        );
        terminal.claim = Some(claim(&terminal, ClaimDisposition::Active));

        let mut in_flight = task(2, "alpha.lifecycle");
        in_flight.workflow.body.deferral = Some(
            "2026-09-24T06:00:00.000Z"
                .parse()
                .expect("fixture deferral"),
        );
        in_flight.statement.body.requirements = wire::EnvironmentRequirements::one(
            EnvironmentRequirement::journal_schema(2).expect("fixture requirement"),
        );
        in_flight.claim = Some(claim(&in_flight, ClaimDisposition::Active));

        let mut deferred = task(3, "alpha.lifecycle");
        deferred.workflow.body.deferral = Some(
            "2026-09-24T06:00:00.000Z"
                .parse()
                .expect("fixture deferral"),
        );
        deferred.statement.body.requirements = wire::EnvironmentRequirements::one(
            EnvironmentRequirement::journal_schema(2).expect("fixture requirement"),
        );

        let mut blocked = task(4, "alpha.lifecycle");
        blocked.statement.body.requirements = wire::EnvironmentRequirements::one(
            EnvironmentRequirement::journal_schema(2).expect("fixture requirement"),
        );

        let ready = task(5, "alpha.lifecycle");
        let board = board(vec![terminal, in_flight, deferred, blocked, ready]);
        let projection = WorkProjection::from_board(&board);

        assert_eq!(
            projection
                .tasks
                .iter()
                .map(|task| task.availability)
                .collect::<Vec<_>>(),
            [
                WorkAvailability::Terminal,
                WorkAvailability::InFlight,
                WorkAvailability::Deferred,
                WorkAvailability::Blocked,
                WorkAvailability::Ready,
            ]
        );
        assert_eq!(projection.tasks[0].working_claimant(), None);
        assert_eq!(
            projection.tasks[1].working_claimant(),
            Some(&Actor::Operator),
        );
        assert!(projection.tasks[1].environment_blocked);
        assert!(projection.tasks[2].environment_blocked);
        assert!(projection.tasks[3].environment_blocked);
    }

    #[test]
    fn visibility_qualified_stems_share_counts_without_collapsing_projects() {
        let mut active = task(1, "alpha.runtime");
        active.claim = Some(claim(&active, ClaimDisposition::Active));
        let ready = task(2, "alpha.render");
        let hidden = task(3, ".alpha.audit");
        let mut terminal = task(4, "alpha.runtime");
        terminal.workflow.body.status = "completed".to_owned();
        let board = board(vec![active, ready, hidden, terminal]);
        let projection = WorkProjection::from_board(&board);

        assert_eq!(projection.stems.len(), 2);
        assert_eq!(
            projection
                .stems
                .iter()
                .map(|stem| (&stem.identity.visibility, stem.identity.stem.as_str()))
                .collect::<Vec<_>>(),
            [
                (&WorkVisibility::Public, "alpha"),
                (&WorkVisibility::Hidden, "alpha"),
            ]
        );
        assert!(!projection.stems[0].identity.hidden());
        assert!(projection.stems[1].identity.hidden());
        assert_eq!(
            projection.stems[0].counts,
            WorkCounts {
                in_flight: 1,
                ready: 1,
                terminal: 1,
                ..WorkCounts::default()
            }
        );
        assert_eq!(projection.stems[1].counts.ready, 1);
        assert_eq!(
            projection.stems[0]
                .projects
                .iter()
                .map(|&index| projection.projects[index].project)
                .collect::<Vec<_>>(),
            ["alpha.render", "alpha.runtime"]
        );
        assert!(projection.stems[0].counts.has_open());
        assert!(
            !WorkCounts {
                terminal: 1,
                ..WorkCounts::default()
            }
            .has_open()
        );
        let public_scope = WorkScope::Stem(projection.stems[0].identity.clone());
        let hidden_scope = WorkScope::Stem(projection.stems[1].identity.clone());
        assert_eq!(
            public_scope.stem().map(ToString::to_string).as_deref(),
            Some("alpha.*")
        );
        assert_eq!(
            hidden_scope.stem().map(ToString::to_string).as_deref(),
            Some(".alpha.*")
        );
        assert!(projection.contains_scope(&public_scope));
        assert!(projection.contains_scope(&hidden_scope));
        assert_eq!(projection.scope_counts(&public_scope).open(), 2);
        assert_eq!(projection.scope_counts(&hidden_scope).open(), 1);

        let absent = WorkScope::Stem(WorkStemIdentity {
            visibility: WorkVisibility::Public,
            stem: "gone".to_owned(),
        });
        assert!(!projection.contains_scope(&absent));
        assert_eq!(projection.scope_counts(&absent), projection.counts());
    }

    #[test]
    fn relationships_claim_dispositions_and_ranking_are_deterministic() {
        let mut complete = task(1, "graph.core");
        complete.workflow.body.completion = Some(
            "2026-09-23T06:00:00.000Z"
                .parse()
                .expect("fixture completion"),
        );
        let mut prerequisite = task(2, "graph.core");
        prerequisite.claim = Some(claim(&prerequisite, ClaimDisposition::Lapsed));
        let missing = stamp(99);
        let mut blocked = task(3, "graph.core");
        blocked.statement.body.dependencies = vec![
            complete.statement.body.born.clone(),
            prerequisite.statement.body.born.clone(),
            missing.clone(),
        ];
        let mut freed = task(4, "graph.core");
        freed.claim = Some(claim(&freed, ClaimDisposition::Freed));

        let board = board(vec![complete, prerequisite, blocked, freed]);
        let projection = WorkProjection::from_board(&board);
        assert_eq!(projection.tasks[1].availability, WorkAvailability::Ready);
        assert_eq!(projection.tasks[2].availability, WorkAvailability::Blocked);
        assert_eq!(projection.tasks[3].availability, WorkAvailability::Ready);
        assert_eq!(
            projection.tasks[2]
                .blocked_dependencies()
                .map(|dependency| (dependency.born, dependency.task))
                .collect::<Vec<_>>(),
            [
                (&projection.tasks[1].row.statement.body.born, Some(1)),
                (&missing, None),
            ]
        );
        assert_eq!(projection.tasks[1].dependents, [WorkDependent { task: 2 }]);

        let lane = &projection.projects[0];
        assert_eq!(lane.project, "graph.core");
        assert_eq!(lane.stem, 0);
        assert_eq!(
            lane.tasks
                .iter()
                .map(|&index| projection.tasks[index].availability)
                .collect::<Vec<_>>(),
            [
                WorkAvailability::Ready,
                WorkAvailability::Ready,
                WorkAvailability::Blocked,
                WorkAvailability::Terminal,
            ]
        );
        assert_eq!(lane.tasks, [1, 3, 2, 0]);
        assert_eq!(projection.tasks[2].project, 0);
        assert_eq!(lane.counts.unavailable(), 1);
    }

    const OPEN_LANES: [WorkAvailability; 4] = [
        WorkAvailability::InFlight,
        WorkAvailability::Ready,
        WorkAvailability::Blocked,
        WorkAvailability::Deferred,
    ];

    fn titles<'a>(clusters: &[WorkCluster<'_, 'a>]) -> Vec<&'a str> {
        clusters
            .iter()
            .flat_map(|cluster| &cluster.tasks)
            .map(|task| task.row().presentation_title.as_str())
            .collect()
    }

    fn arrangement(order: WorkOrder, clustering: WorkClustering) -> WorkArrangement {
        WorkArrangement { order, clustering }
    }

    #[test]
    fn one_board_follows_the_resident_offer_and_clusters_by_first_card() {
        let mut active = task(6, "beta.core");
        active.claim = Some(claim(&active, ClaimDisposition::Active));
        let mut done = task(1, "alpha.render");
        done.workflow.body.completion = Some(
            "2026-09-23T06:00:00.000Z"
                .parse()
                .expect("fixture completion"),
        );
        let mut assigned = task(2, "beta.core");
        assigned.workflow.body.assignment = Some(Actor::Spice);
        // Board rows arrive in neither offer, time, nor project order.
        let mut board = board(vec![
            task(9, "alpha.render"),
            task(7, "beta.core"),
            active,
            task(4, ".alpha.audit"),
            done,
            task(5, "alpha.render"),
            assigned,
        ]);
        // The resident offers no hidden work; one stale stamp is ignored.
        board.offer = [7, 5, 99, 9, 2].map(stamp).to_vec();
        let projection = WorkProjection::from_board(&board);
        let all = WorkScope::AllOpen;
        let ready = |order, clustering| {
            projection.lane(
                WorkAvailability::Ready,
                &all,
                arrangement(order, clustering),
            )
        };

        let offered = ready(WorkOrder::Offer, WorkClustering::None);
        assert!(matches!(offered.as_slice(), [lane] if lane.key == WorkClusterKey::Lane));
        assert_eq!(
            titles(&offered),
            ["task 7", "task 5", "task 9", "task 2", "task 4"],
            "the next offer leads every project; unoffered work follows in queue order"
        );
        assert_eq!(
            titles(&projection.lane(WorkAvailability::Ready, &all, WorkArrangement::default())),
            titles(&offered),
            "the offer order is the default arrangement"
        );
        assert_eq!(
            titles(&ready(WorkOrder::Newest, WorkClustering::None)),
            ["task 9", "task 7", "task 5", "task 4", "task 2"]
        );

        let by_project = ready(WorkOrder::Offer, WorkClustering::Project);
        assert_eq!(
            by_project
                .iter()
                .map(|cluster| cluster.key)
                .collect::<Vec<_>>(),
            [
                WorkClusterKey::Project("beta.core"),
                WorkClusterKey::Project("alpha.render"),
                WorkClusterKey::Project(".alpha.audit"),
            ],
            "clusters follow their first card, so the next offer still leads"
        );
        assert_eq!(
            titles(&by_project),
            ["task 7", "task 2", "task 5", "task 9", "task 4"]
        );
        assert_eq!(
            titles(&ready(WorkOrder::Newest, WorkClustering::Project)),
            ["task 9", "task 5", "task 7", "task 2", "task 4"]
        );
        for order in [WorkOrder::Offer, WorkOrder::Newest] {
            for clustering in [
                WorkClustering::None,
                WorkClustering::Project,
                WorkClustering::Phase,
            ] {
                let mut placed = OPEN_LANES
                    .into_iter()
                    .flat_map(|lane| {
                        titles(&projection.lane(lane, &all, arrangement(order, clustering)))
                    })
                    .collect::<Vec<_>>();
                placed.sort_unstable();
                assert_eq!(
                    placed,
                    ["task 2", "task 4", "task 5", "task 6", "task 7", "task 9"],
                    "{order:?}/{clustering:?} places every open task in exactly one shared lane"
                );
            }
        }
        assert_eq!(
            titles(&projection.lane(WorkAvailability::InFlight, &all, WorkArrangement::default())),
            ["task 6"]
        );

        let public_alpha = WorkScope::Stem(projection.stems[0].identity.clone());
        assert_eq!(
            titles(&projection.lane(
                WorkAvailability::Ready,
                &public_alpha,
                WorkArrangement::default()
            )),
            ["task 5", "task 9"],
            "a stem scope narrows the one board without splitting it"
        );
    }

    #[test]
    fn owners_read_as_agent_names_and_unknown_lanes_stay_distinguishable() {
        let lane = "11111111-1111-4111-8111-111111111111"
            .parse::<CanonicalUuid>()
            .expect("fixture lane");
        let other = "44444444-4444-4444-8444-444444444444"
            .parse::<CanonicalUuid>()
            .expect("fixture lane");
        let mut member = LaneMember {
            lane,
            name: None,
            worktree: "/work/spice-rust-h".to_owned(),
            life: "steer".to_owned(),
            vendor: "omp".parse().expect("fixture vendor"),
            model: None,
            effort: None,
            occupant: None,
            session: None,
            disposition: "seated".to_owned(),
            liveness: wire::intent::LaneLiveness::Working,
            bound: true,
            jobs: Vec::new(),
        };
        let actor = Actor::Lane(lane);
        assert_eq!(owner_label(&actor, Some(&member)), "spice-rust-h");
        member.name = Some("builder".to_owned());
        assert_eq!(owner_label(&actor, Some(&member)), "builder");
        assert_eq!(owner_label(&actor, None), "lane 11111111");
        assert_eq!(
            owner_label(&Actor::Lane(other), Some(&member)),
            "lane 44444444",
            "a member of another lane never lends its name"
        );
        assert_eq!(owner_label(&Actor::Operator, None), "Operator");

        let mut held = task(1, "alpha.core");
        held.workflow.body.assignment = Some(Actor::Spice);
        held.claim = Some(claim(&held, ClaimDisposition::Active));
        let mut lapsed = task(2, "alpha.core");
        lapsed.workflow.body.assignment = Some(Actor::Spice);
        lapsed.claim = Some(claim(&lapsed, ClaimDisposition::Lapsed));
        let board = board(vec![held, lapsed, task(3, "alpha.core")]);
        let projection = WorkProjection::from_board(&board);
        assert_eq!(projection.tasks[0].owner(), Some(&Actor::Operator));
        assert_eq!(projection.tasks[1].owner(), Some(&Actor::Spice));
        assert_eq!(projection.tasks[2].owner(), None);
        assert_eq!(
            projection.tasks[0].working_claimant(),
            Some(&Actor::Operator),
        );
        assert_eq!(projection.tasks[1].working_claimant(), None);
        assert_eq!(projection.tasks[2].working_claimant(), None);
    }
}
