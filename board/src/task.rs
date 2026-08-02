use std::collections::BTreeSet;
use std::fmt;

use wire::TaskCreationAuthority;
use wire::intent::ProjectVisibility;
use wire::{ClaimHead, LeaseHead, TaskHandle, TaskHead, WorkflowHead};

/// Public scheduler roots compiled into every project policy.
pub const BUILTIN_PUBLIC_STEMS: [&str; 2] = ["task", "serve"];

/// Hidden system roots compiled into every project policy, in declaration order.
pub const BUILTIN_HIDDEN_STEMS: [&str; 2] = ["oops", "maxim_proposal"];

/// One captured project vocabulary shared by admission and allocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectPolicy {
    public_stems: BTreeSet<String>,
    hidden_stems: Vec<String>,
    hidden_lookup: BTreeSet<String>,
    public_min_depth: u64,
    public_max_depth: u64,
}

impl ProjectPolicy {
    /// Resolve compiled roots with tracked additions and public depth bounds.
    pub fn new(
        public_additions: impl IntoIterator<Item = String>,
        hidden_additions: impl IntoIterator<Item = String>,
        public_min_depth: u64,
        public_max_depth: u64,
    ) -> Result<Self, ProjectPolicyError> {
        if public_min_depth == 0 || public_min_depth > public_max_depth {
            return Err(ProjectPolicyError::Depth {
                minimum: public_min_depth,
                maximum: public_max_depth,
            });
        }
        let public_stems = BUILTIN_PUBLIC_STEMS
            .into_iter()
            .map(str::to_owned)
            .chain(public_additions)
            .collect::<BTreeSet<_>>();
        let additions = hidden_additions.into_iter().collect::<Vec<_>>();
        let mut hidden_stems = Vec::new();
        let mut hidden_lookup = BTreeSet::new();
        for stem in BUILTIN_HIDDEN_STEMS
            .into_iter()
            .map(str::to_owned)
            .chain(additions.iter().cloned())
        {
            if !valid_hidden_segment(&stem) {
                let index = additions.iter().position(|candidate| candidate == &stem);
                return Err(ProjectPolicyError::HiddenStem { index, stem });
            }
            if public_stems.contains(&stem) {
                let index = additions.iter().position(|candidate| candidate == &stem);
                return Err(ProjectPolicyError::HiddenPublicCollision { index, stem });
            }
            if hidden_lookup.insert(stem.clone()) {
                hidden_stems.push(stem);
            }
        }
        Ok(Self {
            public_stems,
            hidden_stems,
            hidden_lookup,
            public_min_depth,
            public_max_depth,
        })
    }

    /// Admit and classify one authoritative project spelling.
    pub fn admit(&self, text: &str) -> Result<Project, ProjectError> {
        if let Some(body) = text.strip_prefix('.') {
            let segments = hidden_segments(body)
                .ok_or_else(|| ProjectError::HiddenSpelling(text.to_owned()))?;
            let stem = segments[0];
            if !self.hidden_lookup.contains(stem) {
                return Err(ProjectError::UnknownHiddenStem(stem.to_owned()));
            }
            Ok(Project {
                text: text.to_owned(),
                visibility: ProjectVisibility::Hidden {
                    stem: stem.to_owned(),
                },
            })
        } else {
            let segments = public_segments(text)
                .ok_or_else(|| ProjectError::PublicSpelling(text.to_owned()))?;
            let depth = segments.len() as u64;
            if !(self.public_min_depth..=self.public_max_depth).contains(&depth) {
                return Err(ProjectError::PublicDepth {
                    project: text.to_owned(),
                    depth,
                    minimum: self.public_min_depth,
                    maximum: self.public_max_depth,
                });
            }
            let stem = segments[0];
            if !self.public_stems.contains(stem) {
                return Err(ProjectError::UnknownPublicStem(stem.to_owned()));
            }
            Ok(Project {
                text: text.to_owned(),
                visibility: ProjectVisibility::Public,
            })
        }
    }

    /// Admit one project under the authority selected at the task-creation
    /// command boundary.
    pub fn admit_creation(
        &self,
        text: &str,
        authority: TaskCreationAuthority,
    ) -> Result<Project, ProjectError> {
        let project = self.admit(text)?;
        if authority == TaskCreationAuthority::Manual
            && let ProjectVisibility::Hidden { stem } = project.visibility()
        {
            return Err(ProjectError::SystemReserved(stem.clone()));
        }
        Ok(project)
    }

    /// Fully resolved hidden roots in allocator classification order.
    #[must_use]
    pub fn hidden_stems(&self) -> &[String] {
        &self.hidden_stems
    }
}

/// Why a captured project vocabulary could not be constructed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectPolicyError {
    Depth { minimum: u64, maximum: u64 },
    HiddenStem { index: Option<usize>, stem: String },
    HiddenPublicCollision { index: Option<usize>, stem: String },
}

impl fmt::Display for ProjectPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Depth { minimum, maximum } => write!(
                formatter,
                "public project depth must satisfy 0 < minimum <= maximum: {minimum}, {maximum}"
            ),
            Self::HiddenStem { index, stem } => write!(
                formatter,
                "hidden stem{} must match [0-9a-z_]+ without a leading dot: {stem:?}",
                index.map_or_else(String::new, |index| format!(" at index {index}"))
            ),
            Self::HiddenPublicCollision { index, stem } => write!(
                formatter,
                "hidden stem{} conflicts with public stem {stem:?}",
                index.map_or_else(String::new, |index| format!(" at index {index}"))
            ),
        }
    }
}

impl std::error::Error for ProjectPolicyError {}

/// A validated and once-classified project path carried by a task statement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Project {
    text: String,
    visibility: ProjectVisibility,
}

impl Project {
    /// The project as written in the authoritative statement.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Captured visibility classification shared by every consumer.
    #[must_use]
    pub const fn visibility(&self) -> &ProjectVisibility {
        &self.visibility
    }

    /// Verify and retain a classification that crossed the typed board answer.
    fn from_answer(text: &str, visibility: ProjectVisibility) -> Result<Self, ProjectError> {
        match &visibility {
            ProjectVisibility::Public => {
                public_segments(text)
                    .ok_or_else(|| ProjectError::PublicSpelling(text.to_owned()))?;
            }
            ProjectVisibility::Hidden { stem } => {
                let segments = text
                    .strip_prefix('.')
                    .and_then(hidden_segments)
                    .ok_or_else(|| ProjectError::HiddenSpelling(text.to_owned()))?;
                if segments[0] != stem {
                    return Err(ProjectError::VisibilityMismatch {
                        project: text.to_owned(),
                        classified_stem: stem.clone(),
                    });
                }
            }
        }
        Ok(Self {
            text: text.to_owned(),
            visibility,
        })
    }
}

impl fmt::Display for Project {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.text)
    }
}

/// Why one project spelling could not enter the typed board.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectError {
    PublicSpelling(String),
    PublicDepth {
        project: String,
        depth: u64,
        minimum: u64,
        maximum: u64,
    },
    UnknownPublicStem(String),
    HiddenSpelling(String),
    UnknownHiddenStem(String),
    /// Hidden roots are addressable only by typed system creation routes.
    SystemReserved(String),
    VisibilityMismatch {
        project: String,
        classified_stem: String,
    },
}

impl fmt::Display for ProjectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PublicSpelling(project) => write!(
                formatter,
                "public project segments must match [0-9a-z_]+: {project:?}"
            ),
            Self::PublicDepth {
                project,
                depth,
                minimum,
                maximum,
            } => write!(
                formatter,
                "public project {project:?} has depth {depth}, outside {minimum}..={maximum}"
            ),
            Self::UnknownPublicStem(stem) => {
                write!(formatter, "public project stem {stem:?} is not resolved")
            }
            Self::HiddenSpelling(project) => write!(
                formatter,
                "hidden project must have a leading dot and [0-9a-z_]+ segments: {project:?}"
            ),
            Self::UnknownHiddenStem(stem) => {
                write!(formatter, "hidden project stem {stem:?} is not resolved")
            }
            Self::SystemReserved(stem) => write!(
                formatter,
                "hidden project stem {stem:?} is reserved for system task creation"
            ),
            Self::VisibilityMismatch {
                project,
                classified_stem,
            } => write!(
                formatter,
                "hidden project {project:?} does not match classified stem {classified_stem:?}"
            ),
        }
    }
}

impl std::error::Error for ProjectError {}

fn public_segments(project: &str) -> Option<Vec<&str>> {
    let segments = project.split('.').collect::<Vec<_>>();
    segments
        .iter()
        .all(|segment| valid_public_segment(segment))
        .then_some(segments)
}

fn hidden_segments(project: &str) -> Option<Vec<&str>> {
    let segments = project.split('.').collect::<Vec<_>>();
    segments
        .iter()
        .all(|segment| valid_hidden_segment(segment))
        .then_some(segments)
}

fn valid_public_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn valid_hidden_segment(segment: &str) -> bool {
    valid_public_segment(segment)
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
        projects: &ProjectPolicy,
    ) -> Result<Self, TaskError> {
        let project = projects.admit(&statement.project)?;
        Self::finish(statement, workflow, claim, lease, project)
    }

    /// Project typed read-answer heads without deriving visibility again.
    pub fn from_answer(
        statement: TaskHead,
        workflow: WorkflowHead,
        claim: Option<ClaimHead>,
        lease: Option<LeaseHead>,
        visibility: ProjectVisibility,
    ) -> Result<Self, TaskError> {
        let project = Project::from_answer(&statement.project, visibility)?;
        Self::finish(statement, workflow, claim, lease, project)
    }

    fn finish(
        statement: TaskHead,
        workflow: WorkflowHead,
        claim: Option<ClaimHead>,
        lease: Option<LeaseHead>,
        project: Project,
    ) -> Result<Self, TaskError> {
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

    /// Validated and classified project.
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
    Project(ProjectError),
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
            Self::Project(project) => project.fmt(formatter),
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

impl From<ProjectError> for TaskError {
    fn from(error: ProjectError) -> Self {
        Self::Project(error)
    }
}
