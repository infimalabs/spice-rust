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

use crate::OccupantAddress;
use crate::envelope::{Actor, Seq};
use crate::head::TranscriptSourceHead;
use crate::ident::{CanonicalUuid, Digest, Instant};
use crate::witness::{Phase, TaskStamp};

#[path = "body/transcript.rs"]
mod transcript;

pub use transcript::{JournalIngested, TranscriptPosition, TranscriptSpan, TranscriptUsage};

mod publication;
pub use publication::{ProofSealed, PublishedTarget, RackAdmitted, ReleasePublished};

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
///
/// Operator identity is repository Git configuration, never a setting. Facts
/// recorded while settings still carried identity keep those fields so their
/// bodies round-trip exactly, but the fields are private: [`Self::new`] is the
/// only way to mint a body, and no reader can treat them as current.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalSettingsChanged {
    /// Whether supported agent vendors launch in fast mode.
    pub fast_mode: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    operator_name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    operator_email: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    operator: Option<crate::HumanIdentity>,
}

impl GlobalSettingsChanged {
    /// A settings change that records global fast mode and nothing else.
    #[must_use]
    pub const fn new(fast_mode: bool) -> Self {
        Self {
            fast_mode,
            operator_name: None,
            operator_email: None,
            operator: None,
        }
    }
}

/// A directive was created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerSent {
    /// Durable directive identity, equal to the command that created it.
    pub directive: CanonicalUuid,
    /// Operator submit action that may fan out into target-specific directives.
    ///
    /// Every operator send carries it; peer and resident directives omit it.
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
    /// Anchor task whose room context a peer route froze at publication.
    ///
    /// Absent on operator direction, peer replies, direct peer routes, and
    /// every message older than room routes.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub room: Option<crate::rooms::RoomAnchor>,
    /// Steering control the directive drives; absent on plain prose.
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
            room: None,
            control: Some(crate::peer_reply_control(&self.answered, self.answer)),
            attachments: Vec::new(),
            expires: self.expires,
        }
    }
}

/// An authored MSG packet opened one peer message to one exact recipient.
///
/// The packet's assistant transcript record is the source evidence, and the
/// resident journals one fact per admitted recipient in a single guarded
/// write. The directive identity is derived from the source packet and the
/// address the route named, so rereading the same packet never opens a second
/// message. The opened directive is ordinary peer steering bound to the
/// recipient lane, which whichever occupant sits there hears and answers;
/// answering it replies to `from`. A route naming an earlier occupant of a
/// lane the residency now seats with a proved successor delivers to that
/// successor and keeps the named address as `addressed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SteerMessaged {
    /// Durable message identity, derived from the source packet and recipient.
    pub directive: CanonicalUuid,
    /// Message key.
    pub key: String,
    /// Recipient lane, the message's destination.
    pub lane: CanonicalUuid,
    /// Recipient occupant the residency seated when the packet published: the
    /// route's evidence, not its destination.
    pub occupant: CanonicalUuid,
    /// Lane whose occupant authored the packet.
    pub from: CanonicalUuid,
    /// Occupant whose captured transcript authored the packet.
    pub sender: CanonicalUuid,
    /// The packet's exact body.
    pub text: String,
    /// Assistant transcript locus of the packet's MSG header.
    #[serde(with = "sourced_transcript_locus")]
    pub locus: TranscriptLocus,
    /// Anchor task whose room context the route named, frozen at publication;
    /// absent on a direct route and on every message older than room routes.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub room: Option<crate::rooms::RoomAnchor>,
    /// The earlier address the route named when `occupant` is its proved
    /// successor on the same lane; absent when the route named `occupant`'s
    /// own address.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub addressed: Option<OccupantAddress>,
    /// Instant the message expires without a disposition.
    pub expires: Instant,
}

impl SteerMessaged {
    /// The directive this message opens, as the publication steering folds.
    #[must_use]
    pub fn as_sent(&self) -> SteerSent {
        SteerSent {
            directive: self.directive,
            fanout: None,
            key: self.key.clone(),
            lane: self.lane,
            text: self.text.clone(),
            from: Some(self.from),
            room: self.room.clone(),
            control: None,
            attachments: Vec::new(),
            expires: self.expires,
        }
    }
}

/// An authored MSG (or a retained ASK) opened one peer directive while its
/// proved destination lane was vacant.
///
/// The original occupant address proves the logical lane but is not a seated
/// recipient. The directive stays live for that lane's next occupant without
/// changing its key or expiry, and peer traffic grants no wake authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SteerQueued {
    /// Durable message identity, derived from the source packet and address.
    pub directive: CanonicalUuid,
    /// Message key.
    pub key: String,
    /// Vacant logical lane that inherits this directive at its next seat.
    pub lane: CanonicalUuid,
    /// Exact historical address whose unique birth proved `lane`.
    pub recipient: OccupantAddress,
    /// Lane whose occupant authored the packet.
    pub from: CanonicalUuid,
    /// Occupant whose captured transcript authored the packet.
    pub sender: CanonicalUuid,
    /// The packet's exact body.
    pub text: String,
    /// Assistant transcript locus of the packet's header.
    #[serde(with = "sourced_transcript_locus")]
    pub locus: TranscriptLocus,
    /// Frozen room context, when the route named one.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub room: Option<crate::rooms::RoomAnchor>,
    /// Instant the queued directive expires without a disposition.
    pub expires: Instant,
}

impl SteerQueued {
    /// The lane-scoped directive this queued route opens.
    #[must_use]
    pub fn as_sent(&self) -> SteerSent {
        SteerSent {
            directive: self.directive,
            fanout: None,
            key: self.key.clone(),
            lane: self.lane,
            text: self.text.clone(),
            from: Some(self.from),
            room: self.room.clone(),
            control: None,
            attachments: Vec::new(),
            expires: self.expires,
        }
    }
}

/// An authored MSG packet resolved against the residency and published to no
/// one.
///
/// The refusal is the packet's settled judgment: a later reread of the same
/// source record, including startup recovery, finds it and leaves the packet
/// unpublished even when the cause of the refusal has since changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SteerWithheld {
    /// Lane whose occupant authored the packet.
    pub from: CanonicalUuid,
    /// Occupant whose captured transcript authored the packet.
    pub sender: CanonicalUuid,
    /// Assistant transcript locus of the packet's MSG header.
    #[serde(with = "sourced_transcript_locus")]
    pub locus: TranscriptLocus,
    /// Stable refusal reason.
    pub reason: String,
    /// What made the packet unpublishable when it was judged.
    pub detail: String,
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
    /// Evidence derived without a durable tape generation, such as the current
    /// taste surface, omits it. Fact families whose authority comes from a
    /// captured transcript enforce presence on their own body codec.
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

/// Enforce source qualification on authority families that only current tape
/// records may create while retaining the shared optional locus for taste
/// evidence whose current contract has no tape generation.
mod sourced_transcript_locus {
    use serde::{Deserialize as _, Deserializer, Serialize as _, Serializer};

    use super::TranscriptLocus;

    pub(super) fn serialize<S>(locus: &TranscriptLocus, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if locus.source.is_none() {
            return Err(serde::ser::Error::custom(
                "source-qualified transcript locus omits its tape generation",
            ));
        }
        locus.serialize(serializer)
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<TranscriptLocus, D::Error>
    where
        D: Deserializer<'de>,
    {
        let locus = TranscriptLocus::deserialize(deserializer)?;
        if locus.source.is_none() {
            return Err(serde::de::Error::custom(
                "source-qualified transcript locus omits its tape generation",
            ));
        }
        Ok(locus)
    }
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
    #[serde(with = "sourced_transcript_locus")]
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
    #[serde(with = "sourced_transcript_locus")]
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
    #[serde(with = "sourced_transcript_locus")]
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
    ConsentAsked, ConsentDenied, ConsentGranted, ConsentLapsed, ConsentQueueCause, ConsentQueued,
};

#[path = "body/lane.rs"]
mod lane;

pub use lane::{
    LaneArranged, LaneCancelled, LaneCompacted, LaneDegraded, LaneDrained, LaneEffortRetuned,
    LaneExited, LaneFastRetuned, LaneHeld, LaneImported, LaneKilled, LaneLaunchIdentity,
    LaneRemoved, LaneRenewalChanged, LaneRenewed, LaneResumed, LaneRetuned, LaneRuntimeRetuned,
    LaneSeated, LaneSessionDeclared, LaneSignalled, LaneSourceBoundary, LaneStarted, LaneTimedOut,
    RtkHealth, RtkHealthState, TasteDecision, TasteJudgeAttempt, TasteJudgeReply, TasteJudged,
    TasteReminded, TeamArranged, TeamConfigured, TeamStanding, VendorFailureInput,
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

/// A retired fact crossed once into inert current-schema evidence.
///
/// The outer envelope keeps its durable position, attribution, and causal
/// links. `fact` preserves the retired discriminant and body as opaque JSON:
/// current readers validate this marker but never decode that nested payload as
/// an executable fact. The retained payloads current projections still read
/// are a judge ruling from before provenance-complete judgments, see
/// [`JournalRetained::taste_ruling`], a task note from before notes became
/// room observations, see [`JournalRetained::task_note`], and an operator
/// Design nudge, see [`crate::rooms::RetainedNudge`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalRetained {
    /// Release that last owned the retired fact.
    pub release: String,
    /// Retired fact discriminant and body, preserved as inert evidence.
    pub fact: serde_json::Value,
}

impl JournalRetained {
    /// The judge ruling this evidence retains, when it retains one.
    ///
    /// Before a ruling carried its full provenance, the judge recorded only
    /// the taste, the evidence locus, and the verdict as `taste::confirmed` or
    /// `taste::cleared`. Those rulings still count toward a taste's standing
    /// judgment tally and read back in its history, without the provenance a
    /// current [`TasteJudged`] carries.
    #[must_use]
    pub fn taste_ruling(&self) -> Option<RetainedTasteRuling> {
        let confirmed = match self.fact.get("kind")?.as_str()? {
            "taste::confirmed" => true,
            "taste::cleared" => false,
            _ => return None,
        };
        let field = |name: &str| Some(self.fact.get(name)?.as_str()?.to_owned());
        Some(RetainedTasteRuling {
            taste: field("taste")?,
            locus: field("locus")?,
            confirmed,
        })
    }

    /// The task note this evidence retains, when it retains one.
    ///
    /// A `task::noted` fact appended free text to its task before notes became
    /// room observations. A retained note that is no taste-review audit still
    /// reads as an authored observation in its task's room, and every retained
    /// note stays a historical change to its task.
    #[must_use]
    pub fn task_note(&self) -> Option<RetainedTaskNote> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            #[serde(rename = "kind")]
            _kind: serde::de::IgnoredAny,
            task: TaskStamp,
            note: String,
        }
        if self.fact.get("kind")?.as_str()? != RetainedTaskNote::KIND {
            return None;
        }
        let Wire { task, note, .. } = Wire::deserialize(&self.fact).ok()?;
        Some(RetainedTaskNote { task, note })
    }
}

/// A judge ruling retained from before provenance-complete judgments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedTasteRuling {
    /// Taste the judge ruled on.
    pub taste: String,
    /// Evidence locus the judge ruled at.
    pub locus: String,
    /// Whether the judge confirmed the taste rather than clearing it.
    pub confirmed: bool,
}

/// A task note retained from before notes became room observations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedTaskNote {
    /// Annotated task.
    pub task: TaskStamp,
    /// Note text.
    pub note: String,
}

impl RetainedTaskNote {
    /// Kind the note carried before it crossed into retained evidence.
    pub const KIND: &'static str = "task::noted";

    /// The note `fact` retains, when it is retained evidence of one.
    #[must_use]
    pub fn read(fact: &crate::AnyFact) -> Option<Self> {
        fact.as_body::<JournalRetained>()?.task_note()
    }
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

/// The resident released coordination authority without ending detached lane work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpDark {
    /// Resident epoch releasing authority.
    pub epoch: CanonicalUuid,
}
