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

use crate::body::{
    ConsentQueueCause, DirectiveAttachment, Severity, SourceBoundary, TranscriptLocus,
    TranscriptPosition,
};
use crate::envelope::{Actor, Origin, Seq};
use crate::fact::FactKind;
use crate::head::{ResidentState, TapeLocus, TrustState};
use crate::ident::{CanonicalUuid, Digest, Instant, ReleaseNotesIdentity};
use crate::witness::{Answer, Disposition, Name, Phase, TaskHandle};

mod board;
pub use board::{
    BoardActorRoute, BoardAnswer, BoardGraph, BoardHandout, BoardProjectPolicy, BoardRead,
    BoardStatement, BoardTask, DocLedger, Positioned, ProjectVisibility, TextAnswer,
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

/// File the exact findings projected by one registered gate run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateFindings {
    /// Digest of the complete typed battery report that produced these rows.
    pub report: Digest,
    /// Findings projected from that report, in gate order.
    pub findings: Vec<GateFindingWork>,
}

/// One gate finding ready to become deferred board work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateFindingWork {
    /// Stable gate-side finding identity.
    pub identity: Digest,
    /// Finding severity.
    pub severity: Severity,
    /// Evidence locus the finding cites.
    pub locus: String,
    /// Board task title.
    pub title: String,
    /// Board task description.
    pub description: String,
    /// Criteria that decide whether the filed task is done.
    pub acceptance: Vec<String>,
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
    /// Replacement priority, when the allocator's ranking of this task moves.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub priority: Option<String>,
    /// Replacement project, when the task moves to a different audience.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub project: Option<String>,
}

/// Withdraw a task from the board with a recorded reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDelete {
    /// Task the operator withdraws.
    pub task: TaskHandle,
    /// Why the task leaves the board; required, because a withdrawal that
    /// records no reason is indistinguishable from one made in error.
    pub reason: String,
    /// Override a live claim rather than refusing the withdrawal.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub force_claimed: bool,
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

/// Select task-effort rows from the one replayable effort fold.
///
/// Absence means every task. Filtering is a read concern rather than reducer
/// state, so both the filtered and unfiltered CLI surfaces consume the same
/// complete materialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffortRead {
    /// One task to retain, or every task when absent.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub task: Option<TaskHandle>,
}

/// Exact vendor launch identity that makes one effort value comparable.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffortIdentity {
    /// Vendor that emitted the accounted transcript.
    pub vendor: Name,
    /// Concrete model supplied to the vendor launch.
    pub model: String,
    /// Reasoning effort supplied to the vendor launch.
    pub effort: String,
}

/// Checked normalized usage totals.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffortUsage {
    /// Completed vendor turns.
    pub turns: u64,
    /// Uncached input tokens.
    pub input_tokens: u64,
    /// Cached input tokens.
    pub cached_input_tokens: u64,
    /// Input tokens written to cache.
    pub cache_write_input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Reasoning output tokens, absent when any contributing sample omitted the counter.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub reasoning_output_tokens: Option<u64>,
}

/// One source-contiguous, occupant- and lineage-attributed part of a phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffortSegmentAnswer {
    /// Lane whose transcript supplies this segment.
    pub lane: CanonicalUuid,
    /// Occupant credited with this segment.
    pub occupant: CanonicalUuid,
    /// Canvas lineage credited at this exact source locus.
    pub lineage: CanonicalUuid,
    /// Exact vendor session identity covering this source locus.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub session: Option<String>,
    /// Opaque transcript generation.
    pub source: CanonicalUuid,
    /// First included record boundary.
    pub from: TranscriptPosition,
    /// First excluded record boundary.
    pub to: TranscriptPosition,
    /// Comparable launch identity, absent only on a reason-coded unavailable phase.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub identity: Option<EffortIdentity>,
    /// Usage wholly contained by this half-open segment.
    pub usage: EffortUsage,
}

/// Stable reason one phase cannot contribute a numeric sizing value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EffortUnavailableCode {
    MissingStart,
    MissingEnd,
    IncompleteBoundary,
    TranscriptMissing,
    SourceGap,
    SourceReplaced,
    ClaimFreed,
    ClaimLapsed,
    ClaimStolen,
    TaskDeleted,
    BrokenCarry,
    OccupantDisagreement,
    SessionDisagreement,
    LineageDisagreement,
    MalformedUsage,
    UntaggedLaunch,
    MixedLaunch,
    ArithmeticOverflow,
}

impl EffortUnavailableCode {
    /// Stable wire spelling of this code.
    ///
    /// A surface that must name a reason would otherwise encode the value and
    /// read one word back out of the bytes, which is a decoder standing in for
    /// a noun. The match is exhaustive, so a code added without a spelling
    /// fails to compile, and a test pins every spelling against the encoder so
    /// the two cannot drift into two different words for one reason.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingStart => "missing-start",
            Self::MissingEnd => "missing-end",
            Self::IncompleteBoundary => "incomplete-boundary",
            Self::TranscriptMissing => "transcript-missing",
            Self::SourceGap => "source-gap",
            Self::SourceReplaced => "source-replaced",
            Self::ClaimFreed => "claim-freed",
            Self::ClaimLapsed => "claim-lapsed",
            Self::ClaimStolen => "claim-stolen",
            Self::TaskDeleted => "task-deleted",
            Self::BrokenCarry => "broken-carry",
            Self::OccupantDisagreement => "occupant-disagreement",
            Self::SessionDisagreement => "session-disagreement",
            Self::LineageDisagreement => "lineage-disagreement",
            Self::MalformedUsage => "malformed-usage",
            Self::UntaggedLaunch => "untagged-launch",
            Self::MixedLaunch => "mixed-launch",
            Self::ArithmeticOverflow => "arithmetic-overflow",
        }
    }
}

/// One reason-coded unavailable observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffortUnavailable {
    /// Stable code suitable for grouping and tests.
    pub code: EffortUnavailableCode,
    /// Recorded or evidence-specific detail; never a guessed replacement value.
    pub detail: String,
}

/// Numeric or withheld phase result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum EffortAvailability {
    /// Closed, fully covered, and identically tagged.
    Available {
        /// Tuple every segment agreed on.
        identity: EffortIdentity,
        /// Checked sum of every segment.
        usage: EffortUsage,
        /// Claim-to-terminal wall time in whole milliseconds.
        wall_ms: u64,
    },
    /// Visible but excluded from every numeric aggregate.
    Unavailable {
        /// Every independently observed reason, in stable code/detail order.
        reasons: Vec<EffortUnavailable>,
    },
}

/// One claim-to-terminal workflow-phase attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffortPhaseAnswer {
    /// Task this phase belongs to.
    pub task: TaskHandle,
    /// Workflow phase name at the opening claim.
    pub phase: Phase,
    /// Zero-based workflow attempt index advanced by each phase transition.
    pub phase_index: u32,
    /// ClaimWon fact opening this attempt.
    pub opening: Seq,
    /// TaskDone or TaskReviewed fact closing it, absent when interrupted.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub closing: Option<Seq>,
    /// Renewal facts observed without splitting the phase.
    pub renewals: u32,
    /// Exact recorded opening boundary.
    pub opening_boundary: SourceBoundary,
    /// Exact recorded closing boundary, absent when no terminal fact exists.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub closing_boundary: Option<SourceBoundary>,
    /// Verified source-contiguous attribution pieces.
    pub segments: Vec<EffortSegmentAnswer>,
    /// Numeric answer or explicit withholding reasons.
    pub availability: EffortAvailability,
}

/// Comparable totals and integer averages for one exact identity and phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffortAggregateAnswer {
    /// Exact tuple defining comparability.
    pub identity: EffortIdentity,
    /// Workflow phase partition.
    pub phase: Phase,
    /// Available phases in the partition.
    pub count: u64,
    /// Checked token totals.
    pub usage_sum: EffortUsage,
    /// Per-phase integer token averages.
    pub usage_average: EffortUsage,
    /// Checked wall-time sum.
    pub wall_ms_sum: u64,
    /// Per-phase integer wall-time average.
    pub wall_ms_average: u64,
}

/// The one typed answer shared by text, JSON, and later Canvas consumers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffortAnswer {
    /// Task phases in opening-fact, task, phase-index, segment-locus order.
    pub phases: Vec<EffortPhaseAnswer>,
    /// Comparable partitions in identity then phase order.
    pub aggregates: Vec<EffortAggregateAnswer>,
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
    /// Follow-up statements this review creates behind the reviewed task.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub then: Vec<ReviewFollowup>,
    /// Existing tasks wired to follow the reviewed task.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub followup: Vec<TaskHandle>,
}

/// One follow-up statement a review creates, paired with its minted handle.
///
/// The caller mints the handle and witnesses its absence, the way `task add`
/// does, so the whole batch is decided before the review writes anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewFollowup {
    /// Handle minted for the statement.
    pub task: TaskHandle,
    /// The statement to create.
    pub add: TaskAdd,
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
        /// Presentation state established by this fact, when applicable.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::optional::present"
        )]
        stage: Option<crate::body::SteeringStage>,
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
