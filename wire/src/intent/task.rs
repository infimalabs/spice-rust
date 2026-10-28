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
    /// One plain sentence saying why the task matters; the write refuses an
    /// absent or empty pitch.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pitch: String,
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
    /// Declared Plan or Design intent the task opens with.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub intent: Option<crate::TaskIntent>,
    /// Existing Plan or Design context this Plan prepares within.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub preparation: Option<TaskHandle>,
    /// Whether creation must atomically place the caller's lane in possession.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub claim: bool,
    /// Record editable Draft content instead of filed work.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub draft: bool,
}

/// Add one acceptance-bearing line-item Plan under the current opportunistic
/// OOPS Design generation.
///
/// The route owns the flow, preparation, and construction contract. A caller
/// names only the public owner project, or lets one active public claim supply
/// it, so this DTO cannot become generic task creation. `design` names either
/// the exact current generation or the intended absent generation atomically
/// opened with this report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskOopsAdd {
    /// Current or intended opportunistic OOPS Design generation.
    pub design: TaskStamp,
    /// Exact account of the unexpected behavior, retained as its Plan title.
    pub title: String,
    /// Explicit causal authority when no active claim owns the report.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub origin: Option<Origin>,
    /// Public owner project; omission uses the caller's one active public
    /// claim project.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub project: Option<String>,
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
    /// Replacement pitch, when the pitch moves.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub pitch: Option<String>,
    /// Replacement description, when the description moves; empty clears it.
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
    /// Dedicated declaration or withdrawal of Plan or Design intent.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub intent: Option<crate::TaskIntentEdit>,
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
    /// Replacement phase route; only an unfiled Draft's route moves.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub flow: Option<String>,
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

/// Close a task as completed on the operator's word.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskClose {
    /// Task the operator closes.
    pub task: TaskHandle,
    /// Evidence or reason the task is complete; recorded as its validation.
    pub validation: String,
    /// Override a live claim rather than refusing the closure.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub force_claimed: bool,
}

/// Conclude an open, unheld Design on the operator's word.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskConclude {
    /// Design the operator concludes.
    pub task: TaskHandle,
    /// Done, or collapsed with the reason its premise did not hold.
    pub conclusion: crate::body::DesignConclusion,
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

/// Clear the current deferral, optionally claiming the filed task atomically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskWake {
    /// Filed deferred task being woken.
    pub task: TaskHandle,
    /// Wake and atomically claim the task in its current lifecycle state. A
    /// Gathering Design remains unsteeped and gathering input.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub claim: bool,
}

/// File one unfiled Draft as ordinary work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskFile {
    /// Draft task being filed.
    pub task: TaskHandle,
    /// Public project receiving the filed task; omission keeps its owner.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub into: Option<String>,
    /// Pending Plan or Design that controls the filed output.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub preparation: Option<TaskHandle>,
    /// File and atomically claim the resulting ready task.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub claim: bool,
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

/// Add or remove dependency edges between existing tasks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDepends {
    /// Task that follows.
    pub task: TaskHandle,
    /// Tasks it must follow.
    pub after: Vec<TaskHandle>,
    /// Remove the named edges instead of adding them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove: bool,
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
    /// Why a Plan's plan phase completes without constructing work.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub disposition: Option<crate::body::PlanDisposition>,
}

impl TaskDone {
    /// High bits naming an operation [`Self::operation_for_claim`] derived.
    const CLAIM_OPERATION_PREFIX: u128 = 0x7370_6963_652d_0000_0000_0000_0000_0000;

    /// Derive the one default completion operation owned by a claim.
    #[must_use]
    pub fn operation_for_claim(claim: Seq) -> CanonicalUuid {
        CanonicalUuid::new(uuid::Uuid::from_u128(
            Self::CLAIM_OPERATION_PREFIX | u128::from(claim.get()),
        ))
    }

    /// The claim whose default completion operation `operation` is, when
    /// [`Self::operation_for_claim`] derived it.
    #[must_use]
    pub fn claim_for_operation(operation: CanonicalUuid) -> Option<Seq> {
        let value = operation.get().as_u128();
        if value & !u128::from(u64::MAX) != Self::CLAIM_OPERATION_PREFIX {
            return None;
        }
        Seq::new(u64::try_from(value & u128::from(u64::MAX)).ok()?)
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

/// The answer a task command returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskAnswer {
    /// Taste hold the transaction left standing on the task's statement,
    /// first so its author reads every match before anything else.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub taste: Option<crate::TasteHold>,
    /// Current acceptance authority, independent of the statement's taste hold.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub completion_contract: Option<Box<str>>,
    /// Task the answer concerns.
    pub task: TaskHandle,
    /// Its status after the transaction.
    pub status: String,
    /// Durable completion operation still running before this task may move.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub completion: Option<Box<TaskCompletionAnswer>>,
    /// Active claim minted in the same transaction, when creation also boards the caller.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub claim: Option<Box<ClaimAnswer>>,
    /// Pending controlled tasks closed atomically with the named task.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub closed: Vec<TaskHandle>,
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

/// The answer a claim command returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimAnswer {
    /// Task whose possession moved.
    pub task: TaskHandle,
    /// The won task's statement on fresh assignment, whole, because its
    /// holder works from it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub statement: Option<Box<ClaimStatement>>,
    /// Actor holding it after the transaction, absent when possession was released.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub holder: Option<Actor>,
    /// Resulting lease expiry for legacy explicit-renewal records.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub expiry: Option<Box<crate::Instant>>,
    /// Task generation accepted by `task next`, so the holder knows completion
    /// is admitted against the current statement.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub acknowledged: Option<u64>,
    /// The lease sits at the indefinite approval horizon and stayed there
    /// while `task next` accepted the generation.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub held: bool,
    /// Non-blocking overlap warnings discovered from the claim-time board.
    #[serde(default, skip_serializing_if = "claim_warnings_empty")]
    pub warnings: Box<[ClaimWarning]>,
    /// A steal found no live holder: it claimed the task with nobody
    /// displaced, as a capture would have.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub nothing_stolen: bool,
    /// Executable creation-context command or stable unavailable reason on fresh assignment.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub creation_context: Option<Box<str>>,
    /// Originating steering directive rendered for fresh task hydration.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub origin_context: Option<Box<str>>,
    /// The task's room conversation, newest first under the interactive read
    /// bounds, with the `task show` command that pages the rest; absent when
    /// the room holds no publication.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub room_context: Option<Box<str>>,
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
    /// What claiming did with commits a previous holder left parked on the task.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub transplant: Option<ClaimTransplant>,
}

fn claim_warnings_empty(warnings: &[ClaimWarning]) -> bool {
    warnings.is_empty()
}

/// A won task's statement as its holder works from it: title, pitch, and the
/// whole description and acceptance (a Design's open questions).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimStatement {
    /// Authored title.
    pub title: String,
    /// One plain sentence saying why the task matters.
    pub pitch: String,
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
    /// Current own, linked, or unspecified acceptance contract.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub completion_contract: Option<Box<str>>,
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

/// Parked commits replayed onto the lane HEAD a claim fast-forwarded to.
///
/// Every parked commit lands in exactly one list, except the one the replay
/// stopped on, which `conflict` names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimTransplant {
    /// Lane HEAD the parked commits were replayed onto.
    pub onto: String,
    /// Commits replayed cleanly, oldest first, with their new identities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applied: Vec<TransplantedCommit>,
    /// Commits whose changes the lane already held, so nothing was replayed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub present: Vec<String>,
    /// Commits Git no longer holds, which could not be replayed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    /// The replay Git stopped on, left in the worktree for the claimant.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub conflict: Option<TransplantConflict>,
    /// Worktree snapshot restored uncommitted, or waiting behind a paused replay.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub snapshot: Option<String>,
}

/// One parked commit and the commit its replay created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransplantedCommit {
    /// Parked commit identity.
    pub parked: String,
    /// Commit created on the claimant's lane.
    pub commit: String,
}

/// A replay paused in the worktree as a Git cherry-pick with conflicts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransplantConflict {
    /// Parked commit whose cherry-pick stopped.
    pub commit: String,
    /// Paths Git left conflicted.
    pub paths: Vec<String>,
    /// Later parked commits queued behind it in Git's cherry-pick sequence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remaining: Vec<String>,
    /// How to finish the replay.
    pub remedy: Box<str>,
}

/// One active claim whose acceptance names the same implementation surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimWarning {
    /// Other in-flight task.
    pub task: TaskHandle,
    /// Lane holding the other claim.
    pub lane: CanonicalUuid,
    /// Rare exact identifiers both statements name, then their strongest
    /// shared weighted terms.
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

/// Retained explicit-renewal request evidence; no current command executes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimRenew {
    /// Task whose possession was refreshed.
    pub task: TaskHandle,
    /// Requested lease duration in whole seconds.
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
