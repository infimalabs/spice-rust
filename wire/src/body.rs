//! One body DTO per fact kind.
//!
//! Every kind in the closed vocabulary registers exactly one body here, and the
//! registry in [`crate::fact`] binds them. A body's fields are the nouns its
//! own kind names, or the head content that kind points at; a field anchored in
//! neither does not belong in the body.
//!
//! Bodies flatten beside the discriminant, so their field names are the fact
//! object's own keys and cannot collide with `kind`.

use serde::{Deserialize, Serialize};

use crate::Name;
use crate::envelope::{Actor, Seq};
use crate::head::TranscriptSourceHead;
use crate::ident::{CanonicalUuid, Digest, Instant};
use crate::witness::{Phase, TaskHandle};

mod publication;
pub use publication::{ProofSealed, PublishedTarget, ReleasePublished};

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
/// therefore spells exactly one value, and every boundary a live producer
/// proves is written explicitly.
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
    /// migration rather than a fallback because it is total and because it
    /// produces a value that states in [`Self::UNRECORDED`] what it was
    /// migrated from.
    ///
    /// It is also the only producer of [`Self::Incomplete`] anywhere. Every
    /// live producer admits a claim before it writes and a claim carries a
    /// lane, so none of them has nothing to ask about; one that cannot read the
    /// boundary it asked for fails loudly rather than recording *unknown*.
    #[must_use]
    pub fn unrecorded() -> Self {
        Self::Incomplete {
            reason: Self::UNRECORDED.to_owned(),
        }
    }

    /// Whether this is the migrated stand-in for a fact stored without the field.
    ///
    /// Omitting the key on encode restores the exact bytes a stored fact
    /// arrived as. The match is on the reason rather than on the variant so
    /// that it can only ever be true of the migration's own value: an
    /// incomplete boundary carrying any other reason encodes explicitly, and no
    /// answer a producer actually gave can be dropped on the way back out.
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
    /// Project and phase occupied when the claim was minted.
    ///
    /// Absence is admitted only for facts written before journal schema 4.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub cell: Option<crate::ClaimCell>,
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

/// How urgently one stable directive is currently presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SteeringStage {
    /// The original publication has not crossed its first silence horizon.
    Published,
    /// Silence crossed the first horizon.
    Reminded,
    /// Silence crossed the second horizon.
    Escalated,
    /// Silence crossed the final pre-expiry horizon.
    Overdue,
}

impl SteeringStage {
    /// Stable lowercase spelling used on operator surfaces.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Published => "published",
            Self::Reminded => "reminded",
            Self::Escalated => "escalated",
            Self::Overdue => "overdue",
        }
    }

    /// The next presentation state, absent at the last pre-expiry state.
    #[must_use]
    pub const fn next(self) -> Option<Self> {
        match self {
            Self::Published => Some(Self::Reminded),
            Self::Reminded => Some(Self::Escalated),
            Self::Escalated => Some(Self::Overdue),
            Self::Overdue => None,
        }
    }
}

impl std::fmt::Display for SteeringStage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One durable presentation transition in a directive's audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SteeringTransition {
    /// State entered at this transition.
    pub stage: SteeringStage,
    /// Journal instant at which the transition was recorded.
    pub at: Instant,
}

/// One stable directive crossed a named silence horizon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SteerEscalated {
    /// Durable identity retained from publication.
    pub directive: CanonicalUuid,
    /// Human key retained from publication.
    pub key: String,
    /// Lane the directive addresses.
    pub lane: CanonicalUuid,
    /// Named presentation state entered by this fact.
    pub stage: SteeringStage,
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

/// The operator withdrew a live directive without an agent disposition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerCanceled {
    /// Withdrawn directive key.
    pub key: String,
    /// Lane the directive addressed.
    pub lane: CanonicalUuid,
}

mod interaction;
pub use interaction::{
    ConsentAsked, ConsentDenied, ConsentFloored, ConsentGranted, ConsentQueueCause, ConsentQueued,
    LaneArranged, LaneCompacted, LaneDegraded, LaneDrained, LaneExited, LaneHeld, LaneImported,
    LaneKilled, LaneLaunchIdentity, LaneRenewed, LaneResumed, LaneSeated, LaneSessionDeclared,
    LaneSourceBoundary, LaneStarted, LineageArranged, LineageConfigured, LineageStanding,
    TasteCleared, TasteConfirmed, TasteReminded,
};

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

/// A requested gate finding dispatch completed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateFindingsFiled {
    /// Digest of the complete typed battery report that was dispatched.
    pub report: Digest,
    /// Number of projected findings consumed.
    pub findings: u32,
    /// Number of deferred board tasks created.
    pub created: u32,
    /// Number of open board tasks reused.
    pub reused: u32,
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
