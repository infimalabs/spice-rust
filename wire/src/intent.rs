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
//! Several kinds share one shape on purpose. Freeing and lapsing a claim both
//! name a task and nothing else, so they share [`TaskRef`]; renewal has its
//! own shape because it also names the requested lease duration.

use serde::{Deserialize, Serialize};

use crate::body::{ConsentQueueCause, DirectiveAttachment, TranscriptLocus};
use crate::envelope::{Actor, Origin, Seq};
use crate::fact::FactKind;
use crate::head::{LaneResidency, ResidentState, TapeLocus, TrustState};
use crate::ident::{CanonicalUuid, Digest, Instant};
use crate::witness::{Answer, Disposition, Name, Phase, TaskHandle};

mod board;
pub use board::{
    BoardAnswer, BoardGraph, BoardHandout, BoardRead, BoardStatement, BoardTask, DocLedger,
    Positioned, ProjectVisibility, TextAnswer,
};

/// Add one task as direct intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskAdd {
    /// One-line statement of the work.
    pub title: String,
    /// One resolved public project, or absent for private work.
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
    /// Caller-authored creation provenance, or the new task itself when absent.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub origin: Option<Origin>,
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

/// Add one task through the fixed server-owned `.oops` route.
///
/// The route owns the project, flow, and dependency policy. None crosses the
/// wire, so a caller cannot turn this DTO into generic hidden-project creation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskOopsAdd {
    /// One-line account of the unexpected behavior.
    pub title: String,
    /// Durable detail attached as the first note.
    pub description: String,
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

/// Extend the authenticated actor's active claim within its durable maximum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimRenew {
    /// Task whose possession is refreshed.
    pub task: TaskHandle,
    /// Requested positive lease duration in whole seconds.
    pub lease_seconds: u64,
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
    /// Opaque typed attachments retained with the directive.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<DirectiveAttachment>,
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
    /// Independently steerable member seat being created.
    pub lane: CanonicalUuid,
    /// Canvas lineage whose lane receives the member.
    pub lineage_id: CanonicalUuid,
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
    /// Fresh linked worktree required only by import.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub worktree: Option<String>,
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
    /// Exact attachments retained with the directive.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<DirectiveAttachment>,
    /// Instant at which the directive lapses.
    pub expires: Instant,
}

/// One durable assistant-authored message returned by an inbox read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentMessage {
    /// Journal position that uniquely identifies and orders the message.
    pub seq: Seq,
    /// Instant at which the resident committed the decoded prose.
    pub at: Instant,
    /// Independently steerable member that produced the message.
    pub producer: CanonicalUuid,
    /// Exact occupant that authored the message.
    pub occupant: CanonicalUuid,
    /// Canonical worktree occupied when the message was authored.
    pub worktree: String,
    /// Assistant-authored text.
    pub text: String,
    /// Vendor phase when its current dialect reports one.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub phase: Option<String>,
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
    /// Durable assistant prose in journal order.
    pub messages: Vec<AgentMessage>,
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

mod binding;
pub use binding::{AnswerShape, Binding, BodyShape, CommandAnswer, Intent};
