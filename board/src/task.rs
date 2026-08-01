use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use wire::{ClaimHead, LeaseHead, TaskHandle, TaskHead, WorkflowHead};

/// A validated public project path carried by a task statement head.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Project(String);

impl Project {
    /// The project as written in the authoritative statement.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Project {
    type Err = TaskError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let body = text.strip_prefix('.').unwrap_or(text);
        let valid = !body.is_empty()
            && body.split('.').all(|segment| {
                !segment.is_empty()
                    && segment
                        .bytes()
                        .next()
                        .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
                    && segment.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                    })
            });
        if valid {
            Ok(Self(text.to_owned()))
        } else {
            Err(TaskError::Project(text.to_owned()))
        }
    }
}

impl fmt::Display for Project {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The uppercase family carried by a task handle.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Family(String);

impl Family {
    /// Derive the family from the already-validated wire handle.
    #[must_use]
    pub fn of(handle: &TaskHandle) -> Self {
        let family = handle
            .as_str()
            .split_once('-')
            .expect("TaskHandle always contains a separator")
            .0;
        Self(family.to_owned())
    }

    /// The family spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Family {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One dependency edge derived from an authoritative task statement head.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Dependency {
    /// Task that cannot proceed yet.
    pub task: TaskHandle,
    /// Prerequisite task.
    pub after: TaskHandle,
}

/// One task's current journal-head bundle.
///
/// This is a checked view of wire heads, not a mutable board record. Replacing
/// any field requires committing the corresponding fact and head update in the
/// journal first and constructing a new `Entry` from that answer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    statement: TaskHead,
    workflow: WorkflowHead,
    claim: Option<ClaimHead>,
    lease: Option<LeaseHead>,
    project: Project,
    family: Family,
}

impl Entry {
    /// Validate one current task-head bundle.
    pub fn new(
        statement: TaskHead,
        workflow: WorkflowHead,
        claim: Option<ClaimHead>,
        lease: Option<LeaseHead>,
    ) -> Result<Self, TaskError> {
        let project = statement.project.parse()?;
        let family = Family::of(&statement.handle);
        let mut seen = BTreeSet::new();
        for dependency in &statement.dependencies {
            if dependency == &statement.handle {
                return Err(TaskError::SelfDependency(statement.handle.clone()));
            }
            if !seen.insert(dependency) {
                return Err(TaskError::DuplicateDependency {
                    task: statement.handle.clone(),
                    after: dependency.clone(),
                });
            }
        }
        admit_workflow(&workflow)?;

        Ok(Self {
            statement,
            workflow,
            claim,
            lease,
            project,
            family,
        })
    }

    /// Current statement head.
    #[must_use]
    pub const fn statement(&self) -> &TaskHead {
        &self.statement
    }

    /// Current workflow head.
    #[must_use]
    pub const fn workflow(&self) -> &WorkflowHead {
        &self.workflow
    }

    /// Current claim head, when any claim has been recorded.
    #[must_use]
    pub const fn claim(&self) -> Option<&ClaimHead> {
        self.claim.as_ref()
    }

    /// Current lease head, when a claim carries a lease.
    #[must_use]
    pub const fn lease(&self) -> Option<&LeaseHead> {
        self.lease.as_ref()
    }

    /// Validated public project.
    #[must_use]
    pub const fn project(&self) -> &Project {
        &self.project
    }

    /// Family derived from the handle.
    #[must_use]
    pub const fn family(&self) -> &Family {
        &self.family
    }

    /// Dependency edges in statement order.
    pub fn dependencies(&self) -> impl Iterator<Item = Dependency> + '_ {
        self.statement
            .dependencies
            .iter()
            .cloned()
            .map(|after| Dependency {
                task: self.statement.handle.clone(),
                after,
            })
    }
}

/// A task-head bundle that cannot enter board policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskError {
    /// Project spelling is outside the dotted authority vocabulary.
    Project(String),
    /// A task named itself as a prerequisite.
    SelfDependency(TaskHandle),
    /// One prerequisite appeared more than once.
    DuplicateDependency { task: TaskHandle, after: TaskHandle },
    /// No station was declared.
    EmptyFlow,
    /// One station appeared more than once.
    DuplicatePhase(wire::Phase),
    /// Current station is not a member of its declared flow.
    PhaseOutsideFlow(wire::Phase),
    /// Workflow status is outside the release-one vocabulary.
    WorkflowStatus(String),
    /// Completion presence disagrees with workflow status.
    CompletionMismatch { status: String },
}

fn admit_workflow(workflow: &WorkflowHead) -> Result<(), TaskError> {
    if workflow.flow.is_empty() {
        return Err(TaskError::EmptyFlow);
    }
    let mut seen = BTreeSet::new();
    for phase in &workflow.flow {
        if !seen.insert(phase) {
            return Err(TaskError::DuplicatePhase(phase.clone()));
        }
    }
    if !seen.contains(&workflow.phase) {
        return Err(TaskError::PhaseOutsideFlow(workflow.phase.clone()));
    }
    match workflow.status.as_str() {
        "pending" if workflow.completion.is_none() => Ok(()),
        "completed" if workflow.completion.is_some() => Ok(()),
        "pending" | "completed" => Err(TaskError::CompletionMismatch {
            status: workflow.status.clone(),
        }),
        _ => Err(TaskError::WorkflowStatus(workflow.status.clone())),
    }
}

impl fmt::Display for TaskError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Project(project) => write!(
                formatter,
                "project must be lower-case dotted words with an optional reserved prefix: {project:?}"
            ),
            Self::SelfDependency(task) => write!(formatter, "task {task} depends on itself"),
            Self::DuplicateDependency { task, after } => {
                write!(formatter, "task {task} repeats dependency {after}")
            }
            Self::EmptyFlow => formatter.write_str("workflow flow is empty"),
            Self::DuplicatePhase(phase) => write!(formatter, "workflow repeats phase {phase}"),
            Self::PhaseOutsideFlow(phase) => {
                write!(
                    formatter,
                    "current phase {phase} is outside the workflow flow"
                )
            }
            Self::WorkflowStatus(status) => {
                write!(formatter, "workflow status {status:?} is unknown")
            }
            Self::CompletionMismatch { status } => write!(
                formatter,
                "workflow status {status:?} disagrees with completion presence"
            ),
        }
    }
}

impl std::error::Error for TaskError {}
