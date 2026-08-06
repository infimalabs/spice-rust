//! Target-keyed state for the plural lane composer.
//!
//! The browser owns elements and focus, but target routing, quote identity,
//! Markdown payloads, and optimistic restore are ordinary state transitions.
//! Keeping those transitions here makes the shipped plural behavior reachable
//! from native tests instead of testing a second, simplified implementation.

use std::collections::BTreeMap;

use wire::body::DirectiveAttachment;

use crate::composer_markdown::{Attachment, QuotedReply};

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
    pub(crate) body: String,
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
            body: String::new(),
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

    /// Build one request from one target without observing sibling drafts.
    pub(crate) fn prepare(&self, target: &str) -> Option<PreparedSubmission> {
        let snapshot = self.get(target)?.clone();
        let message = compose(&snapshot);
        if message.trim().is_empty() {
            return None;
        }
        Some(PreparedSubmission {
            message,
            attachments: snapshot.attachments.clone(),
            snapshot,
        })
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
}

/// Route a quote to its producing member, or to the lineage host if absent.
pub(crate) fn quote_target(members: &[String], producer: Option<&str>) -> Option<String> {
    producer
        .filter(|producer| members.iter().any(|member| member == producer))
        .map(str::to_owned)
        .or_else(|| members.first().cloned())
}

fn sort_quotes(quotes: &mut [QuoteDraft]) {
    quotes.sort_by(|left, right| {
        right
            .order
            .cmp(&left.order)
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
        })
        .collect::<Vec<_>>();
    let attachments = draft
        .attachments
        .iter()
        .map(|attachment| Attachment {
            name: &attachment.name,
            data_url: &attachment.data_url,
        })
        .collect::<Vec<_>>();
    crate::composer_markdown::compose(&draft.body, &quotes, &attachments)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(name: &str) -> DirectiveAttachment {
        DirectiveAttachment {
            name: name.to_owned(),
            content_type: "image/png".to_owned(),
            data_url: format!("data:image/png;base64,{name}"),
        }
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

    #[test]
    fn quote_routing_prefers_the_producer_and_falls_back_to_the_first_member() {
        let members = vec!["agent-a".to_owned(), "agent-b".to_owned()];
        assert_eq!(
            quote_target(&members, Some("agent-b")).as_deref(),
            Some("agent-b")
        );
        assert_eq!(
            quote_target(&members, Some("departed-agent")).as_deref(),
            Some("agent-a")
        );
        assert_eq!(quote_target(&[], Some("agent-b")), None);
    }

    #[test]
    fn quote_identity_deduplicates_without_conflating_equal_text_and_sorts_newest_first() {
        let mut store = DraftStore::default();
        assert_eq!(
            store.add_quote(
                "agent-a".to_owned(),
                "message-old".to_owned(),
                10,
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
        assert_eq!(identities, ["message-new", "message-old"]);

        store.remove_quote("agent-a".to_owned(), "message-old");
        assert_eq!(store.get("agent-a").unwrap().quotes.len(), 1);
    }

    #[test]
    fn preparation_builds_one_markdown_file_and_carries_only_target_images() {
        let mut store = DraftStore::default();
        store.set_body("agent-a".to_owned(), "body".to_owned());
        store.add_quote(
            "agent-a".to_owned(),
            "message".to_owned(),
            1,
            "quoted\nline".to_owned(),
            None,
        );
        store.set_quote_body("agent-a".to_owned(), "message", "reply".to_owned());
        store
            .draft_mut("agent-a".to_owned())
            .attachments
            .push(attachment("a"));
        store
            .draft_mut("agent-b".to_owned())
            .attachments
            .push(attachment("b"));

        let prepared = store.prepare("agent-a").unwrap();
        assert_eq!(
            prepared.message,
            "body\n\n> quoted\n> line\n\nreply\n\n![a](data:image/png;base64,a)"
        );
        assert_eq!(prepared.attachments, [attachment("a")]);
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
            "sent quote".to_owned(),
            None,
        );
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
            "newer-quote".to_owned(),
            2,
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
            ["newer-quote", "sent-quote"]
        );
        assert_eq!(store.get("agent-b").unwrap().body, "untouched");
    }
}
