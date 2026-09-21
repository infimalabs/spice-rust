//! Target-keyed state for the plural lane composer.
//!
//! The browser owns elements and focus, but target routing, quote identity,
//! Markdown payloads, and optimistic restore are ordinary state transitions.
//! Keeping those transitions here makes the shipped plural behavior reachable
//! from native tests instead of testing a second, simplified implementation.

use std::collections::BTreeMap;

use wire::body::DirectiveAttachment;
use wire::{AgentMessage, CanonicalUuid};

use crate::composer_markdown::QuotedReply;
use crate::runtime::LaneMember;

/// Every draft belonging to one independently steerable target.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ComposerDraft {
    pub(crate) body: String,
    pub(crate) quotes: Vec<QuoteDraft>,
    pub(crate) attachments: Vec<DirectiveAttachment>,
    pub(crate) issue: Option<String>,
}

/// One quoted source message and the reply being composed beneath it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QuoteDraft {
    pub(crate) identity: String,
    pub(crate) quote: String,
    pub(crate) preview: Option<String>,
    pub(crate) display_millis: i64,
    pub(crate) body: String,
    pub(crate) attachments: Vec<DirectiveAttachment>,
    pub(crate) issue: Option<String>,
    order: u64,
}

/// Whether adding a quote created a band or found its existing identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QuotePlacement {
    Added,
    Existing,
}

/// The exact target-specific payload detached for one optimistic send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PreparedSubmission {
    pub(crate) message: String,
    pub(crate) attachments: Vec<DirectiveAttachment>,
    pub(crate) snapshot: ComposerDraft,
}

/// Stable composer state keyed by the lane target rather than render order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DraftStore(BTreeMap<String, ComposerDraft>);

impl DraftStore {
    pub(crate) fn get(&self, target: &str) -> Option<&ComposerDraft> {
        self.0.get(target)
    }

    pub(crate) fn draft_mut(&mut self, target: String) -> &mut ComposerDraft {
        self.0.entry(target).or_default()
    }

    pub(crate) fn set_body(&mut self, target: String, body: String) {
        self.draft_mut(target).body = body;
    }

    pub(crate) fn add_quote(
        &mut self,
        target: String,
        identity: String,
        order: u64,
        display_millis: i64,
        quote: String,
        preview: Option<String>,
    ) -> QuotePlacement {
        let draft = self.draft_mut(target);
        if draft
            .quotes
            .iter()
            .any(|candidate| candidate.identity == identity)
        {
            return QuotePlacement::Existing;
        }
        draft.quotes.push(QuoteDraft {
            identity,
            quote,
            preview,
            display_millis,
            body: String::new(),
            attachments: Vec::new(),
            issue: None,
            order,
        });
        sort_quotes(&mut draft.quotes);
        QuotePlacement::Added
    }

    pub(crate) fn remove_quote(&mut self, target: String, identity: &str) {
        self.draft_mut(target)
            .quotes
            .retain(|quote| quote.identity != identity);
    }

    pub(crate) fn set_quote_body(&mut self, target: String, identity: &str, body: String) {
        if let Some(quote) = self
            .draft_mut(target)
            .quotes
            .iter_mut()
            .find(|quote| quote.identity == identity)
        {
            quote.body = body;
        }
    }

    pub(crate) fn clear(&mut self, target: &str) {
        self.0.remove(target);
    }

    pub(crate) fn is_unsafe(&self) -> bool {
        self.0.values().any(|draft| {
            !draft.body.is_empty() || !draft.quotes.is_empty() || !draft.attachments.is_empty()
        })
    }

    /// Build one request from one target without observing sibling drafts.
    pub(crate) fn prepare(&self, target: &str) -> Option<PreparedSubmission> {
        let snapshot = self.get(target)?.clone();
        let message = compose(&snapshot);
        if message.trim().is_empty() {
            return None;
        }
        Some(PreparedSubmission {
            message,
            attachments: snapshot
                .quotes
                .iter()
                .flat_map(|quote| quote.attachments.iter())
                .chain(snapshot.attachments.iter())
                .cloned()
                .collect(),
            snapshot,
        })
    }

    /// Detach only the primary body and its direct attachments for broadcast.
    pub(crate) fn prepare_primary(&self, target: &str) -> Option<PreparedSubmission> {
        let draft = self.get(target)?;
        if draft.body.trim().is_empty() {
            return None;
        }
        let snapshot = ComposerDraft {
            body: draft.body.clone(),
            quotes: Vec::new(),
            attachments: draft.attachments.clone(),
            issue: draft.issue.clone(),
        };
        Some(PreparedSubmission {
            message: compose(&snapshot),
            attachments: snapshot.attachments.clone(),
            snapshot,
        })
    }

    /// Clear the primary broadcast source without disturbing its quote shards.
    pub(crate) fn clear_primary(&mut self, target: &str) {
        let remove = self.0.get_mut(target).is_some_and(|draft| {
            draft.body.clear();
            draft.attachments.clear();
            draft.issue = None;
            draft.quotes.is_empty()
        });
        if remove {
            self.0.remove(target);
        }
    }

    /// Restore a refused snapshot ahead of anything typed since its send.
    pub(crate) fn restore(&mut self, target: String, snapshot: ComposerDraft) {
        let current = self.0.remove(&target).unwrap_or_default();
        let body = [snapshot.body, current.body]
            .into_iter()
            .filter(|body| !body.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        let mut attachments = snapshot.attachments;
        attachments.extend(current.attachments);
        let mut quotes = snapshot.quotes;
        for current_quote in current.quotes {
            if let Some(restored) = quotes
                .iter_mut()
                .find(|quote| quote.identity == current_quote.identity)
            {
                restored.body = [restored.body.as_str(), current_quote.body.as_str()]
                    .into_iter()
                    .filter(|body| !body.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                restored.order = restored.order.max(current_quote.order);
                restored.quote = current_quote.quote;
                restored.preview = current_quote.preview;
                restored.display_millis = current_quote.display_millis;
                restored.attachments.extend(current_quote.attachments);
                restored.issue = current_quote.issue.or(restored.issue.take());
            } else {
                quotes.push(current_quote);
            }
        }
        sort_quotes(&mut quotes);
        self.0.insert(
            target,
            ComposerDraft {
                body,
                quotes,
                attachments,
                issue: current.issue.or(snapshot.issue),
            },
        );
    }

    /// Restore authored submissions ahead of anything typed since they left.
    pub(crate) fn restore_all(
        &mut self,
        target: String,
        snapshots: impl DoubleEndedIterator<Item = ComposerDraft>,
    ) {
        for snapshot in snapshots.rev() {
            self.restore(target.clone(), snapshot);
        }
    }
}

/// The lane a quote taken from `message` composes into.
///
/// A quote answers the agent that wrote it, so a seated producer wins even when
/// it sits in a different member than the card: an agent that moved seats still
/// receives replies to prose it wrote from the old one.
///
/// Prose outlives the seat that produced it, so the author is not always seated.
/// That case resolves to the lane the card is rendered in rather than to a
/// member drawn out of the team: quoting a departed agent composes into the
/// lane the operator is already looking at, and no unrelated agent is handed
/// direction it never earned. One occupant seated in two members at once is not
/// a state the resident reports; were it ever reported the answer stays the
/// card's own lane, so this decision reads the same under every ordering of
/// `members` and nothing in it depends on which member sorts first.
pub(crate) fn quote_lane(members: &[LaneMember], message: &AgentMessage) -> CanonicalUuid {
    let mut seated = members
        .iter()
        .filter(|member| member.occupant == Some(message.author));
    match (seated.next(), seated.next()) {
        (Some(producer), None) => producer.lane,
        _ => message.lane,
    }
}

fn sort_quotes(quotes: &mut [QuoteDraft]) {
    quotes.sort_by(|left, right| {
        left.order
            .cmp(&right.order)
            .then_with(|| left.identity.cmp(&right.identity))
    });
}

pub(crate) fn compose(draft: &ComposerDraft) -> String {
    let quotes = draft
        .quotes
        .iter()
        .map(|quote| QuotedReply {
            quote: &quote.quote,
            body: &quote.body,
            attachments: &quote.attachments,
        })
        .collect::<Vec<_>>();
    crate::composer_markdown::compose(&draft.body, &quotes, &draft.attachments)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &str = "iVBORw0KGgo=";

    fn attachment(name: &str) -> DirectiveAttachment {
        DirectiveAttachment::from_base64(name, "image/png", PNG).expect("fixture attachment admits")
    }

    #[test]
    fn target_keys_preserve_one_draft_per_member_across_reordering() {
        let mut store = DraftStore::default();
        store.set_body("agent-a".to_owned(), "draft a".to_owned());
        store.set_body("agent-b".to_owned(), "draft b".to_owned());

        let rerendered = ["agent-b", "agent-a"];
        assert_eq!(store.get(rerendered[0]).unwrap().body, "draft b");
        assert_eq!(store.get(rerendered[1]).unwrap().body, "draft a");
        assert_eq!(store.0.len(), 2);
    }

    const LANE_A: &str = "11111111-1111-4111-8111-111111111111";
    const LANE_B: &str = "22222222-2222-4222-8222-222222222222";
    const SEATED_A: &str = "33333333-3333-4333-8333-333333333333";
    const SEATED_B: &str = "44444444-4444-4444-8444-444444444444";
    const DEPARTED: &str = "55555555-5555-4555-8555-555555555555";

    fn uuid(text: &str) -> CanonicalUuid {
        text.parse().expect("the fixture spells a canonical uuid")
    }

    fn seat(lane: &str, occupant: Option<&str>) -> LaneMember {
        LaneMember {
            lane: uuid(lane),
            name: None,
            worktree: format!("worktree-{lane}"),
            life: "steer".to_owned(),
            vendor: "claude".parse().expect("the fixture spells a vendor name"),
            model: Some("sonnet".to_owned()),
            effort: Some("high".to_owned()),
            occupant: occupant.map(uuid),
            session: Some("session-fixture".to_owned()),
            disposition: "seated".to_owned(),
            liveness: wire::intent::LaneLiveness::Working,
            bound: true,
            jobs: Vec::new(),
        }
    }

    fn message(lane: &str, author: &str) -> AgentMessage {
        AgentMessage {
            lane: uuid(lane),
            author: uuid(author),
            locus: wire::stream::MessageLocus {
                member: 1,
                offset: 0,
                length: 8,
                event: 0,
            },
            authored: "2030-04-12T18:44:09.271Z".parse().expect("fixture instant"),
            authored_synthetic: false,
            observed: None,
            activity: wire::AgentActivityKind::Prose,
            final_answer: false,
            text: "quoted prose".to_owned(),
            badges: Default::default(),
            automatic_speech: None,
            unavailable_tool_terminals: Vec::new(),
            boundary: None,
            media: Vec::new(),
        }
    }

    /// A seated author receives the quote even from another member's card.
    #[test]
    fn a_seated_author_receives_the_quote_from_whichever_card_carries_it() {
        let members = vec![seat(LANE_A, Some(SEATED_A)), seat(LANE_B, Some(SEATED_B))];
        // The card sits in lane A while the prose was written by the occupant
        // that now sits in lane B, which is the seat-change case the preference
        // exists for: the reply follows the agent, not the tile.
        assert_eq!(
            quote_lane(&members, &message(LANE_A, SEATED_B)),
            uuid(LANE_B),
            "a seated author elsewhere in the team still receives its replies"
        );
        assert_eq!(
            quote_lane(&members, &message(LANE_B, SEATED_B)),
            uuid(LANE_B),
            "the ordinary case targets the author's own seat"
        );
    }

    /// An unseated author's quote stays in the lane whose card carries it.
    ///
    /// The first member is deliberately vacant and the card is deliberately the
    /// second one: with a single member, or with the card in the first member,
    /// every candidate rule returns the same lane and the fixture would pass
    /// against the row-order pick this replaced. Both orderings are asserted
    /// because a rule that reads member order can only be caught by permuting
    /// the members it reads.
    #[test]
    fn an_unseated_author_composes_into_the_card_lane_whatever_the_member_order() {
        let vacant_first = vec![seat(LANE_A, None), seat(LANE_B, Some(SEATED_B))];
        let vacant_last = vec![seat(LANE_B, Some(SEATED_B)), seat(LANE_A, None)];
        for members in [vacant_first, vacant_last] {
            assert_eq!(
                quote_lane(&members, &message(LANE_B, DEPARTED)),
                uuid(LANE_B),
                "a departed author's quote belongs to the lane being looked at"
            );
            assert_eq!(
                quote_lane(&members, &message(LANE_A, DEPARTED)),
                uuid(LANE_A),
                "including when that lane is the vacant one"
            );
        }
    }

    /// One occupant claimed by two seats resolves without reading their order.
    #[test]
    fn an_ambiguous_author_resolves_to_the_card_lane_rather_than_a_seat() {
        let duplicated = vec![seat(LANE_A, Some(SEATED_B)), seat(LANE_B, Some(SEATED_B))];
        let reversed = vec![seat(LANE_B, Some(SEATED_B)), seat(LANE_A, Some(SEATED_B))];
        for members in [duplicated, reversed] {
            assert_eq!(
                quote_lane(&members, &message(LANE_A, SEATED_B)),
                uuid(LANE_A),
                "an unrepresentable team still answers the card's own lane"
            );
        }
    }

    #[test]
    fn quote_identity_deduplicates_without_conflating_equal_text_and_sorts_oldest_first() {
        let mut store = DraftStore::default();
        assert_eq!(
            store.add_quote(
                "agent-a".to_owned(),
                "message-old".to_owned(),
                10,
                1_000,
                "same text".to_owned(),
                None,
            ),
            QuotePlacement::Added
        );
        assert_eq!(
            store.add_quote(
                "agent-a".to_owned(),
                "message-new".to_owned(),
                30,
                3_000,
                "same text".to_owned(),
                None,
            ),
            QuotePlacement::Added
        );
        assert_eq!(
            store.add_quote(
                "agent-a".to_owned(),
                "message-old".to_owned(),
                10,
                1_000,
                "changed copy is ignored for an existing identity".to_owned(),
                None,
            ),
            QuotePlacement::Existing
        );

        let identities = store
            .get("agent-a")
            .unwrap()
            .quotes
            .iter()
            .map(|quote| quote.identity.as_str())
            .collect::<Vec<_>>();
        assert_eq!(identities, ["message-old", "message-new"]);

        store.remove_quote("agent-a".to_owned(), "message-old");
        assert_eq!(store.get("agent-a").unwrap().quotes.len(), 1);
    }

    #[test]
    fn preparation_keeps_images_with_their_text_blocks_and_target() {
        let mut store = DraftStore::default();
        let quote_attachment = attachment("quote.png");
        let primary_attachment = attachment(r"a](one)\雪.png");
        store.set_body("agent-a".to_owned(), "body".to_owned());
        store.add_quote(
            "agent-a".to_owned(),
            "message".to_owned(),
            1,
            1_000,
            "quoted\nline".to_owned(),
            None,
        );
        store.set_quote_body("agent-a".to_owned(), "message", "reply".to_owned());
        store
            .draft_mut("agent-a".to_owned())
            .quotes
            .first_mut()
            .expect("the quote exists")
            .attachments
            .push(quote_attachment.clone());
        store
            .draft_mut("agent-a".to_owned())
            .attachments
            .push(primary_attachment.clone());
        store
            .draft_mut("agent-b".to_owned())
            .attachments
            .push(attachment("b"));

        let prepared = store.prepare("agent-a").unwrap();
        assert!(
            prepared
                .attachments
                .iter()
                .all(|media| !prepared.message.contains(&media.data_url)),
            "directive prose must not duplicate its attachment payloads",
        );
        assert_eq!(
            prepared.message,
            format!(
                "> quoted\n> line\n\nreply\n\n{}\n\nbody\n\n{}",
                quote_attachment
                    .reference_markdown()
                    .expect("fixture Markdown renders"),
                primary_attachment
                    .reference_markdown()
                    .expect("fixture Markdown renders"),
            ),
        );
        assert_eq!(prepared.attachments, [quote_attachment, primary_attachment]);
    }

    #[test]
    fn optimistic_clear_and_refusal_restore_touch_only_the_sent_target() {
        let mut store = DraftStore::default();
        store.set_body("agent-a".to_owned(), "sent".to_owned());
        store.set_body("agent-b".to_owned(), "untouched".to_owned());
        store
            .draft_mut("agent-a".to_owned())
            .attachments
            .push(attachment("sent"));
        store.add_quote(
            "agent-a".to_owned(),
            "sent-quote".to_owned(),
            1,
            1_000,
            "sent quote".to_owned(),
            None,
        );
        store.set_quote_body("agent-a".to_owned(), "sent-quote", "sent reply".to_owned());
        store
            .draft_mut("agent-a".to_owned())
            .quotes
            .first_mut()
            .expect("the sent quote exists")
            .attachments
            .push(attachment("quote-sent"));
        let snapshot = store.prepare("agent-a").unwrap().snapshot;

        store.clear("agent-a");
        assert!(store.get("agent-a").is_none());
        assert_eq!(store.get("agent-b").unwrap().body, "untouched");

        store.set_body("agent-a".to_owned(), "newer".to_owned());
        store
            .draft_mut("agent-a".to_owned())
            .attachments
            .push(attachment("newer"));
        store.add_quote(
            "agent-a".to_owned(),
            "sent-quote".to_owned(),
            2,
            2_000,
            "sent quote".to_owned(),
            None,
        );
        store.set_quote_body(
            "agent-a".to_owned(),
            "sent-quote",
            "typed after send".to_owned(),
        );
        store
            .draft_mut("agent-a".to_owned())
            .quotes
            .first_mut()
            .expect("the current quote exists")
            .attachments
            .push(attachment("quote-newer"));
        store.add_quote(
            "agent-a".to_owned(),
            "newer-quote".to_owned(),
            3,
            3_000,
            "newer quote".to_owned(),
            None,
        );
        store.restore("agent-a".to_owned(), snapshot);

        let restored = store.get("agent-a").unwrap();
        assert_eq!(restored.body, "sent\n\nnewer");
        assert_eq!(
            restored
                .attachments
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            ["sent", "newer"]
        );
        assert_eq!(
            restored
                .quotes
                .iter()
                .map(|quote| quote.identity.as_str())
                .collect::<Vec<_>>(),
            ["sent-quote", "newer-quote"]
        );
        let restored_quote = &restored.quotes[0];
        assert_eq!(restored_quote.body, "sent reply\n\ntyped after send");
        assert_eq!(
            restored_quote
                .attachments
                .iter()
                .map(|attachment| attachment.name.as_str())
                .collect::<Vec<_>>(),
            ["quote-sent", "quote-newer"]
        );
        assert_eq!(store.get("agent-b").unwrap().body, "untouched");
    }

    #[test]
    fn broadcast_preparation_detaches_only_primary_content_and_restores_it_once() {
        let mut store = DraftStore::default();
        store.set_body("agent-a".to_owned(), "broadcast body".to_owned());
        store
            .draft_mut("agent-a".to_owned())
            .attachments
            .push(attachment("primary"));
        store.add_quote(
            "agent-a".to_owned(),
            "quote".to_owned(),
            1,
            1_000,
            "quoted source".to_owned(),
            None,
        );
        store.set_quote_body("agent-a".to_owned(), "quote", "local reply".to_owned());
        store
            .draft_mut("agent-a".to_owned())
            .quotes
            .first_mut()
            .expect("the quote exists")
            .attachments
            .push(attachment("quoted"));

        let prepared = store
            .prepare_primary("agent-a")
            .expect("primary body prepares");
        assert!(prepared.message.starts_with("broadcast body"));
        assert!(prepared.message.contains("![primary]"));
        assert!(!prepared.message.contains("quoted source"));
        assert!(!prepared.message.contains("local reply"));
        assert_eq!(prepared.attachments.len(), 1);

        store.clear_primary("agent-a");
        let remaining = store.get("agent-a").expect("quote draft remains");
        assert!(remaining.body.is_empty());
        assert!(remaining.attachments.is_empty());
        assert_eq!(remaining.quotes.len(), 1);

        store.restore("agent-a".to_owned(), prepared.snapshot);
        let restored = store.get("agent-a").expect("source snapshot restores");
        assert_eq!(restored.body, "broadcast body");
        assert_eq!(restored.attachments.len(), 1);
        assert_eq!(restored.quotes.len(), 1);
        assert_eq!(restored.quotes[0].body, "local reply");
    }

    #[test]
    fn unsafe_state_and_batch_restore_share_the_complete_draft_shape() {
        let mut store = DraftStore::default();
        assert!(!store.is_unsafe());
        store.set_body("agent-a".to_owned(), "typed after send".to_owned());
        assert!(store.is_unsafe());

        store.restore_all(
            "agent-a".to_owned(),
            vec![
                ComposerDraft {
                    body: "first sent".to_owned(),
                    attachments: vec![attachment("first")],
                    ..ComposerDraft::default()
                },
                ComposerDraft {
                    body: "second queued".to_owned(),
                    attachments: vec![attachment("second")],
                    ..ComposerDraft::default()
                },
            ]
            .into_iter(),
        );
        let restored = store.get("agent-a").unwrap();
        assert_eq!(
            restored.body,
            "first sent\n\nsecond queued\n\ntyped after send"
        );
        assert_eq!(
            restored
                .attachments
                .iter()
                .map(|attachment| attachment.name.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );

        store.clear("agent-a");
        store.add_quote(
            "agent-a".to_owned(),
            "source".to_owned(),
            1,
            1_000,
            "quoted".to_owned(),
            None,
        );
        assert!(
            store.is_unsafe(),
            "an untouched quote is still authored intent"
        );
    }
}
