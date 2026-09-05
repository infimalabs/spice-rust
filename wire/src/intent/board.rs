//! Actor-facing board intent, evidence, and presentation DTOs.

use serde::{Deserialize, Serialize};

use crate::{Actor, TaskHandle};

/// Read the validated current board, optionally restricted to one family.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardRead {
    /// Upper-case task family to retain, when the caller names one.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub family: Option<String>,
}

/// Render one deterministic graph from the validated current board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardGraph {
    /// Closed board graph view name.
    pub view: String,
    /// Historical barrier requested by the caller, when present.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub at: Option<String>,
}

/// Compose one deterministic handout from the validated current board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardHandout {
    /// Declared output spelling selected by the caller.
    pub output: String,
    /// Historical barrier requested by the caller, when present.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub at: Option<String>,
}

/// Emit the canonical authored-document ledger for one task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocLedger {
    /// Task whose current statement becomes canonical Markdown.
    pub task: TaskHandle,
}

/// One authoritative head body together with the fact that established it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Positioned<T> {
    /// Fact position stored beside the current head.
    pub position: crate::Seq,
    /// Typed current head body.
    pub body: T,
}

/// Visibility classification captured when a task project is admitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum ProjectVisibility {
    /// An ordinary scheduler route visible to actors and allocation.
    Public,
    /// A system-owned route retained as evidence but excluded from public work.
    Hidden {
        /// Resolved first project segment, without the leading dot.
        stem: String,
    },
}

/// Whether one exact task transcript coordinate can be rehydrated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TaskTranscriptRecovery {
    /// The registered vendor session and exact source boundary are available.
    Transcript {
        /// Vendor session identity owning the source.
        session: String,
    },
    /// The journal preserves the boundary, but no executable transcript lookup exists.
    Unavailable {
        /// Evidence-specific reason the lookup cannot be rendered.
        reason: String,
    },
}

/// Compact provenance for the claim that opened the current task phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskClaimContext {
    /// ClaimWon fact opening the current attempt.
    pub claimed: crate::Seq,
    /// Lane bound when the claim was won.
    pub lane: crate::CanonicalUuid,
    /// Occupant bound when the claim was won.
    pub occupant: crate::CanonicalUuid,
    /// Resident generation that minted the claim.
    pub epoch: crate::CanonicalUuid,
    /// Complete accounted source boundary recorded on the claim.
    pub source: crate::body::SourceBoundary,
    /// Executable recovery coordinate, or an explicit reason none exists.
    pub recovery: TaskTranscriptRecovery,
}

/// Lifecycle boundary whose Git objects supply the current task's repository context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskGitContextKind {
    /// Repository state observed when the claim opened.
    Claim,
    /// Repository state published when an implementation phase completed.
    Completion,
    /// Repository state published when a review completed.
    Review,
}

impl TaskGitContextKind {
    /// Stable text spelling used by task detail output.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claim => "claim",
            Self::Completion => "completion",
            Self::Review => "review",
        }
    }
}

/// One compact, fact-attributed Git boundary selected for task recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskGitContext {
    /// Fact that recorded this boundary.
    pub fact: crate::Seq,
    /// Lifecycle role of that fact.
    pub kind: TaskGitContextKind,
    /// Exact repository object identities recorded by the lifecycle fact.
    pub boundary: crate::body::GitBoundary,
}

impl ProjectVisibility {
    /// Whether this project is system-owned hidden evidence.
    #[must_use]
    pub const fn is_hidden(&self) -> bool {
        matches!(self, Self::Hidden { .. })
    }

    /// Hidden first segment, without the leading dot.
    #[must_use]
    pub fn hidden_stem(&self) -> Option<&str> {
        match self {
            Self::Public => None,
            Self::Hidden { stem } => Some(stem),
        }
    }
}

/// Actor-facing task statement evidence, excluding authored title prose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardStatement {
    /// Public task name derived from `project` and `born`.
    pub handle: crate::TaskHandle,
    /// Sole durable task identity.
    pub born: crate::TaskStamp,
    /// Fact that first placed this task in the queue.
    pub queue_origin: crate::Seq,
    /// Longer authored context.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub description: Option<String>,
    /// Criteria that decide whether the work is done.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance: Vec<String>,
    /// Scheduler project grouping.
    pub project: String,
    /// Authored task labels.
    pub tags: Vec<String>,
    /// Tasks that must complete first.
    pub dependencies: Vec<crate::TaskStamp>,
    /// Creation provenance, immutable across statement revisions.
    pub provenance: crate::Origin,
    /// Creation-time transcript authority, immutable across statement revisions.
    pub creation: crate::body::TaskCreationContext,
    /// Fact that established this statement revision.
    pub revision: crate::Seq,
}

impl From<&crate::TaskHead> for BoardStatement {
    fn from(statement: &crate::TaskHead) -> Self {
        Self {
            handle: crate::TaskHandle::for_project(&statement.project, &statement.born),
            born: statement.born.clone(),
            queue_origin: statement.queue_origin,
            description: statement.description.clone(),
            acceptance: statement.acceptance.clone(),
            project: statement.project.clone(),
            tags: statement.tags.clone(),
            dependencies: statement.dependencies.clone(),
            provenance: statement.provenance.clone(),
            creation: statement.creation.clone(),
            revision: statement.revision,
        }
    }
}

impl BoardStatement {
    /// Reconstruct a client-side statement using its actor-facing title.
    #[must_use]
    pub fn with_title(&self, title: &str) -> crate::TaskHead {
        crate::TaskHead {
            born: self.born.clone(),
            queue_origin: self.queue_origin,
            title: title.to_owned(),
            description: self.description.clone(),
            acceptance: self.acceptance.clone(),
            project: self.project.clone(),
            tags: self.tags.clone(),
            dependencies: self.dependencies.clone(),
            provenance: self.provenance.clone(),
            creation: self.creation.clone(),
            revision: self.revision,
        }
    }
}

/// One complete current task evidence bundle in a board read answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardTask {
    /// Classification derived during board admission from captured policy.
    pub visibility: ProjectVisibility,
    /// Server-resolved actor thread that created this task.
    pub origin_thread: String,
    /// Actor-facing title derived from that classification.
    pub presentation_title: String,
    /// Current statement evidence projected without authored title prose.
    pub statement: Positioned<BoardStatement>,
    pub workflow: Positioned<crate::WorkflowHead>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub claim: Option<Positioned<crate::ClaimHead>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub lease: Option<Positioned<crate::LeaseHead>>,
    /// Claim-opening transcript coordinates, without embedded transcript content.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub claim_context: Option<TaskClaimContext>,
    /// Latest repository lifecycle boundary, without embedded Git object content.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub git_context: Option<TaskGitContext>,
}

/// Server-resolved public task route metadata for one board snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardProjectPolicy {
    /// Public project roots admitted by the resident, in lexical order.
    pub public_stems: Vec<String>,
    /// Fewest dotted segments admitted for a public project.
    pub public_min_depth: u64,
    /// Most dotted segments admitted for a public project.
    pub public_max_depth: u64,
}

/// Server-resolved predecessor-compatible route for one board principal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardActorRoute {
    /// Actor identity used by predecessor task routing.
    pub actor: String,
    /// Exact actor-private project selected by the route.
    pub project: String,
    /// Exact creation-origin thread selected by the route.
    pub origin_thread: String,
}

impl BoardTask {
    /// Project authoritative heads into one actor-facing task row.
    #[must_use]
    pub fn from_authoritative(
        visibility: ProjectVisibility,
        origin_thread: String,
        statement: Positioned<crate::TaskHead>,
        workflow: Positioned<crate::WorkflowHead>,
        claim: Option<Positioned<crate::ClaimHead>>,
        lease: Option<Positioned<crate::LeaseHead>>,
    ) -> Self {
        let presentation_title = statement.body.title.clone();
        let statement = Positioned {
            position: statement.position,
            body: BoardStatement::from(&statement.body),
        };
        Self {
            visibility,
            origin_thread,
            presentation_title,
            statement,
            workflow,
            claim,
            lease,
            claim_context: None,
            git_context: None,
        }
        .into_actor_facing()
    }

    /// Attach compact recovery references derived from the same authoritative snapshot.
    #[must_use]
    pub fn with_context(
        mut self,
        claim_context: Option<TaskClaimContext>,
        git_context: Option<TaskGitContext>,
    ) -> Self {
        self.claim_context = claim_context;
        self.git_context = git_context;
        self
    }

    /// Normalize this row to the title its actor may retain.
    #[must_use]
    pub fn into_actor_facing(mut self) -> Self {
        if self.visibility.is_hidden() {
            self.presentation_title = "Hidden task".to_owned();
            self.statement.body.description = None;
            self.statement.body.acceptance.clear();
        }
        self
    }

    /// Client-side statement reconstructed from actor-facing evidence.
    #[must_use]
    pub fn client_statement(&self) -> crate::TaskHead {
        self.statement.body.with_title(&self.presentation_title)
    }

    /// Actor-facing title that does not expose hidden system task prose.
    #[must_use]
    pub fn presentation_title(&self) -> &str {
        &self.presentation_title
    }

    /// Whether this row belongs to pending public work inventory.
    #[must_use]
    pub fn is_active_public(&self) -> bool {
        !self.visibility.is_hidden()
            && self.workflow.body.status == "pending"
            && self.workflow.body.completion.is_none()
    }
}

/// Validated current board heads returned from one snapshot barrier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardAnswer {
    /// Journal schema that owns the returned heads.
    pub schema: u32,
    /// Greatest durable fact position covered by every returned head.
    pub cursor: crate::Seq,
    /// Principal derived from the capability that requested this snapshot.
    pub principal: Actor,
    /// Public project route captured by the authority that validated the tasks.
    pub project_policy: BoardProjectPolicy,
    /// Complete actor-specific route captured by the snapshot authority.
    pub actor_route: BoardActorRoute,
    /// Urgency distance the resident captured as one equivalent allocation band.
    pub allocation_band: u64,
    /// Principal's most recently claimed allocator cell, when history has one.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub recent_cell: Option<crate::ClaimCell>,
    /// Complete current task evidence bundles in ascending handle order.
    pub tasks: Vec<BoardTask>,
    /// Current resident and lane head used by possession witnesses.
    pub residency: Positioned<crate::ResidencyHead>,
}

impl BoardAnswer {
    /// Pending public routes only; hidden evidence never enters active inventory.
    pub fn active_public_tasks(&self) -> impl Iterator<Item = &BoardTask> {
        self.tasks.iter().filter(|task| task.is_active_public())
    }
}

/// One deterministic resident-rendered document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextAnswer {
    pub text: String,
}
