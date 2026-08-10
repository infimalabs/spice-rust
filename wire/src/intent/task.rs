//! Task, claim, gate-finding, and effort command contracts.
//!
//! Commands may name projected handles for lookup. Creation plans and durable
//! answers cross the admission boundary with inception stamps, so a later
//! project edit only changes presentation.

use serde::{Deserialize, Serialize};

use crate::body::{SourceBoundary, TranscriptPosition};
use crate::envelope::{Actor, Origin, Seq};
use crate::ident::{CanonicalUuid, Digest, Instant};
use crate::witness::{Name, Phase, TaskHandle, TaskStamp};

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
    /// Caller-authored creation provenance, or server-resolved causal authority.
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
    /// Explicit causal authority when no active claim owns the filing.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub origin: Option<Origin>,
}

/// File the exact findings folded from one registered gate run.
///
/// No caller-authored finding rows cross this boundary; the resident decodes
/// `evidence` and folds every row from that report itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateFindings {
    /// Digest of the canonical focused report bytes.
    pub report: Digest,
    /// Canonical focused report JSON admitted by Gate before folding.
    pub evidence: String,
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
    pub task: TaskStamp,
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

/// One follow-up statement a review creates, paired with its inception stamp.
///
/// The caller mints the stamp and witnesses its absence, the way `task add`
/// does, so the whole batch is decided before the review writes anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewFollowup {
    /// Sole identity minted for the statement.
    pub task: TaskStamp,
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
