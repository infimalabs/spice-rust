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

/// A task completed under a re-proven active claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDone {
    /// Completed task.
    pub task: TaskHandle,
    /// Evidence the completion rests on.
    pub validation: String,
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
    /// Instant the directive expires without a disposition.
    pub expires: Instant,
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

/// A live directive was acknowledged at a transcript locus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerAcked {
    /// Acknowledged directive key.
    pub key: String,
    /// Lane that acknowledged.
    pub lane: CanonicalUuid,
    /// Transcript locus carrying the acknowledgement.
    pub locus: String,
}

/// A live directive was declined at a transcript locus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerDeclined {
    /// Declined directive key.
    pub key: String,
    /// Lane that declined.
    pub lane: CanonicalUuid,
    /// Transcript locus carrying the refusal.
    pub locus: String,
    /// Stated reason the directive cannot be done.
    pub reason: String,
}

/// An ACK named no live directive; the unmatched key was recorded and nothing retired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerMissed {
    /// Unmatched key the acknowledgement named.
    pub key: String,
    /// Lane that named it.
    pub lane: CanonicalUuid,
    /// Transcript locus the unmatched key appeared at.
    pub locus: String,
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
    /// Created lane.
    pub lane: CanonicalUuid,
    /// Capability minted with the lane.
    pub capability: CanonicalUuid,
    /// Worktree the lane is bound to.
    pub worktree: String,
    /// Closed lane-life spelling selected for the seat.
    pub life: String,
    /// Vendor selected for occupants of this seat.
    pub vendor: Name,
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
