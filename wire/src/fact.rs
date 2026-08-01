//! The closed fact vocabulary.
//!
//! Each kind is a plane-crossing verb. A kind is a path of single words and
//! always appears as a field, never as an object key. The set is closed: a
//! spelling absent from this enum is not a fact.

use serde::{Deserialize, Serialize};

/// Every admitted fact discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum FactKind {
    /// A task statement and initial workflow head were created.
    #[serde(rename = "task::added")]
    TaskAdded,
    /// A claimed phase completed and workflow advanced.
    #[serde(rename = "task::phased")]
    TaskPhased,
    /// A task completed under a re-proven active claim.
    #[serde(rename = "task::done")]
    TaskDone,
    /// A review verdict landed with reviewer and assignment witnesses.
    #[serde(rename = "task::reviewed")]
    TaskReviewed,
    /// The allocator seated a reviewer under the anti-self-review witness.
    #[serde(rename = "task::assigned")]
    TaskAssigned,
    /// A durable task note appended.
    #[serde(rename = "task::noted")]
    TaskNoted,
    /// A task received a wakeable deferral.
    #[serde(rename = "task::deferred")]
    TaskDeferred,
    /// A task's deferral cleared.
    #[serde(rename = "task::woken")]
    TaskWoken,
    /// An unsettled task statement changed.
    #[serde(rename = "task::edited")]
    TaskEdited,

    /// Exclusive possession was minted from current eligibility and claim heads.
    #[serde(rename = "claim::won")]
    ClaimWon,
    /// Possession passed to a successor with ancestor lineage preserved.
    #[serde(rename = "claim::carried")]
    ClaimCarried,
    /// Possession was released and the task became eligible again.
    #[serde(rename = "claim::freed")]
    ClaimFreed,
    /// The lease deadline passed and the resident expired possession.
    #[serde(rename = "claim::lapsed")]
    ClaimLapsed,
    /// The resident extended a live, epoch-bound lease within its maximum.
    #[serde(rename = "claim::renewed")]
    ClaimRenewed,

    /// The exact normalized plan digest landed at its witnessed board barrier.
    #[serde(rename = "doc::applied")]
    DocApplied,

    /// A directive was created.
    #[serde(rename = "steer::sent")]
    SteerSent,
    /// The same intent was sent again with lineage and escalation carried.
    #[serde(rename = "steer::resent")]
    SteerResent,
    /// A live directive was acknowledged at a transcript locus.
    #[serde(rename = "steer::acked")]
    SteerAcked,
    /// A live directive was declined at a transcript locus.
    #[serde(rename = "steer::declined")]
    SteerDeclined,
    /// An ACK named no live directive; nothing retired.
    #[serde(rename = "steer::missed")]
    SteerMissed,
    /// A directive reached expiry without a valid disposition.
    #[serde(rename = "steer::lapsed")]
    SteerLapsed,
    /// The operator withdrew a live directive.
    #[serde(rename = "steer::canceled")]
    SteerCanceled,

    /// An ACP permission request arrived, bound to its cause.
    #[serde(rename = "consent::asked")]
    ConsentAsked,
    /// A named rule or operator allowed the request.
    #[serde(rename = "consent::granted")]
    ConsentGranted,
    /// A named rule or operator denied the request.
    #[serde(rename = "consent::denied")]
    ConsentDenied,
    /// The request pended to the operator while reversible work continued.
    #[serde(rename = "consent::queued")]
    ConsentQueued,
    /// A non-ACP lane's consent floor was recorded once for its occupancy.
    #[serde(rename = "consent::floored")]
    ConsentFloored,

    /// A lane and its capability were created.
    #[serde(rename = "lane::seated")]
    LaneSeated,
    /// An occupant became live under the lane.
    #[serde(rename = "lane::started")]
    LaneStarted,
    /// The resident observed the occupant end.
    #[serde(rename = "lane::exited")]
    LaneExited,
    /// The occupant crossed a context compaction boundary.
    #[serde(rename = "lane::compacted")]
    LaneCompacted,
    /// A successor occupant was seated with its ancestor named.
    #[serde(rename = "lane::renewed")]
    LaneRenewed,
    /// A graceful occupant end completed.
    #[serde(rename = "lane::drained")]
    LaneDrained,
    /// Work run without the resident left a marker recognized once.
    #[serde(rename = "lane::degraded")]
    LaneDegraded,
    /// Repeated young occupant deaths paused automatic re-seating.
    #[serde(rename = "lane::held")]
    LaneHeld,
    /// A held lane returned to seating eligibility.
    #[serde(rename = "lane::resumed")]
    LaneResumed,

    /// The judge confirmed a taste at an evidence locus.
    #[serde(rename = "taste::confirmed")]
    TasteConfirmed,
    /// The judge cleared a taste at an evidence locus.
    #[serde(rename = "taste::cleared")]
    TasteCleared,

    /// An expected listening event remained absent past its armed window.
    #[serde(rename = "canary::alarmed")]
    CanaryAlarmed,
    /// Listening meters disagreed: activity existed without decoded context.
    #[serde(rename = "canary::contradicted")]
    CanaryContradicted,

    /// An unproven vendor version first ran with residue measurement armed.
    #[serde(rename = "vendor::armed")]
    VendorArmed,

    /// An executable surface digest changed and entered quarantine.
    #[serde(rename = "trust::pended")]
    TrustPended,
    /// The operator approved the exact executable digest.
    #[serde(rename = "trust::granted")]
    TrustGranted,
    /// The operator withdrew approval from the exact executable digest.
    #[serde(rename = "trust::revoked")]
    TrustRevoked,

    /// The resident recognized a repository commit and bound its receipt verdict.
    #[serde(rename = "commit::landed")]
    CommitLanded,

    /// The resident recognized a new gate finding; the Finding head opened.
    #[serde(rename = "gate::found")]
    GateFound,
    /// A standing finding resolved at its evidence; the Finding head closed.
    #[serde(rename = "gate::cleared")]
    GateCleared,

    /// The one forward schema migration finalized before epoch mint.
    #[serde(rename = "journal::migrated")]
    JournalMigrated,
    /// A quiesced backup bundle was produced with its enumerated tape manifest.
    #[serde(rename = "journal::backed")]
    JournalBacked,
    /// A restore completed and the new epoch recognized the exchanged authority.
    #[serde(rename = "journal::restored")]
    JournalRestored,

    /// The resident acquired the kernel writer lock and minted its epoch.
    #[serde(rename = "up::seated")]
    UpSeated,
    /// The resident stopped admission and began a graceful end.
    #[serde(rename = "up::quiescing")]
    UpQuiescing,
    /// The resident began preserving admitted work and ending occupants.
    #[serde(rename = "up::draining")]
    UpDraining,
    /// Grace expired and the resident began ending unfinished occupants.
    #[serde(rename = "up::terminating")]
    UpTerminating,
    /// Bounded escalation began against occupants that survived termination.
    #[serde(rename = "up::killing")]
    UpKilling,
    /// Occupants ended; verification of claims, seals, and residue began.
    #[serde(rename = "up::verifying")]
    UpVerifying,
    /// The verified terminal state committed before releasing authority.
    #[serde(rename = "up::dark")]
    UpDark,

    /// The resident recognized a valid evidence bundle.
    #[serde(rename = "proof::sealed")]
    ProofSealed,

    /// The sealed manifest and each target artifact digest published.
    #[serde(rename = "release::published")]
    ReleasePublished,
}

/// An internally tagged fact: its discriminant beside its kind-specific body.
///
/// The body is a type parameter because each kind registers exactly one body
/// DTO; the registry that binds kinds to bodies and consuming folds is
/// generated separately and is not a second decoder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact<F> {
    /// Discriminant from the closed vocabulary.
    pub kind: FactKind,
    /// Kind-specific body, flattened beside the discriminant.
    #[serde(flatten)]
    pub body: F,
}

impl<F> Fact<F> {
    /// Pair a discriminant with its body.
    pub const fn new(kind: FactKind, body: F) -> Self {
        Self { kind, body }
    }
}
