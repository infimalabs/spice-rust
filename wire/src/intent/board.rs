//! Actor-facing board intent, evidence, and presentation DTOs.

use serde::{Deserialize, Serialize};

use crate::{Actor, TaskHandle};

/// Largest terminal-task page one board read accepts.
pub const TASK_HISTORY_PAGE_LIMIT_MAX: u16 = 100;
/// Largest UTF-8 search query one terminal-task history read accepts.
pub const TASK_HISTORY_QUERY_BYTES_MAX: usize = 256;

/// Stable keyset cursor for terminal tasks in newest-workflow-first order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskHistoryCursor {
    /// Durable workflow head ordering this terminal task.
    pub workflow: crate::Seq,
    /// Durable task identity breaking equal-head ties.
    pub task: crate::TaskStamp,
}

/// Metadata for one bounded terminal-task page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskHistoryPage {
    /// Cursor supplied by the request, absent for the first page.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub before: Option<TaskHistoryCursor>,
    /// Accepted maximum number of rows in this page.
    pub limit: u16,
    /// Total terminal rows at this answer's snapshot barrier.
    pub total: u64,
    /// Cursor for the next page, absent once this snapshot is exhausted.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub next: Option<TaskHistoryCursor>,
}

/// Read an explicit projection of one authoritative board snapshot.
///
/// `All` remains complete; `Open` and `History` name their exclusions explicitly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case", deny_unknown_fields)]
pub enum BoardRead {
    /// The complete board, optionally restricted to a public handle family.
    All {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::optional::present"
        )]
        family: Option<String>,
    },
    /// Every currently nonterminal task, without terminal history.
    Open,
    /// One bounded keyset page of terminal tasks.
    History {
        /// Last row returned by the preceding page.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::optional::present"
        )]
        before: Option<TaskHistoryCursor>,
        /// Case-insensitive substring matched against handle, title, project,
        /// and description. Empty or whitespace-only text is unfiltered.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::optional::present"
        )]
        query: Option<String>,
        /// Include only tasks completed at or before this instant.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::optional::present"
        )]
        completed_before: Option<crate::Instant>,
        /// Maximum rows returned by this page.
        limit: u16,
    },
    /// Named durable identities plus the context required by the caller.
    Tasks {
        tasks: Vec<crate::TaskStamp>,
        /// Include the transitive dependency closure of the selected tasks.
        dependencies: bool,
        /// Also select every active claim held by the authenticated principal.
        claims: bool,
        /// Also return the durable notes on each named task in `tasks`.
        notes: bool,
    },
    /// Trust rulings and residency authority, without task rows.
    Trust,
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
/// Read one task's current evidence and durable note history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDetail {
    /// Sole durable task identity.
    pub task: crate::TaskStamp,
}

/// One attributed durable note in journal order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDetailNote {
    /// Fact position; this is the note's durable identity and ordering key.
    pub position: crate::Seq,
    /// Commit time recorded by the journal.
    pub at: crate::Instant,
    /// Principal that appended the note.
    pub author: Actor,
    /// Authored note text.
    pub text: String,
}

/// Current evidence and complete durable notes for one selected task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDetailAnswer {
    /// Current task heads projected at the reply's snapshot barrier.
    pub task: BoardTask,
    /// Bounded browser-renderable artifacts in task citation order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub media: Vec<crate::Media>,
    /// Notes ordered by ascending durable journal position.
    pub notes: Vec<TaskDetailNote>,
}

/// Most roots one relationship read admits.
pub const TASK_RELATION_ROOT_LIMIT: usize = 8;
/// Deepest bounded relationship expansion one request admits.
pub const TASK_RELATION_DEPTH_LIMIT: u8 = 4;
/// Most typed nodes one relationship answer admits.
pub const TASK_RELATION_NODE_LIMIT: u16 = 128;
/// Most typed edges one relationship answer emits.
pub const TASK_RELATION_EDGE_LIMIT: usize = 512;
/// Most omitted edges named explicitly at a truncation frontier.
pub const TASK_RELATION_FRONTIER_LIMIT: usize = 64;

/// One bounded on-demand relationship projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRelations {
    /// Durable task identities at the center of the projection.
    pub roots: Vec<crate::TaskStamp>,
    /// Explicit relation families to project.
    pub relations: Vec<TaskRelationKind>,
    /// Maximum edge distance from any root.
    pub depth: u8,
    /// Maximum number of returned nodes, including roots.
    pub node_limit: u16,
}

/// Closed relation families available to a task relationship read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskRelationKind {
    /// Immutable creation provenance, including task-parent origins.
    Origin,
    /// Current task dependency edges in both traversal directions.
    Dependency,
    /// Completed reviewer handoffs joined to their phase authors.
    Review,
    /// Durable task lifecycle facts.
    Lifecycle,
    /// Current active claim ownership.
    Claim,
}

/// Stable identity of one relationship node.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TaskRelationNodeRef {
    /// One durable task.
    Task { task: crate::TaskStamp },
    /// One non-task causal origin.
    Origin { origin: crate::Origin },
    /// One durable completed review.
    Review {
        task: crate::TaskStamp,
        position: crate::Seq,
    },
    /// One durable lifecycle fact.
    Lifecycle {
        task: crate::TaskStamp,
        position: crate::Seq,
    },
    /// One current active claim head.
    Claim {
        task: crate::TaskStamp,
        position: crate::Seq,
    },
}

/// Closed lifecycle events exposed by relationship projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskLifecycleKind {
    Added,
    Edited,
    Phased,
    Completed,
    Assigned,
    Reviewed,
    Deferred,
    Woken,
    Deleted,
}

/// One typed node in a bounded task relationship answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TaskRelationNode {
    Task {
        task: crate::TaskStamp,
        handle: crate::TaskHandle,
        title: String,
        phase: crate::Phase,
        status: String,
    },
    Origin {
        origin: crate::Origin,
    },
    Review {
        task: crate::TaskStamp,
        position: crate::Seq,
        phase: crate::Phase,
        author: Actor,
        reviewer: Actor,
        verdict: crate::body::Verdict,
    },
    Lifecycle {
        task: crate::TaskStamp,
        position: crate::Seq,
        at: crate::Instant,
        actor: Actor,
        event: TaskLifecycleKind,
    },
    Claim {
        task: crate::TaskStamp,
        position: crate::Seq,
        actor: Actor,
        lane: crate::CanonicalUuid,
        occupant: crate::CanonicalUuid,
        disposition: crate::ClaimDisposition,
    },
}

impl TaskRelationNode {
    /// Stable identity used by edges and truncation metadata.
    #[must_use]
    pub fn identity(&self) -> TaskRelationNodeRef {
        match self {
            Self::Task { task, .. } => TaskRelationNodeRef::Task { task: task.clone() },
            Self::Origin { origin } => TaskRelationNodeRef::Origin {
                origin: origin.clone(),
            },
            Self::Review { task, position, .. } => TaskRelationNodeRef::Review {
                task: task.clone(),
                position: *position,
            },
            Self::Lifecycle { task, position, .. } => TaskRelationNodeRef::Lifecycle {
                task: task.clone(),
                position: *position,
            },
            Self::Claim { task, position, .. } => TaskRelationNodeRef::Claim {
                task: task.clone(),
                position: *position,
            },
        }
    }
}

/// One stable typed relationship edge.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRelationEdge {
    pub relation: TaskRelationKind,
    pub from: TaskRelationNodeRef,
    pub to: TaskRelationNodeRef,
}

/// One omitted edge at the bounded expansion frontier.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRelationFrontier {
    /// Depth at which the omitted target would have appeared.
    pub depth: u8,
    /// Exact omitted relationship.
    pub edge: TaskRelationEdge,
}

/// Explicit metadata for a depth, node, edge, or frontier bound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRelationTruncation {
    /// First omitted edges in stable order.
    pub frontier: Vec<TaskRelationFrontier>,
    /// Additional omitted edges beyond the bounded frontier list.
    pub omitted: u32,
}

/// One bounded typed relationship projection at a journal snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRelationsAnswer {
    pub snapshot: crate::Seq,
    pub roots: Vec<crate::TaskStamp>,
    pub nodes: Vec<TaskRelationNode>,
    pub edges: Vec<TaskRelationEdge>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub truncation: Option<TaskRelationTruncation>,
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

/// Current task statement evidence, with the title carried once beside it.
///
/// Environmental requirements remain on the authoritative [`crate::TaskHead`]
/// until the requirement-aware board surface projects their observed standing;
/// this pre-existing actor view must not expose a partial requirements model.
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
    /// Typed environmental facts required before allocation.
    #[serde(
        default,
        skip_serializing_if = "crate::EnvironmentRequirements::is_empty",
        deserialize_with = "crate::EnvironmentRequirements::present"
    )]
    pub requirements: crate::EnvironmentRequirements,
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
            requirements: statement.requirements.clone(),
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
    /// Reconstruct a client-side statement from this row and its title.
    ///
    /// This actor view predates environmental standing, so its client-side
    /// reconstruction is unconstrained. Authoritative allocation always reads
    /// the journal's [`crate::TaskHead`] rather than this presentation copy.
    #[must_use]
    pub fn with_title(&self, title: &str) -> crate::TaskHead {
        crate::TaskHead {
            born: self.born.clone(),
            queue_origin: self.queue_origin,
            title: title.to_owned(),
            description: self.description.clone(),
            acceptance: self.acceptance.clone(),
            requirements: self.requirements.clone(),
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
    /// Immutable creator team for lane-authored tasks.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub creator_team: Option<crate::CanonicalUuid>,
    /// Authored title, carried beside the statement rather than inside it.
    pub presentation_title: String,
    /// Current statement evidence; the title sits in `presentation_title`.
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
    /// Whether the current freed claim came from an explicit release command.
    #[serde(default, skip_serializing_if = "is_false")]
    pub explicit_release: bool,
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
    /// Commits retained on the task-owned ref after an explicit release.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub parked_commits: Option<crate::body::TaskParkedCommits>,
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
    /// Project authoritative heads into one task row.
    ///
    /// A hidden project is one the allocator does not offer by default, not a
    /// confidence: nothing here rewrites a title or drops authored prose, and
    /// [`Self::is_active_public`] remains the only thing visibility decides.
    #[must_use]
    pub fn from_authoritative(
        visibility: ProjectVisibility,
        origin_thread: String,
        creator_team: Option<crate::CanonicalUuid>,
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
            creator_team,
            presentation_title,
            statement,
            workflow,
            claim,
            lease,
            explicit_release: false,
            claim_context: None,
            git_context: None,
            parked_commits: None,
        }
    }
    /// Attach whether the current claim head records an explicit release.
    #[must_use]
    pub const fn with_explicit_release(mut self, explicit_release: bool) -> Self {
        self.explicit_release = explicit_release;
        self
    }

    /// Attach compact recovery references derived from the same authoritative snapshot.
    #[must_use]
    pub fn with_context(
        mut self,
        claim_context: Option<TaskClaimContext>,
        git_context: Option<TaskGitContext>,
        parked_commits: Option<crate::body::TaskParkedCommits>,
    ) -> Self {
        self.claim_context = claim_context;
        self.git_context = git_context;
        self.parked_commits = parked_commits;
        self
    }

    /// Client-side statement reconstructed from actor-facing evidence.
    #[must_use]
    pub fn client_statement(&self) -> crate::TaskHead {
        self.statement.body.with_title(&self.presentation_title)
    }

    /// Authored title of this task.
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
    /// Principal's most recent allocator cell, included by full-board reads.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub recent_cell: Option<crate::ClaimCell>,
    /// Selected current task evidence; history pages are newest-workflow-first,
    /// while every other scope retains durable task-stamp order.
    pub tasks: Vec<BoardTask>,
    /// Open-read allocation order: the successive choices of the resident
    /// allocator's own selection step for a draining lane with no locality
    /// history, so the first stamp is the task offered next. Standing is held
    /// fixed across steps. Empty for every other read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub offer: Vec<crate::TaskStamp>,
    /// Every steering key whose direction caused non-deleted work, mapped to
    /// that work's handles in stable order. Open reads carry the whole index,
    /// so a quoted direction keeps its Work after that work settles.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub origins: std::collections::BTreeMap<String, Vec<crate::TaskHandle>>,
    /// Terminal-page metadata, present only for [`BoardRead::History`].
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub history: Option<TaskHistoryPage>,
    /// Durable notes on the named tasks of a [`BoardRead::Tasks`] read that
    /// asked for them, each task's notes in ascending journal position.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub notes: std::collections::BTreeMap<crate::TaskStamp, Vec<TaskDetailNote>>,
    /// Current executable-configuration trust heads in ascending digest order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trust: Vec<Positioned<crate::TrustHead>>,
    /// Product version of the resident appliance that captured this snapshot.
    pub appliance: crate::manifest::ProductVersion,
    /// Current resident and lane head used by possession witnesses.
    pub residency: Positioned<crate::ResidencyHead>,
}

impl BoardAnswer {
    /// Pending public routes only; hidden evidence never enters active inventory.
    pub fn active_public_tasks(&self) -> impl Iterator<Item = &BoardTask> {
        self.tasks.iter().filter(|task| task.is_active_public())
    }

    /// Compile the environmental observations carried by this snapshot.
    #[must_use]
    pub fn environment(&self) -> crate::Environment {
        crate::Environment::journal_schema(self.schema)
            .expect("an admitted board schema is positive")
            .with_trust_heads(self.trust.iter().map(|head| head.body.clone()))
            .with_appliance(self.appliance.clone())
    }
}

/// One deterministic resident-rendered document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextAnswer {
    pub text: String,
}

const fn is_false(value: &bool) -> bool {
    !*value
}
