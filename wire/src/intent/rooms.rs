//! Bounded room-coordination and working-group command DTOs.

use serde::{Deserialize, Serialize};

use crate::rooms::{
    QuestionClosure, QuestionResultRef, RoomAuthorityStamp, RoomReceiptDisposition, RoomReceiptRef,
    RoomRelevanceVerdict, RoomSource, RoomSourceUnavailable,
};
pub use crate::rooms::{WorkingGroupArrange, WorkingGroupMove};
use crate::{OccupantAddress, SequenceCode, TaskHandle, TaskStamp};

/// Start or resume one bounded preparation contribution in an anchor room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomCoordinate {
    /// Current rendered handle of the anchor task.
    pub room: TaskHandle,
    /// Current base-52 address of the occupant asked to contribute.
    pub target: OccupantAddress,
    /// Whether the operator is starting or resuming this coordination.
    pub action: RoomCoordinateAction,
    /// One unresolved criterion the contribution must address.
    pub criterion: String,
    /// Finite useful contribution or check requested from the lane.
    pub contribution: String,
    /// Explicit condition at which the lane must stop and disposition work.
    pub stop: String,
    /// Automated resumes the resident may issue on new input, from 1 through
    /// [`crate::rooms::ROOM_RESUME_LIMIT`]; absent ends any earlier rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_resume: Option<u8>,
}

/// Operator vocabulary for a scoped room-coordination request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoomCoordinateAction {
    /// Begin coordination from quiescence.
    Start,
    /// Continue with new scope or evidence.
    Resume,
}

/// Put one room aside without force-releasing native work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomPutAside {
    /// Current rendered handle of the anchor task.
    pub room: TaskHandle,
}

/// Read one room's current coordination projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomCoordinationRead {
    /// Current rendered handle of the anchor task.
    pub room: TaskHandle,
}

/// Read one conversation publication's text from its owning source: an
/// operator's from its journal fact, a lane's from its transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomConversationText {
    /// The admitted MSG, ASK, or ANS.
    pub publication: RoomAuthorityStamp,
}

/// One conversation publication's text, or why its source cannot supply it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomConversationTextAnswer {
    /// The publication read.
    pub publication: RoomAuthorityStamp,
    /// Its text at the read barrier.
    pub content: RoomConversationContent,
}

/// What a conversation publication's owning source supplied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RoomConversationContent {
    /// The exact authored text.
    Text { text: String },
    /// A journal fact or retained artifact the publication cites rather than
    /// quotes.
    Linked { source: RoomSource },
    /// The source does not supply it, for a typed reason.
    Unavailable { unavailable: RoomSourceUnavailable },
}

/// One operator control over its own operator items or the questions it
/// answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "control", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RoomControl {
    /// ACK settles exact pending operator items; NAK settles them and declines
    /// an operator ASK's pending result.
    Dispose {
        disposition: RoomReceiptDisposition,
        keys: Vec<RoomReceiptRef>,
        clause: String,
    },
    /// Answer one pending question whose stored respondent is the operator.
    Answer {
        result: QuestionResultRef,
        /// Optional asserted context; it must equal the stored one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<TaskHandle>,
        text: String,
    },
    /// Return one lapsed operator item to pending under the same key.
    Recover { item: RoomReceiptRef },
    /// Assess one shadow relevance observation for quality measurement.
    Assess {
        observation: RoomAuthorityStamp,
        verdict: RoomRelevanceVerdict,
        explanation: String,
    },
}

/// Decline or cancel one pending question result.
///
/// The stored respondent declines and the stored requester cancels; the
/// operator closes for the other party only when `override` says so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomClose {
    pub result: QuestionResultRef,
    pub closure: QuestionClosure,
    pub clause: String,
    /// The operator acts for a stored lane party.
    #[serde(rename = "override")]
    pub override_party: bool,
}

/// What one room control or close moved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomControlAnswer {
    /// Each operator item moved, with the state it moved to.
    pub items: Vec<RoomItemMove>,
    /// Each question result moved, in the corresponding input-item order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub results: Vec<RoomResultMove>,
    /// The answer publication an operator answer recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<RoomAuthorityStamp>,
    /// Key of the answer's return directive to a lane requester.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returned: Option<String>,
    /// The shadow observation an assessment settled, with its verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assessed: Option<(RoomAuthorityStamp, RoomRelevanceVerdict)>,
}

/// One operator item a control moved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomItemMove {
    /// The item.
    pub item: RoomReceiptRef,
    /// Its state after the control: `acked`, `declined`, `cancelled`, or
    /// `pending` after recovery.
    pub state: String,
}

/// One question result a control moved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomResultMove {
    /// The result.
    pub result: QuestionResultRef,
    /// Its state after the control: `answered`, `declined`, or `cancelled`.
    pub state: String,
}

/// One live scoped request projected from ordinary steering authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomCoordinationRequest {
    /// Human-sized steering key.
    pub key: String,
    /// Exact addressed occupant at admission.
    pub target: OccupantAddress,
}

/// A stop request that remains visible while its exact native claim is active.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomCoordinationStop {
    /// Human-sized steering key.
    pub key: String,
    /// Exact addressed holder at admission.
    pub target: OccupantAddress,
    /// Exact claim position, rendered as native base-52.
    pub claim: SequenceCode,
}

/// Current coordination state of one anchor room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomCoordinationView {
    /// Current rendered anchor handle.
    pub room: TaskHandle,
    /// Live unanswered coordination requests, in admission order.
    pub requests: Vec<RoomCoordinationRequest>,
    /// Current holder's still-applicable stop request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<RoomCoordinationStop>,
    /// The live automated-resume rule's budget, when the latest record carries one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<RoomResumeBudget>,
    /// Presentation instruction local consumers apply without persisting geometry.
    pub hide: bool,
}

/// How much of one automated-resume rule the resident has spent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomResumeBudget {
    /// Automated resumes the operator authorized.
    pub limit: u8,
    /// Automated resumes already issued under it.
    pub spent: u8,
}

/// Read the current working-group partition.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingGroupRead {}

/// One room anchor as resolved against the current task projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "standing", rename_all = "kebab-case")]
pub enum WorkingGroupAnchor {
    /// The anchor task still exists and has this current rendered handle.
    Available {
        /// Immutable room and task identity.
        born: TaskStamp,
        /// Handle rendered from the task's current project.
        handle: TaskHandle,
    },
    /// The anchor identity remains in saved organization, but the task is unavailable.
    Unavailable {
        /// Immutable room and task identity.
        born: TaskStamp,
    },
}

/// One current working group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingGroupView {
    /// Stable repository-shared identity.
    pub group: RoomAuthorityStamp,
    /// Room anchors in arranged order, resolved through current task authority.
    pub anchors: Vec<WorkingGroupAnchor>,
}

/// The current working-group partition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingGroupPage {
    /// Revision a change must witness.
    pub revision: u64,
    /// Current groups in operator order.
    pub groups: Vec<WorkingGroupView>,
}

/// Result of one working-group change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingGroupChange {
    /// Partition revision the change committed.
    pub revision: u64,
    /// Current transition outcome.
    pub status: WorkingGroupChangeStatus,
}

/// Current working-group change outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkingGroupChangeStatus {
    /// The design moved and every group it touched still stands.
    Moved,
    /// The design moved out of a group that it alone held, so that group
    /// ended without changing any task or room.
    Ended,
    /// The whole partition was restated.
    Arranged,
}
