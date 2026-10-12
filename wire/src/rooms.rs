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

use crate::{
    CanonicalUuid, Digest, MessageLocus, Name, SequenceCode, TaskHandle, TaskPreparation,
    TaskStamp, TranscriptSourceHead,
};

/// The identity of a room: exactly the anchor task's durable birth stamp.
pub type RoomAnchor = TaskStamp;
/// Most room anchors one saved working group may associate.
pub const WORKING_GROUP_ANCHOR_LIMIT: usize = 100;

/// A current nine-digit native moment stamp for non-task room authority.
///
/// Working groups and retained candidates need durable identities but are not
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

/// Stable identity of one retained candidate under its controlling task.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateRef {
    /// Plan or Design controlling the candidate and any selected outputs.
    pub controller: TaskStamp,
    /// Candidate identity in the controller's namespace.
    pub candidate: RoomAuthorityStamp,
}

/// Stable identity of one critique of an exact retained candidate.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CritiqueRef {
    /// Exact candidate the critique assesses.
    pub candidate: CandidateRef,
    /// Minimal-width native sequence within that candidate.
    pub critique: SequenceCode,
}

/// Stable identity of one coverage statement over an exact retained candidate.
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
    /// One retained candidate.
    Candidate { candidate: CandidateRef },
    /// One critique of an exact candidate.
    Critique { critique: CritiqueRef },
    /// One coverage statement over an exact candidate.
    Coverage { coverage: CoverageRef },
    /// One decision under a controlling task.
    Decision { decision: DecisionRef },
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
}

/// Why a requested room source cannot currently be resolved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RoomSourceUnavailable {
    /// The referenced authority does not exist at the read barrier.
    Missing {},
    /// The authority exists but is outside the caller's visibility.
    Hidden {},
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
    /// Operator composer text is owned directly by the admitted journal fact.
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
}

/// One scoped operator request to coordinate an anchor through native steering.
///
/// The steering fact owns the request text. This fact only links that existing
/// authority into the room timeline, so it never copies conversation bytes or
/// creates another obligation lifecycle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomCoordinationStarted {
    /// Anchor task; this is the room identity.
    pub room: RoomAnchor,
    /// Native moment identity of this room-timeline event.
    pub publication: RoomAuthorityStamp,
    /// Human-sized key of the ordinary steering request.
    pub key: String,
    /// Exact base-52 occupant address asked to make the bounded contribution.
    pub target: crate::OccupantAddress,
}

/// A live anchor holder asked to checkpoint and release through native task
/// disposition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomStopRequest {
    /// Human-sized key of the ordinary steering request.
    pub key: String,
    /// Exact base-52 address of the holder at admission.
    pub target: crate::OccupantAddress,
    /// Exact claim head the request applies to, rendered as native base-52.
    pub claim: SequenceCode,
}

/// One operator Put-aside action recorded in an anchor's room timeline.
///
/// The listed keys are ordinary steering dispositions. A stop request remains
/// visible only while its exact native claim head remains active.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomPutAside {
    /// Anchor task; this is the room identity.
    pub room: RoomAnchor,
    /// Native moment identity of this room-timeline event.
    pub publication: RoomAuthorityStamp,
    /// Unanswered room-coordination requests canceled by this action.
    pub canceled: Vec<String>,
    /// Request to the current anchor holder, absent when the anchor was idle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<RoomStopRequest>,
}

/// Native room-only publication command body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomPost {
    /// Current rendered handle resolved to the room's anchor birth stamp.
    pub room: TaskHandle,
    /// Complete operator-authored publication bytes.
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

/// Repository-shared organization of ordered room anchors.
///
/// A working group owns only its identity, display name, and ordered anchor
/// associations. It cannot encode lanes, members, claims, reads, work, or
/// foreground geometry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "WorkingGroupWire", into = "WorkingGroupWire")]
pub struct WorkingGroup {
    /// Stable repository-shared identity.
    pub group: RoomAuthorityStamp,
    /// Operator-visible name.
    pub name: String,
    /// Room anchors in operator order.
    pub anchors: Vec<RoomAnchor>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkingGroupWire {
    group: RoomAuthorityStamp,
    name: String,
    anchors: Vec<RoomAnchor>,
}

impl TryFrom<WorkingGroupWire> for WorkingGroup {
    type Error = &'static str;

    fn try_from(value: WorkingGroupWire) -> Result<Self, Self::Error> {
        if value.name.trim().is_empty() {
            return Err("working-group name must contain visible text");
        }
        if !all_unique(&value.anchors) {
            return Err("working-group anchors must be unique and ordered");
        }
        if value.anchors.len() > WORKING_GROUP_ANCHOR_LIMIT {
            return Err("working-group anchor count exceeds 100");
        }
        Ok(Self {
            group: value.group,
            name: value.name,
            anchors: value.anchors,
        })
    }
}

impl From<WorkingGroup> for WorkingGroupWire {
    fn from(value: WorkingGroup) -> Self {
        Self {
            group: value.group,
            name: value.name,
            anchors: value.anchors,
        }
    }
}

/// Guarded complete replacement command for one working group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingGroupPut {
    /// Complete intended group state.
    pub group: WorkingGroup,
}

/// Command body removing one working group without touching any room.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingGroupRemove {
    /// Group whose saved organization is removed.
    pub group: RoomAuthorityStamp,
}

/// Immutable fact body recording a complete working-group state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingGroupStored {
    /// Complete guarded state after the mutation.
    pub group: WorkingGroup,
}

/// Immutable fact body recording working-group removal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingGroupRemoved {
    /// Removed saved organization; anchor rooms remain unchanged.
    pub group: RoomAuthorityStamp,
}

/// Open relation from a retained candidate to exact supporting authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateReference {
    /// Open lower-case relation name, such as `contract`, `evidence`, or `cut`.
    pub relation: Name,
    /// Exact immutable authority owning the referenced bytes.
    pub source: RoomSource,
}

/// Exact retained candidate presented for critique and decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RetainedCandidateWire", into = "RetainedCandidateWire")]
pub struct RetainedCandidate {
    /// Stable candidate identity under its controller.
    pub candidate: CandidateRef,
    /// Governed input tasks whose current composition the candidate assumes.
    pub governed_inputs: Vec<TaskStamp>,
    /// Gated output tasks proposed by the candidate.
    pub outputs: Vec<TaskStamp>,
    /// Exact source links, with open relation names for later evidence families.
    pub references: Vec<CandidateReference>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedCandidateWire {
    candidate: CandidateRef,
    governed_inputs: Vec<TaskStamp>,
    outputs: Vec<TaskStamp>,
    references: Vec<CandidateReference>,
}

impl TryFrom<RetainedCandidateWire> for RetainedCandidate {
    type Error = &'static str;

    fn try_from(value: RetainedCandidateWire) -> Result<Self, Self::Error> {
        if value.outputs.is_empty() {
            return Err("retained candidate must name at least one gated output");
        }
        if !all_unique(&value.governed_inputs) || !all_unique(&value.outputs) {
            return Err("candidate inputs and outputs must each be unique");
        }
        if value
            .outputs
            .iter()
            .any(|output| value.governed_inputs.contains(output))
        {
            return Err("one task cannot be both governed input and selected output");
        }
        if value.references.is_empty() {
            return Err("retained candidate must link its retained authority");
        }
        Ok(Self {
            candidate: value.candidate,
            governed_inputs: value.governed_inputs,
            outputs: value.outputs,
            references: value.references,
        })
    }
}

impl From<RetainedCandidate> for RetainedCandidateWire {
    fn from(value: RetainedCandidate) -> Self {
        Self {
            candidate: value.candidate,
            governed_inputs: value.governed_inputs,
            outputs: value.outputs,
            references: value.references,
        }
    }
}

/// Command body retaining one exact candidate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateRetain {
    /// Exact candidate and retained authority.
    pub candidate: RetainedCandidate,
}

/// Immutable fact body retaining one exact candidate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateRetained {
    /// Exact candidate and retained authority.
    pub candidate: RetainedCandidate,
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

/// One source-backed critique of an exact retained candidate.
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

/// Negative conclusion that deliberately authorizes no output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NegativeDecision {
    /// Candidate or approach was rejected on the retained basis.
    Rejected,
    /// Investigation concluded that no execution work is justified.
    NoWork,
}

/// Operator decision over preparation authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DecisionOutcome {
    /// Candidate is accepted for a separate exact execution authorization.
    Accepted { candidate: CandidateRef },
    /// Candidate returns for further preparation and remains unauthorized.
    Returned { candidate: CandidateRef },
    /// Preparation concludes negatively without a candidate or authorization.
    Negative { conclusion: NegativeDecision },
}

/// One source-backed operator decision under a controlling task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "PreparationDecisionWire", into = "PreparationDecisionWire")]
pub struct PreparationDecision {
    /// Stable decision identity.
    pub decision: DecisionRef,
    /// Exact authority owning the decision basis.
    pub source: RoomSource,
    /// Accepted, returned, or negative/no-work outcome.
    pub outcome: DecisionOutcome,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparationDecisionWire {
    decision: DecisionRef,
    source: RoomSource,
    outcome: DecisionOutcome,
}

impl TryFrom<PreparationDecisionWire> for PreparationDecision {
    type Error = &'static str;

    fn try_from(value: PreparationDecisionWire) -> Result<Self, Self::Error> {
        let candidate = match &value.outcome {
            DecisionOutcome::Accepted { candidate } | DecisionOutcome::Returned { candidate } => {
                Some(candidate)
            }
            DecisionOutcome::Negative { .. } => None,
        };
        if candidate.is_some_and(|candidate| candidate.controller != value.decision.controller) {
            return Err("decision and candidate must share one controlling task");
        }
        Ok(Self {
            decision: value.decision,
            source: value.source,
            outcome: value.outcome,
        })
    }
}

impl From<PreparationDecision> for PreparationDecisionWire {
    fn from(value: PreparationDecision) -> Self {
        Self {
            decision: value.decision,
            source: value.source,
            outcome: value.outcome,
        }
    }
}

/// Command body recording one preparation decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparationDecide {
    /// Source-backed decision to record.
    pub decision: PreparationDecision,
}

/// Immutable fact body recording one preparation decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparationDecided {
    /// Source-backed decision that was recorded.
    pub decision: PreparationDecision,
}

/// Exact selected execution authority following an accepted decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    try_from = "ExecutionAuthorizationWire",
    into = "ExecutionAuthorizationWire"
)]
pub struct ExecutionAuthorization {
    /// Accepted operator decision authorizing this exact selection.
    pub decision: DecisionRef,
    /// Exact retained candidate selected by that decision.
    pub candidate: CandidateRef,
    /// Candidate input tasks governed by this authorization.
    pub governed_inputs: Vec<TaskStamp>,
    /// Exact nonempty output set made eligible; no later output inherits it.
    pub selected_outputs: Vec<TaskStamp>,
    /// Exact current coverage statements supporting the selection.
    pub coverage: Vec<CoverageRef>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutionAuthorizationWire {
    decision: DecisionRef,
    candidate: CandidateRef,
    governed_inputs: Vec<TaskStamp>,
    selected_outputs: Vec<TaskStamp>,
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
        if value.coverage.is_empty() {
            return Err("positive authorization requires explicit current coverage");
        }
        if value
            .coverage
            .iter()
            .any(|coverage| coverage.candidate != value.candidate)
        {
            return Err("authorization coverage must assess the selected candidate");
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

/// Command body atomically completing accepted preparation and authorizing its
/// exact selected outputs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionAuthorize {
    /// Exact authority to admit with final preparation completion.
    pub authorization: ExecutionAuthorization,
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
    fn working_groups_encode_only_name_and_ordered_unique_room_anchors() {
        let value = serde_json::json!({
            "group": "1kKRRsDR1",
            "name": "OOPS grooming",
            "anchors": ["1kKRRsDR0", "1kKRRsDR2"]
        });
        let group: WorkingGroup = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(group).unwrap(), value);

        let duplicate = serde_json::json!({
            "group": "1kKRRsDR1",
            "name": "OOPS grooming",
            "anchors": ["1kKRRsDR0", "1kKRRsDR0"]
        });
        assert!(serde_json::from_value::<WorkingGroup>(duplicate).is_err());
        let oversized = serde_json::json!({
            "group": "1kKRRsDR1",
            "name": "OOPS grooming",
            "anchors": (0..=WORKING_GROUP_ANCHOR_LIMIT)
                .map(|micros| TaskStamp::for_micros(micros as i64).unwrap().to_string())
                .collect::<Vec<_>>()
        });
        assert!(serde_json::from_value::<WorkingGroup>(oversized).is_err());
        let smuggled_membership = serde_json::json!({
            "group": "1kKRRsDR1",
            "name": "OOPS grooming",
            "anchors": [],
            "lanes": []
        });
        assert!(serde_json::from_value::<WorkingGroup>(smuggled_membership).is_err());
    }

    #[test]
    fn candidates_and_authorization_retain_exact_referential_sets() {
        let candidate_ref = candidate();
        let candidate_value = serde_json::json!({
            "candidate": candidate_ref,
            "governed_inputs": ["1kKRRsDR2"],
            "outputs": ["1kKRRsDR3", "1kKRRsDR4"],
            "references": [{"relation": "contract", "source": {"kind": "journal", "fact": "2"}}]
        });
        let retained: RetainedCandidate = serde_json::from_value(candidate_value.clone()).unwrap();
        assert_eq!(serde_json::to_value(retained).unwrap(), candidate_value);

        let coverage = CoverageRef {
            candidate: candidate(),
            coverage: SequenceCode::new(3),
        };
        let authorization_value = serde_json::json!({
            "decision": {"controller": "1kKRRsDR0", "decision": "4"},
            "candidate": candidate(),
            "governed_inputs": ["1kKRRsDR2"],
            "selected_outputs": ["1kKRRsDR3"],
            "coverage": [coverage]
        });
        let authorization: ExecutionAuthorization =
            serde_json::from_value(authorization_value.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(authorization).unwrap(),
            authorization_value
        );

        let mut mismatched = authorization_value;
        mismatched["coverage"][0]["candidate"]["candidate"] = serde_json::json!("1kKRRsDR5");
        assert!(serde_json::from_value::<ExecutionAuthorization>(mismatched).is_err());
    }

    #[test]
    fn negative_decisions_carry_basis_but_no_execution_authorization() {
        let value = serde_json::json!({
            "decision": {"controller": "1kKRRsDR0", "decision": "5"},
            "source": {"kind": "journal", "fact": "6"},
            "outcome": {"kind": "negative", "conclusion": "no-work"}
        });
        let decision: PreparationDecision = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&decision).unwrap(), value);
        assert!(matches!(
            decision.outcome,
            DecisionOutcome::Negative {
                conclusion: NegativeDecision::NoWork
            }
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
