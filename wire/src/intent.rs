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

use crate::body::ConsentQueueCause;
use crate::command::CommandKind;
use crate::envelope::Actor;
use crate::head::{ResidentState, TrustState};
use crate::ident::{CanonicalUuid, Digest, Instant};
use crate::witness::{Answer, Disposition, Name, Phase, TaskHandle};

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
    /// Directive being retired.
    pub directive: CanonicalUuid,
    /// The typed terminal outcome.
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

/// Seat a lane in an operator-created worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneSeat {
    /// Lane being seated.
    pub lane: CanonicalUuid,
    /// Worktree path the operator created.
    pub worktree: String,
}

/// Change a seated lane's state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneState {
    /// Lane whose state moves.
    pub lane: CanonicalUuid,
    /// State it moves to.
    pub state: ResidentState,
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
    /// [`LaneSeat`]
    LaneSeat,
    /// [`LaneState`]
    LaneState,
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
    /// [`StateAnswer`]
    State,
    /// [`TasteAnswer`]
    Taste,
    /// [`RecognitionAnswer`]
    Recognition,
}

/// One decoded successful reply body, tagged by the shape its command selected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum CommandAnswer {
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
    /// [`AnswerShape::State`]
    State(StateAnswer),
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
            Self::Task(_) => AnswerShape::Task,
            Self::Claim(_) => AnswerShape::Claim,
            Self::Doc(_) => AnswerShape::Doc,
            Self::Directive(_) => AnswerShape::Directive,
            Self::Inbox(_) => AnswerShape::Inbox,
            Self::Consent(_) => AnswerShape::Consent,
            Self::ConsentQueue(_) => AnswerShape::ConsentQueue,
            Self::ConsentPolicy(_) => AnswerShape::ConsentPolicy,
            Self::Trust(_) => AnswerShape::Trust,
            Self::State(_) => AnswerShape::State,
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
            Self::Task => CommandAnswer::Task(crate::decode(bytes)?),
            Self::Claim => CommandAnswer::Claim(crate::decode(bytes)?),
            Self::Doc => CommandAnswer::Doc(crate::decode(bytes)?),
            Self::Directive => CommandAnswer::Directive(crate::decode(bytes)?),
            Self::Inbox => CommandAnswer::Inbox(crate::decode(bytes)?),
            Self::Consent => CommandAnswer::Consent(crate::decode(bytes)?),
            Self::ConsentQueue => CommandAnswer::ConsentQueue(crate::decode(bytes)?),
            Self::ConsentPolicy => CommandAnswer::ConsentPolicy(crate::decode(bytes)?),
            Self::Trust => CommandAnswer::Trust(crate::decode(bytes)?),
            Self::State => CommandAnswer::State(crate::decode(bytes)?),
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
            Self::LaneSeat => Binding::of(B::LaneSeat, A::State),
            Self::LaneState => Binding::of(B::LaneState, A::State),
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
    /// [`BodyShape::LaneSeat`]
    LaneSeat(LaneSeat),
    /// [`BodyShape::LaneState`]
    LaneState(LaneState),
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
            Self::LaneSeat(_) => BodyShape::LaneSeat,
            Self::LaneState(_) => BodyShape::LaneState,
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
        Ok(match self {
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
            Self::SteerSend => Intent::SteerSend(crate::decode(bytes)?),
            Self::SteerInbox => Intent::SteerInbox(crate::decode(bytes)?),
            Self::DirectiveRef => Intent::DirectiveRef(crate::decode(bytes)?),
            Self::SteerDispose => Intent::SteerDispose(crate::decode(bytes)?),
            Self::ConsentDecide => Intent::ConsentDecide(crate::decode(bytes)?),
            Self::ConsentRead => Intent::ConsentRead(crate::decode(bytes)?),
            Self::ConsentPolicy => Intent::ConsentPolicy(crate::decode(bytes)?),
            Self::TrustRef => Intent::TrustRef(crate::decode(bytes)?),
            Self::LaneSeat => Intent::LaneSeat(crate::decode(bytes)?),
            Self::LaneState => Intent::LaneState(crate::decode(bytes)?),
            Self::TasteRecord => Intent::TasteRecord(crate::decode(bytes)?),
            Self::Recognize => Intent::Recognize(crate::decode(bytes)?),
            Self::ReleasePublish => Intent::ReleasePublish(crate::decode(bytes)?),
            Self::UpChange => Intent::UpChange(crate::decode(bytes)?),
        })
    }
}
