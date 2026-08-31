use dialect::Priority;

use crate::task::Entry;

/// Durable queue-age scalar derived from a task's sole birth fact.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Urgency(u64);

impl Urgency {
    /// Score one entry from its immutable queue origin.
    ///
    /// Older creation facts score higher. No later statement, dependency,
    /// deferral, phase, assignment, or claim mutation can reset the origin.
    #[must_use]
    pub fn from_heads(entry: &Entry) -> Self {
        Self(u64::MAX - entry.statement().queue_origin.get())
    }

    /// Numeric score for rendering and diagnostics.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

pub(crate) fn declared_priority(entry: &Entry) -> i64 {
    entry
        .statement()
        .tags
        .iter()
        .filter_map(|tag| priority_from_tag(tag))
        .map(priority_score)
        .max()
        .unwrap_or(0)
}

/// The prefix every priority tag carries on the statement.
pub const PRIORITY_TAG_PREFIX: &str = "priority:";

/// Read one declared priority off a statement tag, which may not be one.
///
/// Priority reaches a task as a statement tag rather than a workflow field, so
/// a spelling this refuses is not an error anyone sees: it scores zero and the
/// task ranks below every task that declared a priority. Matching is exact
/// because this reads stored tags rather than authored input -- a door that
/// accepts a caller's word normalizes it with [`Priority::parse`] first, so
/// what reaches a tag is already the canonical spelling.
#[must_use]
pub fn priority_from_tag(tag: &str) -> Option<Priority> {
    priority_from_wire(tag.strip_prefix(PRIORITY_TAG_PREFIX)?)
}

/// Read one declared priority off the word a stored tag carries.
///
/// Every reader of a stored priority goes through here, so what the board
/// renders and what the allocator scores cannot disagree about which spellings
/// count.
#[must_use]
pub fn priority_from_wire(value: &str) -> Option<Priority> {
    Priority::ALL
        .into_iter()
        .find(|priority| priority.as_str() == value)
}

/// Read one priority a caller wrote, in either form this build uses.
///
/// The board renders a priority as a single letter, so an agent reading a row
/// and correcting what it just read types that letter back. Accepting the word
/// alone would refuse the exact spelling the board printed, and the predecessor
/// takes both. Case and surrounding space are the caller's business either way.
#[must_use]
pub fn priority_from_authored(value: &str) -> Option<Priority> {
    let value = value.trim();
    Priority::ALL
        .into_iter()
        .find(|priority| priority_label(*priority).eq_ignore_ascii_case(value))
        .or_else(|| Priority::parse(value))
}

/// The whole statement tag that declares one priority.
#[must_use]
pub fn priority_tag(priority: Priority) -> String {
    format!("{PRIORITY_TAG_PREFIX}{}", priority.as_str())
}

/// What one declared priority contributes to the allocation scalar.
#[must_use]
pub const fn priority_score(priority: Priority) -> i64 {
    match priority {
        Priority::Critical => 5_000,
        Priority::High => 4_000,
        Priority::Medium => 3_000,
        Priority::Low => 2_000,
        Priority::None => 1_000,
    }
}

/// The single-letter form the board renders for one declared priority.
#[must_use]
pub const fn priority_label(priority: Priority) -> &'static str {
    match priority {
        Priority::Critical => "C",
        Priority::High => "H",
        Priority::Medium => "M",
        Priority::Low => "L",
        Priority::None => "N",
    }
}
