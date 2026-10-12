//! Task and claim lifecycle fact bodies.
//!
//! Every task reference here is the durable birth stamp. Public handles
//! belong to folds and commands; no fact can make a rendered stem
//! authoritative again.

use super::*;

/// The sole transcript stream retained as executable task creation context.
pub const TASK_CREATION_CONTEXT_STREAM: &str = "session";

/// Why no executable transcript coordinate can be retained for a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskCreationUnavailable {
    /// The operator created the task outside an authenticated lane transcript.
    OperatorActor,
    /// Spice created the task without a lane-authored command.
    SpiceActor,
    /// The task predates creation-context authority and immutable history proves no boundary.
    LegacyEvidenceMissing,
    /// Legacy evidence names only a stream the current context command cannot rehydrate.
    LegacyStreamUnregistered,
}

impl TaskCreationUnavailable {
    /// Stable reason-code spelling used by human and machine task reads.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OperatorActor => "operator-actor",
            Self::SpiceActor => "spice-actor",
            Self::LegacyEvidenceMissing => "legacy-evidence-missing",
            Self::LegacyStreamUnregistered => "legacy-stream-unregistered",
        }
    }
}

/// Immutable transcript authority captured when a task is created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TaskCreationContext {
    /// A lane-authored task was admitted after this exact accounted boundary.
    Transcript {
        /// Exact vendor session identity causally active for the creating lane.
        session: String,
        /// Source-aware position of the next complete record at task admission.
        boundary: TranscriptSourceHead,
        /// Creator team captured under the same residency guard as the boundary.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::optional::present"
        )]
        creator_team: Option<CanonicalUuid>,
    },
    /// No authoritative transcript coordinate exists for this task.
    Unavailable {
        /// Closed reason no executable rehydration command can be offered.
        reason: TaskCreationUnavailable,
    },
}

impl TaskCreationContext {
    /// Build one explicit unavailable context.
    #[must_use]
    pub const fn unavailable(reason: TaskCreationUnavailable) -> Self {
        Self::Unavailable { reason }
    }

    /// Immutable creator team, absent for non-lane and migrated task creation.
    #[must_use]
    pub const fn creator_team(&self) -> Option<CanonicalUuid> {
        match self {
            Self::Transcript { creator_team, .. } => *creator_team,
            Self::Unavailable { .. } => None,
        }
    }
}

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
    /// The resumable vendor session or its pending operator direction changed.
    #[serde(rename = "input-changed")]
    InputChanged,
    /// A young-death hold reached its deadline.
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
    /// Sole task identity minted from the statement's birth instant.
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
    /// Typed facts the execution environment must satisfy together.
    #[serde(
        default,
        skip_serializing_if = "crate::EnvironmentRequirements::is_empty",
        deserialize_with = "crate::EnvironmentRequirements::present"
    )]
    pub requirements: crate::EnvironmentRequirements,
    /// Dotted project stem.
    pub project: String,
    /// Free tags carried on the statement.
    pub tags: Vec<String>,
    /// Tasks this one waits behind.
    pub depends: Vec<TaskStamp>,
    /// Declared Plan or Design intent the statement opens with.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub intent: Option<crate::TaskIntent>,
    /// Preparation association admitted atomically with this task.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub preparation: Option<crate::TaskPreparation>,
    /// Ordered phases the workflow head will walk.
    pub flow: Vec<Phase>,
    /// Phase the initial workflow head sits in.
    pub phase: Phase,
    /// Statement revision the head opens at.
    pub revision: u32,
    /// Creation-time transcript authority, immutable for the task's lifetime.
    pub creation: TaskCreationContext,
}

/// The workflow head moved to another phase: a claimed phase completed, or an
/// edit that cleared a taste hold withdrew the plan phase the hold inserted.
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
    /// Replayable terminal response for the completion operation.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub terminal: Option<TaskCompletionTerminal>,
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

/// Durable fields needed to reconstruct one terminal task response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskCompletionTerminal {
    /// Identity returned by the initiating running acknowledgement.
    pub operation: CanonicalUuid,
    /// Workflow status returned by the completing transaction.
    pub status: String,
    /// Exact next action projected by that transaction.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub next: Option<Box<str>>,
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
    /// Reviewer's note on a clean verdict; a finding carries its note in `finding`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub note: Option<String>,
    /// Correction tasks the finding linked, in the order the reviewer named them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub followups: Vec<TaskStamp>,
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
pub const TASK_EDIT_FIELD_SUMMARY_CHARS: usize = 160;

/// One bounded changed field carried for claimant notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskFieldEdit {
    /// Contract field whose value changed.
    pub field: String,
    /// Bounded presentation of the value the claimant accepted.
    pub before: String,
    /// Bounded presentation of the replacement value.
    pub after: String,
}

/// Claim-bound delivery authority an earlier build captured with a task edit.
///
/// Retained so journaled edits keep decoding. Claimants now learn of every
/// task change through its generation, so no writer produces this.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskEditNotice {
    /// Public handle produced by this exact statement revision.
    pub handle: crate::TaskHandle,
    /// Lane possessing the task when it changed.
    pub lane: CanonicalUuid,
    /// Fact that established the claim generation, stable across renewal carry.
    pub claim: Seq,
    /// Statement head the editor replaced.
    pub previous: Seq,
    /// Bounded changed fields in their stable presentation order.
    pub changes: Vec<TaskFieldEdit>,
}

/// An unsettled task statement changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEdited {
    /// Edited task.
    pub task: TaskStamp,
    /// Statement revision the edit produced.
    pub revision: u32,
    /// Complete dependency set when this edit changed it.
    ///
    /// Ordinary statement edits and facts written before relationship-aware
    /// invalidation omit this field.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub dependencies: Option<Vec<TaskStamp>>,
    /// Explicit intent change this edit admitted.
    ///
    /// The fold of creation intent and these changes is the statement head's
    /// declared intent; edits that leave intent alone omit this field.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub intent: Option<crate::TaskIntentEdit>,
    /// Claimant notice authority recorded by an earlier build; never written.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub notice: Option<TaskEditNotice>,
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

/// The operator closed a task as completed without holding its claim.
///
/// Completion ordinarily rests on the claimant's proof (`task::done`); this
/// is the operator's own lever, so the fact records the phase it cut short and
/// its stated validation instead. `claimed` names the actor whose live claim
/// the closure overrode, when it overrode one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskClosed {
    /// Closed task.
    pub task: TaskStamp,
    /// Operator's stated evidence or reason the task is complete.
    pub validation: String,
    /// Phase the workflow held when the operator closed it.
    pub phase: Phase,
    /// Actor whose live claim the closure overrode, when it overrode one.
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

/// Possession rebound to current coordination authority with lineage preserved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimCarried {
    /// Carried task.
    pub task: TaskStamp,
    /// Lane the possession stays bound to.
    pub lane: CanonicalUuid,
    /// Current occupant; it may be unchanged across resident replacement.
    pub occupant: CanonicalUuid,
    /// Occupant the possession came from; equal when only the epoch changed.
    pub ancestor: CanonicalUuid,
    /// Resident epoch that carried it.
    pub epoch: CanonicalUuid,
    /// Accounted transcript the current authority takes over at.
    ///
    /// Carrying the possession does not end the phase, so this is a boundary
    /// *within* one interval rather than a closing one: it names the exact
    /// source locus where the preceding authority's accounting stops and the
    /// current authority's begins.
    #[serde(
        default = "SourceBoundary::unrecorded",
        skip_serializing_if = "SourceBoundary::is_unrecorded"
    )]
    pub source: SourceBoundary,
}

/// Possession was rebound by its lane after residency moved ahead of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimRebound {
    /// Rebound task.
    pub task: TaskStamp,
    /// Lane the possession remains bound to.
    pub lane: CanonicalUuid,
    /// Current occupant receiving possession.
    pub occupant: CanonicalUuid,
    /// Occupant named by the predecessor authority being recovered.
    pub ancestor: CanonicalUuid,
    /// Current resident epoch authorizing recovery.
    pub epoch: CanonicalUuid,
    /// Resulting lease expiry.
    pub until: Instant,
    /// Durable maximum the recovery stayed within.
    pub maximum: Instant,
    /// Accounted transcript boundary where current authority takes over.
    pub source: SourceBoundary,
}

/// Commits and an optional dirty worktree snapshot detached on task release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskParkedCommits {
    /// Newest published commit the lane shared; release returned the lane here.
    pub baseline: String,
    /// Original saved tip, recoverable by object ID while Git retains it.
    pub head: String,
    /// Original commit identities in oldest-first ancestry order.
    pub commits: Vec<String>,
    /// Unreferenced worktree snapshot, restored without creating a lane commit.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub snapshot: Option<String>,
}

/// Possession was released and the task became eligible again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimFreed {
    /// Released task.
    pub task: TaskStamp,
    /// Actor that held the possession.
    pub actor: Actor,
    /// Commits retained for this task while no lane owns it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub parked: Option<TaskParkedCommits>,
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

/// A live lease crossed its warning horizon and opened ordinary lane steering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimLeaseWarned {
    /// Warned task.
    pub task: TaskStamp,
    /// Exact claim head this warning generation observed.
    pub claim: Seq,
    /// Exact lease head this warning generation observed.
    pub lease: Seq,
    /// Lane holding the claim when the warning fired.
    pub lane: CanonicalUuid,
    /// Occupant holding the lane when the warning fired.
    pub occupant: CanonicalUuid,
    /// Resident epoch authorizing the warning.
    pub epoch: CanonicalUuid,
    /// Lease expiry whose warning generation this fact records.
    pub expiry: Instant,
    /// Durable directive identity opened for the lane.
    pub directive: CanonicalUuid,
    /// Human-sized steering key.
    pub key: String,
    /// Actionable warning text presented through ordinary steering.
    pub text: String,
}
