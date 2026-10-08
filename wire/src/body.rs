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

use crate::envelope::{Actor, Seq};
use crate::head::TranscriptSourceHead;
use crate::ident::{CanonicalUuid, Digest, Instant};
use crate::witness::{Phase, TaskStamp};

#[path = "body/transcript.rs"]
mod transcript;

pub use transcript::{JournalIngested, TranscriptPosition, TranscriptSpan, TranscriptUsage};

mod publication;
pub use publication::{ProofSealed, PublishedTarget, ReleasePublished};

mod task;
pub use task::*;

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

/// The global app settings changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalSettingsChanged {
    /// Whether supported agent vendors launch in fast mode.
    pub fast_mode: bool,
    /// Explicit operator display name; absent in predecessor facts and when Git supplies it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub operator_name: Option<String>,
    /// Explicit operator email; absent in predecessor facts and when Git supplies it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub operator_email: Option<String>,
    /// Effective identity at the time of this mutation; absent in predecessor facts.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub operator: Option<crate::OperatorIdentity>,
}

/// A directive was created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerSent {
    /// Durable directive identity, equal to the command that created it.
    pub directive: CanonicalUuid,
    /// Operator submit action that may fan out into target-specific directives.
    ///
    /// Facts written before fanout provenance omit this value. Current operator
    /// sends always carry it; consumers must use their named compatibility rule
    /// for an absent value rather than infer equality from transcript loci.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub fanout: Option<CanonicalUuid>,
    /// Directive key.
    pub key: String,
    /// Lane the directive addresses.
    pub lane: CanonicalUuid,
    /// Directive text.
    pub text: String,
    /// Seated lane that told this directive as a peer message.
    ///
    /// Absent on operator and resident direction, which is every fact written
    /// before peer messages existed. Present only when the creating fact's
    /// actor is exactly this lane.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub from: Option<CanonicalUuid>,
    /// Control channel the directive drives; absent on plain prose.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub control: Option<String>,
    /// Admitted browsable images carried with this direction.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<DirectiveAttachment>,
    /// Instant the directive expires without a disposition.
    pub expires: Instant,
}

/// A lane's answer to a live peer message opened a reply to the lane that told it.
///
/// Minted in the same write that retires the answered peer message, so the
/// reply exists exactly when the answer does. The reply is ordinary steering
/// for the lane that told the peer message; answering it earns no reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerReplied {
    /// Durable reply identity, derived from the answered directive.
    pub directive: CanonicalUuid,
    /// Reply key.
    pub key: String,
    /// Lane the reply addresses: the lane that told the answered peer message.
    pub lane: CanonicalUuid,
    /// Lane that answered the peer message, speaking as the reply's sender.
    pub from: CanonicalUuid,
    /// Key of the answered peer message.
    pub answered: String,
    /// How the answering lane answered.
    pub answer: crate::PeerAnswer,
    /// The answer as spoken: marker, answered key, and the header's clause.
    pub text: String,
    /// Instant the reply expires without a disposition.
    pub expires: Instant,
}

impl SteerReplied {
    /// The directive this reply opens, as the publication steering folds.
    #[must_use]
    pub fn as_sent(&self) -> SteerSent {
        SteerSent {
            directive: self.directive,
            fanout: None,
            key: self.key.clone(),
            lane: self.lane,
            text: self.text.clone(),
            from: Some(self.from),
            control: Some(crate::peer_reply_control(&self.answered, self.answer)),
            attachments: Vec::new(),
            expires: self.expires,
        }
    }
}

/// Compatibility spelling for the one shared media value.
pub type DirectiveAttachment = crate::Media;

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
    /// Opaque identity of the durable tape generation carrying this record.
    ///
    /// Legacy facts predate source-aware transcript evidence. Consumers must
    /// treat an absent source as lane-scoped rather than guessing that equal
    /// numeric coordinates came from one tape.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub source: Option<CanonicalUuid>,
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
    ConsentAsked, ConsentDenied, ConsentFloored, ConsentGranted, ConsentLapsed, ConsentQueueCause,
    ConsentQueued,
};

#[path = "body/lane.rs"]
mod lane;

pub use lane::{
    LaneArranged, LaneCancelled, LaneCompacted, LaneDegraded, LaneDrained, LaneEffortRetuned,
    LaneExited, LaneFastRetuned, LaneHeld, LaneImported, LaneKilled, LaneLaunchIdentity,
    LaneRemoved, LaneRenewalChanged, LaneRenewed, LaneResumed, LaneRetuned, LaneRuntimeRetuned,
    LaneSeated, LaneSessionDeclared, LaneSignalled, LaneSourceBoundary, LaneStarted, LaneTimedOut,
    RtkHealth, RtkHealthState, TasteCleared, TasteConfirmed, TasteDecision, TasteJudgeAttempt,
    TasteJudgeReply, TasteJudged, TasteReminded, TeamArranged, TeamConfigured, TeamStanding,
    VendorFailureInput,
};

/// A seated occupant exceeded a call-count or active-silence narration bound.
///
/// One prod is one row. `moments` counts calls since visible prose; `count`
/// is shared by call and elapsed-time thresholds, so the same reminder is not
/// repeated when both become due. Prose or turn completion records a
/// `NarrationCleared` row. The meter reads the newest prod and clearance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NarrationProdded {
    /// Lane whose occupant went silent.
    pub lane: CanonicalUuid,
    /// Occupant that kept working without prose.
    pub occupant: CanonicalUuid,
    /// Tool calls issued since the occupant's last authored prose.
    pub moments: u32,
    /// Ordinal of this prod within the streak, from one.
    pub count: u32,
}

/// A prodded occupant's silent streak ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NarrationCleared {
    /// Lane whose occupant was prodded.
    pub lane: CanonicalUuid,
    /// Occupant the streak belonged to.
    pub occupant: CanonicalUuid,
    /// Tool calls the streak ran to.
    pub moments: u32,
    /// Prods the streak drew.
    pub prods: u32,
    /// What ended the streak.
    pub relief: NarrationRelief,
}

/// What ended a prodded occupant's silent streak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NarrationRelief {
    /// The occupant authored visible prose.
    Prose,
    /// The occupant's turn completed without prose.
    Turn,
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
    /// Canonical worktree whose executable surface produced this digest.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub worktree: Option<String>,
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

/// A gate finding dispatch completed under the retired filing path.
///
/// Historical only: the filing command is gone, and this body exists so the
/// journal keeps decoding the `gate::filed` facts it already holds.
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

/// The resident released coordination authority without ending detached lane work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpDark {
    /// Resident epoch releasing authority.
    pub epoch: CanonicalUuid,
}
