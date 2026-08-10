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
//! name a task and nothing else, so they share
//! [`TaskRef`](crate::intent::TaskRef); renewal has its own shape because it
//! also names the requested lease duration.

// That link is crate-rooted, not bare, and has to stay that way. This module is
// documented from two places: `pub mod intent;` in `lib.rs` carries an outer
// comment as well, and rustdoc resolves the merged text in the scope that
// declaration sits in — the crate root, where a bare `TaskRef` is not in scope
// precisely because that outer comment's subject is that this module is
// deliberately not re-exported flat.

use serde::{Deserialize, Serialize};

use crate::body::{ConsentQueueCause, DirectiveAttachment, TranscriptLocus};
use crate::envelope::{Actor, Origin, Seq};
use crate::fact::FactKind;
use crate::head::{ResidentState, TapeLocus, TrustState};
use crate::ident::{CanonicalUuid, Digest, Instant, ReleaseNotesIdentity};
use crate::witness::{Answer, Disposition, Name, TaskHandle};

mod board;
pub use board::{
    BoardActorRoute, BoardAnswer, BoardGraph, BoardHandout, BoardProjectPolicy, BoardRead,
    BoardStatement, BoardTask, DocLedger, Positioned, ProjectVisibility, TextAnswer,
};

mod task;
pub use task::{
    ClaimCarry, ClaimRenew, ClaimWin, DocApply, EffortAggregateAnswer, EffortAnswer,
    EffortAvailability, EffortIdentity, EffortPhaseAnswer, EffortRead, EffortSegmentAnswer,
    EffortUnavailable, EffortUnavailableCode, EffortUsage, GateFindings, ReviewFollowup, TaskAdd,
    TaskAssign, TaskDefer, TaskDelete, TaskDepends, TaskDone, TaskEdit, TaskNote, TaskOopsAdd,
    TaskPhase, TaskRef, TaskReview,
};

/// Append one durable directive to a lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SteerSend {
    /// Lane the directive lands on.
    pub lane: CanonicalUuid,
    /// The intent being sent.
    pub message: String,
    /// Admitted browsable images retained with the directive.
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

/// Rebuild one lane's disposable steering delivery from durable authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SteerRepair {
    /// Lane whose current journal-derived rendering is republished.
    pub lane: CanonicalUuid,
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
    /// Topology revision observed before this seat was requested.
    pub topology_revision: u64,
    /// Independently steerable member seat being created.
    pub lane: CanonicalUuid,
    /// Canvas lineage whose lane receives the member.
    pub lineage_id: CanonicalUuid,
    /// Operator-configured agent display name, absent when unconfigured.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub name: Option<String>,
    /// Worktree path the operator created.
    pub worktree: String,
    /// Closed lane-life spelling selected for this seat.
    pub life: String,
    /// Vendor selected for this seat.
    pub vendor: Name,
    /// Existing vendor session identity to resume, absent for a fresh launch.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub resume: Option<String>,
}

/// The closed task-subscription policy selected for a Canvas lineage.
///
/// This is task-subscription policy. It is deliberately distinct from
/// [`LaneAction::Drain`], which tears down an occupant process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineageLife {
    /// The operator steers task selection explicitly.
    Steer,
    /// The lineage follows work it creates or claims.
    Drive,
    /// The lineage can draw from all assignable work.
    Drain,
}

impl LineageLife {
    /// Stable wire spelling retained by the Canvas control.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Steer => "steer",
            Self::Drive => "drive",
            Self::Drain => "drain",
        }
    }

    /// Task-selection scope this policy authorizes for the lineage.
    #[must_use]
    pub const fn scope(self) -> TaskSubscriptionScope {
        match self {
            Self::Steer => TaskSubscriptionScope::Manual,
            Self::Drive => TaskSubscriptionScope::ManualAndAutoProject,
            Self::Drain => TaskSubscriptionScope::AllAssignable,
        }
    }

    /// Parse one stable task-subscription policy spelling.
    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "steer" => Some(Self::Steer),
            "drive" => Some(Self::Drive),
            "drain" => Some(Self::Drain),
            _ => None,
        }
    }
}

/// Closed task-selection scope represented by one lineage subscription policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskSubscriptionScope {
    /// Use only the task filters the operator stored explicitly.
    Manual,
    /// Use stored filters and automatically follow projects the lineage creates or claims.
    ManualAndAutoProject,
    /// Dissolve the lineage boundary and admit every assignable task.
    AllAssignable,
}

/// Change the task-subscription policy shared by one Canvas lineage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineageConfigure {
    /// Topology revision observed before this configuration was requested.
    pub topology_revision: u64,
    /// Stable Canvas lineage identifying the group.
    pub lineage_id: CanonicalUuid,
    /// One seated member to move into this lineage, or absent to preserve membership.
    ///
    /// Moving a member to a fresh lineage creates a team. Moving it to its own
    /// lane identity removes it from a team. The resident keeps every other
    /// member and subscription authoritative in the same transaction.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub member: Option<CanonicalUuid>,
    /// Authoritative task-subscription policy to commit for the lineage.
    pub life: LineageLife,
    /// Replacement operator-pinned project set, or absent to preserve it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub manual_projects: Option<Vec<String>>,
}

/// One Canvas lineage and the seated lanes it holds, in operator order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineageArrangement {
    /// Stable Canvas lineage identifying the group.
    pub lineage_id: CanonicalUuid,
    /// Every lane this lineage holds after the arrangement commits.
    pub lanes: Vec<CanonicalUuid>,
}

/// Rearrange every Canvas lineage: their order, and which lanes each holds.
///
/// The operator drags one team lane to reorder the teams, or drops one team
/// lane onto another to join them. Both gestures are the same durable
/// commitment -- a complete restatement of the topology -- so the authority
/// never has to reconcile a reorder against a concurrent join. Stating the
/// whole arrangement rather than one delta is what makes the two gestures a
/// single law: a request that names a lane twice, drops one, or invents one is
/// refused as a whole rather than half-applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineageArrange {
    /// Topology revision of the complete arrangement being restated.
    pub topology_revision: u64,
    /// Every surviving lineage, in the order the operator arranged them.
    pub lineages: Vec<LineageArrangement>,
}

/// Place one seated lane immediately before another seated lane.
///
/// The target lane supplies both the durable insertion point and the target
/// lineage. Naming one target instead of a client-computed index keeps the
/// command meaningful when UUID order and visual order differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneArrange {
    /// Topology revision observed before this member move was requested.
    pub topology_revision: u64,
    /// Existing member whose primary composer moves.
    pub lane: CanonicalUuid,
    /// Existing member the moved lane is placed immediately before.
    pub before: CanonicalUuid,
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
    /// Identity of the exact curated release-note bytes in the approved plan.
    ///
    /// `HistoricalAbsent` represents only a command receipt migrated from
    /// journal schema 4; the live release door refuses it before reading staged
    /// publication data. The key is required even when its historical value is
    /// absent.
    pub notes: ReleaseNotesIdentity,
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
    /// Best-effort learning continuation run after a successful completion.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub learning: Option<Box<TaskLearning>>,
    /// Exact CLI command admitted as the next action, when one is available.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub next: Option<Box<str>>,
}

/// Bounded task-completion learning distillation report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskLearning {
    /// Records atomically retained after confirmation and eviction.
    pub stored: u64,
    /// Bounded candidates extracted from the exact claim-to-completion span.
    pub candidates: u64,
    /// One reason for each candidate the independent judge skipped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
    /// Whole-operation skip reason when no record could be retained.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub reason: Option<String>,
    /// Bounded diagnostic detail for the skip, when useful.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub detail: Option<String>,
}

/// The answer a claim command returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimAnswer {
    /// Task whose possession moved.
    pub task: TaskHandle,
    /// Actor holding it after the transaction.
    pub holder: Actor,
    /// Non-blocking overlap warnings discovered from the claim-time board.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<ClaimWarning>,
    /// Exact CLI command admitted as the next action, when one is available.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub next: Option<Box<str>>,
}

/// One active claim whose acceptance names the same implementation surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimWarning {
    /// Other in-flight task.
    pub task: TaskHandle,
    /// Lane holding the other claim.
    pub lane: CanonicalUuid,
    /// Exact normalized acceptance terms shared by both tasks.
    pub surface: Vec<String>,
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
    /// Current standing of the disposable inbox delivery after the command.
    #[serde(default, skip_serializing_if = "DeliveryStanding::is_unknown")]
    pub delivery: Box<DeliveryStanding>,
}

/// Observable standing of one journal-to-inbox fold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "standing", rename_all = "lowercase", deny_unknown_fields)]
pub enum DeliveryStanding {
    /// No admitted progress record exists for this lane yet.
    #[default]
    Unknown,
    /// The rendering at `source` was durably published.
    Published {
        /// Journal horizon from which the rendering was derived.
        source: Seq,
    },
    /// Publication failed and the exact current work is parked for repair.
    Parked {
        /// Journal horizon from which the failed rendering was derived.
        source: Seq,
        /// Pending steering keys the failed rendering would have carried.
        keys: Vec<String>,
        /// Publication failure retained verbatim for diagnosis.
        failure: String,
        /// Exact operator command that retries from current authority.
        repair: String,
    },
}

impl DeliveryStanding {
    /// True only before an observable fold record has been consulted.
    #[must_use]
    pub const fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }
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
    /// First line authored outside quoted context, when the producer found one.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub preview: Option<String>,
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
    /// Commit instant of the original publication.
    pub authored: Instant,
    /// Current presentation urgency on the stable directive.
    pub stage: crate::body::SteeringStage,
    /// Complete ordered presentation-transition audit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub escalations: Vec<crate::body::SteeringTransition>,
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
    /// Current durable progress or degradation record for the lane rendering.
    #[serde(default, skip_serializing_if = "DeliveryStanding::is_unknown")]
    pub delivery: DeliveryStanding,
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

/// Result of explicitly repairing one lane's inbox fold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryAnswer {
    /// Lane whose rendering was rebuilt.
    pub lane: CanonicalUuid,
    /// Standing recorded only after publication or parking completed.
    pub delivery: Box<DeliveryStanding>,
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

/// Dynamic liveness of one lane at the read barrier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LaneLiveness {
    /// The occupant was seated or emitted selected-record activity recently.
    Running,
    /// The supervised occupant has been quiet for the complete stale horizon.
    RunningStale,
    /// No occupant is supervised in this durable seat.
    Vacant,
}

impl LaneLiveness {
    /// Exact operator-facing wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::RunningStale => "running-stale",
            Self::Vacant => "vacant",
        }
    }
}

/// One capability-free lane row returned by [`LaneRead`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneAnswerRow {
    /// Stable independently steerable member identity.
    pub lane: CanonicalUuid,
    /// Canvas lineage whose lane contains this member.
    pub lineage_id: CanonicalUuid,
    /// Operator-configured agent display name, absent when unconfigured.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub name: Option<String>,
    /// Canonical worktree bound to the lane.
    pub worktree: String,
    /// Closed lane-life spelling governing re-seating.
    pub life: String,
    /// Vendor selected for occupants of this lane.
    pub vendor: Name,
    /// Concrete model supplied to the declared session's vendor launch.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub model: Option<String>,
    /// Reasoning effort supplied to the declared session's vendor launch.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub effort: Option<String>,
    /// Observable GIT-04 outcome for the declared occupant launch.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub git: Option<String>,
    /// Current occupant, absent between seats.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub occupant: Option<CanonicalUuid>,
    /// Occupant ancestry, oldest first.
    pub lineage: Vec<CanonicalUuid>,
    /// Exact resumable vendor session identity, absent before declaration.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub session: Option<String>,
    /// Whether this seat has resumable session authority.
    pub bound: bool,
    /// Durable occupancy disposition.
    pub disposition: String,
    /// Dynamic supervised live/quiet projection at this read barrier.
    pub liveness: LaneLiveness,
}

/// Current lane state read at one resident barrier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneAnswer {
    /// Resident generation that owns the table.
    pub epoch: CanonicalUuid,
    /// Resident state at the read barrier.
    pub state: ResidentState,
    /// Monotonic authority for this exact topology and policy snapshot.
    pub topology_revision: u64,
    /// Current lane rows in stable lane order.
    pub lanes: Vec<LaneAnswerRow>,
    /// One authoritative task-subscription policy per returned lineage.
    pub subscriptions: Vec<crate::LineageSubscription>,
}

/// The answer a lane seating or lifecycle command returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneChangeAnswer {
    /// Lane whose disposition changed.
    pub lane: CanonicalUuid,
    /// Topology authority after the transaction committed.
    pub topology_revision: u64,
    /// Disposition held before the transaction.
    pub from: String,
    /// Disposition held after the transaction.
    pub to: String,
    /// GIT-04 outcome for a launch transition; absent for non-launch changes.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub git: Option<Box<str>>,
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
        /// Stable directive identity, absent only from legacy unprojected provenance.
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
        /// Event-local stage, or the preserved current stage on terminal facts.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::optional::present"
        )]
        stage: Option<crate::body::SteeringStage>,
        /// Complete ordered presentation-transition audit retained by the stable directive.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        escalations: Vec<crate::body::SteeringTransition>,
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
