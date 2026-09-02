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
        display_millis: i64,
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
        quote: Option<String>,
        attachment: DirectiveAttachment,
    },
    RemoveAttachment {
        lane: String,
        quote: Option<String>,
        index: usize,
    },
    SetIssue {
        lane: String,
        quote: Option<String>,
        issue: String,
    },
    Clear {
        lane: String,
    },
    RestoreAll {
        lane: String,
        drafts: Vec<ComposerDraft>,
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
                display_millis,
                quote,
                preview,
            } => {
                next.0
                    .add_quote(lane, identity, order, display_millis, quote, preview);
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
            ComposerAction::AddAttachment {
                lane,
                quote,
                attachment,
            } => {
                let draft = next.0.draft_mut(lane);
                let attachment_count = draft.attachments.len()
                    + draft
                        .quotes
                        .iter()
                        .map(|quote| quote.attachments.len())
                        .sum::<usize>();
                if let Some((attachments, issue)) = attachment_state_mut(draft, quote.as_deref()) {
                    if attachment_count >= MAX_ATTACHMENTS {
                        *issue = Some("Maximum images attached.".to_owned());
                    } else {
                        attachments.push(attachment);
                        *issue = None;
                    }
                }
            }
            ComposerAction::RemoveAttachment { lane, quote, index } => {
                let draft = next.0.draft_mut(lane);
                if let Some((attachments, _)) = attachment_state_mut(draft, quote.as_deref())
                    && index < attachments.len()
                {
                    attachments.remove(index);
                }
            }
            ComposerAction::SetIssue { lane, quote, issue } => {
                if let Some((_, current_issue)) =
                    attachment_state_mut(next.0.draft_mut(lane), quote.as_deref())
                {
                    *current_issue = Some(issue);
                }
            }
            ComposerAction::Clear { lane } => {
                next.0.clear(&lane);
            }
            ComposerAction::RestoreAll { lane, drafts } => {
                next.0.restore_all(lane, drafts.into_iter());
            }
        }
        Rc::new(next)
    }
}

fn attachment_state_mut<'a>(
    draft: &'a mut ComposerDraft,
    quote_identity: Option<&str>,
) -> Option<(&'a mut Vec<DirectiveAttachment>, &'a mut Option<String>)> {
    match quote_identity {
        None => Some((&mut draft.attachments, &mut draft.issue)),
        Some(identity) => draft
            .quotes
            .iter_mut()
            .find(|quote| quote.identity == identity)
            .map(|quote| (&mut quote.attachments, &mut quote.issue)),
    }
}

impl ComposerDrafts {
    pub(super) fn get(&self, lane: &str) -> Option<&ComposerDraft> {
        self.0.get(lane)
    }

    pub(in crate::browser) fn is_unsafe(&self) -> bool {
        self.0.is_unsafe()
    }
}
