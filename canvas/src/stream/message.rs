//! Generation-owned retention and publication of captured transcript activity.

use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex};

use wire::{AgentMessage, Fault, FaultClass};

use super::Failure;

/// Newest visible transcript cards retained per lane for a reconnecting Canvas.
pub(super) const MESSAGE_CAPACITY: usize = 256;

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

    pub(super) fn snapshot(&self) -> Result<Vec<AgentMessage>, MessagePublishError> {
        self.retained
            .lock()
            .map(|messages| messages.iter().cloned().collect())
            .map_err(|_| MessagePublishError::Unavailable)
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
        if visible_message(&message) {
            if retained
                .iter()
                .filter(|existing| existing.lane == message.lane && visible_message(existing))
                .count()
                == self.feed.capacity
                && let Some(oldest) = retained
                    .iter()
                    .position(|existing| existing.lane == message.lane && visible_message(existing))
            {
                retained.remove(oldest);
            }
        } else {
            retained.retain(|existing| existing.lane != message.lane || visible_message(existing));
        }
        retained.push_back(message.clone());
        drop(retained);
        let _ = self.feed.updates.send(message);
        Ok(true)
    }
}

/// Whether one activity consumes a lane's visible-card retention budget.
fn visible_message(message: &AgentMessage) -> bool {
    message.activity.is_prose() || !message.media.is_empty()
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
