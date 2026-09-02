//! Canonical browser-card reduction for transcript message streams.

use super::*;

use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct MessageCardKey {
    lane: CanonicalUuid,
    locus: MessageLocus,
}

impl From<&AgentMessage> for MessageCardKey {
    fn from(message: &AgentMessage) -> Self {
        Self {
            lane: message.lane,
            locus: message.locus,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MessageCardKind {
    Prose,
    ToolMedia,
    Compaction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MessageCardFingerprint<'a> {
    kind: MessageCardKind,
    author: CanonicalUuid,
    authored: &'a wire::Instant,
    authored_synthetic: bool,
    observed: Option<&'a wire::Instant>,
    text: &'a str,
    unavailable_tool_terminals: &'a [String],
    media: &'a [wire::Media],
}

fn card_kind(message: &AgentMessage) -> Option<MessageCardKind> {
    match message.activity {
        AgentActivityKind::Prose => Some(MessageCardKind::Prose),
        AgentActivityKind::Tool if !message.media.is_empty() => Some(MessageCardKind::ToolMedia),
        AgentActivityKind::Tool => None,
        AgentActivityKind::Compaction => Some(MessageCardKind::Compaction),
    }
}

fn fingerprint(message: &AgentMessage) -> Option<MessageCardFingerprint<'_>> {
    Some(MessageCardFingerprint {
        kind: card_kind(message)?,
        author: message.author,
        authored: &message.authored,
        authored_synthetic: message.authored_synthetic,
        observed: message.observed.as_ref(),
        text: &message.text,
        unavailable_tool_terminals: &message.unavailable_tool_terminals,
        media: &message.media,
    })
}

#[derive(PartialEq, Eq, Default)]
pub(super) struct MessageCards {
    entries: Vec<Rc<AgentMessage>>,
    limit: Option<usize>,
}

impl MessageCards {
    pub(super) fn entries(&self) -> &[Rc<AgentMessage>] {
        &self.entries
    }

    pub(super) const fn limit(&self) -> Option<usize> {
        self.limit
    }

    /// Replace the retained projection with one authoritative bounded snapshot.
    ///
    /// Equal source-and-content entries retain their allocation identity. Batch
    /// order does not become display order; every path uses the same chronology.
    pub(super) fn snapshot(&mut self, limit: usize, incoming: &[AgentMessage]) -> bool {
        let mut next = Vec::new();
        let mut seen = HashSet::new();
        for message in incoming {
            let key = MessageCardKey::from(message);
            if fingerprint(message).is_none() || !seen.insert(key) {
                continue;
            }
            let retained = self
                .entries
                .iter()
                .find(|existing| {
                    MessageCardKey::from(existing.as_ref()) == key
                        && fingerprint(existing) == fingerprint(message)
                })
                .cloned()
                .unwrap_or_else(|| Rc::new(message.clone()));
            next.push(retained);
        }
        next.sort_by_key(|message| message_chronology_key(message));
        trim_all_lanes(&mut next, limit);
        let changed = self.entries != next;
        self.entries = next;
        self.limit = Some(limit);
        changed
    }

    /// Merge live or paged records through the same identity and fingerprint law.
    pub(super) fn merge<'a>(
        &mut self,
        incoming: impl IntoIterator<Item = &'a AgentMessage>,
    ) -> bool {
        let previous = self.entries.clone();
        let mut mutated = false;
        let mut touched = BTreeSet::new();
        for message in incoming {
            touched.insert(message.lane);
            let key = MessageCardKey::from(message);
            let existing = self
                .entries
                .iter()
                .position(|entry| MessageCardKey::from(entry.as_ref()) == key);
            match (existing, fingerprint(message)) {
                (Some(index), Some(next)) if fingerprint(&self.entries[index]) == Some(next) => {}
                (Some(index), Some(_)) => {
                    self.entries[index] = Rc::new(message.clone());
                    mutated = true;
                }
                (Some(index), None) => {
                    self.entries.remove(index);
                    mutated = true;
                }
                (None, Some(_)) => {
                    self.entries.push(Rc::new(message.clone()));
                    mutated = true;
                }
                (None, None) => {}
            }
        }
        if mutated {
            self.entries
                .sort_by_key(|message| message_chronology_key(message));
            if let Some(limit) = self.limit {
                for lane in touched {
                    trim_agent_message_lane(&mut self.entries, lane, limit);
                }
            }
        }
        // A batch can mutate transiently and reduce to the same projection.
        // Restore its original `Rc`s so a no-op also preserves allocations.
        if self.entries == previous {
            self.entries = previous;
            false
        } else {
            true
        }
    }
}

pub(super) fn message_chronology_key(
    message: &AgentMessage,
) -> (i64, CanonicalUuid, u64, u64, u64, u32) {
    (
        agent_message_display_millis(message),
        message.lane,
        message.locus.member,
        message.locus.offset,
        message.locus.length,
        message.locus.event,
    )
}

fn trim_all_lanes(messages: &mut Vec<Rc<AgentMessage>>, limit: usize) {
    let lanes = messages
        .iter()
        .map(|message| message.lane)
        .collect::<BTreeSet<_>>();
    for lane in lanes {
        trim_agent_message_lane(messages, lane, limit);
    }
}

pub(super) fn trim_agent_message_lane(
    messages: &mut Vec<Rc<AgentMessage>>,
    lane: CanonicalUuid,
    limit: usize,
) {
    let activities = messages
        .iter()
        .filter(|message| message.lane == lane)
        .map(|message| message.activity)
        .collect::<Vec<_>>();
    let mut discard = AgentActivityKind::retention_start(&activities, limit);
    messages.retain(|message| {
        if message.lane != lane || discard == 0 {
            true
        } else {
            discard -= 1;
            false
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(offset: u64, activity: AgentActivityKind) -> AgentMessage {
        AgentMessage {
            lane: "018f82d4-6f59-7d06-92e3-bce9131163e5"
                .parse()
                .expect("fixture lane"),
            author: "018f82d4-6f59-7d06-92e3-bce9131163ef"
                .parse()
                .expect("fixture author"),
            locus: MessageLocus {
                member: 1,
                offset,
                length: 1,
                event: 0,
            },
            authored: "2030-04-12T18:44:09.271Z".parse().expect("fixture instant"),
            authored_synthetic: false,
            observed: None,
            activity,
            text: offset.to_string(),
            unavailable_tool_terminals: Vec::new(),
            media: Vec::new(),
        }
    }

    #[test]
    fn tool_activity_requires_owning_media_to_become_a_card() {
        assert_eq!(card_kind(&message(1, AgentActivityKind::Tool)), None);
        assert_eq!(
            card_kind(&message(2, AgentActivityKind::Compaction)),
            Some(MessageCardKind::Compaction)
        );
    }

    #[test]
    fn zero_capacity_live_replay_is_a_stable_noop() {
        let mut cards = MessageCards::default();
        cards.snapshot(0, &[]);
        let prose = message(1, AgentActivityKind::Prose);

        assert!(!cards.merge(std::iter::once(&prose)));
        assert!(!cards.merge(std::iter::once(&prose)));
        assert!(cards.entries().is_empty());
    }
}
