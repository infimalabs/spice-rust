use std::collections::BTreeSet;
use std::fmt;

use wire::{ClaimHead, LeaseHead, TaskHandle, TaskHead, TaskStamp, WorkflowHead};

/// A validated project label carried by a task statement.
///
/// A project is one exact label: dot-separated nonempty `[0-9a-z_]+` runs,
/// compared whole. Its dots group nothing, so `oops` and `oops.design` are
/// unrelated projects; only the task handle reads the last run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Project(String);

impl Project {
    /// The project a task files under when its author names none.
    pub const DEFAULT: &str = "task";

    /// Admit one project spelling.
    ///
    /// # Errors
    ///
    /// Refuses anything but dot-separated nonempty `[0-9a-z_]+` runs.
    pub fn admit(text: &str) -> Result<Self, ProjectError> {
        if text.split('.').all(valid_run) {
            Ok(Self(text.to_owned()))
        } else {
            Err(ProjectError(text.to_owned()))
        }
    }

    /// The project as written in the authoritative statement.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Project {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Why one project spelling could not enter the typed board.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectError(String);

impl fmt::Display for ProjectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "project must be one lowercase word of [0-9a-z_]: {:?}",
            self.0
        )
    }
}

impl std::error::Error for ProjectError {}

fn valid_run(run: &str) -> bool {
    !run.is_empty()
        && run
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
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
    ) -> Result<Self, TaskError> {
        let project = Project::admit(&statement.project)?;
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

    /// Validated project label.
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
    /// Project spelling is not dot-separated `[0-9a-z_]+` runs.
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
