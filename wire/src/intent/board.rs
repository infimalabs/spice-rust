//! Actor-facing board intent, evidence, and presentation DTOs.

use serde::{Deserialize, Serialize};

use super::TeamLife;
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
/// `All` remains complete; `Next`, `Open`, and `History` name their exclusions
/// explicitly.
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
    /// The allocator's bounded live-frontier decision for one principal.
    Next,
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
        /// Include only tasks the app team created, or whose most recent
        /// winning claim was held by a lane now in that team. Absent reads
        /// every team's history.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::optional::present"
        )]
        team: Option<crate::CanonicalUuid>,
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
    /// Include settled tasks and their durable historical relations.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub all: bool,
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

/// The operator's closure of a task, read back from its `task::closed` fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDetailClosure {
    /// Position of the closure fact.
    pub position: crate::Seq,
    /// Commit time recorded by the journal.
    pub at: crate::Instant,
    /// Principal that closed the task.
    pub author: Actor,
    /// Stated evidence or reason the task is complete.
    pub validation: String,
    /// Phase the workflow held when the task was closed.
    pub phase: crate::Phase,
    /// Actor whose live claim the closure overrode, when it overrode one.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub claimed: Option<Actor>,
}

/// One workflow step on a task's history, read back from its lifecycle fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDetailStep {
    /// Position of the step's fact.
    pub position: crate::Seq,
    /// Commit time recorded by the journal.
    pub at: crate::Instant,
    /// Principal that took the step.
    pub author: Actor,
    /// What the step recorded.
    pub step: TaskStep,
}

/// What one workflow step recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskStep {
    /// A claimed phase completed on its stated validation.
    Done {
        /// Phase the completion closed.
        phase: crate::Phase,
        /// Evidence the completion rests on.
        validation: String,
    },
    /// The workflow advanced into a phase.
    Phased {
        /// Phase the workflow entered.
        phase: crate::Phase,
    },
    /// A review verdict landed.
    Reviewed {
        /// How the review closed.
        verdict: crate::body::Verdict,
        /// A finding with its note, or a clean verdict's note.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::optional::present"
        )]
        text: Option<String>,
        /// Correction tasks the review linked.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        followups: Vec<crate::TaskStamp>,
    },
    /// The operator concluded an open Design.
    Concluded {
        /// Done, or collapsed with the stated reason its premise failed.
        conclusion: crate::body::DesignConclusion,
    },
}

/// One acknowledged steering directive on a task's history: a task or claim
/// fact whose `ack:` origin names it, or a `steer::acked` fact by which the
/// task's holder acknowledged operator steering while holding the task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskAcknowledgment {
    /// Position of the carrying fact.
    pub position: crate::Seq,
    /// Kind of that fact.
    pub kind: crate::FactKind,
    /// Steering key the fact names.
    pub key: String,
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
    /// The operator's closure, when the task was closed rather than done.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub closure: Option<TaskDetailClosure>,
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
    /// One non-task cause: a steering directive, document, or operator Draft.
    Origin { origin: crate::TaskProvenance },
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
    Filed,
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
        status: crate::WorkflowStatusKind,
    },
    /// One non-task cause; a parentless task has no origin node.
    Origin { origin: crate::TaskProvenance },
    /// One completed review. `phase` and `author` name the most recent
    /// completed phase before it and that phase's author; both are absent when
    /// no phase completed first, as in a flow that begins with review.
    Review {
        task: crate::TaskStamp,
        position: crate::Seq,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phase: Option<crate::Phase>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        author: Option<Actor>,
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

/// Most lanes one task-timeline read follows.
pub const TASK_TIMELINE_LANE_LIMIT: usize = 32;
/// Largest task-timeline page one read accepts.
pub const TASK_TIMELINE_PAGE_LIMIT_MAX: u16 = 100;

/// Page the task-workflow transitions a set of lanes caused, newest first.
///
/// A transition belongs to the lane that caused it or that it binds: a claim
/// to its lane, a review or reviewer seat to its reviewer, and any other
/// lifecycle fact to the lane that authored it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTimeline {
    /// Distinct lanes whose transitions the page follows.
    pub lanes: Vec<crate::CanonicalUuid>,
    /// Oldest position the preceding page returned; absent for the newest page.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub before: Option<crate::Seq>,
    /// Maximum transitions returned by this page.
    pub limit: u16,
}

/// One closed task-workflow transition kind a timeline reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TaskTimelineEvent {
    Added {},
    Edited {},
    /// The workflow head advanced into `phase`.
    Phased {
        phase: crate::Phase,
    },
    Completed {},
    /// The lane was seated as reviewer of `phase`.
    Assigned {
        phase: crate::Phase,
    },
    Reviewed {
        verdict: crate::body::Verdict,
    },
    Deferred {},
    Woken {},
    Filed {},
    Deleted {},
    /// The lane won possession.
    Claimed {},
    /// Possession passed to the lane's current occupant.
    Carried {},
    /// The lane recovered possession after residency moved ahead of it.
    Rebound {},
    /// The lane released possession.
    Released {},
    /// The lane's lease expired.
    Lapsed {},
}

/// One attributed task-workflow transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTimelineEntry {
    /// Journal position of the fact that recorded the transition.
    pub position: crate::Seq,
    pub at: crate::Instant,
    pub lane: crate::CanonicalUuid,
    pub task: crate::TaskStamp,
    pub handle: crate::TaskHandle,
    pub title: String,
    pub event: TaskTimelineEvent,
}

/// One newest-first page of a lane set's task-workflow transitions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTimelineAnswer {
    pub snapshot: crate::Seq,
    /// The request's lanes in canonical order.
    pub lanes: Vec<crate::CanonicalUuid>,
    /// Transitions in descending journal position.
    pub entries: Vec<TaskTimelineEntry>,
    /// `before` for the next older page, absent once this snapshot is exhausted.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub next: Option<crate::Seq>,
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
    /// Filing standing, independent of project identity and visibility.
    #[serde(default, skip_serializing_if = "crate::TaskFiling::is_filed")]
    pub filing: crate::TaskFiling,
    /// Declared scheduling priority.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub priority: Option<crate::TaskPriority>,
    /// Board-owned taste-review hold.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub taste: Option<crate::TasteHold>,
    /// Task-document node slug that places this task under a document row.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub document_slug: Option<String>,
    /// Evidence of a resident-filed taste proposal.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub taste_proposal: Option<crate::TasteProposal>,
    /// Tasks that must complete first.
    pub dependencies: Vec<crate::TaskStamp>,
    /// Declared Plan or Design intent.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub intent: Option<crate::TaskIntent>,
    /// Explicit native preparation context, separate from scheduling.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub preparation: Option<crate::TaskPreparation>,
    /// Selected decision whose execution authorization released this output.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub authorization: Option<crate::rooms::DecisionRef>,
    /// Question results required answered before positive phase completion.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<crate::rooms::QuestionResultRef>,
    /// Creation provenance, immutable across statement revisions.
    pub provenance: crate::TaskProvenance,
    /// Creation-time transcript authority, immutable across statement revisions.
    pub creation: crate::body::TaskCreationContext,
    /// Fact that established this statement revision.
    pub revision: crate::Seq,
}

impl From<&crate::TaskHead> for BoardStatement {
    fn from(statement: &crate::TaskHead) -> Self {
        Self {
            handle: crate::TaskHandle::for_filing(
                &statement.project,
                &statement.born,
                statement.filing,
            ),
            born: statement.born.clone(),
            queue_origin: statement.queue_origin,
            description: statement.description.clone(),
            acceptance: statement.acceptance.clone(),
            requirements: statement.requirements.clone(),
            project: statement.project.clone(),
            filing: statement.filing,
            priority: statement.priority,
            taste: statement.taste.clone(),
            document_slug: statement.document_slug.clone(),
            taste_proposal: statement.taste_proposal.clone(),
            dependencies: statement.dependencies.clone(),
            intent: statement.intent,
            preparation: statement.preparation.clone(),
            authorization: statement.authorization.clone(),
            evidence: statement.evidence.clone(),
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
            filing: self.filing,
            priority: self.priority,
            taste: self.taste.clone(),
            document_slug: self.document_slug.clone(),
            taste_proposal: self.taste_proposal.clone(),
            dependencies: self.dependencies.clone(),
            intent: self.intent,
            preparation: self.preparation.clone(),
            authorization: self.authorization.clone(),
            evidence: self.evidence.clone(),
            provenance: self.provenance.clone(),
            creation: self.creation.clone(),
            revision: self.revision,
        }
    }
}

/// Why one pending task can no longer pass its preparation gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskStranding {
    /// Controller whose terminal negative disposition made release impossible.
    pub controller: TaskHandle,
    /// Exact operator command that settles this task explicitly.
    pub settle: String,
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
    /// Whether the current working-group partition holds this task.
    #[serde(default, skip_serializing_if = "is_false")]
    pub grouped: bool,
    /// Whether this Design is active under its group's shared stance.
    #[serde(default, skip_serializing_if = "is_false")]
    pub design_active: bool,
    /// This Design's pass-cycle evidence, so a client allocates from the
    /// same projection the resident does; empty for any other task.
    #[serde(
        default,
        skip_serializing_if = "crate::rooms::DesignCycleFacts::is_empty"
    )]
    pub design_cycle: crate::rooms::DesignCycleFacts,
    /// Decision on the latest unwithdrawn candidate version naming this
    /// output, whether its verdict released or rejected the output; absent
    /// while that version is open or no decision ruled on it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub preparation_decision: Option<crate::rooms::DecisionRef>,
    /// The reason recorded when the operator concluded this Design collapsed.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub collapse_reason: Option<String>,
    /// Terminal controller that can no longer release this pending task.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub stranded: Option<TaskStranding>,
    /// Pending work this task's negative settlement would close atomically.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controlled: Vec<TaskHandle>,
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
    /// Commits saved by object ID after an explicit release.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub parked_commits: Option<crate::body::TaskParkedCommits>,
}

/// Current heads needed to witness one `task next` decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NextWorkTask {
    pub statement: Positioned<crate::TaskHead>,
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
}

/// One passive Design request owed at a lane's post-completion break.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignRequest {
    /// Design whose room accepts the answer.
    pub design: crate::TaskHandle,
    /// Completed task whose terms selected the Design.
    pub task: crate::TaskHandle,
    /// Current Design title.
    pub title: String,
    /// Current Design statement revision.
    pub statement: crate::SequenceCode,
    /// One-line Design context.
    pub pitch: String,
    /// In-order prefix of whole open questions bounded for delivery.
    pub questions: Vec<String>,
    /// Distinctive shared terms that selected this Design.
    pub terms: Vec<String>,
}

/// Exact pending execution-board classification when the allocator has no
/// candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NextWorkCounts {
    /// Current claims.
    pub active: usize,
    /// Lapsed claims.
    pub stale: usize,
    /// Closed execution gates with no advancing path.
    pub blocked: usize,
    /// Advancing prerequisite chains and Plan control.
    pub workflow_waiting: usize,
    /// Open Designs in each pass-cycle state.
    pub designs: crate::rooms::DesignCycleCounts,
    /// Explicitly deferred work.
    pub deferred: usize,
    /// Parked OOPS rows.
    pub oops: usize,
}

/// One closed allocator decision captured at the board snapshot barrier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum NextWorkState {
    /// The principal already owns this sole active claim.
    Active { task: NextWorkTask },
    /// The globally highest-ranked eligible task and its claim witnesses.
    Candidate { task: NextWorkTask },
    /// A completed claim owes one room contribution or leave before allocation.
    DesignRequest { request: DesignRequest },
    /// No task is eligible under this team policy and board standing.
    Dry {
        board: NextWorkCounts,
        team: TeamLife,
    },
}

/// Server-resolved public project bounds for one board snapshot. Any valid
/// root within them is admissible and readable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardProjectPolicy {
    /// Fewest dotted segments admitted for a public project.
    pub public_min_depth: u64,
    /// Most dotted segments admitted for a public project.
    pub public_max_depth: u64,
}

/// Server-resolved default read route for one board principal: every public
/// row, plus any row created on the principal's own origin thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardActorRoute {
    /// Actor identity the route reads for.
    pub actor: String,
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
            grouped: false,
            design_active: false,
            design_cycle: crate::rooms::DesignCycleFacts::default(),
            preparation_decision: None,
            collapse_reason: None,
            stranded: None,
            controlled: Vec::new(),
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

    /// Attach whether the current working-group partition holds this task.
    #[must_use]
    pub const fn with_grouped(mut self, grouped: bool) -> Self {
        self.grouped = grouped;
        self
    }

    /// Attach whether this Design is active under its group's shared stance.
    #[must_use]
    pub const fn with_design_active(mut self, active: bool) -> Self {
        self.design_active = active;
        self
    }

    /// Attach this Design's pass-cycle evidence.
    #[must_use]
    pub const fn with_design_cycle(mut self, cycle: crate::rooms::DesignCycleFacts) -> Self {
        self.design_cycle = cycle;
        self
    }

    /// Attach the decision classifying this output for selection waiting.
    #[must_use]
    pub fn with_preparation_decision(
        mut self,
        decision: Option<crate::rooms::DecisionRef>,
    ) -> Self {
        self.preparation_decision = decision;
        self
    }

    /// Attach the reason the operator's collapse of this Design recorded.
    #[must_use]
    pub fn with_collapse_reason(mut self, reason: Option<String>) -> Self {
        self.collapse_reason = reason;
        self
    }

    /// Attach a terminal preparation controller that can no longer release this task.
    #[must_use]
    pub fn with_stranding(mut self, stranded: Option<TaskStranding>) -> Self {
        self.stranded = stranded;
        self
    }

    /// Attach pending work this task still gates through preparation control.
    #[must_use]
    pub fn with_controlled(mut self, controlled: Vec<TaskHandle>) -> Self {
        self.controlled = controlled;
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

    /// Whether this filed row belongs to pending public work inventory.
    #[must_use]
    pub fn is_active_public(&self) -> bool {
        !self.visibility.is_hidden()
            && self.statement.body.filing.is_filed()
            && self.workflow.body.status.is_pending()
    }

    /// This Design's lifecycle standing; `None` for a task without Design
    /// intent.
    #[must_use]
    pub fn design_standing(&self) -> Option<DesignStanding> {
        DesignStanding::of(
            self.statement.body.intent,
            &self.workflow.body,
            self.design_active,
            self.collapse_reason.as_deref(),
        )
    }

    /// Whether a lane may take, or already holds, this Design's pass.
    #[must_use]
    pub const fn design_availability(&self) -> crate::rooms::DesignAvailability {
        if matches!(&self.claim, Some(claim) if claim.body.is_active()) {
            crate::rooms::DesignAvailability::Claimed
        } else if self.design_active {
            crate::rooms::DesignAvailability::Unclaimed
        } else {
            crate::rooms::DesignAvailability::Passive
        }
    }
}
/// Whether the current heads describe an open Design.
///
/// This is the canonical constant-time rule used by authority and presentation
/// surfaces. A Design-intent task in an intake phase is ordinary claimed work,
/// not an open Design.
#[must_use]
pub fn is_open_design(intent: Option<crate::TaskIntent>, workflow: &crate::WorkflowHead) -> bool {
    intent == Some(crate::TaskIntent::Design)
        && workflow.status.is_pending()
        && workflow.phase.as_str() == "design"
}

/// One Design's lifecycle standing. An open Design is passive outside every
/// working group and active inside one; only the operator concludes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DesignStanding {
    /// Open outside every working group: no path grants it a claim.
    Passive,
    /// Open inside a working group: lanes may claim or be coordinated onto it.
    Active,
    /// The operator concluded it done.
    Done,
    /// The operator concluded it collapsed. A retained record concluded
    /// before collapse recorded a reason carries none.
    Collapsed {
        /// Why its premise did not hold.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// The operator withdrew it.
    Deleted,
}

impl DesignStanding {
    /// Derive the standing every board, task, group, room, and browser read
    /// shows from current heads, so terminal settlement outranks any retained
    /// group membership; `None` for a task without Design intent.
    #[must_use]
    pub fn of(
        intent: Option<crate::TaskIntent>,
        workflow: &crate::WorkflowHead,
        active: bool,
        collapse_reason: Option<&str>,
    ) -> Option<Self> {
        if intent != Some(crate::TaskIntent::Design) {
            return None;
        }
        Some(match workflow.status {
            crate::WorkflowStatus::Settled {
                conclusion: crate::WorkflowConclusion::Negative(crate::WorkflowNegative::Deleted),
                ..
            } => Self::Deleted,
            crate::WorkflowStatus::Settled {
                conclusion: crate::WorkflowConclusion::Positive { .. },
                ..
            } => Self::Done,
            crate::WorkflowStatus::Settled {
                conclusion: crate::WorkflowConclusion::Negative(crate::WorkflowNegative::Closed),
                ..
            } => Self::Collapsed {
                reason: collapse_reason.map(str::to_owned),
            },
            crate::WorkflowStatus::Pending(_) if !is_open_design(intent, workflow) => return None,
            crate::WorkflowStatus::Pending(_) if active => Self::Active,
            crate::WorkflowStatus::Pending(_) => Self::Passive,
        })
    }

    /// Whether the Design is still open.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        matches!(self, Self::Passive | Self::Active)
    }

    /// Stable machine key.
    #[must_use]
    pub const fn key(&self) -> &'static str {
        match self {
            Self::Passive => "passive",
            Self::Active => "active",
            Self::Done => "done",
            Self::Collapsed { .. } => "collapsed",
            Self::Deleted => "deleted",
        }
    }
}

impl std::fmt::Display for DesignStanding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Passive => {
                formatter.write_str("open passive (no Explore or Converge working group)")
            }
            Self::Active => {
                formatter.write_str("open active (in an Explore or Converge working group)")
            }
            Self::Done => formatter.write_str("concluded done"),
            Self::Collapsed {
                reason: Some(reason),
            } => write!(formatter, "concluded collapsed: {reason}"),
            Self::Collapsed { reason: None } => {
                formatter.write_str("concluded collapsed (no reason recorded)")
            }
            Self::Deleted => formatter.write_str("deleted"),
        }
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
    /// Typed allocator decision, present only for [`BoardRead::Next`].
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub next: Option<NextWorkState>,
    /// Selected current task evidence; history pages are newest-workflow-first,
    /// while every other scope retains durable task-stamp order.
    pub tasks: Vec<BoardTask>,
    /// Open-read allocation order: the successive choices of the resident
    /// allocator's own selection step for a draining lane with no locality
    /// history, so the first stamp is the task offered next. Standing is held
    /// fixed across steps. Empty for every other read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub offer: Vec<crate::TaskStamp>,
    /// Open-read rows whose prerequisite or controller gate has no path
    /// through current advancing work. This is execution trouble, in durable
    /// identity order. Empty for every other read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked: Vec<crate::TaskStamp>,
    /// Open-read rows held by normal Plan/Design control or an unresolved
    /// prerequisite chain that still reaches advancing work. These remain
    /// allocator-ineligible but are not execution trouble. Durable identity
    /// order; empty for every other read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflow_waiting: Vec<crate::TaskStamp>,
    /// Every open Design's pass-cycle state among this read's rows, judged
    /// once from the whole live board so each surface renders the same
    /// value; a Design awaiting decision is the operator's selection wait.
    /// Empty for [`BoardRead::Next`].
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub design_cycles: std::collections::BTreeMap<crate::TaskStamp, crate::rooms::DesignCycle>,
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
    /// Every acknowledged steering directive joined to each named task through
    /// an `ack:` task/claim origin or the holder's operator `steer::acked` fact.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub acknowledgments: std::collections::BTreeMap<crate::TaskStamp, Vec<TaskAcknowledgment>>,
    /// The operator's closure of each named task that was closed.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub closures: std::collections::BTreeMap<crate::TaskStamp, TaskDetailClosure>,
    /// Workflow steps on the named tasks of a [`BoardRead::Tasks`] read that
    /// asked for notes, each task's steps in ascending journal position.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub steps: std::collections::BTreeMap<crate::TaskStamp, Vec<TaskDetailStep>>,
    /// Current executable-configuration trust heads in ascending digest order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trust: Vec<Positioned<crate::TrustHead>>,
    /// Product version of the resident appliance that captured this snapshot.
    pub appliance: crate::manifest::ProductVersion,
    /// Current resident and lane head used by possession witnesses.
    pub residency: Positioned<crate::ResidencyHead>,
    /// The peer address every residency lane offers at this barrier.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub addresses: std::collections::BTreeMap<crate::CanonicalUuid, crate::SeatAddress>,
}

impl BoardAnswer {
    /// Pending public routes only; hidden evidence never enters active inventory.
    pub fn active_public_tasks(&self) -> impl Iterator<Item = &BoardTask> {
        self.tasks.iter().filter(|task| task.is_active_public())
    }

    /// Whether `task` is a Design the operator's decision holds.
    #[must_use]
    pub fn awaiting_selection(&self, task: &crate::TaskStamp) -> bool {
        self.design_cycles.get(task) == Some(&crate::rooms::DesignCycle::AwaitingDecision)
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
