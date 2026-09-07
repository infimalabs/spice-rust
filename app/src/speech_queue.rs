//! Global speech arbitration and bounded automatic-narration cursors.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Maximum number of per-agent narration cursors retained by one browser.
pub const MAX_NARRATION_CURSORS: usize = 256;

/// One request entering the browser-global speech arbiter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpeechRequest {
    lane: Option<String>,
    message_key: Option<String>,
    text: String,
    automatic: bool,
}

impl SpeechRequest {
    /// Operator-requested speech for one exact rendered message.
    #[must_use]
    pub fn message(lane: String, message_key: String, text: String) -> Self {
        Self {
            lane: Some(lane),
            message_key: Some(message_key),
            text,
            automatic: false,
        }
    }

    /// Automatic narration from one rendered lineage.
    #[must_use]
    pub fn narration(lane: String, message_key: String, text: String) -> Self {
        Self {
            lane: Some(lane),
            message_key: Some(message_key),
            text,
            automatic: true,
        }
    }

    /// Stable rendered message identity carried by this request.
    #[must_use]
    pub fn message_key(&self) -> Option<&str> {
        self.message_key.as_deref()
    }
}

/// One request admitted as either the committed head or queued tail.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpeechEntry {
    id: u64,
    request: SpeechRequest,
}

impl SpeechEntry {
    /// Queue identity used to reject late renderer and media callbacks.
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.id
    }

    /// Text handed to the selected speech backend.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.request.text
    }

    /// Rendered lane that originated this entry, when it has one.
    #[must_use]
    pub fn lane(&self) -> Option<&str> {
        self.request.lane.as_deref()
    }

    /// Rendered message that owns this entry, when speech came from a card.
    #[must_use]
    pub fn message_key(&self) -> Option<&str> {
        self.request.message_key.as_deref()
    }
}

/// Result of offering one request to the queue.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Enqueue {
    /// The queue was idle and this entry became its committed head.
    Start(SpeechEntry),
    /// A head already owns playback; this request remains queued.
    Queued,
}

/// Result of settling one asynchronous playback identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Advance {
    /// The identity was superseded and may change nothing.
    Ignored,
    /// The accepted head settled and left no successor.
    Idle,
    /// The accepted head settled and this queued successor became current.
    Start(SpeechEntry),
}

/// Queue action caused by an unmarked pause from the external media element.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExternalPause {
    /// The media callback belonged to an entry that no longer owns playback.
    Ignored,
    /// Manual playback stopped the whole speech pipeline.
    Stopped,
    /// Automatic narration settled and had no surviving successor.
    Idle,
    /// Automatic narration settled and this queued successor became current.
    Start(SpeechEntry),
}

/// Result of suppressing automatic playback while preserving explicit work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SuppressAutomatic {
    /// No automatic head owned playback; any manual head remains current.
    Preserved,
    /// The automatic head was stopped and no manual successor remains.
    Idle,
    /// The automatic head was stopped and this manual successor became current.
    Start(SpeechEntry),
}

/// The one queue deciding which browser speech request owns playback.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SpeechQueue {
    next_id: u64,
    active: Option<SpeechEntry>,
    pending: VecDeque<SpeechEntry>,
}

impl SpeechQueue {
    /// Preserve the active head and queue one request behind it.
    ///
    /// Automatic bursts retain only their newest not-yet-started entry per
    /// lineage. Manual requests remain explicit FIFO acts.
    pub fn enqueue(&mut self, request: SpeechRequest) -> Enqueue {
        let entry = SpeechEntry {
            id: self.next_identity(),
            request,
        };
        if self.active.is_none() {
            self.active = Some(entry.clone());
            return Enqueue::Start(entry);
        }
        if entry.request.automatic
            && let Some(lane) = entry.lane()
        {
            self.pending.retain(|candidate| {
                !(candidate.request.automatic && candidate.lane() == Some(lane))
            });
        }
        self.pending.push_back(entry);
        Enqueue::Queued
    }

    /// Settle only the current head and select its oldest surviving successor.
    pub fn finish(&mut self, id: u64) -> Advance {
        if self.active.as_ref().map(SpeechEntry::id) != Some(id) {
            return Advance::Ignored;
        }
        self.active = self.pending.pop_front();
        self.active.clone().map_or(Advance::Idle, Advance::Start)
    }

    /// Apply an unmarked pause from OS media controls or a lock screen.
    ///
    /// A paused narration clip is a recoverable interruption, so its queued
    /// successor may continue. A paused manual clip is an external stop and
    /// clears the global queue. Deliberate controller pauses never call this
    /// method.
    pub fn external_pause(&mut self, id: u64) -> ExternalPause {
        let Some(active) = self.active.as_ref() else {
            return ExternalPause::Ignored;
        };
        if active.id() != id {
            return ExternalPause::Ignored;
        }
        if !active.request.automatic {
            self.stop();
            return ExternalPause::Stopped;
        }
        match self.finish(id) {
            Advance::Ignored => unreachable!("the current speech identity was checked above"),
            Advance::Idle => ExternalPause::Idle,
            Advance::Start(entry) => ExternalPause::Start(entry),
        }
    }

    /// Abandon all speech immediately. Repetition changes nothing.
    pub fn stop(&mut self) {
        self.active = None;
        self.pending.clear();
    }

    /// Stop and discard automatic speech without disturbing explicit playback.
    pub fn suppress_automatic(&mut self) -> SuppressAutomatic {
        self.pending.retain(|entry| !entry.request.automatic);
        if !self
            .active
            .as_ref()
            .is_some_and(|entry| entry.request.automatic)
        {
            return SuppressAutomatic::Preserved;
        }
        self.active = self.pending.pop_front();
        self.active
            .clone()
            .map_or(SuppressAutomatic::Idle, SuppressAutomatic::Start)
    }

    /// Remove only queued work from one lineage, preserving the committed head.
    pub fn abandon_lane(&mut self, lane: &str) {
        self.pending
            .retain(|entry| entry.lane().is_none_or(|candidate| candidate != lane));
    }

    /// Whether a renderer or media callback still belongs to the current head.
    #[must_use]
    pub fn is_active(&self, id: u64) -> bool {
        self.active.as_ref().map(SpeechEntry::id) == Some(id)
    }

    /// Whether any clip or renderer currently owns the queue.
    #[must_use]
    pub const fn active(&self) -> bool {
        self.active.is_some()
    }

    /// Exact rendered message owned by the current head, when it has one.
    #[must_use]
    pub fn active_message_key(&self) -> Option<&str> {
        self.active.as_ref().and_then(SpeechEntry::message_key)
    }

    /// Number of entries waiting behind the committed head.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    fn next_identity(&mut self) -> u64 {
        self.next_id = self.next_id.wrapping_add(1);
        self.next_id
    }
}

/// One agent message offered to automatic narration in visible order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NarrationMessage {
    /// Stable agent lane that produced the message.
    pub lane: String,
    /// Stable message identity within that lane.
    pub key: String,
    /// Prose to enqueue if the cursor admits it as live.
    pub text: String,
    /// Resident-classified utterance eligible for default Speak mode.
    pub automatic_speech: Option<String>,
}

/// One message that crossed a per-agent cursor as live narration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NarrationLine {
    /// Stable agent lane that produced the message.
    pub lane: String,
    /// Stable rendered message identity.
    pub key: String,
    /// Prose to enqueue through the global arbiter.
    pub text: String,
    /// Resident-classified utterance eligible for default Speak mode.
    pub automatic_speech: Option<String>,
}

/// Bounded, recency-ordered per-agent narration cursors.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NarrationCursors {
    rows: VecDeque<(String, String)>,
}

impl NarrationCursors {
    /// Restore browser-persisted rows, keeping one newest row per agent.
    #[must_use]
    pub fn restore(rows: Vec<(String, String)>) -> Self {
        let mut cursors = Self::default();
        for (lane, key) in rows {
            cursors.update(lane, key);
        }
        cursors
    }

    /// Observe one rendered lineage and return only messages newer than every
    /// known per-agent cursor. A materialization never narrates its history.
    pub fn observe(
        &mut self,
        materialization: bool,
        messages: &[NarrationMessage],
    ) -> Vec<NarrationLine> {
        let unseen = self.unseen(materialization, messages);
        let mut newest = BTreeMap::<String, String>::new();
        for message in messages {
            newest.insert(message.lane.clone(), message.key.clone());
        }
        for (lane, key) in newest {
            self.update(lane, key);
        }
        let mut admitted = BTreeSet::new();
        messages
            .iter()
            .rev()
            .filter(|message| {
                unseen.contains(&(message.lane.clone(), message.key.clone()))
                    && admitted.insert(message.lane.clone())
            })
            .map(|message| NarrationLine {
                lane: message.lane.clone(),
                key: message.key.clone(),
                text: message.text.clone(),
                automatic_speech: message.automatic_speech.clone(),
            })
            .collect()
    }

    /// Rows suitable for bounded browser persistence, oldest first.
    #[must_use]
    pub fn rows(&self) -> Vec<(String, String)> {
        self.rows.iter().cloned().collect()
    }

    /// Number of agent cursors currently retained.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether browser persistence currently carries no agent cursor.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn unseen(
        &self,
        materialization: bool,
        messages: &[NarrationMessage],
    ) -> BTreeSet<(String, String)> {
        let mut by_lane = BTreeMap::<&str, Vec<&NarrationMessage>>::new();
        for message in messages {
            by_lane.entry(&message.lane).or_default().push(message);
        }
        let mut unseen = BTreeSet::new();
        for (lane, lane_messages) in by_lane {
            let start = if materialization {
                lane_messages.len()
            } else {
                self.cursor(lane)
                    .and_then(|cursor| {
                        lane_messages
                            .iter()
                            .position(|message| message.key == cursor)
                            .map(|index| index + 1)
                    })
                    .unwrap_or(lane_messages.len())
            };
            unseen.extend(
                lane_messages[start..]
                    .iter()
                    .map(|message| (message.lane.clone(), message.key.clone())),
            );
        }
        unseen
    }

    fn cursor(&self, lane: &str) -> Option<&str> {
        self.rows
            .iter()
            .find_map(|(candidate, key)| (candidate == lane).then_some(key.as_str()))
    }

    fn update(&mut self, lane: String, key: String) {
        self.rows.retain(|(candidate, _)| candidate != &lane);
        self.rows.push_back((lane, key));
        while self.rows.len() > MAX_NARRATION_CURSORS {
            self.rows.pop_front();
        }
    }
}
