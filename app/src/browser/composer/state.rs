//! Browser reducer adapting the target-neutral composer store to Yew.
//!
//! Lifetime and speech policy belong to `controls`; this module owns only draft
//! state so a second local lifetime reducer cannot bypass resident authority.

use std::rc::Rc;

use wire::body::DirectiveAttachment;
use yew::prelude::*;

use super::MAX_ATTACHMENTS;
use crate::composer_drafts::{ComposerDraft, DraftStore};

#[derive(Clone, Default, PartialEq)]
pub(in crate::browser) struct ComposerDrafts(pub(super) DraftStore);

pub(in crate::browser) enum ComposerAction {
    SetBody {
        lane: String,
        body: String,
    },
    AddQuote {
        lane: String,
        identity: String,
        order: u64,
        quote: String,
        preview: Option<String>,
    },
    RemoveQuote {
        lane: String,
        identity: String,
    },
    SetQuoteBody {
        lane: String,
        identity: String,
        body: String,
    },
    AddAttachment {
        lane: String,
        attachment: DirectiveAttachment,
    },
    RemoveAttachment {
        lane: String,
        index: usize,
    },
    SetIssue {
        lane: String,
        issue: String,
    },
    Clear {
        lane: String,
    },
    Restore {
        lane: String,
        draft: ComposerDraft,
    },
}

impl Reducible for ComposerDrafts {
    type Action = ComposerAction;

    fn reduce(self: Rc<Self>, action: Self::Action) -> Rc<Self> {
        let mut next = (*self).clone();
        match action {
            ComposerAction::SetBody { lane, body } => next.0.set_body(lane, body),
            ComposerAction::AddQuote {
                lane,
                identity,
                order,
                quote,
                preview,
            } => {
                next.0.add_quote(lane, identity, order, quote, preview);
            }
            ComposerAction::RemoveQuote { lane, identity } => {
                next.0.remove_quote(lane, &identity);
            }
            ComposerAction::SetQuoteBody {
                lane,
                identity,
                body,
            } => {
                next.0.set_quote_body(lane, &identity, body);
            }
            ComposerAction::AddAttachment { lane, attachment } => {
                let draft = next.0.draft_mut(lane);
                if draft.attachments.len() >= MAX_ATTACHMENTS {
                    draft.issue = Some("Maximum images attached.".to_owned());
                } else {
                    draft.attachments.push(attachment);
                    draft.issue = None;
                }
            }
            ComposerAction::RemoveAttachment { lane, index } => {
                let draft = next.0.draft_mut(lane);
                if index < draft.attachments.len() {
                    draft.attachments.remove(index);
                }
            }
            ComposerAction::SetIssue { lane, issue } => {
                next.0.draft_mut(lane).issue = Some(issue);
            }
            ComposerAction::Clear { lane } => {
                next.0.clear(&lane);
            }
            ComposerAction::Restore { lane, draft } => {
                next.0.restore(lane, draft);
            }
        }
        Rc::new(next)
    }
}

impl ComposerDrafts {
    pub(super) fn get(&self, lane: &str) -> Option<&ComposerDraft> {
        self.0.get(lane)
    }
}
