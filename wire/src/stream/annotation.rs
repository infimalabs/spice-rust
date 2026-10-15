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
    /// Images retained by this directive, each with its exact quote target.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub media_quotes: Vec<crate::MediaQuote>,
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
    /// Exact destination of an authored peer message, or the occupant whose
    /// acknowledgment this card presents for an operator MSG, ASK, or ANS.
    /// Classification badges do not change this provenance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient: Option<MessageDestination>,
    /// Protocol kind of the room publication whose write opened this
    /// directive, when an MSG, ASK, or ANS delivered it. The context then
    /// renders that packet's body without its header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packet: Option<MessageSendKind>,
    /// Result an ANS packet context answers. Other packet kinds carry none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<crate::rooms::QuestionResultRef>,
    /// The delivery a top-level operator direction card presents. Only a
    /// plain operator direction, delivered rather than answered, carries one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<MessageDelivery>,
}

/// One plain operator direction as its receivers' transcripts recorded it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageDelivery {
    /// Operator submit action shared by every target-specific directive the
    /// send fanned out to; a consumer presents one card per send.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fanout: Option<crate::CanonicalUuid>,
    /// Every occupant whose transcript recorded the delivery, in display order.
    pub receivers: Vec<MessageDestination>,
}

/// A peer message's immutable destination, independent of current seating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageDestination {
    /// Lane the published message addressed.
    pub lane: crate::CanonicalUuid,
    /// Occupant seated there when the message was published.
    pub occupant: crate::CanonicalUuid,
}

impl MessageDirective {
    /// Whether this context names a non-empty UTF-8 range and a steering target.
    #[must_use]
    pub fn is_valid_for(&self, text: &str) -> bool {
        let (Ok(start), Ok(end)) = (usize::try_from(self.start), usize::try_from(self.end)) else {
            return false;
        };
        let Some(key) = (match &self.quote_target {
            QuoteTarget::Steering { key, .. } => Some(key.as_str()),
            _ => None,
        }) else {
            return false;
        };
        start < end
            && text.get(start..end).is_some()
            && self.media_quotes.iter().all(|media| {
                !media.name.is_empty()
                    && matches!(
                        &media.target,
                        QuoteTarget::SteeringMedia {
                            key: media_key,
                            ..
                        } if media_key == key
                    )
            })
            && (self.recipient.is_none()
                || (self.sender.is_some() && matches!(self.actor, crate::Actor::Lane(_)))
                || (self.packet.is_some() && matches!(self.actor, crate::Actor::Operator)))
            && (self.result.is_some() == (self.packet == Some(MessageSendKind::Ans)))
            && self.delivery.as_ref().is_none_or(|delivery| {
                !delivery.receivers.is_empty()
                    && matches!(self.actor, crate::Actor::Operator)
                    && self.sender.is_none()
                    && self.recipient.is_none()
                    && self.packet.is_none()
            })
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

/// One authored MSG, ASK, or ANS packet the resident presents in its author's
/// card as an outgoing quote of the packet's body.
///
/// The range covers the rendered quote block exactly. The packet's header
/// does not survive presentation; this annotation carries what it said:
/// packet kind, the exact occupant that wrote it, every occupant its addresses
/// name in header order, whether it names the operator, and every distinct
/// room context in header order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageSend {
    /// Inclusive UTF-8 byte offset where the rendered quote block begins.
    pub start: u32,
    /// Exclusive UTF-8 byte offset immediately after the rendered quote block.
    pub end: u32,
    /// Lane whose occupant authored the packet.
    pub from: crate::CanonicalUuid,
    /// Exact occupant that authored the packet.
    pub sender: crate::CanonicalUuid,
    /// Every addressed occupant, in header order.
    pub recipients: Vec<MessageRecipient>,
    /// Whether one or more selectors name the operator.
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub operator: bool,
    /// Every distinct task/room context, in first header order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contexts: Vec<crate::TaskHandle>,
    /// Authenticated protocol kind. Absence on retained annotations means MSG.
    #[serde(default, skip_serializing_if = "MessageSendKind::is_msg")]
    pub kind: MessageSendKind,
    /// Result an ANS packet names. Other packet kinds carry no result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<crate::rooms::QuestionResultRef>,
}

/// One occupant addressed by a packet header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageRecipient {
    /// Address the header spelled.
    pub address: crate::OccupantAddress,
    /// Lane the addressed occupant was born on.
    pub lane: crate::CanonicalUuid,
    /// Occupant the address names.
    pub occupant: crate::CanonicalUuid,
}

/// Authenticated kind of an authored packet presented as an outgoing quote.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageSendKind {
    /// Ordinary one-way message.
    #[default]
    Msg,
    /// Question opening a result with one audience.
    Ask,
    /// Answer to one named result.
    Ans,
}

impl MessageSendKind {
    /// Whether this is the retained/default MSG spelling.
    #[must_use]
    pub const fn is_msg(&self) -> bool {
        matches!(self, Self::Msg)
    }
}

impl MessageSend {
    /// Whether this send names an exact non-empty UTF-8 range and a valid
    /// destination/result shape for its authenticated packet kind.
    #[must_use]
    pub fn is_valid_for(&self, text: &str) -> bool {
        let (Ok(start), Ok(end)) = (usize::try_from(self.start), usize::try_from(self.end)) else {
            return false;
        };
        let audiences = self.recipients.len() + usize::from(self.operator);
        let route_is_valid = match self.kind {
            MessageSendKind::Msg => {
                self.result.is_none() && (audiences > 0 || !self.contexts.is_empty())
            }
            MessageSendKind::Ask => {
                self.result.is_none() && audiences == 1 && self.contexts.len() <= 1
            }
            MessageSendKind::Ans => {
                self.result.is_some() && audiences == 0 && self.contexts.len() <= 1
            }
        };
        start < end
            && text.get(start..end).is_some()
            && route_is_valid
            && self
                .recipients
                .iter()
                .enumerate()
                .all(|(index, recipient)| {
                    self.recipients[..index]
                        .iter()
                        .all(|earlier| earlier.address != recipient.address)
                })
            && self.contexts.iter().enumerate().all(|(index, context)| {
                !self.contexts[..index]
                    .iter()
                    .any(|earlier| earlier == context)
            })
    }
}

/// Whether sends are exact, ordered, non-overlapping, and disjoint from every
/// directive context.
#[must_use]
pub fn valid_message_sends(
    text: &str,
    sends: &[MessageSend],
    directives: &[MessageDirective],
) -> bool {
    sends.iter().all(|send| send.is_valid_for(text))
        && sends.windows(2).all(|pair| pair[0].end <= pair[1].start)
        && sends.iter().all(|send| {
            directives
                .iter()
                .all(|directive| directive.end <= send.start || send.end <= directive.start)
        })
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
