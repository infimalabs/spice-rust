use std::collections::BTreeSet;
use std::fmt;

use wire::intent::ProjectVisibility;
use wire::{ClaimHead, LeaseHead, TaskHandle, TaskHead, TaskStamp, WorkflowHead};

/// Hidden system roots compiled into every project policy, in declaration order.
pub const BUILTIN_HIDDEN_STEMS: [&str; 1] = ["maxim_proposal"];

/// One non-serializable, server-owned hidden task creation route.
///
/// Each constructor fixes the hidden stem. Callers may supply only a
/// route-owned descendant, never a free-form project that could cross-target
/// another hidden creator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemTaskRoute {
    owner: SystemTaskOwner,
    descendants: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SystemTaskOwner {
    Proposal,
    Configured(String),
}

impl SystemTaskOwner {
    fn stem(&self) -> &str {
        match self {
            Self::Proposal => "maxim_proposal",
            Self::Configured(stem) => stem,
        }
    }
}

impl SystemTaskRoute {
    /// One server-owned proposal route and its derived descendant.
    #[must_use]
    pub fn proposal(descendants: impl IntoIterator<Item = String>) -> Self {
        Self {
            owner: SystemTaskOwner::Proposal,
            descendants: descendants.into_iter().collect(),
        }
    }

    /// One configured server-owned route and its derived descendant.
    #[must_use]
    pub fn configured(
        stem: impl Into<String>,
        descendants: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            owner: SystemTaskOwner::Configured(stem.into()),
            descendants: descendants.into_iter().collect(),
        }
    }

    fn project(&self) -> String {
        let mut project = format!(".{}", self.owner.stem());
        for descendant in &self.descendants {
            project.push('.');
            project.push_str(descendant);
        }
        project
    }
}

/// One captured project vocabulary shared by admission and allocation.
///
/// A public project needs only its spelling and dotted depth: any valid root
/// segment is admissible and readable. Hidden roots stay closed to the
/// resolved hidden vocabulary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectPolicy {
    hidden_stems: Vec<String>,
    hidden_lookup: BTreeSet<String>,
    public_min_depth: u64,
    public_max_depth: u64,
}

impl ProjectPolicy {
    /// Resolve compiled hidden roots with tracked additions and public depth
    /// bounds.
    pub fn new(
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
            if hidden_lookup.insert(stem.clone()) {
                hidden_stems.push(stem);
            }
        }
        Ok(Self {
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
            Ok(Project {
                text: text.to_owned(),
                visibility: ProjectVisibility::Public,
            })
        }
    }

    /// Admit one caller-authored project for manual task creation.
    pub fn admit_manual_creation(&self, text: &str) -> Result<Project, ProjectError> {
        let project = self.admit(text)?;
        if let ProjectVisibility::Hidden { stem } = project.visibility() {
            return Err(ProjectError::SystemReserved(stem.clone()));
        }
        Ok(project)
    }

    /// Derive and admit the exact hidden project owned by a server route.
    pub fn admit_system_creation(&self, route: &SystemTaskRoute) -> Result<Project, ProjectError> {
        if let SystemTaskOwner::Configured(stem) = &route.owner
            && BUILTIN_HIDDEN_STEMS.contains(&stem.as_str())
        {
            return Err(ProjectError::SystemRouteCollision(stem.clone()));
        }
        self.admit(&route.project())
    }

    /// Fully resolved hidden roots in allocator classification order.
    #[must_use]
    pub fn hidden_stems(&self) -> &[String] {
        &self.hidden_stems
    }

    /// Fewest dotted segments admitted for a public project.
    #[must_use]
    pub const fn public_min_depth(&self) -> u64 {
        self.public_min_depth
    }

    /// Most dotted segments admitted for a public project.
    #[must_use]
    pub const fn public_max_depth(&self) -> u64 {
        self.public_max_depth
    }
}

/// Why a captured project vocabulary could not be constructed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectPolicyError {
    Depth { minimum: u64, maximum: u64 },
    HiddenStem { index: Option<usize>, stem: String },
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
    HiddenSpelling(String),
    UnknownHiddenStem(String),
    /// Hidden roots are addressable only by typed system creation routes.
    SystemReserved(String),
    /// A configured creator cannot impersonate a compiled route owner.
    SystemRouteCollision(String),
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
            Self::SystemRouteCollision(stem) => write!(
                formatter,
                "configured system task route conflicts with compiled owner {stem:?}"
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
    pub task: TaskStamp,
    /// Prerequisite task.
    pub after: TaskStamp,
}

/// Ownership preference carried by a phase transition or a Design nudge while
/// its handoff window is open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhaseHandoffKind {
    /// The named lane should take the row: the recipient of a Design nudge.
    /// A completion that continues its own task carries its claim instead and
    /// opens no window.
    Continue,
    /// Another eligible lane should take the next phase when one is available.
    Independent,
}

/// Durable phase-transition evidence projected into allocator input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhaseHandoff {
    kind: PhaseHandoffKind,
    prior: wire::Actor,
    until: wire::Instant,
}

impl PhaseHandoff {
    /// Construct one handoff from the completed phase's durable boundary.
    #[must_use]
    pub const fn new(kind: PhaseHandoffKind, prior: wire::Actor, until: wire::Instant) -> Self {
        Self { kind, prior, until }
    }

    /// Ownership policy for the new phase.
    #[must_use]
    pub const fn kind(&self) -> PhaseHandoffKind {
        self.kind
    }

    /// Claimant that completed the prior phase.
    #[must_use]
    pub const fn prior(&self) -> &wire::Actor {
        &self.prior
    }

    /// End of the exclusive handoff opportunity.
    #[must_use]
    pub const fn until(&self) -> wire::Instant {
        self.until
    }
}

/// One task's current journal-head bundle.
///
/// This is a checked view of wire heads, not a mutable board record. Replacing
/// any field requires committing the corresponding fact and head update in the
/// journal first and constructing a new `Entry` from that answer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    statement: TaskHead,
    handle: TaskHandle,
    workflow: WorkflowHead,
    claim: Option<ClaimHead>,
    lease: Option<LeaseHead>,
    creator_team: Option<wire::CanonicalUuid>,
    explicit_release: bool,
    grouped: bool,
    design: wire::rooms::DesignFacts,
    preparation_decision: Option<wire::rooms::DecisionRef>,
    phase_handoff: Option<PhaseHandoff>,
    queued_at: Option<wire::Instant>,
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
        let handle = TaskHandle::for_filing(&statement.project, &statement.born, statement.filing);
        let family = Family::of(&handle);
        let mut seen = BTreeSet::new();
        for dependency in &statement.dependencies {
            if dependency == &statement.born {
                return Err(TaskError::SelfDependency(statement.born.clone()));
            }
            if !seen.insert(dependency) {
                return Err(TaskError::DuplicateDependency {
                    task: statement.born.clone(),
                    after: dependency.clone(),
                });
            }
        }
        admit_workflow(&workflow)?;

        Ok(Self {
            statement,
            handle,
            workflow,
            claim,
            lease,
            creator_team: None,
            explicit_release: false,
            grouped: false,
            design: wire::rooms::DesignFacts::default(),
            preparation_decision: None,
            phase_handoff: None,
            queued_at: None,
            project,
            family,
        })
    }

    /// Current statement head.
    #[must_use]
    pub const fn statement(&self) -> &TaskHead {
        &self.statement
    }

    /// Whether this row is an unfiled draft.
    #[must_use]
    pub const fn is_draft(&self) -> bool {
        self.statement.filing.is_draft()
    }

    /// Whether this row is ordinary filed work.
    #[must_use]
    pub const fn is_filed(&self) -> bool {
        self.statement.filing.is_filed()
    }

    /// Current public handle derived from project and birth stamp.
    #[must_use]
    pub const fn handle(&self) -> &TaskHandle {
        &self.handle
    }

    /// Sole durable task identity.
    #[must_use]
    pub const fn born(&self) -> &TaskStamp {
        &self.statement.born
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

    /// Immutable creator team captured by the task-added fact.
    #[must_use]
    pub const fn creator_team(&self) -> Option<wire::CanonicalUuid> {
        self.creator_team
    }

    /// Attach creator-team projection derived from the immutable creation fact.
    #[must_use]
    pub fn with_creator_team(mut self, creator_team: Option<wire::CanonicalUuid>) -> Self {
        self.creator_team = creator_team;
        self
    }

    /// Attach whether the current freed claim came from an explicit release command.
    #[must_use]
    pub const fn with_explicit_release(mut self, explicit_release: bool) -> Self {
        self.explicit_release = explicit_release;
        self
    }

    /// Attach phase-transition ownership evidence derived from journal facts.
    #[must_use]
    pub fn with_phase_handoff(mut self, phase_handoff: Option<PhaseHandoff>) -> Self {
        self.phase_handoff = phase_handoff;
        self
    }

    /// Current phase-transition ownership evidence, when this phase was reached
    /// from a completed claim.
    #[must_use]
    pub const fn phase_handoff(&self) -> Option<&PhaseHandoff> {
        self.phase_handoff.as_ref()
    }

    /// Attach the commit instant of the fact that first queued this task.
    #[must_use]
    pub const fn with_queued_at(mut self, queued_at: Option<wire::Instant>) -> Self {
        self.queued_at = queued_at;
        self
    }

    /// The commit instant of the queue-origin fact, when the caller supplied
    /// it; a row without one is never overdue.
    #[must_use]
    pub const fn queued_at(&self) -> Option<wire::Instant> {
        self.queued_at
    }

    /// Whether the current freed claim came from an explicit release command.
    #[must_use]
    pub const fn explicit_release(&self) -> bool {
        self.explicit_release
    }

    /// Attach whether the current working-group partition holds this task.
    #[must_use]
    pub const fn with_grouped(mut self, grouped: bool) -> Self {
        self.grouped = grouped;
        self
    }

    /// Whether the current working-group partition holds this task. Only a
    /// Design can stand in a group; a Design outside every group is passive.
    #[must_use]
    pub const fn grouped(&self) -> bool {
        self.grouped
    }

    /// Attach this Design's working-group standing and pass-cycle evidence.
    #[must_use]
    pub const fn with_design(mut self, design: wire::rooms::DesignFacts) -> Self {
        self.design = design;
        self
    }

    /// Whether this Design is active under its working-group stance.
    #[must_use]
    pub const fn design_is_active(&self) -> bool {
        self.design.active
    }

    /// This Design's working-group standing and pass-cycle evidence.
    #[must_use]
    pub const fn design(&self) -> wire::rooms::DesignFacts {
        self.design
    }

    /// Attach the decision classifying this output for selection waiting.
    #[must_use]
    pub fn with_preparation_decision(mut self, decision: Option<wire::rooms::DecisionRef>) -> Self {
        self.preparation_decision = decision;
        self
    }

    /// Decision on the latest unwithdrawn candidate version naming this
    /// output; `None` while that version is open or none ruled on it.
    #[must_use]
    pub const fn preparation_decision(&self) -> Option<&wire::rooms::DecisionRef> {
        self.preparation_decision.as_ref()
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
                task: self.statement.born.clone(),
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
    SelfDependency(TaskStamp),
    /// One prerequisite appeared more than once.
    DuplicateDependency { task: TaskStamp, after: TaskStamp },
    /// No station was declared.
    EmptyFlow,
    /// One station appeared more than once.
    DuplicatePhase(wire::Phase),
    /// Current station is not a member of its declared flow.
    PhaseOutsideFlow(wire::Phase),
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
    Ok(())
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
        }
    }
}

impl std::error::Error for TaskError {}

impl From<ProjectError> for TaskError {
    fn from(error: ProjectError) -> Self {
        Self::Project(error)
    }
}
