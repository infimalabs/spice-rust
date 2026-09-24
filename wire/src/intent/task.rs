//! Task and claim command contracts.
//!
//! Commands may name projected handles for lookup. Creation plans and durable
//! answers cross the admission boundary with birth stamps, so a later
//! project edit only changes presentation.

use serde::{Deserialize, Serialize};

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
    /// Typed environmental facts required before allocation.
    #[serde(
        default,
        skip_serializing_if = "crate::EnvironmentRequirements::is_empty",
        deserialize_with = "crate::EnvironmentRequirements::present"
    )]
    pub requirements: crate::EnvironmentRequirements,
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
    /// Whether creation must atomically place the caller's lane in possession.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub claim: bool,
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

/// File one evidence-backed taste candidate through the fixed hidden proposal route.
///
/// The route and workflow are server-owned. A caller supplies the stable finding
/// and occurrence identities plus review material, but cannot target an
/// arbitrary hidden project or activate tracked policy directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TasteProposalAdd {
    /// Stable identity of the recurring correction theme.
    pub finding: Digest,
    /// Identity of the exact independent-locus set this filing covers.
    pub occurrence: Digest,
    /// Number of independent transcript loci supporting this occurrence.
    pub recurrence: usize,
    /// One-line statement of the proposed review work.
    pub title: String,
    /// Exact cited evidence and the proposed tracked row.
    pub description: String,
    /// Criteria the operator-reviewed proposal must satisfy.
    pub acceptance: Vec<String>,
    /// Causal steering directive that supplied the oldest cited correction.
    pub origin: Origin,
    /// Completed predecessor, present only for a later genuine recurrence.
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
    /// Dedicated replacement or clearing of environmental requirements.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub requirements: Option<TaskRequirementsEdit>,
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

/// One explicit environmental-requirement mutation.
///
/// The dedicated action keeps omission as "unchanged", a nonempty canonical
/// collection as "replace", and clearing as its own spelling. No empty array
/// can be normalized into either intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "lowercase", deny_unknown_fields)]
pub enum TaskRequirementsEdit {
    /// Replace the complete AND-composed collection.
    Set {
        /// Nonempty canonical requirements in environmental-fact order.
        #[serde(deserialize_with = "crate::EnvironmentRequirements::present")]
        requirements: crate::EnvironmentRequirements,
    },
    /// Remove every environmental requirement.
    Clear {},
}

impl TaskRequirementsEdit {
    /// Replace requirements with one necessarily canonical fact.
    #[must_use]
    pub fn one(requirement: crate::EnvironmentRequirement) -> Self {
        Self::Set {
            requirements: crate::EnvironmentRequirements::one(requirement),
        }
    }

    /// Complete replacement collection, or empty when clearing.
    #[must_use]
    pub fn requirements(&self) -> crate::EnvironmentRequirements {
        match self {
            Self::Set { requirements } => requirements.clone(),
            Self::Clear {} => crate::EnvironmentRequirements::default(),
        }
    }
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

/// Wake ordinary deferred work, or promote one parked `.oops` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskWake {
    /// Task being woken or promoted.
    pub task: TaskHandle,
    /// Public project receiving a parked `.oops` row.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub into: Option<String>,
}

/// Name one task and nothing else.
///
/// Claim moves that only identify their subject share this shape.
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
    /// Durable completion identity minted by the initiating client.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub operation: Option<CanonicalUuid>,
}

impl TaskDone {
    /// Derive the one default completion operation owned by a claim.
    #[must_use]
    pub fn operation_for_claim(claim: Seq) -> CanonicalUuid {
        const CLAIM_OPERATION_PREFIX: u128 = 0x7370_6963_652d_0000_0000_0000_0000_0000;
        CanonicalUuid::new(uuid::Uuid::from_u128(
            CLAIM_OPERATION_PREFIX | u128::from(claim.get()),
        ))
    }
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

/// One follow-up statement a review creates, paired with its birth stamp.
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

/// Closed agent-facing guidance attached to a task transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskGuidance {
    /// A plan advance released its claim without reserving the following phase.
    #[serde(
        rename = "phase ownership: this advance released your claim; only the held plan phase was completed. Run spice task next for another assignment; it may select any eligible work and does not guarantee this task's next phase."
    )]
    PlanAdvanceOwnership,
}

impl TaskGuidance {
    /// Exact agent-facing guidance carried on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PlanAdvanceOwnership => concat!(
                "phase ownership: this advance released your claim; only the held plan phase ",
                "was completed. Run spice task next for another assignment; it may select any ",
                "eligible work and does not guarantee this task's next phase."
            ),
        }
    }
}

/// The answer a task command returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskAnswer {
    /// Task the answer concerns.
    pub task: TaskHandle,
    /// Its status after the transaction.
    pub status: String,
    /// Actionable guidance attached to this exact transition.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub guidance: Option<TaskGuidance>,
    /// Durable completion operation still running before this task may move.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub completion: Option<Box<TaskCompletionAnswer>>,
    /// Compatibility-only learning report retained when decoding older answers.
    ///
    /// Current task completion does not produce this field.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub learning: Option<Box<TaskLearning>>,
    /// Active claim minted in the same transaction, when creation also boards the caller.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub claim: Option<Box<ClaimAnswer>>,
    /// Exact CLI command admitted as the next action, when one is available.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub next: Option<Box<str>>,
}

/// One durable long-running task completion handoff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskCompletionAnswer {
    /// Identity minted by the first command and retained by every retry.
    pub operation: CanonicalUuid,
    /// Closed operation state returned before terminal task publication.
    pub state: TaskCompletionState,
}

/// Non-terminal states a task command may return for durable completion work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskCompletionState {
    /// Mandatory evidence is executing outside the resident command loop.
    Running,
}

/// Compatibility shape for a learning report carried by an older task answer.
///
/// This DTO preserves retained answer decoding; it is not a current execution
/// contract.
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
    /// Resulting lease expiry when this command explicitly renewed possession.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub expiry: Option<Box<crate::Instant>>,
    /// Non-blocking overlap warnings discovered from the claim-time board.
    #[serde(default, skip_serializing_if = "claim_warnings_empty")]
    pub warnings: Box<[ClaimWarning]>,
    /// Executable creation-context command or stable unavailable reason on fresh assignment.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub creation_context: Option<Box<str>>,
    /// Exact CLI command admitted as the next action, when one is available.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub next: Option<Box<str>>,
    /// Phase the claim landed in, with the one deliverable it authorizes.
    ///
    /// A lane reads this where it wins the work, because the phase decides
    /// whether the next action touches board state or repository files.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub phase: Option<ClaimPhase>,
}

fn claim_warnings_empty(warnings: &[ClaimWarning]) -> bool {
    warnings.is_empty()
}

/// The phase one claim answer was assigned in, and what it authorizes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimPhase {
    /// Phase as the workflow spells it.
    pub name: Phase,
    /// The one deliverable this phase authorizes, when it names one.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub deliverable: Option<Box<str>>,
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

/// Run the named eligibility rule and atomically win its claim.
///
/// Ordinary allocation rules select from board state. Explicit recovery binds
/// the exact task through its sealed transaction and matching witnesses, so the
/// recovery identity is the sole additional selector carried by this intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimWin {
    /// Named eligibility rule the query runs.
    pub rule: Name,
    /// Exact dirty-handoff transaction, present only for explicit recovery.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub recovery: Option<String>,
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
