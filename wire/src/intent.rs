//! Command intent and its answer: the body a caller sends and the body it
//! gets back.
//!
//! WIRE fixes one rule here: the discriminant selects the body DTO, witness
//! schema, transition, and reply DTO together. The witness half already lives
//! in the registry; this module owns the other two, so every command kind
//! names exactly one intent and one answer.
//!
//! Bodies carry caller intent only. None of them carries an actor, lane,
//! epoch, journal position, result, or fact envelope, because the commit door
//! assigns all of those; a body that named one would let a caller forge it.
//!
//! Several kinds share one shape on purpose. Renewing, freeing, and lapsing a
//! claim all name a task and nothing else, so they share [`TaskRef`] rather
//! than carrying three identical structs that could drift apart.

use serde::{Deserialize, Serialize};

use crate::body::{ConsentQueueCause, TranscriptLocus};
use crate::command::CommandKind;
use crate::envelope::{Actor, Origin, Seq};
use crate::fact::FactKind;
use crate::head::{LaneResidency, ResidentState, TapeLocus, TrustState};
use crate::ident::{CanonicalUuid, Digest, Instant};
use crate::witness::{Answer, Disposition, Name, Phase, TaskHandle};

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
    /// Public task name.
    pub handle: crate::TaskHandle,
    /// Stable task identity.
    pub uuid: crate::CanonicalUuid,
    /// Scheduler project grouping.
    pub project: String,
    /// Authored task labels.
    pub tags: Vec<String>,
    /// Tasks that must complete first.
    pub dependencies: Vec<crate::TaskHandle>,
    /// Creation provenance, immutable across statement revisions.
    pub provenance: crate::Origin,
    /// Fact that established this statement revision.
    pub revision: crate::Seq,
}

impl From<&crate::TaskHead> for BoardStatement {
    fn from(statement: &crate::TaskHead) -> Self {
        Self {
            handle: statement.handle.clone(),
            uuid: statement.uuid,
            project: statement.project.clone(),
            tags: statement.tags.clone(),
            dependencies: statement.dependencies.clone(),
            provenance: statement.provenance.clone(),
            revision: statement.revision,
        }
    }
}

impl BoardStatement {
    /// Reconstruct a client-side statement using its actor-facing title.
    #[must_use]
    pub fn with_title(&self, title: &str) -> crate::TaskHead {
        crate::TaskHead {
            handle: self.handle.clone(),
            uuid: self.uuid,
            title: title.to_owned(),
            project: self.project.clone(),
            tags: self.tags.clone(),
            dependencies: self.dependencies.clone(),
            provenance: self.provenance.clone(),
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
}

impl BoardTask {
    /// Normalize a hidden row to the sole actor-facing title.
    pub fn redact_hidden_title(&mut self) {
        if self.visibility.is_hidden() {
            self.presentation_title = "Hidden task".to_owned();
        }
    }

    /// Client-side statement reconstructed from actor-facing evidence.
    #[must_use]
    pub fn statement_head(&self) -> crate::TaskHead {
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

/// Add one task as direct intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskAdd {
    /// One-line statement of the work.
    pub title: String,
    /// One-to-three-segment public project, or absent for private work.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub project: Option<String>,
    /// Longer body context.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub description: Option<String>,
    /// Criteria that decide whether the work is done.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance: Vec<String>,
    /// Non-default phase route, when the caller means one.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub flow: Option<String>,
    /// Handles this task must follow.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends: Vec<TaskHandle>,
}

/// Replace an unsettled task statement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskEdit {
    /// Task whose statement is replaced.
    pub task: TaskHandle,
    /// Replacement title, when the title moves.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub title: Option<String>,
    /// Replacement description, when the description moves.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub description: Option<String>,
    /// Replacement criteria, when they move.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance: Vec<String>,
}

/// Append an observation without replacing the statement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskNote {
    /// Task the observation attaches to.
    pub task: TaskHandle,
    /// The observation.
    pub note: String,
}

/// Set a wakeable deferral.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDefer {
    /// Task being deferred.
    pub task: TaskHandle,
    /// Instant the deferral wakes at.
    pub wait: Instant,
}

/// Name one task and nothing else.
///
/// Waking a task, and every claim move that only identifies its subject,
/// share this shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRef {
    /// The subject.
    pub task: TaskHandle,
}

/// Wire a dependency edge between two existing tasks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDepends {
    /// Task that follows.
    pub task: TaskHandle,
    /// Tasks it must follow.
    pub after: Vec<TaskHandle>,
}

/// Advance the claimed phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskPhase {
    /// Task being advanced.
    pub task: TaskHandle,
    /// Phase it advances into.
    pub phase: Phase,
}

/// Complete the phase held by the caller's active claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDone {
    /// Task being completed.
    pub task: TaskHandle,
    /// How completion was verified.
    pub validation: String,
}

/// Record an assigned review verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskReview {
    /// Task being reviewed.
    pub task: TaskHandle,
    /// The verdict.
    pub finding: String,
    /// Supporting prose.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub note: Option<String>,
}

/// Seat a reviewer under the anti-self-review rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskAssign {
    /// Task being assigned.
    pub task: TaskHandle,
    /// Actor seated as reviewer.
    pub reviewer: Actor,
}

/// Run the eligibility query and atomically win its claim.
///
/// No handle appears: naming one would make this a request for a particular
/// task rather than a query, and the query and the win share one transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimWin {
    /// Named eligibility rule the query runs.
    pub rule: Name,
}

/// Pass possession to a successor with ancestor lineage preserved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimCarry {
    /// Task whose possession moves.
    pub task: TaskHandle,
    /// Actor receiving possession.
    pub successor: Actor,
}

/// Commit the exact normalized preview at its witnessed board barrier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocApply {
    /// Digest of the normalized preview being committed.
    pub preview: Digest,
    /// Canonical task-document bytes whose digest was previewed.
    pub document: String,
}

/// Append one durable directive to a lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SteerSend {
    /// Lane the directive lands on.
    pub lane: CanonicalUuid,
    /// The intent being sent.
    pub message: String,
}

/// Read pending direction visible to the authenticated principal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SteerInbox {
    /// Lane selected by an operator, or the authenticated lane when absent.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub lane: Option<CanonicalUuid>,
}

/// Name one directive and nothing else.
///
/// Resending and cancelling both identify a directive without adding to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectiveRef {
    /// The subject directive.
    pub directive: CanonicalUuid,
}

/// Retire a live directive with a typed disposition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SteerDispose {
    /// Key-shaped token the caller expects a transcript recognition to name.
    pub token: String,
    /// Final disposition the transcript marker must state.
    pub disposition: Disposition,
}

/// Decide one pending consent request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentDecide {
    /// Request being decided.
    pub request: CanonicalUuid,
    /// The decision.
    pub answer: Answer,
}

/// Read the pending consent queue and its current renewal budgets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentRead {}

/// Read the tracked consent policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentPolicy {}

/// Name one executable digest and nothing else.
///
/// Pending, granting, and revoking trust each name the exact digest; the
/// standing they move it to is the command kind, not a body field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustRef {
    /// The exact executable digest.
    pub digest: Digest,
}

/// Read the current lane table without mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneRead {}

/// Seat a lane in an operator-created worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneSeat {
    /// Lane being seated.
    pub lane: CanonicalUuid,
    /// Worktree path the operator created.
    pub worktree: String,
    /// Closed lane-life spelling selected for this seat.
    pub life: String,
    /// Vendor selected for this seat.
    pub vendor: Name,
}

/// Change a seated lane's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LaneAction {
    /// Gracefully end the current occupant and leave the seat vacant.
    Drain,
    /// Launch a new occupant into a vacant seat.
    Wake,
}

/// Change a seated lane's lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneState {
    /// Lane whose state moves.
    pub lane: CanonicalUuid,
    /// Lifecycle action the operator selected.
    pub action: LaneAction,
}

/// Select one lane's continuity account, or the operator-wide account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneRecall {
    /// Lane whose continuity is requested; absent for the operator-wide view.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub lane: Option<CanonicalUuid>,
}

/// Record a taste verdict at its evidence locus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TasteRecord {
    /// Bag the verdict lands in.
    pub name: Name,
    /// Digest of the tracked taste configuration judged against.
    pub digest: Digest,
    /// Where the judged prose sits.
    pub locus: String,
}

/// Recognize evidence that arrived from outside the journal.
///
/// Canary, vendor, commit, and proof evidence all reduce to one observation
/// key; recognition is idempotent on that key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recognize {
    /// Observation key that makes recognition idempotent.
    pub key: Digest,
}

/// Publish the sealed manifest and each target artifact digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleasePublish {
    /// Observation key that makes publication idempotent.
    pub key: Digest,
    /// Digest of the canonical target manifest.
    pub manifest: Digest,
}

/// Drive the resident to the requested lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpChange {
    /// State the resident is asked to reach.
    pub state: ResidentState,
}

/// The answer a task command returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskAnswer {
    /// Task the answer concerns.
    pub task: TaskHandle,
    /// Its status after the transaction.
    pub status: String,
}

/// The answer a claim command returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimAnswer {
    /// Task whose possession moved.
    pub task: TaskHandle,
    /// Actor holding it after the transaction.
    pub holder: Actor,
}

/// The answer a document application returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocAnswer {
    /// Digest of the preview that was committed.
    pub preview: Digest,
    /// Handles the application touched.
    pub touched: Vec<TaskHandle>,
}

/// The answer a steering command returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectiveAnswer {
    /// Directive the answer concerns.
    pub directive: CanonicalUuid,
    /// Human-sized key quoted by inbox and acknowledgment prose.
    pub key: String,
    /// Its disposition after the transaction.
    pub disposition: Disposition,
}

/// One pending directive returned by an inbox read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingDirective {
    /// Durable directive identity.
    pub directive: CanonicalUuid,
    /// Human-sized acknowledgment key.
    pub key: String,
    /// Direction text.
    pub text: String,
    /// Control channel attached to the direction, when present.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub control: Option<String>,
    /// Instant at which the directive lapses.
    pub expires: Instant,
}

/// Pending direction for one lane at a resident snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneInbox {
    /// Lane this row describes.
    pub lane: CanonicalUuid,
    /// Occupant currently seated in the lane, when any.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub occupant: Option<CanonicalUuid>,
    /// Pending directives in journal order.
    pub pending: Vec<PendingDirective>,
}

/// Snapshot-positioned steering inbox answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboxAnswer {
    /// Resident generation that produced the answer.
    pub epoch: CanonicalUuid,
    /// Visible lanes in canonical lane order.
    pub lanes: Vec<LaneInbox>,
}

/// The answer a consent decision returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentAnswer {
    /// Request that was decided.
    pub request: CanonicalUuid,
    /// The recorded decision.
    pub answer: Answer,
}

/// One current bounded-rule budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentBudgetAnswer {
    /// Rule whose grants are metered.
    pub rule: Name,
    /// Grants admitted between renewals.
    pub bound: u32,
    /// Grants still available in the current renewal window.
    pub remaining: u32,
}

/// One request currently waiting on the operator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentPendingAnswer {
    /// Pending request identity.
    pub request: CanonicalUuid,
    /// Lane that raised the request.
    pub lane: CanonicalUuid,
    /// Operation awaiting consent.
    pub operation: String,
    /// Rule or posture that queued the request.
    pub rule: Name,
    /// Request-bound reason the operation queued.
    pub cause: ConsentQueueCause,
    /// Current renewal state when this request exhausted a bounded rule.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub budget: Option<ConsentBudgetAnswer>,
}

/// The pending consent queue and all current bounded-rule budgets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentQueueAnswer {
    /// Requests waiting on the operator, ordered by request identity.
    pub pending: Vec<ConsentPendingAnswer>,
    /// Current budgets in tracked policy order.
    pub budgets: Vec<ConsentBudgetAnswer>,
}

/// One answer available to tracked consent policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConsentPolicyDecision {
    /// Permit the operation.
    Allow,
    /// Refuse the operation.
    Deny,
    /// Ask the operator.
    Queue,
}

/// One ordered tracked consent rule and its current budget state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentRuleAnswer {
    /// Stable rule identity.
    pub name: Name,
    /// Exact operation or glob pattern the rule matches.
    pub tool: String,
    /// Decision produced by the rule.
    pub answer: ConsentPolicyDecision,
    /// Configured grant bound, when present.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub budget: Option<u32>,
    /// Grants currently available, when the rule is bounded.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub remaining: Option<u32>,
}

/// The tracked consent posture and ordered rule table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentPolicyAnswer {
    /// Decision used when no rule matches.
    pub posture: ConsentPolicyDecision,
    /// Rules in first-match order.
    pub rules: Vec<ConsentRuleAnswer>,
}

/// The answer a trust command returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustAnswer {
    /// Digest whose standing moved.
    pub digest: Digest,
    /// Its standing after the transaction.
    pub standing: TrustState,
}

/// The answer a lane or resident state change returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateAnswer {
    /// State held before the transaction.
    pub from: ResidentState,
    /// State held after it.
    pub to: ResidentState,
}

/// Current lane state read at one resident barrier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneAnswer {
    /// Resident generation that owns the table.
    pub epoch: CanonicalUuid,
    /// Resident state at the read barrier.
    pub state: ResidentState,
    /// Current lane rows in stable lane order.
    pub lanes: Vec<LaneResidency>,
}

/// The answer a lane seating or lifecycle command returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneChangeAnswer {
    /// Lane whose disposition changed.
    pub lane: CanonicalUuid,
    /// Disposition held before the transaction.
    pub from: String,
    /// Disposition held after the transaction.
    pub to: String,
}

/// One bounded continuity event retained in a recall account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecallEntry {
    /// Journal position of the event.
    pub seq: Seq,
    /// Closed fact kind observed there.
    pub kind: FactKind,
    /// Principal that authored the event.
    pub actor: Actor,
    /// Durable origin carried by the event.
    pub origin: Origin,
    /// Lane named by the event, absent for repository-wide facts.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub lane: Option<CanonicalUuid>,
    /// Safe continuity evidence projected from selected fact bodies.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub provenance: Option<RecallProvenance>,
}

/// Closed, redacted evidence carried by a continuity chronology entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RecallProvenance {
    /// Exact durable vendor session authority.
    Session {
        /// Occupant that declared the session.
        occupant: CanonicalUuid,
        /// Worktree bound to the session launch.
        worktree: String,
        /// Vendor that admitted the declaration.
        vendor: Name,
        /// Resident generation that observed the declaration.
        epoch: CanonicalUuid,
        /// Selected child stream carrying the declaration.
        stream: String,
        /// Exact resumable vendor identity.
        identity: String,
        /// Durable bytes carrying the declaration.
        locus: TapeLocus,
    },
    /// One typed compaction boundary recognized from selected-stream bytes.
    Compaction {
        /// Occupant that crossed the boundary.
        occupant: CanonicalUuid,
        /// Selected child stream carrying the boundary.
        stream: String,
        /// Durable bytes carrying the boundary.
        locus: TapeLocus,
    },
    /// Redacted steering lineage or disposition evidence.
    Steering {
        /// Directive identity, absent for terminal facts whose body carries only its key.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::optional::present"
        )]
        directive: Option<CanonicalUuid>,
        /// Stable directive key.
        key: String,
        /// Current disposition established by the fact.
        disposition: Disposition,
        /// Prior key named by a resend, when this is a lineage rung.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::optional::present"
        )]
        ancestor: Option<String>,
        /// Resident generation authenticating transcript evidence, when applicable.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::optional::present"
        )]
        epoch: Option<CanonicalUuid>,
        /// Exact assistant-prose evidence, absent for send, resend, or cancellation.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::optional::present"
        )]
        locus: Option<TranscriptLocus>,
    },
    /// Successor ancestry without exposing the operating-system process group.
    Renewal {
        /// Successor occupant.
        occupant: CanonicalUuid,
        /// Occupant the successor descends from.
        ancestor: CanonicalUuid,
        /// Resident generation that admitted the successor.
        epoch: CanonicalUuid,
    },
}

/// Ranked, bounded continuity account served at one journal barrier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecallAnswer {
    /// Composite journal barrier every field was derived from.
    pub barrier: Seq,
    /// Canonical repository worktree serving the account.
    pub repository: String,
    /// Requested lane, absent for an operator-wide account.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub lane: Option<CanonicalUuid>,
    /// Current resident and lane state.
    pub residency: LaneAnswer,
    /// Most recent relevant events, newest first and explicitly bounded.
    pub recent: Vec<RecallEntry>,
}

/// The answer a taste verdict returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TasteAnswer {
    /// Bag the verdict landed in.
    pub name: Name,
    /// Whether the judge confirmed the hit.
    pub confirmed: bool,
}

/// The answer a recognition returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecognitionAnswer {
    /// Observation key that was recognized.
    pub key: Digest,
    /// False when the key had already been recognized.
    pub fresh: bool,
}

/// Which intent DTO a command kind's body decodes into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BodyShape {
    /// [`BoardRead`]
    BoardRead,
    /// [`BoardGraph`]
    BoardGraph,
    /// [`BoardHandout`]
    BoardHandout,
    /// [`DocLedger`]
    DocLedger,
    /// [`TaskAdd`]
    TaskAdd,
    /// [`TaskEdit`]
    TaskEdit,
    /// [`TaskNote`]
    TaskNote,
    /// [`TaskDefer`]
    TaskDefer,
    /// [`TaskRef`]
    TaskRef,
    /// [`TaskDepends`]
    TaskDepends,
    /// [`TaskPhase`]
    TaskPhase,
    /// [`TaskDone`]
    TaskDone,
    /// [`TaskReview`]
    TaskReview,
    /// [`TaskAssign`]
    TaskAssign,
    /// [`ClaimWin`]
    ClaimWin,
    /// [`ClaimCarry`]
    ClaimCarry,
    /// [`DocApply`]
    DocApply,
    /// [`SteerSend`]
    SteerSend,
    /// [`SteerInbox`]
    SteerInbox,
    /// [`DirectiveRef`]
    DirectiveRef,
    /// [`SteerDispose`]
    SteerDispose,
    /// [`ConsentDecide`]
    ConsentDecide,
    /// [`ConsentRead`]
    ConsentRead,
    /// [`ConsentPolicy`]
    ConsentPolicy,
    /// [`TrustRef`]
    TrustRef,
    /// [`LaneRead`]
    LaneRead,
    /// [`LaneSeat`]
    LaneSeat,
    /// [`LaneState`]
    LaneState,
    /// [`LaneRecall`]
    LaneRecall,
    /// [`TasteRecord`]
    TasteRecord,
    /// [`Recognize`]
    Recognize,
    /// [`ReleasePublish`]
    ReleasePublish,
    /// [`UpChange`]
    UpChange,
}

/// Which answer DTO a command kind's reply carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AnswerShape {
    /// [`BoardAnswer`]
    Board,
    /// [`TextAnswer`]
    Text,
    /// [`TaskAnswer`]
    Task,
    /// [`ClaimAnswer`]
    Claim,
    /// [`DocAnswer`]
    Doc,
    /// [`DirectiveAnswer`]
    Directive,
    /// [`InboxAnswer`]
    Inbox,
    /// [`ConsentAnswer`]
    Consent,
    /// [`ConsentQueueAnswer`]
    ConsentQueue,
    /// [`ConsentPolicyAnswer`]
    ConsentPolicy,
    /// [`TrustAnswer`]
    Trust,
    /// [`LaneAnswer`]
    Lane,
    /// [`LaneChangeAnswer`]
    LaneChange,
    /// [`StateAnswer`]
    State,
    /// [`RecallAnswer`]
    Recall,
    /// [`TasteAnswer`]
    Taste,
    /// [`RecognitionAnswer`]
    Recognition,
}

/// One decoded successful reply body, tagged by the shape its command selected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum CommandAnswer {
    /// [`AnswerShape::Board`]
    Board(Box<BoardAnswer>),
    /// [`AnswerShape::Text`]
    Text(TextAnswer),
    /// [`AnswerShape::Task`]
    Task(TaskAnswer),
    /// [`AnswerShape::Claim`]
    Claim(ClaimAnswer),
    /// [`AnswerShape::Doc`]
    Doc(DocAnswer),
    /// [`AnswerShape::Directive`]
    Directive(DirectiveAnswer),
    /// [`AnswerShape::Inbox`]
    Inbox(InboxAnswer),
    /// [`AnswerShape::Consent`]
    Consent(ConsentAnswer),
    /// [`AnswerShape::ConsentQueue`]
    ConsentQueue(ConsentQueueAnswer),
    /// [`AnswerShape::ConsentPolicy`]
    ConsentPolicy(ConsentPolicyAnswer),
    /// [`AnswerShape::Trust`]
    Trust(TrustAnswer),
    /// [`AnswerShape::Lane`]
    Lane(Box<LaneAnswer>),
    /// [`AnswerShape::LaneChange`]
    LaneChange(LaneChangeAnswer),
    /// [`AnswerShape::State`]
    State(StateAnswer),
    /// [`AnswerShape::Recall`]
    Recall(Box<RecallAnswer>),
    /// [`AnswerShape::Taste`]
    Taste(TasteAnswer),
    /// [`AnswerShape::Recognition`]
    Recognition(RecognitionAnswer),
}

impl CommandAnswer {
    /// The shape this decoded answer carries.
    #[must_use]
    pub const fn shape(&self) -> AnswerShape {
        match self {
            Self::Board(_) => AnswerShape::Board,
            Self::Text(_) => AnswerShape::Text,
            Self::Task(_) => AnswerShape::Task,
            Self::Claim(_) => AnswerShape::Claim,
            Self::Doc(_) => AnswerShape::Doc,
            Self::Directive(_) => AnswerShape::Directive,
            Self::Inbox(_) => AnswerShape::Inbox,
            Self::Consent(_) => AnswerShape::Consent,
            Self::ConsentQueue(_) => AnswerShape::ConsentQueue,
            Self::ConsentPolicy(_) => AnswerShape::ConsentPolicy,
            Self::Trust(_) => AnswerShape::Trust,
            Self::Lane(_) => AnswerShape::Lane,
            Self::LaneChange(_) => AnswerShape::LaneChange,
            Self::State(_) => AnswerShape::State,
            Self::Recall(_) => AnswerShape::Recall,
            Self::Taste(_) => AnswerShape::Taste,
            Self::Recognition(_) => AnswerShape::Recognition,
        }
    }
}

impl AnswerShape {
    /// Decode a successful reply body into the answer this shape names.
    ///
    /// # Errors
    ///
    /// Returns the decoder error when the bytes are not that shape. Answers
    /// refuse unknown fields, so a body spelled for the wrong command is a
    /// decode failure rather than a silently truncated value.
    pub fn decode(self, bytes: &[u8]) -> Result<CommandAnswer, serde_json::Error> {
        Ok(match self {
            Self::Board => CommandAnswer::Board(Box::new(crate::decode(bytes)?)),
            Self::Text => CommandAnswer::Text(crate::decode(bytes)?),
            Self::Task => CommandAnswer::Task(crate::decode(bytes)?),
            Self::Claim => CommandAnswer::Claim(crate::decode(bytes)?),
            Self::Doc => CommandAnswer::Doc(crate::decode(bytes)?),
            Self::Directive => CommandAnswer::Directive(crate::decode(bytes)?),
            Self::Inbox => CommandAnswer::Inbox(crate::decode(bytes)?),
            Self::Consent => CommandAnswer::Consent(crate::decode(bytes)?),
            Self::ConsentQueue => CommandAnswer::ConsentQueue(crate::decode(bytes)?),
            Self::ConsentPolicy => CommandAnswer::ConsentPolicy(crate::decode(bytes)?),
            Self::Trust => CommandAnswer::Trust(crate::decode(bytes)?),
            Self::Lane => CommandAnswer::Lane(Box::new(crate::decode(bytes)?)),
            Self::LaneChange => CommandAnswer::LaneChange(crate::decode(bytes)?),
            Self::State => CommandAnswer::State(crate::decode(bytes)?),
            Self::Recall => CommandAnswer::Recall(Box::new(crate::decode(bytes)?)),
            Self::Taste => CommandAnswer::Taste(crate::decode(bytes)?),
            Self::Recognition => CommandAnswer::Recognition(crate::decode(bytes)?),
        })
    }
}

/// The body and answer a command kind selects.
///
/// WIRE binds these together with the witness schema and the transition, so
/// they are read off one discriminant rather than three independent lookups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// Intent DTO the caller sends.
    pub body: BodyShape,
    /// Answer DTO a successful reply carries.
    pub answer: AnswerShape,
}

impl Binding {
    /// Bind a body to an answer.
    const fn of(body: BodyShape, answer: AnswerShape) -> Self {
        Self { body, answer }
    }
}

impl CommandKind {
    /// The body and answer this kind selects.
    ///
    /// Exhaustive by construction: a kind added without a row here does not
    /// compile, which is the same law the witness registry carries.
    #[must_use]
    pub const fn binding(self) -> Binding {
        use AnswerShape as A;
        use BodyShape as B;

        match self {
            Self::BoardRead => Binding::of(B::BoardRead, A::Board),
            Self::BoardGraph => Binding::of(B::BoardGraph, A::Text),
            Self::BoardHandout => Binding::of(B::BoardHandout, A::Text),
            Self::DocLedger => Binding::of(B::DocLedger, A::Text),
            Self::TaskAdd => Binding::of(B::TaskAdd, A::Task),
            Self::TaskEdit => Binding::of(B::TaskEdit, A::Task),
            Self::TaskNote => Binding::of(B::TaskNote, A::Task),
            Self::TaskDefer => Binding::of(B::TaskDefer, A::Task),
            Self::TaskWake => Binding::of(B::TaskRef, A::Task),
            Self::TaskDepends => Binding::of(B::TaskDepends, A::Task),
            Self::TaskPhase => Binding::of(B::TaskPhase, A::Task),
            Self::TaskDone => Binding::of(B::TaskDone, A::Task),
            Self::TaskReview => Binding::of(B::TaskReview, A::Task),
            Self::TaskAssign => Binding::of(B::TaskAssign, A::Task),
            Self::ClaimWin => Binding::of(B::ClaimWin, A::Claim),
            Self::ClaimCarry => Binding::of(B::ClaimCarry, A::Claim),
            Self::ClaimRenew | Self::ClaimFree | Self::ClaimLapse => {
                Binding::of(B::TaskRef, A::Claim)
            }
            Self::DocApply => Binding::of(B::DocApply, A::Doc),
            Self::SteerSend => Binding::of(B::SteerSend, A::Directive),
            Self::SteerInbox => Binding::of(B::SteerInbox, A::Inbox),
            Self::SteerResend | Self::SteerCancel => Binding::of(B::DirectiveRef, A::Directive),
            Self::SteerDispose => Binding::of(B::SteerDispose, A::Directive),
            Self::ConsentDecide => Binding::of(B::ConsentDecide, A::Consent),
            Self::ConsentRead => Binding::of(B::ConsentRead, A::ConsentQueue),
            Self::ConsentPolicy => Binding::of(B::ConsentPolicy, A::ConsentPolicy),
            Self::TrustPend | Self::TrustGrant | Self::TrustRevoke => {
                Binding::of(B::TrustRef, A::Trust)
            }
            Self::LaneRead => Binding::of(B::LaneRead, A::Lane),
            Self::LaneSeat => Binding::of(B::LaneSeat, A::LaneChange),
            Self::LaneState => Binding::of(B::LaneState, A::LaneChange),
            Self::LaneRecall => Binding::of(B::LaneRecall, A::Recall),
            Self::TasteRecord => Binding::of(B::TasteRecord, A::Taste),
            Self::CanaryRecognize
            | Self::VendorRecognize
            | Self::CommitRecognize
            | Self::ProofRecognize => Binding::of(B::Recognize, A::Recognition),
            Self::ReleasePublish => Binding::of(B::ReleasePublish, A::Recognition),
            Self::UpChange => Binding::of(B::UpChange, A::State),
        }
    }
}

/// One decoded command body, tagged by the shape its kind selected.
///
/// The resident decodes a frame before it knows which handler runs, so the
/// union exists to carry a body that is already typed but not yet dispatched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Intent {
    /// [`BodyShape::BoardRead`]
    BoardRead(BoardRead),
    /// [`BodyShape::BoardGraph`]
    BoardGraph(BoardGraph),
    /// [`BodyShape::BoardHandout`]
    BoardHandout(BoardHandout),
    /// [`BodyShape::DocLedger`]
    DocLedger(DocLedger),
    /// [`BodyShape::TaskAdd`]
    TaskAdd(TaskAdd),
    /// [`BodyShape::TaskEdit`]
    TaskEdit(TaskEdit),
    /// [`BodyShape::TaskNote`]
    TaskNote(TaskNote),
    /// [`BodyShape::TaskDefer`]
    TaskDefer(TaskDefer),
    /// [`BodyShape::TaskRef`]
    TaskRef(TaskRef),
    /// [`BodyShape::TaskDepends`]
    TaskDepends(TaskDepends),
    /// [`BodyShape::TaskPhase`]
    TaskPhase(TaskPhase),
    /// [`BodyShape::TaskDone`]
    TaskDone(TaskDone),
    /// [`BodyShape::TaskReview`]
    TaskReview(TaskReview),
    /// [`BodyShape::TaskAssign`]
    TaskAssign(TaskAssign),
    /// [`BodyShape::ClaimWin`]
    ClaimWin(ClaimWin),
    /// [`BodyShape::ClaimCarry`]
    ClaimCarry(ClaimCarry),
    /// [`BodyShape::DocApply`]
    DocApply(DocApply),
    /// [`BodyShape::SteerSend`]
    SteerSend(SteerSend),
    /// [`BodyShape::SteerInbox`]
    SteerInbox(SteerInbox),
    /// [`BodyShape::DirectiveRef`]
    DirectiveRef(DirectiveRef),
    /// [`BodyShape::SteerDispose`]
    SteerDispose(SteerDispose),
    /// [`BodyShape::ConsentDecide`]
    ConsentDecide(ConsentDecide),
    /// [`BodyShape::ConsentRead`]
    ConsentRead(ConsentRead),
    /// [`BodyShape::ConsentPolicy`]
    ConsentPolicy(ConsentPolicy),
    /// [`BodyShape::TrustRef`]
    TrustRef(TrustRef),
    /// [`BodyShape::LaneRead`]
    LaneRead(LaneRead),
    /// [`BodyShape::LaneSeat`]
    LaneSeat(LaneSeat),
    /// [`BodyShape::LaneState`]
    LaneState(LaneState),
    /// [`BodyShape::LaneRecall`]
    LaneRecall(LaneRecall),
    /// [`BodyShape::TasteRecord`]
    TasteRecord(TasteRecord),
    /// [`BodyShape::Recognize`]
    Recognize(Recognize),
    /// [`BodyShape::ReleasePublish`]
    ReleasePublish(ReleasePublish),
    /// [`BodyShape::UpChange`]
    UpChange(UpChange),
}

impl Intent {
    /// The shape this decoded body carries.
    #[must_use]
    pub const fn shape(&self) -> BodyShape {
        match self {
            Self::BoardRead(_) => BodyShape::BoardRead,
            Self::BoardGraph(_) => BodyShape::BoardGraph,
            Self::BoardHandout(_) => BodyShape::BoardHandout,
            Self::DocLedger(_) => BodyShape::DocLedger,
            Self::TaskAdd(_) => BodyShape::TaskAdd,
            Self::TaskEdit(_) => BodyShape::TaskEdit,
            Self::TaskNote(_) => BodyShape::TaskNote,
            Self::TaskDefer(_) => BodyShape::TaskDefer,
            Self::TaskRef(_) => BodyShape::TaskRef,
            Self::TaskDepends(_) => BodyShape::TaskDepends,
            Self::TaskPhase(_) => BodyShape::TaskPhase,
            Self::TaskDone(_) => BodyShape::TaskDone,
            Self::TaskReview(_) => BodyShape::TaskReview,
            Self::TaskAssign(_) => BodyShape::TaskAssign,
            Self::ClaimWin(_) => BodyShape::ClaimWin,
            Self::ClaimCarry(_) => BodyShape::ClaimCarry,
            Self::DocApply(_) => BodyShape::DocApply,
            Self::SteerSend(_) => BodyShape::SteerSend,
            Self::SteerInbox(_) => BodyShape::SteerInbox,
            Self::DirectiveRef(_) => BodyShape::DirectiveRef,
            Self::SteerDispose(_) => BodyShape::SteerDispose,
            Self::ConsentDecide(_) => BodyShape::ConsentDecide,
            Self::ConsentRead(_) => BodyShape::ConsentRead,
            Self::ConsentPolicy(_) => BodyShape::ConsentPolicy,
            Self::TrustRef(_) => BodyShape::TrustRef,
            Self::LaneRead(_) => BodyShape::LaneRead,
            Self::LaneSeat(_) => BodyShape::LaneSeat,
            Self::LaneState(_) => BodyShape::LaneState,
            Self::LaneRecall(_) => BodyShape::LaneRecall,
            Self::TasteRecord(_) => BodyShape::TasteRecord,
            Self::Recognize(_) => BodyShape::Recognize,
            Self::ReleasePublish(_) => BodyShape::ReleasePublish,
            Self::UpChange(_) => BodyShape::UpChange,
        }
    }
}

impl BodyShape {
    /// Decode a command body into the intent this shape names.
    ///
    /// # Errors
    ///
    /// Returns the decoder error when the bytes are not that shape. Bodies
    /// refuse unknown fields, so a body spelled for the wrong kind is a
    /// decode failure rather than a silently truncated value.
    pub fn decode(self, bytes: &[u8]) -> Result<Intent, serde_json::Error> {
        if self.is_board_body() {
            self.decode_board_body(bytes)
        } else {
            self.decode_runtime_body(bytes)
        }
    }

    const fn is_board_body(self) -> bool {
        matches!(
            self,
            Self::BoardRead
                | Self::BoardGraph
                | Self::BoardHandout
                | Self::DocLedger
                | Self::TaskAdd
                | Self::TaskEdit
                | Self::TaskNote
                | Self::TaskDefer
                | Self::TaskRef
                | Self::TaskDepends
                | Self::TaskPhase
                | Self::TaskDone
                | Self::TaskReview
                | Self::TaskAssign
                | Self::ClaimWin
                | Self::ClaimCarry
                | Self::DocApply
        )
    }

    fn decode_board_body(self, bytes: &[u8]) -> Result<Intent, serde_json::Error> {
        Ok(match self {
            Self::BoardRead => Intent::BoardRead(crate::decode(bytes)?),
            Self::BoardGraph => Intent::BoardGraph(crate::decode(bytes)?),
            Self::BoardHandout => Intent::BoardHandout(crate::decode(bytes)?),
            Self::DocLedger => Intent::DocLedger(crate::decode(bytes)?),
            Self::TaskAdd => Intent::TaskAdd(crate::decode(bytes)?),
            Self::TaskEdit => Intent::TaskEdit(crate::decode(bytes)?),
            Self::TaskNote => Intent::TaskNote(crate::decode(bytes)?),
            Self::TaskDefer => Intent::TaskDefer(crate::decode(bytes)?),
            Self::TaskRef => Intent::TaskRef(crate::decode(bytes)?),
            Self::TaskDepends => Intent::TaskDepends(crate::decode(bytes)?),
            Self::TaskPhase => Intent::TaskPhase(crate::decode(bytes)?),
            Self::TaskDone => Intent::TaskDone(crate::decode(bytes)?),
            Self::TaskReview => Intent::TaskReview(crate::decode(bytes)?),
            Self::TaskAssign => Intent::TaskAssign(crate::decode(bytes)?),
            Self::ClaimWin => Intent::ClaimWin(crate::decode(bytes)?),
            Self::ClaimCarry => Intent::ClaimCarry(crate::decode(bytes)?),
            Self::DocApply => Intent::DocApply(crate::decode(bytes)?),
            _ => unreachable!("board body partition is closed"),
        })
    }

    fn decode_runtime_body(self, bytes: &[u8]) -> Result<Intent, serde_json::Error> {
        Ok(match self {
            Self::SteerSend => Intent::SteerSend(crate::decode(bytes)?),
            Self::SteerInbox => Intent::SteerInbox(crate::decode(bytes)?),
            Self::DirectiveRef => Intent::DirectiveRef(crate::decode(bytes)?),
            Self::SteerDispose => Intent::SteerDispose(crate::decode(bytes)?),
            Self::ConsentDecide => Intent::ConsentDecide(crate::decode(bytes)?),
            Self::ConsentRead => Intent::ConsentRead(crate::decode(bytes)?),
            Self::ConsentPolicy => Intent::ConsentPolicy(crate::decode(bytes)?),
            Self::TrustRef => Intent::TrustRef(crate::decode(bytes)?),
            Self::LaneRead => Intent::LaneRead(crate::decode(bytes)?),
            Self::LaneSeat => Intent::LaneSeat(crate::decode(bytes)?),
            Self::LaneState => Intent::LaneState(crate::decode(bytes)?),
            Self::LaneRecall => Intent::LaneRecall(crate::decode(bytes)?),
            Self::TasteRecord => Intent::TasteRecord(crate::decode(bytes)?),
            Self::Recognize => Intent::Recognize(crate::decode(bytes)?),
            Self::ReleasePublish => Intent::ReleasePublish(crate::decode(bytes)?),
            Self::UpChange => Intent::UpChange(crate::decode(bytes)?),
            _ => unreachable!("runtime body partition is closed"),
        })
    }
}
