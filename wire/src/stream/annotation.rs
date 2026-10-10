//! Resident-authenticated annotations over one presented agent message: exact
//! broadcast marker ranges and the directive contexts the message quotes.

use serde::{Deserialize, Serialize};

use super::{BROADCAST_MARKER, Instant, QuoteTarget};

/// One resident-authenticated broadcast marker at an exact byte range in the
/// presented message text.
///
/// The resident owns both classification and placement. A consumer may replace
/// only this exact range after validating it against [`Self::kind`]; identical
/// marker prose elsewhere remains ordinary text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageClassification {
    /// Inclusive UTF-8 byte offset where the canonical marker begins.
    pub start: u32,
    /// Exclusive UTF-8 byte offset immediately after the canonical marker.
    pub end: u32,
    /// Authenticated semantic presentation at this range.
    pub kind: MessageClassificationKind,
}

impl MessageClassification {
    /// Whether this annotation names its kind's exact canonical marker on UTF-8
    /// boundaries in `text`.
    #[must_use]
    pub fn is_valid_for(&self, text: &str) -> bool {
        let Ok(start) = usize::try_from(self.start) else {
            return false;
        };
        let Ok(end) = usize::try_from(self.end) else {
            return false;
        };
        text.get(start..end) == Some(self.kind.marker())
    }
}
/// One resident-authenticated directive context embedded in an agent card.
///
/// The range identifies the exact rendered context the quote action belongs to;
/// the target authenticates the original journal text independently of marker
/// presentation, and the actor attributes it exactly as a quote of that target
/// would.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageDirective {
    /// Inclusive UTF-8 byte offset where the rendered directive begins.
    pub start: u32,
    /// Exclusive UTF-8 byte offset immediately after the rendered directive.
    pub end: u32,
    /// Canonical steering target supplied by the resident.
    pub quote_target: QuoteTarget,
    /// Actor on the directive's creating fact.
    pub actor: crate::Actor,
    /// Journal instant at which the quoted directive was published.
    pub authored: Instant,
    /// Kind of resident-generated directive this context quotes, from its
    /// typed provenance. The absent wire spelling means ordinary direction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<DirectiveBadge>,
    /// Exact occupant that authored the directive as an MSG packet, which the
    /// context names instead of whoever now represents the sending lane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<crate::CanonicalUuid>,
}

impl MessageDirective {
    /// Whether this context names a non-empty UTF-8 range and a steering target.
    #[must_use]
    pub fn is_valid_for(&self, text: &str) -> bool {
        let (Ok(start), Ok(end)) = (usize::try_from(self.start), usize::try_from(self.end)) else {
            return false;
        };
        start < end
            && text.get(start..end).is_some()
            && matches!(&self.quote_target, QuoteTarget::Steering { .. })
    }
}
/// Whether directive contexts are exact, ordered, and non-overlapping.
#[must_use]
pub fn valid_message_directives(text: &str, directives: &[MessageDirective]) -> bool {
    directives
        .iter()
        .all(|directive| directive.is_valid_for(text))
        && directives
            .windows(2)
            .all(|pair| pair[0].end <= pair[1].start)
}

/// Semantic source-marker presentation authenticated by the resident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MessageClassificationKind {
    /// Broadcast steering.
    Broadcast,
}

impl MessageClassificationKind {
    /// Canonical source marker this semantic presentation replaces.
    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Broadcast => BROADCAST_MARKER,
        }
    }
}

/// Kind of resident-generated directive a quoted context carries.
///
/// Identity comes from the directive's typed provenance — the review-feedback
/// control, a taste reminder, or a confirmed taste judgment — and never from
/// its prose, so the directive text carries no marker for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DirectiveBadge {
    /// Peer-review feedback returned to the phase author.
    Review,
    /// Taste guidance.
    Taste,
}

/// Whether ordered classifications are exact, non-overlapping annotations of
/// their presented text.
#[must_use]
pub fn valid_message_classifications(
    text: &str,
    classifications: &[MessageClassification],
) -> bool {
    classifications
        .iter()
        .all(|classification| classification.is_valid_for(text))
        && classifications
            .windows(2)
            .all(|pair| pair[0].end <= pair[1].start)
}
