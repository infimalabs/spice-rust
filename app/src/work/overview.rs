//! Complete task-set selection and graph policy for the contextual overview.
//!
//! Selection follows the admitted projection's order. Graph construction is
//! intentionally unbounded: protocol relationship limits may bound optional
//! enrichment, but never the roots the operator selected from a board snapshot.

use std::collections::{BTreeMap, BTreeSet};

use wire::TaskStamp;

use super::{WorkAvailability, WorkProjection, WorkScope};

/// Why one complete task set was opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskOverviewContext {
    /// One task control opened its exact task.
    Task,
    /// A board scope opened every current task it contains.
    Scope(WorkScope),
    /// One full dotted project opened every current task it contains.
    Project(String),
    /// A relationship control opened a task neighborhood rooted here.
    Relationships(TaskStamp),
}

impl TaskOverviewContext {
    /// The heading for this context; `team` names a team scope and `group`
    /// names a working group from its identity and Designs.
    #[must_use]
    pub fn label(
        &self,
        team: impl Fn(wire::CanonicalUuid) -> String,
        group: impl Fn(&wire::rooms::RoomAuthorityStamp, &[TaskStamp]) -> String,
    ) -> String {
        match self {
            Self::Task => "Selected task".to_owned(),
            Self::Scope(WorkScope::AllOpen) => "All open execution work".to_owned(),
            Self::Scope(WorkScope::Project(project)) => project.clone(),
            Self::Scope(WorkScope::Planning { project: None }) => "All open Design work".to_owned(),
            Self::Scope(WorkScope::Planning {
                project: Some(project),
            }) => format!("Design work · {project}"),
            Self::Scope(WorkScope::Team { team: id }) => format!("Team {}", team(*id)),
            Self::Scope(WorkScope::Group { group: id, designs }) => group(id, designs),
            Self::Project(project) => format!("Project {project}"),
            Self::Relationships(task) => format!("Related to {task}"),
        }
    }
}

/// The base flavor an overview wears: the base of the namespace or console its
/// selection shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskOverviewBase<'s> {
    /// The page's own base.
    Page,
    /// The Design namespace's base, which the Design global rail wears.
    Design,
    /// One team console's base.
    Team(wire::CanonicalUuid),
    /// One working-group console's base.
    Group(&'s wire::rooms::RoomAuthorityStamp),
}

/// One explicit overview selection. The vector is complete, stable, and
/// deduplicated; there is no distinguished current task or hidden sibling set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskOverviewSelection {
    tasks: Vec<TaskStamp>,
    context: TaskOverviewContext,
}

impl TaskOverviewSelection {
    #[must_use]
    pub fn new(tasks: impl IntoIterator<Item = TaskStamp>, context: TaskOverviewContext) -> Self {
        let mut seen = BTreeSet::new();
        let tasks = tasks
            .into_iter()
            .filter(|task| seen.insert(task.clone()))
            .collect();
        Self { tasks, context }
    }

    #[must_use]
    pub fn task(task: TaskStamp) -> Self {
        Self::new([task], TaskOverviewContext::Task)
    }

    /// Extend this exact selection with one relationship lead, preserving the
    /// admitted root order before the new task.
    #[must_use]
    pub fn with_related_task(&self, task: TaskStamp) -> Self {
        Self::new(
            self.tasks.iter().cloned().chain([task.clone()]),
            TaskOverviewContext::Relationships(task),
        )
    }

    #[must_use]
    pub fn tasks(&self) -> &[TaskStamp] {
        &self.tasks
    }

    #[must_use]
    pub const fn context(&self) -> &TaskOverviewContext {
        &self.context
    }

    #[must_use]
    pub fn first(&self) -> Option<&TaskStamp> {
        self.tasks.first()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }
}

/// How one task node relates to the complete selected set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskOverviewNodeKind {
    Selected,
    External,
    Missing,
}

/// One selected task or one direct dependency visible beside the selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskOverviewNode {
    task: TaskStamp,
    task_index: Option<usize>,
    selected: bool,
    kind: TaskOverviewNodeKind,
}

impl TaskOverviewNode {
    #[must_use]
    pub const fn task(&self) -> &TaskStamp {
        &self.task
    }

    #[must_use]
    pub const fn task_index(&self) -> Option<usize> {
        self.task_index
    }

    #[must_use]
    pub const fn selected(&self) -> bool {
        self.selected
    }

    #[must_use]
    pub const fn kind(&self) -> TaskOverviewNodeKind {
        self.kind
    }
}

/// A dependency edge from prerequisite to dependent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskOverviewEdge {
    prerequisite: usize,
    dependent: usize,
    satisfied: bool,
}

impl TaskOverviewEdge {
    #[must_use]
    pub const fn prerequisite(self) -> usize {
        self.prerequisite
    }

    #[must_use]
    pub const fn dependent(self) -> usize {
        self.dependent
    }

    #[must_use]
    pub const fn satisfied(self) -> bool {
        self.satisfied
    }
}

/// Complete selected roots plus every direct dependency, without protocol
/// root/node caps. Missing and out-of-selection neighbors remain explicit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskOverviewGraph {
    nodes: Vec<TaskOverviewNode>,
    edges: Vec<TaskOverviewEdge>,
}

impl TaskOverviewGraph {
    #[must_use]
    pub fn nodes(&self) -> &[TaskOverviewNode] {
        &self.nodes
    }

    #[must_use]
    pub fn edges(&self) -> &[TaskOverviewEdge] {
        &self.edges
    }
}

impl WorkProjection<'_> {
    /// Every open task in `scope`, in the projection's admitted offer order.
    #[must_use]
    pub fn scope_tasks(&self, scope: &WorkScope) -> Vec<TaskStamp> {
        self.offered
            .iter()
            .map(|&index| &self.tasks[index])
            .filter(|task| {
                task.availability != WorkAvailability::Terminal && self.in_scope(task, scope)
            })
            .map(|task| task.row.statement.body.born.clone())
            .collect()
    }

    /// Every open task in one exact project, in admitted offer order.
    #[must_use]
    pub fn project_tasks(&self, project: &str) -> Vec<TaskStamp> {
        self.offered
            .iter()
            .map(|&index| &self.tasks[index])
            .filter(|task| {
                task.availability != WorkAvailability::Terminal
                    && task.row.statement.body.project == project
            })
            .map(|task| task.row.statement.body.born.clone())
            .collect()
    }

    #[must_use]
    pub fn overview_for_scope(&self, scope: WorkScope) -> TaskOverviewSelection {
        TaskOverviewSelection::new(self.scope_tasks(&scope), TaskOverviewContext::Scope(scope))
    }

    #[must_use]
    pub fn overview_for_project(&self, project: String) -> TaskOverviewSelection {
        TaskOverviewSelection::new(
            self.project_tasks(&project),
            TaskOverviewContext::Project(project),
        )
    }

    /// The base `selection` shows. Team and working-group scopes show their
    /// console; the Design scope, or any nonempty selection of only
    /// Design-owned work, shows the Design namespace; anything else shows the
    /// page.
    #[must_use]
    pub fn overview_base<'s>(&self, selection: &'s TaskOverviewSelection) -> TaskOverviewBase<'s> {
        match &selection.context {
            TaskOverviewContext::Scope(WorkScope::Team { team }) => TaskOverviewBase::Team(*team),
            TaskOverviewContext::Scope(WorkScope::Group { group, .. }) => {
                TaskOverviewBase::Group(group)
            }
            TaskOverviewContext::Scope(WorkScope::Planning { .. }) => TaskOverviewBase::Design,
            _ if !selection.is_empty() && self.only_design_owned(&selection.tasks) => {
                TaskOverviewBase::Design
            }
            _ => TaskOverviewBase::Page,
        }
    }

    /// Whether every task in the deduplicated `tasks` is current and
    /// Design-owned.
    fn only_design_owned(&self, tasks: &[TaskStamp]) -> bool {
        let tasks = tasks.iter().collect::<BTreeSet<_>>();
        self.tasks
            .iter()
            .filter(|task| {
                task.belongs_to_design() && tasks.contains(&task.row.statement.body.born)
            })
            .count()
            == tasks.len()
    }

    /// Build the graph that is available synchronously with the board snapshot.
    #[must_use]
    pub fn overview_graph(&self, selection: &TaskOverviewSelection) -> TaskOverviewGraph {
        let by_task = self
            .tasks
            .iter()
            .enumerate()
            .map(|(index, task)| (task.row.statement.body.born.clone(), index))
            .collect::<BTreeMap<_, _>>();
        let selected = selection.tasks.iter().cloned().collect::<BTreeSet<_>>();
        let mut node_by_task = BTreeMap::new();
        let mut nodes = Vec::with_capacity(selection.tasks.len());

        for task in &selection.tasks {
            let task_index = by_task.get(task).copied();
            let index = nodes.len();
            node_by_task.insert(task.clone(), index);
            nodes.push(TaskOverviewNode {
                task: task.clone(),
                task_index,
                selected: true,
                kind: if task_index.is_some() {
                    TaskOverviewNodeKind::Selected
                } else {
                    TaskOverviewNodeKind::Missing
                },
            });
        }

        let mut edges = Vec::new();
        for dependent in &selection.tasks {
            let Some(&dependent_node) = node_by_task.get(dependent) else {
                continue;
            };
            let Some(&task_index) = by_task.get(dependent) else {
                continue;
            };
            for dependency in &self.tasks[task_index].dependencies {
                let prerequisite = neighbor(
                    &mut nodes,
                    &mut node_by_task,
                    &by_task,
                    &selected,
                    dependency.born,
                );
                edges.push(TaskOverviewEdge {
                    prerequisite,
                    dependent: dependent_node,
                    satisfied: dependency.satisfied,
                });
            }
        }

        TaskOverviewGraph { nodes, edges }
    }
}

/// The node for `task`, appending it as an outside neighbor when the graph
/// does not hold it yet.
fn neighbor(
    nodes: &mut Vec<TaskOverviewNode>,
    node_by_task: &mut BTreeMap<TaskStamp, usize>,
    by_task: &BTreeMap<TaskStamp, usize>,
    selected: &BTreeSet<TaskStamp>,
    task: &TaskStamp,
) -> usize {
    if let Some(&index) = node_by_task.get(task) {
        return index;
    }
    let task_index = by_task.get(task).copied();
    let index = nodes.len();
    node_by_task.insert(task.clone(), index);
    nodes.push(TaskOverviewNode {
        task: task.clone(),
        task_index,
        selected: selected.contains(task),
        kind: if task_index.is_some() {
            TaskOverviewNodeKind::External
        } else {
            TaskOverviewNodeKind::Missing
        },
    });
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work::tests::{board, design, stamp, task};

    #[test]
    fn selection_preserves_first_order_and_deduplicates() {
        let first = stamp(1);
        let second = stamp(2);
        let selection = TaskOverviewSelection::new(
            [second.clone(), first.clone(), second],
            TaskOverviewContext::Task,
        );
        assert_eq!(selection.tasks(), &[stamp(2), first]);
    }

    #[test]
    fn single_and_relationship_entries_keep_exact_root_identity() {
        let single = TaskOverviewSelection::task(stamp(1));
        assert_eq!(single.tasks(), &[stamp(1)]);
        assert_eq!(single.context(), &TaskOverviewContext::Task);

        let related = single.with_related_task(stamp(2));
        assert_eq!(related.tasks(), &[stamp(1), stamp(2)]);
        assert_eq!(
            related.context(),
            &TaskOverviewContext::Relationships(stamp(2)),
        );
    }

    #[test]
    fn scope_selection_uses_offer_order_and_excludes_terminal_tasks() {
        let mut settled = task(4, "alpha.done");
        settled.workflow.body.status = wire::WorkflowStatus::completed(
            "2026-09-23T06:00:00.000Z"
                .parse()
                .expect("fixture completion"),
        );
        let mut board = board(vec![
            task(1, "alpha"),
            task(2, "alpha.other"),
            task(3, "alpha"),
            settled,
        ]);
        board.offer = vec![stamp(3), stamp(1), stamp(2)];
        let work = WorkProjection::from_board(&board);
        let scope = WorkScope::Project("alpha".to_owned());
        assert_eq!(work.scope_tasks(&scope), vec![stamp(3), stamp(1)]);
    }

    #[test]
    fn global_scopes_and_project_select_the_complete_admitted_set() {
        let first = task(1, "alpha.one");
        let parent = task(2, "alpha");
        let other = task(3, "beta.other");
        let last = task(4, "alpha.one");
        let mut board = board(vec![first, parent, other, last]);
        board.offer = vec![stamp(3), stamp(2), stamp(4), stamp(1)];
        let work = WorkProjection::from_board(&board);

        assert_eq!(
            work.overview_for_scope(WorkScope::AllOpen).tasks(),
            &[stamp(3), stamp(2), stamp(4), stamp(1)],
        );
        assert_eq!(
            work.overview_for_scope(WorkScope::Project("alpha.one".to_owned()))
                .tasks(),
            &[stamp(4), stamp(1)],
        );
        assert_eq!(
            work.overview_for_scope(WorkScope::Project("alpha".to_owned()))
                .tasks(),
            &[stamp(2)],
        );
        assert_eq!(
            work.overview_for_project("alpha.one".to_owned()).tasks(),
            &[stamp(4), stamp(1)],
        );
    }

    #[test]
    fn graph_preserves_roots_above_typed_limit_and_names_outside_neighbors() {
        let mut rows = (1..=10)
            .map(|value| task(value, "alpha.graph"))
            .collect::<Vec<_>>();
        rows[0].statement.body.dependencies = vec![stamp(20), stamp(99)];
        rows.push(task(20, "outside.graph"));
        let board = board(rows);
        let work = WorkProjection::from_board(&board);
        let selection = TaskOverviewSelection::new(
            (1..=10).map(stamp),
            TaskOverviewContext::Scope(WorkScope::AllOpen),
        );

        let graph = work.overview_graph(&selection);
        assert_eq!(
            graph
                .nodes()
                .iter()
                .filter(|node| node.selected())
                .map(TaskOverviewNode::task)
                .cloned()
                .collect::<Vec<_>>(),
            (1..=10).map(stamp).collect::<Vec<_>>(),
        );
        assert_eq!(graph.nodes().len(), 12);
        assert_eq!(graph.edges().len(), 2);
        assert_eq!(graph.nodes()[10].kind(), TaskOverviewNodeKind::External);
        assert_eq!(graph.nodes()[11].kind(), TaskOverviewNodeKind::Missing);
    }

    #[test]
    fn an_overview_shows_the_design_base_only_for_design_scope_or_wholly_design_owned_work() {
        let board = board(vec![
            design(1, "alpha.design"),
            design(2, "alpha.design"),
            task(3, "alpha.build"),
        ]);
        let work = WorkProjection::from_board(&board);
        let selection = |tasks: &[i64], context| {
            TaskOverviewSelection::new(tasks.iter().copied().map(stamp), context)
        };
        let design_scope = TaskOverviewContext::Scope(WorkScope::Planning { project: None });

        assert_eq!(
            work.overview_base(&selection(&[1, 2], TaskOverviewContext::Task)),
            TaskOverviewBase::Design
        );
        assert_eq!(
            work.overview_base(&selection(&[1, 3], TaskOverviewContext::Task)),
            TaskOverviewBase::Page,
            "execution work beside a Design keeps the page base"
        );
        assert_eq!(
            work.overview_base(&selection(&[1, 99], TaskOverviewContext::Task)),
            TaskOverviewBase::Page,
            "a task the board does not carry is not known to be Design-owned"
        );
        assert_eq!(
            work.overview_base(&selection(&[], TaskOverviewContext::Task)),
            TaskOverviewBase::Page
        );
        assert_eq!(
            work.overview_base(&selection(&[], design_scope)),
            TaskOverviewBase::Design,
            "the Design scope shows its namespace even while empty"
        );
        let team: wire::CanonicalUuid = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
            .parse()
            .expect("fixture team");
        let team_scope = TaskOverviewContext::Scope(WorkScope::Team { team });
        assert_eq!(
            work.overview_base(&selection(&[1, 2], team_scope)),
            TaskOverviewBase::Team(team),
            "a team scope shows its console even over only Design work"
        );
    }
}
