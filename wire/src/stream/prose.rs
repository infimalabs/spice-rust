//! Scoped authored prose identity and reconciliation across OMP settlement.
use super::{AgentActivityKind, AgentMessage};
use crate::CanonicalUuid;
use serde::{Deserialize, Serialize};

/// Stable identity of one authored OMP prose block across message settlement.
///
/// RPC message identifiers are run-local. The message creation timestamp,
/// tape source, lane and occupant keep distinct runs and messages separate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageProseOrigin {
    /// Durable tape generation carrying the message.
    pub source: CanonicalUuid,
    /// OMP's RPC message identifier.
    pub message: String,
    /// OMP's message creation time, in epoch milliseconds.
    pub timestamp: u64,
    /// Index of the authored content block within the message.
    pub block: u32,
    /// Whether these exact words came from the completed streamed block.
    pub streamed: bool,
    /// Whether this is the tool-intent fallback for normalized commentary.
    pub intent: bool,
    /// Steering keys explicitly answered by these authored words.
    pub answers: Vec<String>,
}

type ProseMessageKey<'a> = (CanonicalUuid, CanonicalUuid, CanonicalUuid, &'a str, u64);

fn prose_message_key(message: &AgentMessage) -> Option<ProseMessageKey<'_>> {
    message.prose_origin.as_ref().map(|origin| {
        (
            message.lane,
            message.author,
            origin.source,
            origin.message.as_str(),
            origin.timestamp,
        )
    })
}

/// Settled prose representations whose completed streamed block is present.
///
/// Only the explicitly scoped provider block identity participates; repeated
/// words or acknowledgments in different messages are never collapsed.
#[must_use]
pub fn superseded_prose<M: std::borrow::Borrow<AgentMessage>>(cards: &[M]) -> Vec<usize> {
    let mut blocks = std::collections::HashSet::new();
    let mut answers = std::collections::HashSet::new();
    for card in cards {
        let message = card.borrow();
        if message.activity != AgentActivityKind::Prose {
            continue;
        }
        let (Some(origin), Some(key)) = (&message.prose_origin, prose_message_key(message)) else {
            continue;
        };
        if origin.streamed {
            blocks.insert((key, origin.block));
            answers.extend(origin.answers.iter().map(|answer| (key, answer.as_str())));
        }
    }
    if blocks.is_empty() {
        return Vec::new();
    }
    cards
        .iter()
        .enumerate()
        .filter_map(|(position, card)| {
            let message = card.borrow();
            let (Some(origin), Some(key)) = (&message.prose_origin, prose_message_key(message))
            else {
                return None;
            };
            (message.activity == AgentActivityKind::Prose
                && !origin.streamed
                && if origin.intent {
                    !origin.answers.is_empty()
                        && origin
                            .answers
                            .iter()
                            .all(|answer| answers.contains(&(key, answer.as_str())))
                } else {
                    blocks.contains(&(key, origin.block))
                })
            .then_some(position)
        })
        .collect()
}

impl AgentMessage {
    /// Whether these prose cards belong to the same scoped provider message.
    #[must_use]
    pub fn same_prose_message(&self, other: &Self) -> bool {
        match (&self.prose_origin, &other.prose_origin) {
            (Some(left), Some(right)) => {
                self.activity == AgentActivityKind::Prose
                    && other.activity == AgentActivityKind::Prose
                    && self.lane == other.lane
                    && self.author == other.author
                    && left.source == right.source
                    && left.message == right.message
                    && left.timestamp == right.timestamp
            }
            _ => false,
        }
    }

    /// Whether a completed streamed block already represents this settled prose
    /// or a tool-intent fallback for those same answered steering keys.
    #[must_use]
    pub fn prose_superseded_by(&self, other: &Self) -> bool {
        let (Some(settled), Some(streamed)) = (&self.prose_origin, &other.prose_origin) else {
            return false;
        };
        self.activity == AgentActivityKind::Prose
            && other.activity == AgentActivityKind::Prose
            && self.lane == other.lane
            && self.author == other.author
            && settled.source == streamed.source
            && settled.message == streamed.message
            && settled.timestamp == streamed.timestamp
            && !settled.streamed
            && streamed.streamed
            && if settled.intent {
                !settled.answers.is_empty()
                    && settled
                        .answers
                        .iter()
                        .all(|answer| streamed.answers.contains(answer))
            } else {
                settled.block == streamed.block
            }
    }
}
