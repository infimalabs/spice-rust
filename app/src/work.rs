//! Pure task projection shared by every Canvas Work surface.
//!
//! The admitted board snapshot is the sole input. This module owns lifecycle
//! precedence, stable stem identity, full-project grouping, relationship
//! summaries, counts, and ranking so chrome and Work views cannot drift.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use wire::TaskStamp;
use wire::head::ClaimDisposition;
use wire::intent::{BoardTask, ProjectVisibility};

use crate::runtime::BoardView;

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
    pub const fn has_open(self) -> bool {
        self.in_flight + self.ready + self.blocked + self.deferred > 0
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
}

/// One full dotted project swimlane.
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
    tasks: Vec<WorkTask<'a>>,
    projects: Vec<WorkProject<'a>>,
    stems: Vec<WorkStem>,
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

        let mut dependent_indexes = vec![Vec::new(); board.tasks.len()];
        for (dependent, task) in board.tasks.iter().enumerate() {
            for dependency in &task.statement.body.dependencies {
                if let Some(&index) = by_born.get(dependency) {
                    dependent_indexes[index].push(dependent);
                }
            }
        }
        for indexes in &mut dependent_indexes {
            indexes
                .sort_by(|left, right| compare_tasks(&board.tasks, &availability, *left, *right));
        }

        let mut stem_builders = BTreeMap::<StemKey<'a>, StemBuilder<'a>>::new();
        for (index, task) in board.tasks.iter().enumerate() {
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

        Self {
            tasks,
            projects,
            stems,
        }
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
}
