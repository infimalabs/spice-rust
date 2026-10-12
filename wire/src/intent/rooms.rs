//! Bounded working-group commands and their current projections.

use serde::{Deserialize, Serialize};

use crate::rooms::RoomAuthorityStamp;
pub use crate::rooms::{WorkingGroupPut, WorkingGroupRemove};
use crate::{Seq, TaskHandle, TaskStamp};

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
