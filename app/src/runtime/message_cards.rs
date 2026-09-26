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
    final_answer: bool,
    classifications: &'a [wire::MessageClassification],
    automatic_speech: Option<&'a wire::AutomaticSpeech>,
    unavailable_tool_terminals: &'a [String],
    media: &'a [wire::Media],
}

fn card_kind(message: &AgentMessage) -> Option<MessageCardKind> {
    match message.activity {
        AgentActivityKind::Prose => Some(MessageCardKind::Prose),
        AgentActivityKind::Tool if !message.media.is_empty() => Some(MessageCardKind::ToolMedia),
        AgentActivityKind::Tool | AgentActivityKind::Reasoning | AgentActivityKind::Turn => None,
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
        final_answer: message.final_answer,
        classifications: &message.classifications,
        automatic_speech: message.automatic_speech.as_ref(),
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
        reconcile_boundaries(&mut next);
        next.sort_by_key(|message| message.chronology_key());
        trim_all_lanes(&mut next, limit);
        let changed = self.entries != next;
        self.entries = next;
        self.limit = Some(limit);
        changed
    }

    /// Merge a same-scope subscription head without discarding hydrated history.
    pub(super) fn reconnect_snapshot(&mut self, limit: usize, incoming: &[AgentMessage]) -> bool {
        let previous = self.entries.clone();
        self.limit = Some(limit);
        let _ = self.merge(incoming);
        trim_all_lanes(&mut self.entries, limit);
        self.entries != previous
    }

    /// Whether a retained-history interval can only extend the visible edge.
    ///
    /// Exact overlaps are harmless, and a preserved compaction may replace its
    /// captured announcement. Every other new card must precede the oldest
    /// retained card in its lane so history cannot rewrite a measured middle
    /// epoch.
    pub(super) fn admits_history(&self, incoming: &[AgentMessage]) -> bool {
        let mut seen = HashSet::new();
        incoming.iter().all(|message| {
            let Some(next) = fingerprint(message) else {
                return true;
            };
            let key = MessageCardKey::from(message);
            if !seen.insert(key) {
                return false;
            }
            if let Some(existing) = self
                .entries
                .iter()
                .find(|existing| MessageCardKey::from(existing.as_ref()) == key)
            {
                return fingerprint(existing) == Some(next);
            }
            if self
                .entries
                .iter()
                .any(|existing| existing.boundary_superseded_by(message))
            {
                return true;
            }
            self.entries
                .iter()
                .filter(|existing| existing.lane == message.lane)
                .min_by_key(|existing| existing.chronology_key())
                .is_none_or(|oldest| message.chronology_key() < oldest.chronology_key())
        })
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
        mutated |= reconcile_boundaries(&mut self.entries);
        if mutated {
            self.entries.sort_by_key(|message| message.chronology_key());
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

/// Drop each captured announcement whose preserved record is also retained.
///
/// The resident already collapses a pair it merges itself, but the two records
/// reach the browser through different frames: the captured announcement
/// arrives live while the preserved record arrives with an older page or a
/// reconnect. The same rule applied here leaves one boundary either way.
fn reconcile_boundaries(entries: &mut Vec<Rc<AgentMessage>>) -> bool {
    let superseded = wire::superseded_boundaries(entries);
    if superseded.is_empty() {
        return false;
    }
    for position in superseded.into_iter().rev() {
        entries.remove(position);
    }
    true
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
            final_answer: false,
            text: offset.to_string(),
            classifications: Vec::new(),
            automatic_speech: None,
            unavailable_tool_terminals: Vec::new(),
            boundary: None,
            media: Vec::new(),
        }
    }

    #[test]
    fn turn_events_do_not_displace_final_answers_in_bounded_card_history() {
        let mut cards = MessageCards::default();
        let mut final_answer = message(1, AgentActivityKind::Prose);
        final_answer.final_answer = true;
        let turn = message(2, AgentActivityKind::Turn);
        let next = message(3, AgentActivityKind::Prose);
        let history = [final_answer, turn, next];

        cards.snapshot(2, &history);
        assert_eq!(
            cards
                .entries()
                .iter()
                .map(|entry| entry.locus.offset)
                .collect::<Vec<_>>(),
            [1, 3],
        );
        assert!(cards.entries()[0].final_answer);

        let mut live = MessageCards::default();
        live.snapshot(2, &history[..1]);
        live.merge(&history[1..]);
        assert_eq!(live.entries(), cards.entries());
    }

    #[test]
    fn live_terminal_revision_replaces_the_same_card_identity() {
        let mut cards = MessageCards::default();
        let ordinary = message(1, AgentActivityKind::Prose);
        cards.snapshot(10, std::slice::from_ref(&ordinary));

        let mut terminal = ordinary;
        terminal.final_answer = true;
        terminal.automatic_speech = Some(wire::AutomaticSpeech {
            kind: wire::AutomaticSpeechKind::Final,
            text: terminal.text.clone(),
        });
        assert!(cards.merge(std::iter::once(&terminal)));
        assert_eq!(cards.entries().len(), 1);
        assert!(cards.entries()[0].final_answer);
        assert_eq!(
            cards.entries()[0].automatic_speech,
            terminal.automatic_speech
        );
    }

    /// A compaction the browser learns about twice renders once.
    ///
    /// The captured announcement arrives live while the vendor's preserved
    /// record of the same compaction arrives with an older page, so the two
    /// reach the browser as separate cards through separate frames.
    #[test]
    fn a_preserved_compaction_replaces_the_announcement_it_duplicates() {
        let mut announced = message(1, AgentActivityKind::Compaction);
        announced.authored_synthetic = true;
        announced.boundary = Some(wire::MessageBoundary {
            id: "cmp_03f3".to_owned(),
            preserved: false,
        });
        let mut preserved = announced.clone();
        preserved.locus.member = 0x8000_0000_0000_0001;
        preserved.authored_synthetic = false;
        preserved.boundary = Some(wire::MessageBoundary {
            id: "cmp_03f3".to_owned(),
            preserved: true,
        });
        let mut unpaired = message(2, AgentActivityKind::Compaction);
        unpaired.text = announced.text.clone();
        unpaired.boundary = Some(wire::MessageBoundary {
            id: "cmp_9999".to_owned(),
            preserved: false,
        });

        let mut live = MessageCards::default();
        live.snapshot(10, &[announced.clone(), unpaired.clone()]);
        assert!(live.merge(std::iter::once(&preserved)));
        assert_eq!(
            live.entries()
                .iter()
                .map(|entry| entry.boundary.clone())
                .collect::<Vec<_>>(),
            [unpaired.boundary.clone(), preserved.boundary.clone()],
            "the announcement gives way to the preserved record, and an \
             unrelated compaction repeating its text stays",
        );

        let mut paged = MessageCards::default();
        paged.snapshot(10, &[preserved.clone(), unpaired.clone()]);
        assert!(
            !paged.merge(std::iter::once(&announced)),
            "an announcement the preserved record already covers changes nothing",
        );
    }

    #[test]
    fn semantic_classifications_replace_the_same_live_card_identity() {
        let mut cards = MessageCards::default();
        let ordinary = message(1, AgentActivityKind::Prose);
        cards.snapshot(10, std::slice::from_ref(&ordinary));

        let mut classified = ordinary;
        classified.text = "[ACK] accepted".to_owned();
        classified.classifications = vec![wire::MessageClassification {
            start: 0,
            end: 5,
            kind: wire::MessageClassificationKind::Acknowledgement,
        }];
        assert!(cards.merge(std::iter::once(&classified)));
        assert_eq!(cards.entries().len(), 1);
        assert_eq!(
            cards.entries()[0].classifications,
            classified.classifications
        );
        assert!(cards.entries()[0].automatic_speech.is_none());
    }

    /// No budget is small enough to drop what an agent just said.
    #[test]
    fn a_zero_card_budget_still_keeps_the_message_floor() {
        let mut cards = MessageCards::default();
        cards.snapshot(0, &[]);
        let prose = message(1, AgentActivityKind::Prose);

        assert!(cards.merge(std::iter::once(&prose)));
        assert!(
            !cards.merge(std::iter::once(&prose)),
            "replaying the same card changes nothing",
        );
        assert_eq!(cards.entries().len(), 1);
    }

    /// A loud peer spends its own floor, never a quiet agent's.
    #[test]
    fn a_busy_lane_does_not_evict_a_quiet_lanes_floor() {
        let quiet = "018f82d4-6f59-7d06-92e3-bce9131163e6"
            .parse::<CanonicalUuid>()
            .expect("quiet lane");
        let mut incoming = (0..5)
            .map(|offset| {
                let mut card = message(offset, AgentActivityKind::Prose);
                card.lane = quiet;
                card
            })
            .collect::<Vec<_>>();
        incoming.extend((100..300).map(|offset| message(offset, AgentActivityKind::Prose)));
        let mut cards = MessageCards::default();
        cards.snapshot(wire::MESSAGE_FLOOR, &incoming);

        assert_eq!(
            cards
                .entries()
                .iter()
                .filter(|card| card.lane == quiet)
                .count(),
            5,
            "a lane below the floor keeps everything it has said",
        );
        assert_eq!(
            cards
                .entries()
                .iter()
                .filter(|card| card.lane != quiet)
                .count(),
            wire::MESSAGE_FLOOR,
        );
    }

    /// The epoch a compaction closed is all this agent has said so far.
    #[test]
    fn a_fresh_compaction_does_not_evict_the_message_floor() {
        let mut incoming = (0..40)
            .map(|offset| message(offset, AgentActivityKind::Prose))
            .collect::<Vec<_>>();
        incoming.push(message(40, AgentActivityKind::Compaction));
        let mut cards = MessageCards::default();
        cards.snapshot(wire::MESSAGE_FLOOR + 1, &incoming);

        assert_eq!(
            cards
                .entries()
                .iter()
                .filter(|card| card.activity.is_prose())
                .count(),
            wire::MESSAGE_FLOOR,
        );
        assert_eq!(
            cards
                .entries()
                .iter()
                .filter(|card| card.activity == AgentActivityKind::Compaction)
                .count(),
            1,
            "the boundary is carried beside the floor, not instead of it",
        );
    }
}
