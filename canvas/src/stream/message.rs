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
    pub(super) more: Vec<wire::MessageHistoryCursor>,
}

/// One source-backed page immediately preceding a retained message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageHistoryPage {
    /// Older visible messages in display chronology.
    pub messages: Vec<AgentMessage>,
    /// Whether another source-backed page remains inside the retention horizon.
    pub more: bool,
}

/// Durable older-message reader installed by the resident that owns the source.
pub trait MessageHistoryProvider: fmt::Debug + Send + Sync {
    /// Read one bounded visible page before `before`.
    ///
    /// `None` means the provider does not own that lane or cursor. Errors name
    /// source failures without weakening the Canvas request admission path.
    fn history(
        &self,
        lane: wire::CanonicalUuid,
        before: Option<&AgentMessage>,
        limit: usize,
    ) -> Result<Option<MessageHistoryPage>, String>;
}

#[derive(Clone, Debug)]
pub(super) struct MessageFeed {
    retained: Arc<Mutex<VecDeque<AgentMessage>>>,
    history: Arc<Mutex<Option<Arc<dyn MessageHistoryProvider>>>>,
    history_open: Arc<Mutex<HashSet<wire::CanonicalUuid>>>,
    history_closed: Arc<Mutex<HashSet<wire::CanonicalUuid>>>,
    pub(super) updates: tokio::sync::broadcast::Sender<AgentMessage>,
    capacity: usize,
}

impl MessageFeed {
    pub(super) fn new() -> Self {
        let (updates, _) = tokio::sync::broadcast::channel(MESSAGE_CAPACITY);
        Self {
            retained: Arc::new(Mutex::new(VecDeque::new())),
            history: Arc::new(Mutex::new(None)),
            history_open: Arc::new(Mutex::new(HashSet::new())),
            history_closed: Arc::new(Mutex::new(HashSet::new())),
            updates,
            capacity: MESSAGE_CAPACITY,
        }
    }

    pub(super) fn initial_snapshot(&self) -> Result<MessageSnapshot, MessagePublishError> {
        let retained = self
            .retained
            .lock()
            .map_err(|_| MessagePublishError::Unavailable)?;
        let mut lanes = retained
            .iter()
            .filter(|message| visible_message(message))
            .map(|message| message.lane)
            .collect::<BTreeSet<_>>();
        let mut selected = HashSet::new();
        let mut more = Vec::new();
        let history_open = self
            .history_open
            .lock()
            .map_err(|_| MessagePublishError::Unavailable)?;
        lanes.extend(history_open.iter().copied());
        for lane in lanes {
            let visible = retained
                .iter()
                .filter(|message| message.lane == lane && visible_message(message))
                .collect::<Vec<_>>();
            let start = visible.len().saturating_sub(INITIAL_MESSAGE_LIMIT);
            if start > 0 || history_open.contains(&lane) {
                more.push(wire::MessageHistoryCursor {
                    lane,
                    before: visible.get(start).map(|message| message.locus),
                });
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
        before: Option<wire::MessageLocus>,
    ) -> Result<Option<MessageHistoryPage>, MessagePublishError> {
        let retained = self
            .retained
            .lock()
            .map_err(|_| MessagePublishError::Unavailable)?;
        let visible = retained
            .iter()
            .filter(|message| message.lane == lane && visible_message(message))
            .collect::<Vec<_>>();
        let before_message = match self.history_boundary(lane, before, &visible)? {
            HistoryBoundary::Unknown => return Ok(None),
            HistoryBoundary::Bootstrap => None,
            HistoryBoundary::Message(index) => Some((*visible[index]).clone()),
        };
        let history = self
            .history
            .lock()
            .map_err(|_| MessagePublishError::Unavailable)?
            .clone();
        let Some(history) = history else {
            return before.map_or(Ok(None), |before| retained_history(&retained, lane, before));
        };
        if self
            .history_closed
            .lock()
            .map_err(|_| MessagePublishError::Unavailable)?
            .contains(&lane)
        {
            return Ok(Some(MessageHistoryPage {
                messages: Vec::new(),
                more: false,
            }));
        }
        let remaining = self.capacity.saturating_sub(visible.len());
        if remaining == 0 {
            self.history_closed
                .lock()
                .map_err(|_| MessagePublishError::Unavailable)?
                .insert(lane);
            self.history_open
                .lock()
                .map_err(|_| MessagePublishError::Unavailable)?
                .remove(&lane);
            return Ok(Some(MessageHistoryPage {
                messages: Vec::new(),
                more: false,
            }));
        }
        let limit = remaining.min(MESSAGE_HISTORY_PAGE_LIMIT);
        drop(retained);
        if let Some(mut page) = history
            .history(lane, before_message.as_ref(), limit)
            .map_err(MessagePublishError::History)?
        {
            self.retain_history_page(lane, before_message.as_ref(), &mut page)?;
            if page.messages.len() >= remaining || !page.more {
                page.more = false;
                self.history_closed
                    .lock()
                    .map_err(|_| MessagePublishError::Unavailable)?
                    .insert(lane);
                self.history_open
                    .lock()
                    .map_err(|_| MessagePublishError::Unavailable)?
                    .remove(&lane);
            }
            return Ok(Some(page));
        }
        let retained = self
            .retained
            .lock()
            .map_err(|_| MessagePublishError::Unavailable)?;
        before.map_or(Ok(None), |before| retained_history(&retained, lane, before))
    }

    fn history_boundary(
        &self,
        lane: wire::CanonicalUuid,
        before: Option<wire::MessageLocus>,
        visible: &[&AgentMessage],
    ) -> Result<HistoryBoundary, MessagePublishError> {
        if let Some(before) = before {
            return Ok(visible
                .iter()
                .position(|message| message.locus == before)
                .map_or(HistoryBoundary::Unknown, HistoryBoundary::Message));
        }
        let open = self
            .history_open
            .lock()
            .map_err(|_| MessagePublishError::Unavailable)?
            .contains(&lane);
        Ok(if open {
            HistoryBoundary::Bootstrap
        } else {
            HistoryBoundary::Unknown
        })
    }

    fn retain_history_page(
        &self,
        lane: wire::CanonicalUuid,
        before: Option<&AgentMessage>,
        page: &mut MessageHistoryPage,
    ) -> Result<(), MessagePublishError> {
        let mut retained = self
            .retained
            .lock()
            .map_err(|_| MessagePublishError::Unavailable)?;
        for message in &mut page.messages {
            if let Some(current) = retained
                .iter()
                .find(|current| current.lane == message.lane && current.locus == message.locus)
            {
                *message = current.clone();
            }
        }
        validate_history_page(lane, before, page)?;
        for message in &page.messages {
            retain_message(&mut retained, message.clone(), self.capacity)?;
        }
        Ok(())
    }
}

enum HistoryBoundary {
    Unknown,
    Bootstrap,
    Message(usize),
}

fn retained_history(
    retained: &VecDeque<AgentMessage>,
    lane: wire::CanonicalUuid,
    before: wire::MessageLocus,
) -> Result<Option<MessageHistoryPage>, MessagePublishError> {
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

fn validate_history_page(
    lane: wire::CanonicalUuid,
    before: Option<&AgentMessage>,
    page: &MessageHistoryPage,
) -> Result<(), MessagePublishError> {
    if page.messages.len() > MESSAGE_HISTORY_PAGE_LIMIT
        || page.messages.iter().any(|message| {
            message.lane != lane
                || !visible_message(message)
                || before.is_some_and(|before| message.chronology_key() >= before.chronology_key())
        })
        || page
            .messages
            .windows(2)
            .any(|pair| pair[0].chronology_key() >= pair[1].chronology_key())
    {
        return Err(MessagePublishError::InvalidHistory);
    }
    Ok(())
}

/// Generation-scoped ingress for agent activity already committed to tape.
#[derive(Clone, Debug)]
pub struct MessagePublisher {
    pub(super) feed: MessageFeed,
}

impl MessagePublisher {
    /// Install the generation's authenticated durable history reader.
    pub fn set_history_provider(
        &self,
        provider: Arc<dyn MessageHistoryProvider>,
    ) -> Result<(), MessagePublishError> {
        self.feed
            .history_closed
            .lock()
            .map_err(|_| MessagePublishError::Unavailable)?
            .clear();
        *self
            .feed
            .history
            .lock()
            .map_err(|_| MessagePublishError::Unavailable)? = Some(provider);
        Ok(())
    }

    /// Name lanes whose initial retained page has older durable history.
    pub fn set_history_open(
        &self,
        lanes: impl IntoIterator<Item = wire::CanonicalUuid>,
    ) -> Result<(), MessagePublishError> {
        let lanes = lanes.into_iter().collect::<HashSet<_>>();
        self.feed
            .history_closed
            .lock()
            .map_err(|_| MessagePublishError::Unavailable)?
            .retain(|lane| !lanes.contains(lane));
        *self
            .feed
            .history_open
            .lock()
            .map_err(|_| MessagePublishError::Unavailable)? = lanes;
        Ok(())
    }

    /// Keep only one initial page plus a look-behind card in each lane.
    pub fn bound_initial_history(&self) -> Result<(), MessagePublishError> {
        let mut retained = self
            .feed
            .retained
            .lock()
            .map_err(|_| MessagePublishError::Unavailable)?;
        let lanes = retained
            .iter()
            .map(|message| message.lane)
            .collect::<BTreeSet<_>>();
        for lane in lanes {
            trim_visible_lane(&mut retained, lane, INITIAL_MESSAGE_LIMIT + 1);
        }
        Ok(())
    }

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
        let inserted = retain_message(&mut retained, message.clone(), self.feed.capacity)?;
        if !inserted {
            return Ok(false);
        }
        drop(retained);
        let _ = self.feed.updates.send(message);
        Ok(true)
    }

    /// Replace one retained prose message with its terminal presentation.
    ///
    /// The durable locus remains immutable. The only admitted revision raises
    /// `final_answer` from false to true and may replace the derived automatic
    /// speech instruction; every authored field and payload byte must match.
    pub fn revise_final(&self, message: AgentMessage) -> Result<bool, MessagePublishError> {
        wire::media::admit_media(&message.media).map_err(MessagePublishError::InvalidMedia)?;
        let mut retained = self
            .feed
            .retained
            .lock()
            .map_err(|_| MessagePublishError::Unavailable)?;
        let revised = revise_final_message(&mut retained, &message)?;
        if !revised {
            return Ok(false);
        }
        drop(retained);
        let _ = self.feed.updates.send(message);
        Ok(true)
    }
}

fn retain_message(
    retained: &mut VecDeque<AgentMessage>,
    message: AgentMessage,
    capacity: usize,
) -> Result<bool, MessagePublishError> {
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
    let lane = message.lane;
    let insertion = retained
        .iter()
        .position(|existing| existing.chronology_key() > message.chronology_key())
        .unwrap_or(retained.len());
    retained.insert(insertion, message);
    if visible {
        trim_visible_lane(retained, lane, capacity);
    }
    Ok(true)
}

fn revise_final_message(
    retained: &mut VecDeque<AgentMessage>,
    message: &AgentMessage,
) -> Result<bool, MessagePublishError> {
    let Some(index) = retained
        .iter()
        .position(|existing| existing.lane == message.lane && existing.locus == message.locus)
    else {
        return Err(MessagePublishError::IdentityConflict);
    };
    let existing = &retained[index];
    if existing == message {
        return Ok(false);
    }
    let mut expected = existing.clone();
    expected.final_answer = true;
    expected.automatic_speech = message.automatic_speech.clone();
    if existing.activity != wire::AgentActivityKind::Prose
        || existing.final_answer
        || !message.final_answer
        || expected != *message
    {
        return Err(MessagePublishError::IdentityConflict);
    }
    retained[index] = message.clone();
    Ok(true)
}

/// Whether one activity consumes a lane's visible-card retention budget.
fn visible_message(message: &AgentMessage) -> bool {
    message.activity.is_visible() || !message.media.is_empty()
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
    /// A durable history reader returned an invalid page shape.
    InvalidHistory,
    /// The durable history source could not answer a page.
    History(String),
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
            Self::InvalidHistory => formatter.write_str("a transcript history page was invalid"),
            Self::History(detail) => {
                write!(formatter, "transcript history is unavailable: {detail}")
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
