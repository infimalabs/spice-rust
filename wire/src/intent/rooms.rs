//! Bounded room-coordination and working-group command DTOs.

use serde::{Deserialize, Serialize};

use crate::rooms::RoomAuthorityStamp;
pub use crate::rooms::{WorkingGroupPut, WorkingGroupRemove};
use crate::{OccupantAddress, Seq, SequenceCode, TaskHandle, TaskStamp};

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
    /// Presentation instruction local consumers apply without persisting geometry.
    pub hide: bool,
}

/// Largest working-group page one command may request.
pub const WORKING_GROUP_PAGE_LIMIT_MAX: u16 = 100;

/// Read one group exactly, or page all current groups in stable identity order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingGroupRead {
    /// Exact group to inspect. When absent, page the current group catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<RoomAuthorityStamp>,
    /// Continue the catalog strictly after this identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<RoomAuthorityStamp>,
    /// Maximum catalog rows, from one through [`WORKING_GROUP_PAGE_LIMIT_MAX`].
    pub limit: u16,
}

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

/// One current saved working group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingGroupView {
    /// Stable repository-shared identity.
    pub group: RoomAuthorityStamp,
    /// Operator-visible name.
    pub name: String,
    /// Room anchors in saved order, resolved through current task authority.
    pub anchors: Vec<WorkingGroupAnchor>,
    /// Current journal head guarding complete replacement or removal.
    pub head: Seq,
}

/// One bounded working-group catalog page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingGroupPage {
    /// Current groups in stable identity order.
    pub groups: Vec<WorkingGroupView>,
    /// Exact requested group that is not currently available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<RoomAuthorityStamp>,
    /// Last returned identity when more groups remain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<RoomAuthorityStamp>,
}

/// Result of one working-group replacement or removal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingGroupChange {
    /// Stable group identity changed by the command.
    pub group: RoomAuthorityStamp,
    /// Current transition outcome.
    pub status: WorkingGroupChangeStatus,
}

/// Current working-group mutation outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkingGroupChangeStatus {
    /// The complete intended group state was stored.
    Stored,
    /// The saved group was removed without changing any task or room.
    Removed,
}
