//! Generation-owned retention and publication of captured transcript activity.

use std::collections::{BTreeSet, HashSet, VecDeque};
use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex};

use wire::{AgentMessage, Fault, FaultClass};

use super::Failure;

/// Newest visible transcript cards retained per lane for a reconnecting Canvas.
pub const MESSAGE_CAPACITY: usize = 256;

/// Newest visible cards published per lane on a cold Canvas subscription.
pub const INITIAL_MESSAGE_LIMIT: usize = 25;

/// Older visible cards returned for one member-local history request.
pub const MESSAGE_HISTORY_PAGE_LIMIT: usize = 50;

pub(super) struct MessageSnapshot {
    pub(super) messages: Vec<AgentMessage>,
    pub(super) more: Vec<wire::CanonicalUuid>,
}

pub(super) struct MessageHistoryPage {
    pub(super) messages: Vec<AgentMessage>,
    pub(super) more: bool,
}

#[derive(Clone, Debug)]
pub(super) struct MessageFeed {
    retained: Arc<Mutex<VecDeque<AgentMessage>>>,
    pub(super) updates: tokio::sync::broadcast::Sender<AgentMessage>,
    capacity: usize,
}

impl MessageFeed {
    pub(super) fn new() -> Self {
        let (updates, _) = tokio::sync::broadcast::channel(MESSAGE_CAPACITY);
        Self {
            retained: Arc::new(Mutex::new(VecDeque::new())),
            updates,
            capacity: MESSAGE_CAPACITY,
        }
    }

    pub(super) fn initial_snapshot(&self) -> Result<MessageSnapshot, MessagePublishError> {
        let retained = self
            .retained
            .lock()
            .map_err(|_| MessagePublishError::Unavailable)?;
        let lanes = retained
            .iter()
            .filter(|message| visible_message(message))
            .map(|message| message.lane)
            .collect::<BTreeSet<_>>();
        let mut selected = HashSet::new();
        let mut more = Vec::new();
        for lane in lanes {
            let visible = retained
                .iter()
                .filter(|message| message.lane == lane && visible_message(message))
                .collect::<Vec<_>>();
            let start = visible.len().saturating_sub(INITIAL_MESSAGE_LIMIT);
            if start > 0 {
                more.push(lane);
            }
            selected.extend(
                visible[start..]
                    .iter()
                    .map(|message| (message.lane, message.locus)),
            );
        }
        let messages = retained
            .iter()
            .filter(|message| {
                !visible_message(message) || selected.contains(&(message.lane, message.locus))
            })
            .cloned()
            .collect();
        Ok(MessageSnapshot { messages, more })
    }

    pub(super) fn history(
        &self,
        lane: wire::CanonicalUuid,
        before: wire::MessageLocus,
    ) -> Result<Option<MessageHistoryPage>, MessagePublishError> {
        let retained = self
            .retained
            .lock()
            .map_err(|_| MessagePublishError::Unavailable)?;
        let visible = retained
            .iter()
            .filter(|message| message.lane == lane && visible_message(message))
            .collect::<Vec<_>>();
        let Some(end) = visible.iter().position(|message| message.locus == before) else {
            return Ok(None);
        };
        let start = end.saturating_sub(MESSAGE_HISTORY_PAGE_LIMIT);
        Ok(Some(MessageHistoryPage {
            messages: visible[start..end]
                .iter()
                .map(|message| (*message).clone())
                .collect(),
            more: start > 0,
        }))
    }
}

/// Generation-scoped ingress for agent activity already committed to tape.
#[derive(Clone, Debug)]
pub struct MessagePublisher {
    pub(super) feed: MessageFeed,
}

impl MessagePublisher {
    /// Retain and broadcast one transcript activity exactly once.
    ///
    /// # Errors
    ///
    /// Returns an error when retained state is unavailable or one durable
    /// transcript identity is reused for different content, or when an
    /// upstream emitter presents media outside the shared admission law.
    pub fn publish(&self, message: AgentMessage) -> Result<bool, MessagePublishError> {
        wire::media::admit_media(&message.media).map_err(MessagePublishError::InvalidMedia)?;
        let mut retained = self
            .feed
            .retained
            .lock()
            .map_err(|_| MessagePublishError::Unavailable)?;
        if let Some(existing) = retained
            .iter()
            .find(|existing| existing.lane == message.lane && existing.locus == message.locus)
        {
            return if existing == &message {
                Ok(false)
            } else {
                Err(MessagePublishError::IdentityConflict)
            };
        }
        let visible = visible_message(&message);
        if !visible {
            retained.retain(|existing| existing.lane != message.lane || visible_message(existing));
        }
        let insertion = retained
            .iter()
            .position(|existing| existing.chronology_key() > message.chronology_key())
            .unwrap_or(retained.len());
        retained.insert(insertion, message.clone());
        if visible {
            trim_visible_lane(&mut retained, message.lane, self.feed.capacity);
        }
        drop(retained);
        let _ = self.feed.updates.send(message);
        Ok(true)
    }
}

/// Whether one activity consumes a lane's visible-card retention budget.
fn visible_message(message: &AgentMessage) -> bool {
    matches!(
        message.activity,
        wire::AgentActivityKind::Prose | wire::AgentActivityKind::Compaction
    ) || !message.media.is_empty()
}

fn trim_visible_lane(
    retained: &mut VecDeque<AgentMessage>,
    lane: wire::CanonicalUuid,
    limit: usize,
) {
    let activities = retained
        .iter()
        .filter(|message| message.lane == lane && visible_message(message))
        .map(|message| message.activity)
        .collect::<Vec<_>>();
    let mut discard = wire::AgentActivityKind::retention_start(&activities, limit);
    retained.retain(|message| {
        if message.lane != lane || !visible_message(message) || discard == 0 {
            true
        } else {
            discard -= 1;
            false
        }
    });
}

/// A capture-backed message could not enter the Canvas view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MessagePublishError {
    /// Typed media failed the shared count, size, type, or byte admission law.
    InvalidMedia(wire::MediaError),
    /// Retained message state could not be acquired.
    Unavailable,
    /// One tape event identity was reused for different content.
    IdentityConflict,
}

impl fmt::Display for MessagePublishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMedia(error) => {
                write!(formatter, "canvas message media is invalid: {error}")
            }
            Self::Unavailable => formatter.write_str("canvas message retention is unavailable"),
            Self::IdentityConflict => {
                formatter.write_str("a transcript event identity named different message content")
            }
        }
    }
}

impl Error for MessagePublishError {}

pub(super) fn message_snapshot_fault(error: MessagePublishError) -> Failure {
    Failure::Fault(Fault {
        class: FaultClass::Environment,
        reason: "stream::message-snapshot-unavailable".to_owned(),
        detail: error.to_string(),
    })
}
