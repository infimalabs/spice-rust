//! Repository-shared working-group organization.

use serde::{Deserialize, Serialize};

use super::{RoomAnchor, RoomAuthorityStamp, WORKING_GROUP_ANCHOR_LIMIT, all_unique};

/// Repository-shared organization of ordered room anchors.
///
/// A working group owns only its identity and ordered anchor associations; it
/// has no name of its own and lives only while it holds a design. It cannot
/// encode lanes, members, claims, reads, work, possession, or foreground
/// geometry: a Design stops only through its own deferral.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "WorkingGroupWire", into = "WorkingGroupWire")]
pub struct WorkingGroup {
    /// Stable repository-shared identity.
    pub group: RoomAuthorityStamp,
    /// Room anchors in operator order.
    pub anchors: Vec<RoomAnchor>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkingGroupWire {
    group: RoomAuthorityStamp,
    anchors: Vec<RoomAnchor>,
}

impl TryFrom<WorkingGroupWire> for WorkingGroup {
    type Error = &'static str;

    fn try_from(value: WorkingGroupWire) -> Result<Self, Self::Error> {
        if !all_unique(&value.anchors) {
            return Err("working-group anchors must be unique and ordered");
        }
        if value.anchors.len() > WORKING_GROUP_ANCHOR_LIMIT {
            return Err("working-group anchor count exceeds 100");
        }
        Ok(Self {
            group: value.group,
            anchors: value.anchors,
        })
    }
}

impl From<WorkingGroup> for WorkingGroupWire {
    fn from(value: WorkingGroup) -> Self {
        Self {
            group: value.group,
            anchors: value.anchors,
        }
    }
}

/// The working-group partition: every current group in operator order, each
/// holding its designs, guarded by one revision.
///
/// A group stands only while it holds a design, no design stands in two
/// groups, and no group appears twice. Revision zero is the empty partition
/// before any grouping write. A group's fast-mode flag is off until engaged
/// and ends with the group.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    try_from = "WorkingGroupPartitionWire",
    into = "WorkingGroupPartitionWire"
)]
pub struct WorkingGroupPartition {
    /// Revision every grouping change is admitted against.
    pub revision: u64,
    /// Current groups in operator order.
    pub groups: Vec<WorkingGroup>,
    /// Standing groups whose fast-mode flag is on, in ascending identity order.
    pub fast: Vec<RoomAuthorityStamp>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkingGroupPartitionWire {
    revision: u64,
    groups: Vec<WorkingGroup>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    fast: Vec<RoomAuthorityStamp>,
}

impl TryFrom<WorkingGroupPartitionWire> for WorkingGroupPartition {
    type Error = &'static str;

    fn try_from(value: WorkingGroupPartitionWire) -> Result<Self, Self::Error> {
        for (index, group) in value.groups.iter().enumerate() {
            if group.anchors.is_empty() {
                return Err("a working group stands only while it holds a design");
            }
            for earlier in &value.groups[..index] {
                if earlier.group == group.group {
                    return Err("a working group appears once in the partition");
                }
                if earlier
                    .anchors
                    .iter()
                    .any(|anchor| group.anchors.contains(anchor))
                {
                    return Err("a design stands in at most one working group");
                }
            }
        }
        if value.fast.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err("fast working groups must be unique and ascending");
        }
        if value
            .fast
            .iter()
            .any(|fast| !value.groups.iter().any(|group| &group.group == fast))
        {
            return Err("only a standing working group carries a fast-mode flag");
        }
        Ok(Self {
            revision: value.revision,
            groups: value.groups,
            fast: value.fast,
        })
    }
}

impl From<WorkingGroupPartition> for WorkingGroupPartitionWire {
    fn from(value: WorkingGroupPartition) -> Self {
        Self {
            revision: value.revision,
            groups: value.groups,
            fast: value.fast,
        }
    }
}

impl WorkingGroupPartition {
    /// The group holding `design`, if any.
    #[must_use]
    pub fn holder(&self, design: &RoomAnchor) -> Option<&WorkingGroup> {
        self.groups
            .iter()
            .find(|group| group.anchors.contains(design))
    }

    /// Whether `group`'s fast-mode flag is on.
    #[must_use]
    pub fn is_fast(&self, group: &RoomAuthorityStamp) -> bool {
        self.fast.binary_search(group).is_ok()
    }

    /// Turn `group`'s fast-mode flag on or off, saying whether it changed.
    pub fn set_fast(&mut self, group: &RoomAuthorityStamp, fast_mode: bool) -> bool {
        match (self.fast.binary_search(group), fast_mode) {
            (Err(at), true) => self.fast.insert(at, group.clone()),
            (Ok(at), false) => {
                self.fast.remove(at);
            }
            _ => return false,
        }
        true
    }

    /// Keep the fast-mode flags of `previous` whose groups still stand here.
    pub fn carry_fast(&mut self, previous: &Self) {
        self.fast = previous
            .fast
            .iter()
            .filter(|fast| self.groups.iter().any(|group| &group.group == *fast))
            .cloned()
            .collect();
    }
}

/// Move one design into a group, founding it under a fresh identity when no
/// group holds that identity, or out of every group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingGroupMove {
    /// The moving design.
    pub design: RoomAnchor,
    /// Destination group, or absent to leave every group.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub to: Option<RoomAuthorityStamp>,
}

/// Restate the whole partition: group order, designs per group, and design
/// order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingGroupArrange {
    /// Every current group in intended order.
    pub groups: Vec<WorkingGroup>,
}

/// End one standing working group and leave each Design ungrouped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingGroupDisband {
    /// The group being ended.
    pub group: RoomAuthorityStamp,
}

/// Turn one standing working group's fast-mode flag on or off.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingGroupFast {
    /// The flagged group.
    pub group: RoomAuthorityStamp,
    /// Whether its fast-mode flag is on.
    pub fast_mode: bool,
}

/// Immutable fact body recording one design's move and the resulting partition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingGroupMoved {
    /// The design that moved.
    pub design: RoomAnchor,
    /// The group it stood in before, if any.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub from: Option<RoomAuthorityStamp>,
    /// The group it stands in after, if any.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub to: Option<RoomAuthorityStamp>,
    /// The complete partition after the move.
    pub partition: WorkingGroupPartition,
}

/// Immutable fact body recording one whole-partition arrangement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkingGroupsArranged {
    /// The complete partition before the arrangement.
    pub from: WorkingGroupPartition,
    /// The complete partition after it.
    pub partition: WorkingGroupPartition,
}
