//! Task and claim lifecycle fact bodies.
//!
//! Every task reference here is the durable inception stamp. Public handles
//! belong to folds and commands; no fact can make a rendered stem
//! authoritative again.

use super::*;

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
    /// Sole task identity minted from the statement's inception instant.
    pub task: TaskStamp,
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
    pub depends: Vec<TaskStamp>,
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
    pub task: TaskStamp,
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

/// Legacy complete-verifier citation retained for pre-retirement facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FullVerifierReceipt {
    /// Exact selected source tree named by every admitted verifier arm.
    pub tree: Digest,
    /// Identity of the canonical full-verifier evidence bundle.
    pub evidence: Digest,
    /// Identity of the complete admitted gate battery inside that bundle.
    pub battery: Digest,
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
    pub task: TaskStamp,
    /// Evidence the completion rests on.
    pub validation: String,
    /// Repository history published before the workflow advanced.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub git: Option<GitBoundary>,
    /// Legacy verifier evidence from pre-retirement completion producers.
    ///
    /// Current producers leave this absent; retaining the field preserves exact
    /// decoding of durable facts written before repository-wide verification was
    /// removed from task completion.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub verifier: Option<FullVerifierReceipt>,
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
    pub task: TaskStamp,
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
    pub task: TaskStamp,
    /// Principal seated as reviewer.
    pub reviewer: Actor,
    /// Phase the assignment covers.
    pub phase: Phase,
}

/// A durable task note appended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskNoted {
    /// Annotated task.
    pub task: TaskStamp,
    /// Note text.
    pub note: String,
}

/// A task received a wakeable deferral.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDeferred {
    /// Deferred task.
    pub task: TaskStamp,
    /// Instant the deferral becomes wakeable.
    pub until: Instant,
}

/// A task's deferral cleared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskWoken {
    /// Woken task.
    pub task: TaskStamp,
}

/// An unsettled task statement changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEdited {
    /// Edited task.
    pub task: TaskStamp,
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
    pub task: TaskStamp,
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
    pub task: TaskStamp,
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
    /// Dirty-handoff transaction whose replay this claim authorizes.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub recovery: Option<String>,
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
    pub task: TaskStamp,
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
    pub task: TaskStamp,
    /// Actor that held the possession.
    pub actor: Actor,
}

/// The lease deadline passed and the resident expired possession.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimLapsed {
    /// Expired task.
    pub task: TaskStamp,
    /// Occupant that lost possession.
    pub occupant: CanonicalUuid,
    /// Lease deadline that passed.
    pub expiry: Instant,
}

/// The resident extended a live, epoch-bound lease within its maximum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimRenewed {
    /// Renewed task.
    pub task: TaskStamp,
    /// Resident epoch permitted to advance the lease.
    pub epoch: CanonicalUuid,
    /// Extended expiry.
    pub until: Instant,
    /// Maximum the extension stayed within.
    pub maximum: Instant,
}
