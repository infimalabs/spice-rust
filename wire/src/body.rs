//! One body DTO per fact kind.
//!
//! Every kind in the closed vocabulary registers exactly one body here, and the
//! registry in [`crate::fact`] binds them. A body's fields are the nouns its
//! WIRE.md vocabulary row names, or the head content that row points at; a
//! field with no anchor in that row does not belong in the body.
//!
//! Bodies flatten beside the discriminant, so their field names are the fact
//! object's own keys and cannot collide with `kind`.

use serde::{Deserialize, Serialize};

use crate::Name;
use crate::envelope::{Actor, Seq};
use crate::head::TranscriptSourceHead;
use crate::ident::{CanonicalUuid, Digest, Instant};
use crate::witness::{Phase, TaskHandle};

/// How a review closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// The reviewer found nothing to record.
    Clean,
    /// The reviewer recorded a finding.
    Finding,
}

/// What returned a held lane to seating eligibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Release {
    /// The operator woke the lane.
    Wake,
    /// The hold reached its deadline.
    Expiry,
}

/// A gate finding's standing weight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Recorded, not blocking.
    Low,
    /// Worth scheduling.
    Medium,
    /// Blocks the surface it names.
    High,
    /// Blocks the release.
    Critical,
}

/// A task statement and initial workflow head were created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskAdded {
    /// Handle the statement was minted under.
    pub task: TaskHandle,
    /// Durable identity behind the handle.
    pub uuid: CanonicalUuid,
    /// Statement title.
    pub title: String,
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
    /// Dotted project stem.
    pub project: String,
    /// Free tags carried on the statement.
    pub tags: Vec<String>,
    /// Tasks this one waits behind.
    pub depends: Vec<TaskHandle>,
    /// Ordered phases the workflow head will walk.
    pub flow: Vec<Phase>,
    /// Phase the initial workflow head sits in.
    pub phase: Phase,
    /// Statement revision the head opens at.
    pub revision: u32,
}

/// A claimed phase completed and workflow advanced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskPhased {
    /// Advanced task.
    pub task: TaskHandle,
    /// Phase the workflow head advanced into.
    pub phase: Phase,
}

/// Exact repository history selected at one task boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitBoundary {
    /// Lane commit that entered the boundary.
    pub head: String,
    /// Commit whose tree and ancestry left the boundary.
    pub integrated: String,
    /// Remote-tracking baseline used by the boundary, when one exists.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub upstream: Option<String>,
    /// Exact baseline commit incorporated by the boundary, when one exists.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub upstream_head: Option<String>,
    /// Lane commits ahead of the observed baseline when the boundary opened.
    pub local_commits: u32,
}

/// Accounted transcript proven at one task lifecycle boundary.
///
/// A phase interval is bounded by two of these, and the usage spent inside it
/// is whatever the accounted transcript advanced between them. The two states
/// are distinct keys rather than one nullable field, for the same reason
/// [`crate::ReplyPosition`] keeps three: a reader must never guess which one it
/// received. [`Self::Proven`] carrying no heads is a real boundary, because a
/// lane whose transcript has not been accounted yet opens at position zero,
/// while [`Self::Incomplete`] is the absence of an answer. Collapsing the two
/// would let a reducer read *no usage* where the truth is *unknown usage*.
///
/// The fields carrying this are not optional, so a reader always holds one of
/// the two states and never a third *absent* one. On the wire a fact stored
/// before boundaries were recorded is the omitted key, which decodes through
/// [`Self::unrecorded`] and encodes back to the same omission. That round trip
/// is not cosmetic: [`crate::AnyFact`] admits a body only when re-encoding it
/// reproduces the supplied fields exactly, so writing the migrated value out
/// instead would make every fact already in the journal undecodable. Absence
/// therefore spells exactly one value, and every boundary a producer can prove
/// or fail to prove is written explicitly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceBoundary {
    /// Every stream accounted for the lane when the boundary was fixed.
    #[serde(rename = "proven")]
    Proven {
        /// Accounted heads, ordered by composed stream key.
        heads: Vec<TranscriptSourceHead>,
    },
    /// The lane's accounted transcript could not be named at the boundary.
    #[serde(rename = "incomplete")]
    Incomplete {
        /// Why the boundary could not be proven.
        reason: String,
    },
}

impl SourceBoundary {
    /// Reason naming the shape a migrated boundary was decoded from.
    pub const UNRECORDED: &'static str =
        "recorded before task lifecycle facts carried source boundaries";

    /// Migrate a fact stored without a boundary field into an explicit one.
    ///
    /// The journal is append-only, so a fact already written can never grow the
    /// field; decode is the only place this migration can happen. It is a
    /// migration rather than a fallback because it is total, it produces a
    /// value that states in [`Self::UNRECORDED`] what it was migrated from, and
    /// it leaves no second spelling of *not proven* behind: a stored fact and a
    /// producer that could not name a boundary both arrive at
    /// [`Self::Incomplete`], differing only in the reason they carry.
    #[must_use]
    pub fn unrecorded() -> Self {
        Self::Incomplete {
            reason: Self::UNRECORDED.to_owned(),
        }
    }

    /// Whether this is the migrated stand-in for a fact stored without the field.
    ///
    /// Only the migration produces this value: a live producer either proves a
    /// boundary or names its own reason for failing to, so omitting the key on
    /// encode restores the exact bytes a stored fact arrived as and never
    /// discards an answer a producer actually gave.
    #[must_use]
    pub fn is_unrecorded(&self) -> bool {
        matches!(self, Self::Incomplete { reason } if reason == Self::UNRECORDED)
    }
}

/// A task completed under a re-proven active claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDone {
    /// Completed task.
    pub task: TaskHandle,
    /// Evidence the completion rests on.
    pub validation: String,
    /// Repository history published before the workflow advanced.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub git: Option<GitBoundary>,
    /// Accounted transcript the completed phase closes at.
    #[serde(
        default = "SourceBoundary::unrecorded",
        skip_serializing_if = "SourceBoundary::is_unrecorded"
    )]
    pub source: SourceBoundary,
}

/// A review verdict landed with reviewer and assignment witnesses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskReviewed {
    /// Reviewed task.
    pub task: TaskHandle,
    /// Principal that reviewed.
    pub reviewer: Actor,
    /// How the review closed.
    pub verdict: Verdict,
    /// Recorded finding; absent on a clean verdict.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub finding: Option<String>,
    /// Repository history published before the review completed.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub git: Option<GitBoundary>,
    /// Accounted transcript the reviewed phase closes at.
    #[serde(
        default = "SourceBoundary::unrecorded",
        skip_serializing_if = "SourceBoundary::is_unrecorded"
    )]
    pub source: SourceBoundary,
}

/// The allocator seated a reviewer under the anti-self-review witness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskAssigned {
    /// Assigned task.
    pub task: TaskHandle,
    /// Principal seated as reviewer.
    pub reviewer: Actor,
    /// Phase the assignment covers.
    pub phase: Phase,
}

/// A durable task note appended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskNoted {
    /// Annotated task.
    pub task: TaskHandle,
    /// Note text.
    pub note: String,
}

/// A task received a wakeable deferral.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDeferred {
    /// Deferred task.
    pub task: TaskHandle,
    /// Instant the deferral becomes wakeable.
    pub until: Instant,
}

/// A task's deferral cleared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskWoken {
    /// Woken task.
    pub task: TaskHandle,
}

/// An unsettled task statement changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEdited {
    /// Edited task.
    pub task: TaskHandle,
    /// Statement revision the edit produced.
    pub revision: u32,
}

/// A task was withdrawn from the board with a recorded reason.
///
/// The journal is the authority and a fact is permanent, so a withdrawal is a
/// terminal workflow status rather than the removal of a row: the statement,
/// its history, and this reason all stay readable. `claimed` records the
/// possessing actor when the withdrawal overrode a live claim, so a forced
/// removal names whose work it took away rather than reporting a bare success.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDeleted {
    /// Withdrawn task.
    pub task: TaskHandle,
    /// Operator's stated reason for the withdrawal.
    pub reason: String,
    /// Actor whose live claim the withdrawal overrode, when it overrode one.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub claimed: Option<Actor>,
}

/// Exclusive possession was minted from the current eligibility and claim heads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimWon {
    /// Possessed task.
    pub task: TaskHandle,
    /// Winning actor.
    pub actor: Actor,
    /// Lane the claim is bound to.
    pub lane: CanonicalUuid,
    /// Occupant holding the lane.
    pub occupant: CanonicalUuid,
    /// Resident epoch that minted the claim.
    pub epoch: CanonicalUuid,
    /// Lease expiry the possession runs to.
    pub until: Instant,
    /// Repository history fixed when possession was minted.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub git: Option<GitBoundary>,
    /// Accounted transcript the possessed phase opens at.
    #[serde(
        default = "SourceBoundary::unrecorded",
        skip_serializing_if = "SourceBoundary::is_unrecorded"
    )]
    pub source: SourceBoundary,
}

/// Possession passed to a successor with ancestor lineage preserved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimCarried {
    /// Carried task.
    pub task: TaskHandle,
    /// Lane the possession stays bound to.
    pub lane: CanonicalUuid,
    /// Successor occupant.
    pub occupant: CanonicalUuid,
    /// Occupant the possession came from.
    pub ancestor: CanonicalUuid,
    /// Resident epoch that carried it.
    pub epoch: CanonicalUuid,
    /// Accounted transcript the successor occupant takes over at.
    ///
    /// Carrying the possession does not end the phase, so this is a boundary
    /// *within* one interval rather than a closing one: it names the exact
    /// source locus where one occupant's spend stops and its successor's begins.
    #[serde(
        default = "SourceBoundary::unrecorded",
        skip_serializing_if = "SourceBoundary::is_unrecorded"
    )]
    pub source: SourceBoundary,
}

/// Possession was released and the task became eligible again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimFreed {
    /// Released task.
    pub task: TaskHandle,
    /// Actor that held the possession.
    pub actor: Actor,
}

/// The lease deadline passed and the resident expired possession.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimLapsed {
    /// Expired task.
    pub task: TaskHandle,
    /// Occupant that lost possession.
    pub occupant: CanonicalUuid,
    /// Lease deadline that passed.
    pub expiry: Instant,
}

/// The resident extended a live, epoch-bound lease within its maximum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimRenewed {
    /// Renewed task.
    pub task: TaskHandle,
    /// Resident epoch permitted to advance the lease.
    pub epoch: CanonicalUuid,
    /// Extended expiry.
    pub until: Instant,
    /// Maximum the extension stayed within.
    pub maximum: Instant,
}

/// The exact normalized plan digest landed atomically at its witnessed board barrier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocApplied {
    /// Normalized plan digest that landed.
    pub digest: Digest,
    /// Board barrier the plan was witnessed against.
    pub barrier: Seq,
    /// Operations the plan carried.
    pub operations: u32,
}

/// A directive was created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerSent {
    /// Durable directive identity, equal to the command that created it.
    pub directive: CanonicalUuid,
    /// Directive key.
    pub key: String,
    /// Lane the directive addresses.
    pub lane: CanonicalUuid,
    /// Directive text.
    pub text: String,
    /// Control channel the directive drives; absent on plain prose.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub control: Option<String>,
    /// Opaque attachment payloads carried with this direction.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<DirectiveAttachment>,
    /// Instant the directive expires without a disposition.
    pub expires: Instant,
}

/// One exact attachment carried by a durable directive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectiveAttachment {
    /// Operator-facing attachment name.
    pub name: String,
    /// Declared media type.
    pub content_type: String,
    /// Exact opaque data URL admitted with the directive.
    pub data_url: String,
}

/// The same intent was sent again with lineage and escalation carried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerResent {
    /// Durable identity of the new rung.
    pub directive: CanonicalUuid,
    /// Key of the resent directive.
    pub key: String,
    /// Key this send descends from.
    pub ancestor: String,
    /// Lane the directive addresses.
    pub lane: CanonicalUuid,
    /// Escalation level the lineage has reached.
    pub escalation: u32,
}

/// A lapsed directive was restored to live standing exactly once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerRecovered {
    /// Durable directive identity retained by the recovery.
    pub directive: CanonicalUuid,
    /// Human-sized key retained by the recovery.
    pub key: String,
    /// Lane the directive remains addressed to.
    pub lane: CanonicalUuid,
    /// Expiry that proves the failed standing being recovered.
    pub lapsed_at: Instant,
    /// Fresh bounded expiry assigned to the restored directive.
    pub expires: Instant,
}

/// Exact assistant-prose evidence inside one durable lane-tape record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptLocus {
    /// Tape member containing the complete vendor record.
    pub member: u64,
    /// Byte offset of the record within its member.
    pub offset: u64,
    /// Byte length of the complete vendor record.
    pub length: u64,
    /// Zero-based typed event index inside the decoded record.
    pub event: u32,
    /// One-based assistant-prose line carrying the token.
    pub line: u32,
    /// One-based byte column at which the token begins.
    pub column: u32,
}

/// A live directive was acknowledged at a transcript locus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerAcked {
    /// Durable directive identity retired by this recognition.
    pub directive: CanonicalUuid,
    /// Acknowledged directive key.
    pub key: String,
    /// Lane that acknowledged.
    pub lane: CanonicalUuid,
    /// Resident generation that authenticated the lane.
    pub epoch: CanonicalUuid,
    /// Transcript locus carrying the acknowledgement.
    pub locus: TranscriptLocus,
}

/// A live directive was declined at a transcript locus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerDeclined {
    /// Durable directive identity retired by this recognition.
    pub directive: CanonicalUuid,
    /// Declined directive key.
    pub key: String,
    /// Lane that declined.
    pub lane: CanonicalUuid,
    /// Resident generation that authenticated the lane.
    pub epoch: CanonicalUuid,
    /// Transcript locus carrying the refusal.
    pub locus: TranscriptLocus,
    /// Stated reason the directive cannot be done.
    pub reason: String,
}

/// Why a transcript acknowledgment recognition retired nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AcknowledgmentMiss {
    /// The acknowledgment marker did not lead its line.
    Buried,
    /// The acknowledgment sat inside quoted prose or code.
    Quoted,
    /// The named token was not a steering key.
    Malformed,
    /// No directive carried the named key.
    Unknown,
    /// The named directive had already retired.
    Historical,
}

/// An ACK named no live directive; the unmatched key was recorded and nothing retired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerMissed {
    /// Unmatched key the acknowledgement named.
    pub key: String,
    /// Lane that named it.
    pub lane: CanonicalUuid,
    /// Resident generation that authenticated the lane.
    pub epoch: CanonicalUuid,
    /// Transcript locus the unmatched key appeared at.
    pub locus: TranscriptLocus,
    /// Scanner classification that prevented retirement.
    pub miss: AcknowledgmentMiss,
}

/// A directive reached expiry without a valid disposition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerLapsed {
    /// Expired directive key.
    pub key: String,
    /// Lane the directive addressed.
    pub lane: CanonicalUuid,
    /// Expiry that passed.
    pub expires: Instant,
}

/// The operator withdrew a live directive; the lineage retired without an agent disposition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerCanceled {
    /// Withdrawn directive key.
    pub key: String,
    /// Lane the directive addressed.
    pub lane: CanonicalUuid,
}

/// An ACP permission request arrived, bound to its cause.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentAsked {
    /// Identity of the permission request.
    pub request: CanonicalUuid,
    /// Lane the request came from.
    pub lane: CanonicalUuid,
    /// Operation permission is asked for.
    pub operation: String,
}

/// A named rule or operator allowed the request; any budget effect is carried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentGranted {
    /// Allowed request.
    pub request: CanonicalUuid,
    /// Lane the request came from.
    pub lane: CanonicalUuid,
    /// Rule or operator that allowed it.
    pub rule: String,
    /// Budget remaining after the grant; absent when the rule carries no budget.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub budget: Option<u32>,
}

/// A named rule or operator denied the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentDenied {
    /// Denied request.
    pub request: CanonicalUuid,
    /// Lane the request came from.
    pub lane: CanonicalUuid,
    /// Rule or operator that denied it.
    pub rule: String,
}

/// Why a consent request pended to the operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ConsentQueueCause {
    /// The deciding rule or posture explicitly requires an operator answer.
    Policy {},
    /// The deciding rule spent its last admitted grant and requires renewal.
    BudgetExhausted {
        /// Grants the named rule admits between renewals.
        bound: u32,
    },
}

/// The request pended to the operator while reversible work could continue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentQueued {
    /// Pending request.
    pub request: CanonicalUuid,
    /// Lane the request came from.
    pub lane: CanonicalUuid,
    /// Rule or posture that queued it.
    pub rule: String,
    /// Whether policy asked directly or a named budget requires renewal.
    pub cause: ConsentQueueCause,
}

/// A non-ACP lane's consent floor was recorded once for its occupancy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentFloored {
    /// Lane the floor covers.
    pub lane: CanonicalUuid,
    /// Occupancy the floor was recorded for.
    pub occupant: CanonicalUuid,
    /// Floor the non-ACP lane runs under.
    pub floor: String,
}

/// A lane and its capability were created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneSeated {
    /// Created independently steerable member seat.
    pub lane: CanonicalUuid,
    /// Canvas lineage whose lane contains the member.
    pub lineage_id: CanonicalUuid,
    /// Capability minted with the lane.
    pub capability: CanonicalUuid,
    /// Operator-configured agent display name, absent on pre-field facts.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub name: Option<String>,
    /// Worktree the lane is bound to.
    pub worktree: String,
    /// Closed lane-life spelling selected for the seat.
    pub life: String,
    /// Vendor selected for occupants of this seat.
    pub vendor: Name,
}

/// One Canvas lineage adopted a task-subscription policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineageConfigured {
    /// Stable Canvas lineage identifying the configured group.
    pub lineage_id: CanonicalUuid,
    /// Member moved into the configured lineage, absent for policy-only changes.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub member: Option<CanonicalUuid>,
    /// Lineage the moved member occupied before this transaction.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub from_lineage: Option<CanonicalUuid>,
    /// Replacement identity assigned to a source team when its host left.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub source_rekeyed_to: Option<CanonicalUuid>,
    /// Prior task-subscription policy before this transaction.
    pub from: crate::intent::LineageLife,
    /// Authoritative task-subscription policy after this transaction.
    pub life: crate::intent::LineageLife,
    /// Prior operator-pinned project set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub from_manual_projects: Vec<String>,
    /// Authoritative operator-pinned project set after this transaction.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub manual_projects: Vec<String>,
}

/// One Canvas lineage and the lanes it holds after an arrangement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineageStanding {
    /// Stable Canvas lineage identifying the group.
    pub lineage_id: CanonicalUuid,
    /// Every lane this lineage holds, in the order the operator arranged them.
    pub lanes: Vec<CanonicalUuid>,
}

/// The operator rearranged the Canvas lineages.
///
/// Both sides are recorded because the gesture is not recoverable from the
/// result alone: a reorder and a join leave different `from` standings behind,
/// and a reader that only saw the outcome could not tell which one the
/// operator performed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineageArranged {
    /// Lineage standings before this transaction, in their prior order.
    pub from: Vec<LineageStanding>,
    /// Lineage standings after this transaction, in the arranged order.
    pub lineages: Vec<LineageStanding>,
}

/// One seated member moved to a durable position, possibly in another lineage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneArranged {
    /// Stable member identity that moved.
    pub lane: CanonicalUuid,
    /// Lineage the member occupied before the move.
    pub from_lineage_id: CanonicalUuid,
    /// Lineage owned by the target member after the move.
    pub lineage_id: CanonicalUuid,
    /// Stable member identity immediately following the moved member.
    pub before: CanonicalUuid,
}

/// Exact model and effort supplied to one vendor launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneLaunchIdentity {
    /// Concrete model supplied to the vendor launch.
    pub model: String,
    /// Reasoning effort supplied to the vendor launch.
    pub effort: String,
}

impl LaneLaunchIdentity {
    /// Build one complete recorded launch identity.
    pub fn new(model: impl Into<String>, effort: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            effort: effort.into(),
        }
    }

    pub(crate) fn from_parts(
        model: Option<String>,
        effort: Option<String>,
    ) -> Result<Option<Self>, &'static str> {
        match (model, effort) {
            (Some(model), Some(effort)) => Ok(Some(Self::new(model, effort))),
            (None, None) => Ok(None),
            (Some(_), None) => Err("lane launch identity is missing effort"),
            (None, Some(_)) => Err("lane launch identity is missing model"),
        }
    }

    pub(crate) fn into_parts(identity: Option<Self>) -> (Option<String>, Option<String>) {
        identity.map_or((None, None), |identity| {
            (Some(identity.model), Some(identity.effort))
        })
    }
}

/// A selected vendor record declared the exact resumable lane session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "LaneSessionDeclaredWire", into = "LaneSessionDeclaredWire")]
pub struct LaneSessionDeclared {
    /// Lane whose session was declared.
    pub lane: CanonicalUuid,
    /// Occupant whose selected stream emitted the declaration.
    pub occupant: CanonicalUuid,
    /// Canonical worktree bound to the launch.
    pub worktree: String,
    /// Vendor dialect that admitted the declaration.
    pub vendor: Name,
    /// Launch identity, absent when the declaration predates its recording.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub launch: Option<LaneLaunchIdentity>,
    /// Resident generation that observed the declaration.
    pub epoch: CanonicalUuid,
    /// Selected child stream carrying records.
    pub stream: String,
    /// Exact resumable vendor identity.
    pub identity: String,
    /// Durable tape member holding the declaration.
    pub member: u64,
    /// Declaration offset within the member.
    pub offset: u64,
    /// Number of declaration bytes.
    pub length: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaneSessionDeclaredWire {
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    worktree: String,
    vendor: Name,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    model: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    effort: Option<String>,
    epoch: CanonicalUuid,
    stream: String,
    identity: String,
    member: u64,
    offset: u64,
    length: u64,
}

impl TryFrom<LaneSessionDeclaredWire> for LaneSessionDeclared {
    type Error = &'static str;

    fn try_from(wire: LaneSessionDeclaredWire) -> Result<Self, Self::Error> {
        Ok(Self {
            lane: wire.lane,
            occupant: wire.occupant,
            worktree: wire.worktree,
            vendor: wire.vendor,
            launch: LaneLaunchIdentity::from_parts(wire.model, wire.effort)?,
            epoch: wire.epoch,
            stream: wire.stream,
            identity: wire.identity,
            member: wire.member,
            offset: wire.offset,
            length: wire.length,
        })
    }
}

impl From<LaneSessionDeclared> for LaneSessionDeclaredWire {
    fn from(declaration: LaneSessionDeclared) -> Self {
        let (model, effort) = LaneLaunchIdentity::into_parts(declaration.launch);
        Self {
            lane: declaration.lane,
            occupant: declaration.occupant,
            worktree: declaration.worktree,
            vendor: declaration.vendor,
            model,
            effort,
            epoch: declaration.epoch,
            stream: declaration.stream,
            identity: declaration.identity,
            member: declaration.member,
            offset: declaration.offset,
            length: declaration.length,
        }
    }
}

/// An occupant became live under the lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneStarted {
    /// Occupied lane.
    pub lane: CanonicalUuid,
    /// Occupant that became live.
    pub occupant: CanonicalUuid,
    /// Resident epoch that observed the start.
    pub epoch: CanonicalUuid,
    /// Process-group identity returned by the sole process door.
    pub group: u32,
}

/// A vacant lane seat moved to a fresh linked worktree before exact-session seating.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneImported {
    /// Lane whose lineage seat is retained.
    pub lane: CanonicalUuid,
    /// Latest occupant in the retained lineage.
    pub ancestor: CanonicalUuid,
    /// Worktree the seat was authoritatively bound to.
    pub from_worktree: String,
    /// Fresh linked worktree receiving the seat.
    pub worktree: String,
    /// Driver dialect retained by the transfer.
    pub vendor: Name,
    /// Exact resumable identity proved before the transfer.
    pub identity: String,
    /// Resident generation that admitted the transfer.
    pub epoch: CanonicalUuid,
}

/// The resident observed the occupant end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneExited {
    /// Vacated lane.
    pub lane: CanonicalUuid,
    /// Occupant that ended.
    pub occupant: CanonicalUuid,
    /// Exit status the resident observed.
    pub code: i32,
}

/// The occupant crossed a context compaction boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneCompacted {
    /// Lane the occupant runs under.
    pub lane: CanonicalUuid,
    /// Occupant that compacted.
    pub occupant: CanonicalUuid,
    /// Selected child stream carrying the boundary record.
    pub stream: String,
    /// Durable tape member holding the boundary record.
    pub member: u64,
    /// Boundary-record offset within the member.
    pub offset: u64,
    /// Number of bytes in the boundary record.
    pub length: u64,
}

/// A successor occupant was seated with its ancestor named.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneRenewed {
    /// Reseated lane.
    pub lane: CanonicalUuid,
    /// Successor occupant.
    pub occupant: CanonicalUuid,
    /// Occupant the successor descends from.
    pub ancestor: CanonicalUuid,
    /// Resident epoch that observed the successor start.
    pub epoch: CanonicalUuid,
    /// Process-group identity returned by the sole process door.
    pub group: u32,
}

/// A graceful occupant end completed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneDrained {
    /// Drained lane.
    pub lane: CanonicalUuid,
    /// Occupant that ended gracefully.
    pub occupant: CanonicalUuid,
}

/// A signal this resident sent ended an occupant.
///
/// Separate from [`LaneExited`] because being signalled is a different ending,
/// not an exit status the occupant chose. A signalled process has no status of
/// its own to report, so recording one would mean publishing the resident's
/// substitute for a missing number as though the occupant had returned it. This
/// kind carries no code for the same reason.
///
/// Both signalling stages record this, not only the kill. An occupant that never
/// trapped the termination signal dies from that signal rather than outliving
/// the grace, and this resident is the reason for the ending either way. What
/// this kind separates is whose decision the ending was, not which signal
/// carried it; the stage is already durable in the shutdown ladder facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneKilled {
    /// Vacated lane.
    pub lane: CanonicalUuid,
    /// Occupant the resident ended.
    pub occupant: CanonicalUuid,
}

/// Work run without the resident left a marker that the next resident recognized once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneDegraded {
    /// Lane the daemonless work ran in.
    pub lane: CanonicalUuid,
    /// Marker the next resident recognized.
    pub marker: String,
}

/// Repeated young occupant deaths paused automatic re-seating.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneHeld {
    /// Held lane.
    pub lane: CanonicalUuid,
    /// Evidence the hold names.
    pub evidence: String,
    /// Deadline the hold names.
    pub until: Instant,
}

/// A held lane returned to seating eligibility by operator wake or expired hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneResumed {
    /// Resumed lane.
    pub lane: CanonicalUuid,
    /// What released the hold.
    pub release: Release,
}

/// The judge confirmed a taste at an evidence locus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TasteConfirmed {
    /// Confirmed taste.
    pub taste: String,
    /// Evidence locus the judge ruled at.
    pub locus: String,
}

/// The judge cleared a taste at an evidence locus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TasteCleared {
    /// Cleared taste.
    pub taste: String,
    /// Evidence locus the judge ruled at.
    pub locus: String,
}

/// A taste fired on authored prose and became one live directive.
///
/// This is one fact, not a fire recorded here and a directive raised somewhere
/// else. It carries the complete initial directive alongside the evidence that
/// justified it, so the interval in which a fire exists but is not yet
/// retirable does not exist: whatever reduces the steering plane sees a live
/// directive the moment the fact lands, and whatever reduces the taste plane
/// sees the evidence from the same row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TasteReminded {
    /// Durable directive identity minted by resident recognition.
    pub directive: CanonicalUuid,
    /// Directive key, spelled exactly as an operator-sent key is.
    pub key: String,
    /// Lane the reminder addresses.
    pub lane: CanonicalUuid,
    /// Taste that fired.
    pub taste: String,
    /// Identity of the taste name and normalized prose the taste fired on.
    ///
    /// Domain tagged, so the same bytes read as a taste trigger and as anything
    /// else can never collide into one identity.
    pub content: Digest,
    /// Occupant whose context the prose was authored in.
    pub occupant: CanonicalUuid,
    /// Journal position opening that occupant's current context epoch.
    ///
    /// This is the latest matching `lane::started`, `lane::renewed`, or
    /// subsequent `lane::compacted` fact. Paired with `content` it makes one
    /// reminder per compaction epoch decidable from the durable stream alone,
    /// rather than from a separate memory of what has fired.
    pub compaction: u64,
    /// Exact transcript locus of the prose that fired the taste.
    pub locus: TranscriptLocus,
    /// Reminder text, carried as ordinary directive text.
    pub text: String,
    /// Instant the reminder expires without a disposition.
    pub expires: Instant,
}

/// An expected listening event remained absent past its armed window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanaryAlarmed {
    /// Lane the window was armed for.
    pub lane: CanonicalUuid,
    /// Listening event that stayed absent.
    pub expected: String,
    /// End of the armed window.
    pub window: Instant,
}

/// Listening meters disagreed: activity existed without decoded context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanaryContradicted {
    /// Lane the meters cover.
    pub lane: CanonicalUuid,
    /// Activity the meters saw.
    pub activity: u64,
    /// Context the decoder produced from it.
    pub decoded: u64,
}

/// An unproven vendor version first ran with residue measurement armed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VendorArmed {
    /// Vendor that ran.
    pub vendor: String,
    /// Unproven version.
    pub version: String,
}

/// An executable surface digest changed and entered quarantine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustPended {
    /// Executable surface.
    pub surface: String,
    /// Digest that entered quarantine.
    pub digest: Digest,
    /// Digest it replaced; absent the first time the surface is seen.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub previous: Option<Digest>,
}

/// The operator approved the exact executable digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustGranted {
    /// Executable surface.
    pub surface: String,
    /// Exact approved digest.
    pub digest: Digest,
}

/// The operator withdrew approval from the exact executable digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustRevoked {
    /// Executable surface.
    pub surface: String,
    /// Exact digest approval was withdrawn from.
    pub digest: Digest,
}

/// The resident recognized a repository commit and bound its receipt verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitLanded {
    /// Recognized repository commit.
    pub commit: String,
    /// Receipt the commit published.
    pub receipt: Digest,
    /// Verdict bound to the receipt.
    pub verdict: Verdict,
}

/// The resident recognized a new gate finding; the Finding head opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateFound {
    /// Stable finding identity.
    pub identity: String,
    /// Finding severity.
    pub severity: Severity,
    /// Evidence locus the finding stands at.
    pub locus: String,
    /// Owner the finding is charged to.
    pub owner: Actor,
    /// Receipt the finding originated from.
    pub receipt: Digest,
}

/// A standing finding resolved at its evidence; the Finding head closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateCleared {
    /// Stable finding identity.
    pub identity: String,
    /// Evidence locus the resolution stands at.
    pub locus: String,
}

/// The one forward schema migration finalized before epoch mint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalMigrated {
    /// Schema the journal left.
    pub from: u32,
    /// Schema the journal reached.
    pub to: u32,
    /// Epoch minted after the migration finalized.
    pub epoch: CanonicalUuid,
}

/// A quiesced backup bundle was produced with its enumerated tape manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalBacked {
    /// Produced backup bundle.
    pub bundle: String,
    /// Enumerated tape manifest carried with the bundle.
    pub manifest: Vec<String>,
}

/// A restore protocol completed and the new epoch recognized the exchanged authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalRestored {
    /// Bundle the restore read.
    pub bundle: String,
    /// New epoch that recognized the exchange.
    pub epoch: CanonicalUuid,
    /// Authority the restore exchanged.
    pub authority: Digest,
}

/// Position of the next complete record in one transcript source generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptPosition {
    /// Member holding the next record's first byte.
    pub member: u64,
    /// Offset of that byte within the member.
    pub offset: u64,
}

/// One durable byte span a transcript record was decoded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptSpan {
    /// Member the bytes live in.
    pub member: u64,
    /// First byte within the member.
    pub offset: u64,
    /// Number of bytes.
    pub length: u64,
}

/// One normalized usage sample beside the exact bytes it was decoded from.
///
/// Every counter is required: a batch that omits one is refused rather than
/// accounted as zero, so an unreported counter can never be spent as free.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptUsage {
    /// Completed vendor turns this sample represents.
    pub turns: u64,
    /// Uncached input tokens.
    pub input_tokens: u64,
    /// Cached input tokens.
    pub cached_input_tokens: u64,
    /// Input tokens written to a cache.
    pub cache_write_input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Reasoning output tokens, absent when the dialect does not report them.
    ///
    /// Absence is not zero: a dialect that reports no reasoning counter omits
    /// the field, while a dialect reporting none spent sends an explicit zero.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub reasoning_output_tokens: Option<u64>,
    /// Exact bytes the sample was decoded from, in source order.
    pub spans: Vec<TranscriptSpan>,
    /// Position of this event within the record that carried it.
    pub ordinal: u64,
}

/// One accounted batch of transcript records advanced a transcript source head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalIngested {
    /// Logical lane whose transcript was accounted.
    pub lane: CanonicalUuid,
    /// Vendor that produced the transcript.
    pub vendor: Name,
    /// Logical stream within that lane and vendor.
    pub stream: String,
    /// Opaque durable source generation the batch was read from.
    pub source: CanonicalUuid,
    /// Generation this batch retired, present only when [`Self::source`] is a
    /// new generation restarting the stream from its origin.
    ///
    /// A reader cannot infer supersession from a source that merely differs:
    /// that reading turns a batch written against a stale head, or a batch that
    /// named the wrong source, into a silent generation change that abandons
    /// the accounted position. Naming the retired generation makes the claim
    /// explicit and lets the guard refuse every batch that does not make it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub supersedes: Option<CanonicalUuid>,
    /// Position the batch continued from.
    pub from: TranscriptPosition,
    /// Position the batch left the source head at.
    pub to: TranscriptPosition,
    /// Every normalized usage sample the batch accounted.
    pub usage: Vec<TranscriptUsage>,
    /// Complete records carrying no usage that the batch accounted.
    pub non_usage_records: u64,
    /// Residue records the batch accounted.
    pub residue_records: u64,
}

/// The resident acquired the kernel writer lock and minted its epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpSeated {
    /// Minted resident epoch.
    pub epoch: CanonicalUuid,
}

/// The resident stopped admission and began a graceful end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpQuiescing {
    /// Resident epoch ending.
    pub epoch: CanonicalUuid,
}

/// The resident began preserving or finishing admitted work and ending occupants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpDraining {
    /// Resident epoch ending.
    pub epoch: CanonicalUuid,
    /// Occupants still live at the transition.
    pub occupants: u32,
}

/// Grace expired and the resident began ending occupants that had not finished.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpTerminating {
    /// Resident epoch ending.
    pub epoch: CanonicalUuid,
    /// Occupants that had not finished.
    pub occupants: u32,
}

/// Bounded escalation began against occupants that survived termination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpKilling {
    /// Resident epoch ending.
    pub epoch: CanonicalUuid,
    /// Occupants that survived termination.
    pub occupants: u32,
}

/// Occupants ended; the resident began verifying claims, tape seals, and residue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpVerifying {
    /// Resident epoch ending.
    pub epoch: CanonicalUuid,
}

/// The resident committed the verified terminal state immediately before releasing authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpDark {
    /// Resident epoch releasing authority.
    pub epoch: CanonicalUuid,
}

/// The resident recognized a valid evidence bundle bound to commit, tree, and manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofSealed {
    /// Commit the bundle is bound to.
    pub commit: String,
    /// Tree the bundle is bound to.
    pub tree: String,
    /// Manifest the bundle is bound to.
    pub manifest: Digest,
}

/// One published target entry and its detached proof receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedTarget {
    /// Target the entry names.
    pub target: String,
    /// Artifact digest published for the target.
    pub artifact: Digest,
    /// Detached proof receipt the artifact published under.
    pub receipt: Digest,
}

/// The sealed release manifest digest, and each target entry's artifact digest, published.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleasePublished {
    /// Sealed release manifest digest.
    pub manifest: Digest,
    /// Each target entry published under its detached proof receipt.
    pub targets: Vec<PublishedTarget>,
}
