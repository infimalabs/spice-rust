//! Native task-room identities and authority DTOs.
//!
//! A room is never assigned another identity: its anchor task's [`TaskStamp`]
//! is the room identity. The types here add only the authority that a task does
//! not already own: source links, saved working-group organization, retained
//! candidates and their review evidence, decisions, and exact output
//! authorization. They do not define another task lifecycle or lane roster.

use std::fmt;
use std::str::FromStr;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

mod contribution;
mod conversation;
mod cycle;
mod decision;
mod maturity;
mod notice;
mod relevance;
mod stop;
mod withdrawal;
mod working_group;

pub use contribution::*;
pub use conversation::*;
pub use cycle::*;
pub use decision::*;
pub use maturity::*;
pub use notice::*;
pub use relevance::*;
pub use stop::*;
pub use withdrawal::*;
pub use working_group::*;

use crate::{
    CanonicalUuid, Digest, MessageLocus, Name, Origin, Seq, SequenceCode, TaskHandle,
    TaskPreparation, TaskStamp, TranscriptSourceHead,
};

/// The identity of a room: exactly the anchor task's durable birth stamp.
pub type RoomAnchor = TaskStamp;
/// Most room anchors one working group value may decode with; admission
/// further caps a group at [`WORKING_GROUP_DESIGN_LIMIT`] designs.
pub const WORKING_GROUP_ANCHOR_LIMIT: usize = 100;
/// Most designs a working group holds, matching the lanes one team admits.
pub const WORKING_GROUP_DESIGN_LIMIT: usize = crate::head::TEAM_MEMBER_LIMIT;

/// A current nine-digit native moment stamp for non-task room authority.
/// Working groups and Design pass proposals need durable identities but are not
/// tasks. This wrapper reuses the native vowel-free base-52 microsecond codec
/// while refusing the eight-digit legacy task spelling; no UUID or decimal
/// protocol handle is introduced.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RoomAuthorityStamp(TaskStamp);

/// Text that does not spell a current room-authority stamp.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[error("room authority stamp must be nine native base-52 characters including a letter: {0:?}")]
pub struct RoomAuthorityStampError(String);

/// Epoch moment that cannot mint a non-decimal current room-authority stamp.
#[derive(Clone, Copy, Debug, thiserror::Error, PartialEq, Eq)]
pub enum RoomAuthorityStampMintError {
    /// Moment is outside the native nine-digit microsecond space.
    #[error(transparent)]
    Moment(#[from] crate::TaskStampMintError),
    /// Its canonical moment spelling would be an ambiguous bare decimal.
    #[error("room authority moment would mint a bare decimal handle: {0}")]
    BareDecimal(i64),
}

impl RoomAuthorityStamp {
    /// Mint one current room-authority identity from Unix microseconds.
    pub fn for_micros(epoch_micros: i64) -> Result<Self, RoomAuthorityStampMintError> {
        let stamp = TaskStamp::for_micros(epoch_micros)?;
        if stamp.as_str().bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(RoomAuthorityStampMintError::BareDecimal(epoch_micros));
        }
        Ok(Self(stamp))
    }

    /// Canonical stored spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Unix microsecond encoded by this identity.
    #[must_use]
    pub fn micros(&self) -> i64 {
        self.0.micros()
    }
}

/// Canonical command attribution for working-group partition changes: the
/// partition is one authority, whatever group a change touches. A move that
/// takes a held Design out of every group commits under that Design's task
/// origin instead, so the stop direction it sends cites the room's own claim.
#[must_use]
pub fn working_group_origin() -> Origin {
    "group:partition"
        .parse()
        .expect("the working-group partition forms a structural origin")
}

impl FromStr for RoomAuthorityStamp {
    type Err = RoomAuthorityStampError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let stamp = text
            .parse::<TaskStamp>()
            .map_err(|_| RoomAuthorityStampError(text.to_owned()))?;
        if text.len() != TaskStamp::WIDTH || text.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(RoomAuthorityStampError(text.to_owned()));
        }
        Ok(Self(stamp))
    }
}

impl fmt::Display for RoomAuthorityStamp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for RoomAuthorityStamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for RoomAuthorityStamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(D::Error::custom)
    }
}

/// Stable identity of one Design pass proposal under its controlling task.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateRef {
    /// Plan or Design controlling the candidate and any selected outputs.
    pub controller: TaskStamp,
    /// Candidate identity in the controller's namespace.
    pub candidate: RoomAuthorityStamp,
}

/// Stable identity of one critique of an exact Design pass proposal.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CritiqueRef {
    /// Exact candidate the critique assesses.
    pub candidate: CandidateRef,
    /// Minimal-width native sequence within that candidate.
    pub critique: SequenceCode,
}

/// Stable identity of one coverage statement over an exact Design pass proposal.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageRef {
    /// Exact candidate the coverage assesses.
    pub candidate: CandidateRef,
    /// Minimal-width native sequence within that candidate.
    pub coverage: SequenceCode,
}

/// Stable identity of one operator decision under a controlling task.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionRef {
    /// Plan or Design whose disposition was decided.
    pub controller: TaskStamp,
    /// Minimal-width native decision sequence under that controller.
    pub decision: SequenceCode,
}

/// Exact subject of one guarded proposal-authority mutation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ProposalRef {
    /// One Design pass proposal.
    Candidate { candidate: CandidateRef },
    /// One critique of an exact candidate.
    Critique { critique: CritiqueRef },
    /// One coverage statement over an exact candidate.
    Coverage { coverage: CoverageRef },
    /// One decision under a controlling task.
    Decision { decision: DecisionRef },
    /// The author's withdrawal of one exact undecided candidate.
    Withdrawal { candidate: CandidateRef },
}

/// Half-open UTF-8 byte range within one decoded event's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextRange {
    /// First contributed byte.
    pub start: u64,
    /// One past the last contributed byte.
    pub end: u64,
}

/// Exact contributed bytes of one decoded transcript event.
///
/// The transcript owns the bytes; this names the accounted generation, the
/// durable vendor record and decoded event, and the range of that event's text
/// contributed, so a reader shares only those bytes and never the transcript.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptHunk {
    /// Accounted logical transcript source.
    pub head: TranscriptSourceHead,
    /// Exact durable vendor record and event.
    pub locus: MessageLocus,
    /// Contributed range of the event's decoded text.
    pub text: TextRange,
}

impl TranscriptHunk {
    /// Whether `other` names these exact bytes, wherever either head's
    /// accounted boundary stood when it was linked.
    #[must_use]
    pub fn same_bytes(&self, other: &Self) -> bool {
        self.head.key() == other.head.key()
            && self.head.source == other.head.source
            && self.locus == other.locus
            && self.text == other.text
    }
}

/// Exact immutable authority that owns room or candidate source bytes.
///
/// These values are links. Transcript bytes remain in tape, journal-authored
/// bytes remain at their fact, and retained artifact bytes remain under their
/// digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RoomSource {
    /// Exact contributed bytes of one decoded event in an accounted transcript
    /// source generation.
    Transcript(TranscriptHunk),
    /// One immutable journal fact, rendered as native base-52 rather than a
    /// bare decimal sequence.
    Journal {
        /// Fact sequence owning the authored bytes.
        fact: SequenceCode,
    },
    /// One retained artifact owns the source bytes.
    Artifact {
        /// Content digest of the retained artifact.
        digest: Digest,
    },
    /// One lane contribution recorded in a Design room; the contribution's
    /// own fact owns its text and cited sources.
    Contribution {
        /// The Design anchor whose room holds the contribution.
        room: RoomAnchor,
        /// Native identity the contribution answered with.
        publication: RoomAuthorityStamp,
    },
}

/// Why a requested room source cannot currently be resolved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RoomSourceUnavailable {
    /// The referenced authority does not exist at the read barrier.
    Missing {},
    /// The authority exists but is outside the caller's visibility.
    Hidden {},
    /// The authority exists but no longer holds the linked bytes.
    Changed {},
    /// The authority exists but cannot be read or decoded now.
    Unreadable {},
    /// Retained bytes belong to a shape this release cannot decode.
    UnsupportedRetained {
        /// Release that owns the unsupported retained representation.
        release: String,
    },
}

/// Source link projected by a read without turning absence into empty success.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RoomSourceView {
    /// The owning authority is available.
    Available {
        /// Exact immutable source link.
        source: RoomSource,
    },
    /// The requested authority is explicit but unavailable.
    Unavailable {
        /// Exact link that could not be resolved.
        source: RoomSource,
        /// Typed reason it could not be projected.
        unavailable: RoomSourceUnavailable,
    },
}

/// Content authority of one room publication.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RoomPublicationContent {
    /// An authored observation, the operator's or a lane's, whose text the
    /// admitted journal fact owns directly.
    Authored { text: String },
    /// Agent text remains owned by its transcript or retained artifact.
    Linked { link: RoomSource },
}

/// Immutable room publication fact body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomPublished {
    /// Anchor task; this is the room identity.
    pub room: RoomAnchor,
    /// Native moment identity of this publication.
    pub publication: RoomAuthorityStamp,
    /// Original bytes or exact source link, never a copied conversation.
    pub content: RoomPublicationContent,
}

/// An authored assistant MSG packet recorded its body in one anchor task's room.
///
/// The packet's transcript owns the bytes. One packet records at most one
/// publication per room, in the same write as the peer messages its directed
/// routes open, so a packet is recorded and sent together or not at all.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomMessaged {
    /// Anchor task; this is the room identity.
    pub room: RoomAnchor,
    /// Native moment identity of this publication.
    pub publication: RoomAuthorityStamp,
    /// The packet's exact body bytes in its author's transcript.
    pub hunk: TranscriptHunk,
    /// Occupant whose captured transcript authored the packet.
    pub author: CanonicalUuid,
    /// Directives this packet's `A#room` routes opened in the same write, in
    /// route order; empty for a room-only packet.
    pub directed: Vec<CanonicalUuid>,
}

/// A lane stopped listening to one anchor task's room.
///
/// The leave declines every Design request it postdates; only a later request
/// restores listening. Authored messages and contributions remain participation
/// records and neither create nor restore listening. It never frees a claim,
/// settles a delivery, or answers a question; native duties keep the lane
/// listening regardless.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomLeft {
    /// Anchor task; this is the room identity.
    pub room: RoomAnchor,
    /// Native moment identity of this publication.
    pub publication: RoomAuthorityStamp,
    /// Logical lane that left; always the authenticated actor.
    pub lane: CanonicalUuid,
    /// The lane's reason for leaving.
    pub clause: String,
}

/// A lane's room catch-up landed in its own transcript, advancing that logical
/// lane's delivery cursor.
///
/// The hunk's transcript head names the lane, so the cursor belongs to the
/// logical lane rather than any one occupant: a renewed occupant continues
/// from it. Each delivery strictly advances the cursor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomDelivered {
    /// Room publication position through which the lane has now been told.
    pub through: SequenceCode,
    /// The landed catch-up's exact bytes in the lane's transcript.
    pub hunk: TranscriptHunk,
}

/// Retained predecessor for the former lane-global catch-up start.
///
/// Current catch-up derives its lower bound from each current task, Design
/// request, or stop authority instead. The journal continues to
/// admit and replay historical horizon facts so an existing repository keeps
/// one valid history across the cutover; this fact claims no delivery,
/// participation, or current listening authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomHorizon {
    /// Logical lane the retained predecessor named.
    pub lane: CanonicalUuid,
}

/// Native room-only publication command body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomPost {
    /// Current rendered handle resolved to the room's anchor birth stamp.
    pub room: TaskHandle,
    /// Complete authored observation bytes.
    pub text: String,
}
/// Successful operator room-publication reply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomPostAnswer {
    /// Current rendered anchor handle.
    pub room: TaskHandle,
    /// Stable native identity of the recorded publication.
    pub publication: RoomAuthorityStamp,
    /// Current listener result observed when the publication was admitted.
    pub outcome: RoomPostOutcome,
}

/// Listener result for one recorded room publication.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RoomPostOutcome {
    /// One or more lanes currently have a reason to receive room catch-up.
    RecordedWithListeners { listeners: Vec<RoomPostListener> },
    /// No lane currently has a reason to receive room catch-up.
    RecordedNoCurrentListener {
        /// Explicit operator action that can establish directed attention.
        next: RoomPostNext,
    },
}

/// Explicit operator action offered when a recorded room has no listener.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RoomPostNext {
    /// Start work or direct a named lane; recording alone does neither.
    StartOrDirect,
}

/// One current listener and the delivery state implied by its availability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomPostListener {
    /// Logical lane identity.
    pub lane: CanonicalUuid,
    /// Current resident topology state.
    pub availability: RoomListenerAvailability,
    /// What recording means for this listener without waking it.
    pub delivery: RoomListenerDelivery,
}

/// Current availability of one room listener.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RoomListenerAvailability {
    /// A current occupant is seated.
    Occupied,
    /// The logical lane is between occupants.
    Vacant,
    /// The lane is known but cannot currently receive.
    Unavailable { disposition: String },
    /// Current topology does not contain the referenced lane.
    Unknown,
}

/// Non-waking delivery consequence for one current listener.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RoomListenerDelivery {
    /// Catch-up may be presented at the lane's next safe boundary.
    AtSafeBoundary,
    /// Delivery remains waiting until the vacant lane next starts.
    WaitsForNextStart,
    /// Delivery remains pending until the lane becomes available.
    PendingAvailability,
}

/// Open relation from a Design pass proposal to exact supporting authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateReference {
    /// Open lower-case relation name, such as `contract`, `evidence`, or `cut`.
    pub relation: Name,
    /// Exact immutable authority owning the referenced bytes.
    pub source: RoomSource,
}

/// What one completed Design pass asks the operator to decide.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DesignPassRecommendation {
    /// Authorize one or more acceptance-bearing outputs the Design gates.
    Work,
    /// Conclude the Design done without fabricating an output.
    Conclude,
    /// Collapse the Design because its premise does not hold.
    Collapse,
}

/// What a lane's Design pass proposes: the recommendation and everything
/// supporting it. The client assembles it from the pass and the Design's
/// room; the proposal's identity, statement head, and claim head are derived
/// from the Design the lane holds, never restated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "DesignPassBodyWire", into = "DesignPassBodyWire")]
pub struct DesignPassBody {
    /// Work or outputless recommendation this pass asks the operator to decide.
    pub recommendation: DesignPassRecommendation,
    /// Governed input tasks whose current composition a work proposal assumes.
    pub governed_inputs: Vec<TaskStamp>,
    /// Acceptance-bearing outputs of any intent named by a work proposal.
    pub outputs: Vec<TaskStamp>,
    /// Nonempty, trimmed explanation for the recommendation.
    pub justification: String,
    /// Exact source links supporting the recommendation.
    pub references: Vec<CandidateReference>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DesignPassBodyWire {
    recommendation: DesignPassRecommendation,
    governed_inputs: Vec<TaskStamp>,
    outputs: Vec<TaskStamp>,
    justification: String,
    references: Vec<CandidateReference>,
}

impl DesignPassBody {
    /// The body these parts make, refused by the same rules a retained
    /// proposal decodes under.
    ///
    /// # Errors
    ///
    /// Returns the rule the parts break.
    pub fn new(
        recommendation: DesignPassRecommendation,
        governed_inputs: Vec<TaskStamp>,
        outputs: Vec<TaskStamp>,
        justification: String,
        references: Vec<CandidateReference>,
    ) -> Result<Self, &'static str> {
        Self::try_from(DesignPassBodyWire {
            recommendation,
            governed_inputs,
            outputs,
            justification,
            references,
        })
    }
}

impl TryFrom<DesignPassBodyWire> for DesignPassBody {
    type Error = &'static str;

    fn try_from(value: DesignPassBodyWire) -> Result<Self, Self::Error> {
        if value.justification.is_empty() || value.justification.trim() != value.justification {
            return Err("a Design pass proposal requires a nonempty trimmed justification");
        }
        match value.recommendation {
            DesignPassRecommendation::Work if value.outputs.is_empty() => {
                return Err("a work proposal must name at least one gated output");
            }
            DesignPassRecommendation::Conclude | DesignPassRecommendation::Collapse
                if !value.governed_inputs.is_empty() || !value.outputs.is_empty() =>
            {
                return Err("an outputless proposal cannot name governed inputs or outputs");
            }
            DesignPassRecommendation::Work
            | DesignPassRecommendation::Conclude
            | DesignPassRecommendation::Collapse => {}
        }
        if !all_unique(&value.governed_inputs) || !all_unique(&value.outputs) {
            return Err("proposal inputs and outputs must each be unique");
        }
        if value
            .outputs
            .iter()
            .any(|output| value.governed_inputs.contains(output))
        {
            return Err("one task cannot be both governed input and proposed output");
        }
        if value.references.is_empty() {
            return Err("a Design pass proposal must link its supporting authority");
        }
        Ok(Self {
            recommendation: value.recommendation,
            governed_inputs: value.governed_inputs,
            outputs: value.outputs,
            justification: value.justification,
            references: value.references,
        })
    }
}

impl From<DesignPassBody> for DesignPassBodyWire {
    fn from(value: DesignPassBody) -> Self {
        Self {
            recommendation: value.recommendation,
            governed_inputs: value.governed_inputs,
            outputs: value.outputs,
            justification: value.justification,
            references: value.references,
        }
    }
}

/// Exact proposal that completes one Design pass.
///
/// Current proposals always name the active claim they release. Proposals
/// crossed from the predecessor protocol may omit it when their immutable
/// receipt does not prove an active claim; that absence is historical fact,
/// not a fabricated claim horizon.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "DesignPassProposalWire", into = "DesignPassProposalWire")]
pub struct DesignPassProposal {
    /// Stable proposal identity under its controller.
    pub candidate: CandidateRef,
    /// Exact Design statement revision this pass answered.
    pub statement: Seq,
    /// Exact active claim head this pass releases, absent only for history
    /// crossed from the predecessor protocol.
    pub claim: Option<Seq>,
    /// Work or outputless recommendation this pass asks the operator to decide.
    pub recommendation: DesignPassRecommendation,
    /// Governed input tasks whose current composition a work proposal assumes.
    pub governed_inputs: Vec<TaskStamp>,
    /// Acceptance-bearing outputs of any intent named by a work proposal.
    pub outputs: Vec<TaskStamp>,
    /// Nonempty, trimmed explanation for the recommendation.
    pub justification: String,
    /// Exact source links supporting the recommendation.
    pub references: Vec<CandidateReference>,
}

impl DesignPassProposal {
    /// The proposal `pass` makes as `candidate`, witnessing the Design
    /// statement head `statement` and the active claim head `claim` it
    /// releases.
    #[must_use]
    pub fn witnessing(
        candidate: CandidateRef,
        statement: Seq,
        claim: Seq,
        pass: DesignPassBody,
    ) -> Self {
        Self {
            candidate,
            statement,
            claim: Some(claim),
            recommendation: pass.recommendation,
            governed_inputs: pass.governed_inputs,
            outputs: pass.outputs,
            justification: pass.justification,
            references: pass.references,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DesignPassProposalWire {
    candidate: CandidateRef,
    statement: Seq,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claim: Option<Seq>,
    recommendation: DesignPassRecommendation,
    governed_inputs: Vec<TaskStamp>,
    outputs: Vec<TaskStamp>,
    justification: String,
    references: Vec<CandidateReference>,
}

impl TryFrom<DesignPassProposalWire> for DesignPassProposal {
    type Error = &'static str;

    fn try_from(value: DesignPassProposalWire) -> Result<Self, Self::Error> {
        let pass = DesignPassBody::try_from(DesignPassBodyWire {
            recommendation: value.recommendation,
            governed_inputs: value.governed_inputs,
            outputs: value.outputs,
            justification: value.justification,
            references: value.references,
        })?;
        Ok(Self {
            candidate: value.candidate,
            statement: value.statement,
            claim: value.claim,
            recommendation: pass.recommendation,
            governed_inputs: pass.governed_inputs,
            outputs: pass.outputs,
            justification: pass.justification,
            references: pass.references,
        })
    }
}

impl From<DesignPassProposal> for DesignPassProposalWire {
    fn from(value: DesignPassProposal) -> Self {
        Self {
            candidate: value.candidate,
            statement: value.statement,
            claim: value.claim,
            recommendation: value.recommendation,
            governed_inputs: value.governed_inputs,
            outputs: value.outputs,
            justification: value.justification,
            references: value.references,
        }
    }
}

/// Command body proposing one exact completed Design pass.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignPropose {
    /// Exact candidate and retained authority.
    pub candidate: DesignPassProposal,
}

/// Immutable fact body retaining one exact candidate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignProposed {
    /// Exact candidate and retained authority.
    pub candidate: DesignPassProposal,
}

/// Result of one critique against an exact candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CritiqueStanding {
    /// No material finding remains in this critique.
    Clean,
    /// The linked critique records a material finding.
    MaterialFinding,
}

/// One source-backed critique of an exact Design pass proposal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateCritique {
    /// Stable critique identity.
    pub critique: CritiqueRef,
    /// Open category for later critique families.
    pub category: Name,
    /// Exact authority owning the critique bytes.
    pub source: RoomSource,
    /// Current conclusion of this critique.
    pub standing: CritiqueStanding,
}

/// Command body recording one candidate critique.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateCritiqueRecord {
    /// Source-backed critique to record.
    pub critique: CandidateCritique,
}

/// Immutable fact body recording one candidate critique.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateCritiqued {
    /// Source-backed critique that was recorded.
    pub critique: CandidateCritique,
}

/// Exact reviewer authority behind candidate coverage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CoverageAuthority {
    /// Explicit operator review.
    Operator {},
    /// Independent lane review, with exact authoring occupant retained.
    Independent {
        /// Logical reviewing lane.
        lane: CanonicalUuid,
        /// Exact occupant that authored the coverage.
        occupant: CanonicalUuid,
    },
    /// Labeled fallback review by the candidate's own authoring lane.
    SelfReview {
        /// Logical reviewing lane.
        lane: CanonicalUuid,
        /// Exact occupant that authored the coverage.
        occupant: CanonicalUuid,
    },
}

/// One source-backed coverage statement over an exact candidate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CandidateCoverageWire", into = "CandidateCoverageWire")]
pub struct CandidateCoverage {
    /// Stable coverage identity.
    pub coverage: CoverageRef,
    /// Exact reviewer authority, preserving self-review distinction.
    pub authority: CoverageAuthority,
    /// Open, nonempty coverage scopes such as `contracts` or `composition`.
    pub scopes: Vec<Name>,
    /// Exact authority owning the review bytes.
    pub source: RoomSource,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidateCoverageWire {
    coverage: CoverageRef,
    authority: CoverageAuthority,
    scopes: Vec<Name>,
    source: RoomSource,
}

impl TryFrom<CandidateCoverageWire> for CandidateCoverage {
    type Error = &'static str;

    fn try_from(value: CandidateCoverageWire) -> Result<Self, Self::Error> {
        if value.scopes.is_empty() || !all_unique(&value.scopes) {
            return Err("candidate coverage scopes must be nonempty and unique");
        }
        Ok(Self {
            coverage: value.coverage,
            authority: value.authority,
            scopes: value.scopes,
            source: value.source,
        })
    }
}

impl From<CandidateCoverage> for CandidateCoverageWire {
    fn from(value: CandidateCoverage) -> Self {
        Self {
            coverage: value.coverage,
            authority: value.authority,
            scopes: value.scopes,
            source: value.source,
        }
    }
}

/// Command body recording one candidate coverage statement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateCoverageRecord {
    /// Source-backed coverage to record.
    pub coverage: CandidateCoverage,
}

/// Immutable fact body recording one candidate coverage statement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateCovered {
    /// Source-backed coverage that was recorded.
    pub coverage: CandidateCoverage,
}

/// Exact selected execution authority created by its operator decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    try_from = "ExecutionAuthorizationWire",
    into = "ExecutionAuthorizationWire"
)]
pub struct ExecutionAuthorization {
    /// Operator decision that selected this exact output subset.
    pub decision: DecisionRef,
    /// Exact Design pass proposal selected by that decision.
    pub candidate: CandidateRef,
    /// Candidate input tasks witnessed by this authorization.
    pub governed_inputs: Vec<TaskStamp>,
    /// Exact nonempty output set made eligible; no later output inherits it.
    pub selected_outputs: Vec<TaskStamp>,
    /// Retained predecessor coverage citations. Current selections leave this
    /// empty because coverage is evidence, never authorization.
    pub coverage: Vec<CoverageRef>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutionAuthorizationWire {
    decision: DecisionRef,
    candidate: CandidateRef,
    governed_inputs: Vec<TaskStamp>,
    selected_outputs: Vec<TaskStamp>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    coverage: Vec<CoverageRef>,
}

impl TryFrom<ExecutionAuthorizationWire> for ExecutionAuthorization {
    type Error = &'static str;

    fn try_from(value: ExecutionAuthorizationWire) -> Result<Self, Self::Error> {
        if value.decision.controller != value.candidate.controller {
            return Err("authorization decision and candidate must share one controller");
        }
        if value.selected_outputs.is_empty()
            || !all_unique(&value.selected_outputs)
            || !all_unique(&value.governed_inputs)
            || !all_unique(&value.coverage)
        {
            return Err("authorization inputs, outputs, and coverage must be exact unique sets");
        }
        if value
            .coverage
            .iter()
            .any(|coverage| coverage.candidate != value.candidate)
        {
            return Err("retained authorization coverage must assess its candidate");
        }
        if value
            .selected_outputs
            .iter()
            .any(|output| value.governed_inputs.contains(output))
        {
            return Err("one task cannot be both governed input and selected output");
        }
        Ok(Self {
            decision: value.decision,
            candidate: value.candidate,
            governed_inputs: value.governed_inputs,
            selected_outputs: value.selected_outputs,
            coverage: value.coverage,
        })
    }
}

impl From<ExecutionAuthorization> for ExecutionAuthorizationWire {
    fn from(value: ExecutionAuthorization) -> Self {
        Self {
            decision: value.decision,
            candidate: value.candidate,
            governed_inputs: value.governed_inputs,
            selected_outputs: value.selected_outputs,
            coverage: value.coverage,
        }
    }
}

/// Immutable fact body recording exact selected execution authorization.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionAuthorized {
    /// Exact authority that was admitted.
    pub authorization: ExecutionAuthorization,
}

/// Command body explicitly adopting an existing task into preparation control.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputAdopt {
    /// Existing output task; running or completed work remains reference-only.
    pub output: TaskHandle,
    /// Existing planning context from which native [`TaskPreparation`] is
    /// derived under task-head witnesses.
    pub preparation: TaskHandle,
}

/// Immutable fact body applying the same native preparation representation to
/// an adopted or promoted output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputControlled {
    /// Controlled output task.
    pub output: TaskStamp,
    /// Stage Zero native preparation/controller/construction association.
    pub preparation: TaskPreparation,
}

fn all_unique<T: PartialEq>(values: &[T]) -> bool {
    values
        .iter()
        .enumerate()
        .all(|(index, value)| !values[index + 1..].contains(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(text: &str) -> TaskStamp {
        text.parse().expect("task stamp")
    }

    fn authority(text: &str) -> RoomAuthorityStamp {
        text.parse().expect("room authority stamp")
    }

    fn name(text: &str) -> Name {
        text.parse().expect("open name")
    }

    fn source(sequence: u64) -> RoomSource {
        RoomSource::Journal {
            fact: SequenceCode::new(sequence),
        }
    }

    fn candidate() -> CandidateRef {
        CandidateRef {
            controller: task("1kKRRsDR0"),
            candidate: authority("1kKRRsDR1"),
        }
    }

    #[test]
    fn room_and_proposal_identities_use_only_native_base52_authority() {
        let anchor = task("1kKRRsDR0");
        let room: RoomAnchor = anchor.clone();
        assert_eq!(room, anchor, "room identity is exactly its anchor task");

        let id = authority("1kKRRsDR1");
        assert_eq!(
            crate::decode::<RoomAuthorityStamp>(&crate::encode(&id).unwrap()).unwrap(),
            id
        );
        let minted = RoomAuthorityStamp::for_micros(1_700_000_000_000_000)
            .expect("current native authority stamp");
        assert_eq!(minted.as_str().len(), TaskStamp::WIDTH);
        assert_eq!(minted.as_str().parse(), Ok(minted));
        assert_eq!(
            RoomAuthorityStamp::for_micros(1),
            Err(RoomAuthorityStampMintError::BareDecimal(1))
        );
        for invalid in [
            "1kKRRsDR",
            "018f82d4-6f59-7d06-92e3-bce913116301",
            "123456789",
            "1kKRRsDRA",
        ] {
            assert!(invalid.parse::<RoomAuthorityStamp>().is_err(), "{invalid}");
        }
    }

    #[test]
    fn proposal_witness_subjects_preserve_each_exact_authority_reference() {
        let candidate = candidate();
        let proposals = [
            ProposalRef::Candidate {
                candidate: candidate.clone(),
            },
            ProposalRef::Critique {
                critique: CritiqueRef {
                    candidate: candidate.clone(),
                    critique: SequenceCode::new(1),
                },
            },
            ProposalRef::Coverage {
                coverage: CoverageRef {
                    candidate: candidate.clone(),
                    coverage: SequenceCode::new(2),
                },
            },
            ProposalRef::Decision {
                decision: DecisionRef {
                    controller: candidate.controller,
                    decision: SequenceCode::new(3),
                },
            },
        ];
        for proposal in proposals {
            let encoded = crate::encode(&proposal).unwrap();
            assert_eq!(crate::decode::<ProposalRef>(&encoded).unwrap(), proposal);
        }
    }

    #[test]
    fn working_groups_decode_only_identity_and_ordered_unique_anchors() {
        let group = serde_json::json!({
            "group": "1kKRRsDR1",
            "anchors": ["1kKRRsDR0", "1kKRRsDR2"]
        });
        let decoded: WorkingGroup = serde_json::from_value(group.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), group);

        let duplicate = serde_json::json!({
            "group": "1kKRRsDR1",
            "anchors": ["1kKRRsDR0", "1kKRRsDR0"]
        });
        assert!(serde_json::from_value::<WorkingGroup>(duplicate).is_err());
        let oversized = serde_json::json!({
            "group": "1kKRRsDR1",
            "anchors": (0..=WORKING_GROUP_ANCHOR_LIMIT)
                .map(|micros| TaskStamp::for_micros(micros as i64).unwrap().to_string())
                .collect::<Vec<_>>()
        });
        assert!(serde_json::from_value::<WorkingGroup>(oversized).is_err());
        let named = serde_json::json!({
            "group": "1kKRRsDR1",
            "name": "Operator batch",
            "anchors": ["1kKRRsDR0"]
        });
        assert!(
            serde_json::from_value::<WorkingGroup>(named).is_err(),
            "a working group carries no operator-given name"
        );
        let smuggled_membership = serde_json::json!({
            "group": "1kKRRsDR1",
            "anchors": ["1kKRRsDR0"],
            "lanes": []
        });
        assert!(serde_json::from_value::<WorkingGroup>(smuggled_membership).is_err());
    }

    #[test]
    fn a_partition_decodes_only_standing_disjoint_groups() {
        let partition = |groups: serde_json::Value| {
            serde_json::from_value::<WorkingGroupPartition>(serde_json::json!({
                "revision": 3,
                "groups": groups
            }))
        };
        let value = serde_json::json!([
            {"group": "1kKRRsDR1", "anchors": ["1kKRRsDR0"]},
            {"group": "1kKRRsDR3", "anchors": ["1kKRRsDR2", "1kKRRsDR4"]}
        ]);
        let decoded = partition(value).unwrap();
        assert_eq!(decoded.revision, 3);
        assert_eq!(
            decoded
                .holder(&"1kKRRsDR4".parse().unwrap())
                .map(|group| group.group.to_string()),
            Some("1kKRRsDR3".to_owned())
        );
        assert!(decoded.holder(&"1kKRRsDR9".parse().unwrap()).is_none());
        for refused in [
            serde_json::json!([{"group": "1kKRRsDR1", "anchors": []}]),
            serde_json::json!([
                {"group": "1kKRRsDR1", "anchors": ["1kKRRsDR0"]},
                {"group": "1kKRRsDR1", "anchors": ["1kKRRsDR2"]}
            ]),
            serde_json::json!([
                {"group": "1kKRRsDR1", "anchors": ["1kKRRsDR0"]},
                {"group": "1kKRRsDR3", "anchors": ["1kKRRsDR0"]}
            ]),
        ] {
            assert!(partition(refused.clone()).is_err(), "{refused}");
        }
    }

    #[test]
    fn candidates_decisions_and_authorization_retain_exact_referential_sets() {
        let candidate_ref = candidate();
        let candidate_value = serde_json::json!({
            "candidate": candidate_ref,
            "statement": 10,
            "claim": 11,
            "recommendation": "work",
            "governed_inputs": ["1kKRRsDR2"],
            "outputs": ["1kKRRsDR3", "1kKRRsDR4"],
            "justification": "The contract supports the proposed split.",
            "references": [{"relation": "contract", "source": {"kind": "journal", "fact": "2"}}]
        });
        let retained: DesignPassProposal = serde_json::from_value(candidate_value.clone()).unwrap();
        assert_eq!(serde_json::to_value(retained).unwrap(), candidate_value);

        let decision_value = serde_json::json!({
            "decision": {"controller": "1kKRRsDR0", "decision": "4"},
            "source": {"kind": "journal", "fact": "3"},
            "outcome": {
                "kind": "resolved",
                "candidate": candidate(),
                "outputs": [
                    {"output": "1kKRRsDR3", "verdict": "accepted"},
                    {"output": "1kKRRsDR4", "verdict": "rejected"}
                ]
            }
        });
        let decision: PreparationDecision = serde_json::from_value(decision_value.clone()).unwrap();
        assert_eq!(serde_json::to_value(decision).unwrap(), decision_value);

        let returned_value = serde_json::json!({
            "decision": {"controller": "1kKRRsDR0", "decision": "5"},
            "source": {"kind": "journal", "fact": "3"},
            "outcome": {
                "kind": "returned",
                "candidate": candidate(),
                "reason": "revise the premise"
            }
        });
        let returned: PreparationDecision = serde_json::from_value(returned_value.clone()).unwrap();
        assert_eq!(serde_json::to_value(returned).unwrap(), returned_value);

        let authorization_value = serde_json::json!({
            "decision": {"controller": "1kKRRsDR0", "decision": "4"},
            "candidate": candidate(),
            "governed_inputs": ["1kKRRsDR2"],
            "selected_outputs": ["1kKRRsDR3"]
        });
        let authorization: ExecutionAuthorization =
            serde_json::from_value(authorization_value.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(authorization).unwrap(),
            authorization_value
        );

        let mut empty = decision_value;
        empty["outcome"]["selected_outputs"] = serde_json::json!([]);
        assert!(serde_json::from_value::<PreparationDecision>(empty).is_err());
    }

    #[test]
    fn output_decisions_round_trip_verdicts_and_optional_comments() {
        let value = serde_json::json!({
            "decision": {"controller": "1kKRRsDR0", "decision": "5"},
            "source": {"kind": "journal", "fact": "3"},
            "outcome": {
                "kind": "resolved",
                "candidate": candidate(),
                "outputs": [
                    {
                        "output": "1kKRRsDR3",
                        "verdict": "accepted",
                        "comment": "meets the contract"
                    },
                    {
                        "output": "1kKRRsDR4",
                        "verdict": "rejected",
                        "comment": "revise the boundary"
                    }
                ],
                "comment": "move the first plan forward"
            }
        });
        let decision: PreparationDecision = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&decision).unwrap(), value);
        let accepted = "1kKRRsDR3".parse::<TaskStamp>().unwrap();
        assert_eq!(
            decision.outcome.accepted_outputs().collect::<Vec<_>>(),
            vec![&accepted]
        );
        assert!(decision.outcome.carries_feedback());

        for invalid in [
            serde_json::json!([]),
            serde_json::json!([
                {"output": "1kKRRsDR3", "verdict": "accepted"},
                {"output": "1kKRRsDR3", "verdict": "rejected"}
            ]),
            serde_json::json!([
                {"output": "1kKRRsDR3", "verdict": "accepted", "comment": " "}
            ]),
        ] {
            let mut invalid_value = value.clone();
            invalid_value["outcome"]["outputs"] = invalid;
            assert!(
                serde_json::from_value::<PreparationDecision>(invalid_value).is_err(),
                "invalid output decision was admitted"
            );
        }
    }

    #[test]
    fn outputless_acceptance_names_one_exact_proposal() {
        let value = serde_json::json!({
            "decision": {"controller": "1kKRRsDR0", "decision": "5"},
            "source": {"kind": "journal", "fact": "6"},
            "outcome": {
                "kind": "accepted",
                "candidate": candidate()
            }
        });
        let decision: PreparationDecision = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&decision).unwrap(), value);
        assert!(matches!(
            decision.outcome,
            DecisionOutcome::Accepted {
                candidate: selected_candidate
            } if selected_candidate == candidate()
        ));
    }

    #[test]
    fn unavailable_source_authority_stays_explicit_and_names_its_release() {
        let source = source(7);
        let view = RoomSourceView::Unavailable {
            source,
            unavailable: RoomSourceUnavailable::UnsupportedRetained {
                release: "0.31.0".to_owned(),
            },
        };
        let encoded = crate::encode(&view).unwrap();
        assert_eq!(crate::decode::<RoomSourceView>(&encoded).unwrap(), view);
        let document = String::from_utf8(encoded).unwrap();
        assert!(document.contains("unsupported-retained"));
        assert!(document.contains("0.31.0"));
    }

    #[test]
    fn critique_and_coverage_categories_remain_open_without_weakening_identity() {
        let critique = CandidateCritique {
            critique: CritiqueRef {
                candidate: candidate(),
                critique: SequenceCode::new(8),
            },
            category: name("composition"),
            source: source(9),
            standing: CritiqueStanding::MaterialFinding,
        };
        let coverage = CandidateCoverage {
            coverage: CoverageRef {
                candidate: candidate(),
                coverage: SequenceCode::new(10),
            },
            authority: CoverageAuthority::Operator {},
            scopes: vec![name("contracts"), name("composition")],
            source: source(11),
        };
        for encoded in [
            crate::encode(&critique).unwrap(),
            crate::encode(&coverage).unwrap(),
        ] {
            assert!(!String::from_utf8(encoded).unwrap().contains("018f82d4-"));
        }
    }
}
