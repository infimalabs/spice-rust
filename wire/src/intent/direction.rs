//! Operator direction history: request, window, and grouped answer DTOs.

use serde::{Deserialize, Serialize};

use crate::envelope::Seq;
use crate::ident::{CanonicalUuid, Instant};
use crate::witness::Disposition;

/// Largest direction group page one history read accepts.
pub const DIRECTION_HISTORY_PAGE_LIMIT_MAX: u16 = 50;
/// Largest UTF-8 text query one direction history read accepts.
pub const DIRECTION_HISTORY_QUERY_BYTES_MAX: usize = 256;
/// Most recipient lanes one direction history filter names.
pub const DIRECTION_HISTORY_LANES_MAX: usize = 64;

/// Read operator-authored direction history, one fanout group per row.
///
/// Every filter applies before paging, and a group is kept or dropped whole:
/// it matches when any recipient matches. Filters of different kinds combine
/// conjunctively.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SteerHistory {
    /// Which slice of the filtered newest-first sequence to return.
    pub window: DirectionWindow,
    /// ASCII case-insensitive substring of the direction text. Empty or
    /// whitespace-only text is unfiltered.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub query: Option<String>,
    /// Keep groups addressing any of these lanes; empty keeps every lane.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lanes: Vec<CanonicalUuid>,
    /// Keep groups with a recipient in any of these dispositions; empty
    /// keeps every disposition. [`Disposition::Live`] selects waiting ones.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dispositions: Vec<Disposition>,
    /// Keep groups authored at or before this instant.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub authored_before: Option<Instant>,
    /// Maximum groups returned, at most [`DIRECTION_HISTORY_PAGE_LIMIT_MAX`].
    pub limit: u16,
}

/// Keyset position of one direction history page.
///
/// Positions are group positions: the first creating fact of a fanout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DirectionWindow {
    /// The newest matching groups.
    Newest,
    /// Matching groups strictly older than this group position.
    Before { position: Seq },
    /// Matching groups strictly newer than this group position, the oldest
    /// of them first in selection but still presented newest first.
    After { position: Seq },
    /// Start at the group holding this directive or fanout identity, then
    /// continue with older matching groups.
    Anchor { identity: CanonicalUuid },
}

/// One bounded page of operator direction history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectionHistoryAnswer {
    /// Matching groups, newest first.
    pub groups: Vec<DirectionGroup>,
    /// Accepted maximum number of groups in this page.
    pub limit: u16,
    /// Total matching groups at this answer's snapshot.
    pub total: u64,
    /// Pass as [`DirectionWindow::Before`] for older groups; absent once
    /// none remain.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub older: Option<Seq>,
    /// Pass as [`DirectionWindow::After`] for newer groups; absent once
    /// none remain.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub newer: Option<Seq>,
}

/// One operator submit action and every directive it fanned out to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectionGroup {
    /// Operator submit action shared by the recipients.
    pub fanout: CanonicalUuid,
    /// First creating fact of the group; its keyset position.
    pub position: Seq,
    /// First line authored outside quoted context, when the producer found one.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub preview: Option<String>,
    /// Commit instant of the original publication.
    pub authored: Instant,
    /// Every recipient in creation order.
    pub recipients: Vec<DirectionRecipient>,
}

/// One lane's directive within a direction group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectionRecipient {
    /// Durable directive identity.
    pub directive: CanonicalUuid,
    /// Lane the directive addressed.
    pub lane: CanonicalUuid,
    /// Human-sized acknowledgment key.
    pub key: String,
    /// Current disposition.
    pub disposition: Disposition,
    /// Retiring fact; absent while the directive is live.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub retirement: Option<DirectionRetirement>,
}

/// Where and when a directive retired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectionRetirement {
    /// Journal position of the retiring fact.
    pub position: Seq,
    /// Commit instant of the retiring fact.
    pub at: Instant,
}
