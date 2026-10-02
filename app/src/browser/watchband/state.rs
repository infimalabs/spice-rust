//! Browser reducer adapting the target-neutral composer store to Yew.
//!
//! Lifetime and speech policy belong to `controls`; this module owns only draft
//! state so a second local lifetime reducer cannot bypass resident authority.

use std::rc::Rc;

use wire::body::DirectiveAttachment;
use yew::prelude::*;

use super::MAX_ATTACHMENTS;
use crate::composer_drafts::{ComposerDraft, DraftImage, DraftStore};
use crate::image_annotation::Shape;

#[derive(Clone, Default, PartialEq)]
pub(in crate::browser) struct ComposerDrafts(pub(super) DraftStore, Option<ComposerIssue>);

/// The latest composer refusal. Its serial distinguishes a repeat of the same
/// text, so every refusal is announced again.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ComposerIssue {
    serial: u64,
    text: String,
}

pub(in crate::browser) enum ComposerAction {
    AddQuote {
        lane: String,
        target: wire::QuoteTarget,
        source_id: String,
        order: u64,
        display_millis: i64,
        preview: String,
    },
    RemoveQuote {
        lane: String,
        identity: String,
    },
    AddAttachment {
        lane: String,
        quote: Option<String>,
        attachment: DirectiveAttachment,
    },
    RemoveAttachment {
        lane: String,
        quote: Option<String>,
        id: u64,
    },
    ApplyImage {
        lane: String,
        quote: Option<String>,
        id: u64,
        revision: u64,
        media: DirectiveAttachment,
        shapes: Rc<Vec<Shape>>,
    },
    /// Announce a refusal of composer input through the shared toast.
    Report {
        issue: String,
    },
    Clear {
        lane: String,
    },
    ClearPrimary {
        lane: String,
    },
    RestoreAll {
        lane: String,
        drafts: Vec<ComposerDraft>,
        current: ComposerDraft,
    },
    /// Text typed into a composer element that is being discarded.
    KeepDetached {
        lane: String,
        quote: Option<String>,
        drawn: String,
        live: String,
    },
}

impl Reducible for ComposerDrafts {
    type Action = ComposerAction;

    fn reduce(self: Rc<Self>, action: Self::Action) -> Rc<Self> {
        let mut next = (*self).clone();
        match action {
            ComposerAction::AddQuote {
                lane,
                target,
                source_id,
                order,
                display_millis,
                preview,
            } => {
                next.0
                    .add_quote(lane, target, source_id, order, display_millis, preview);
            }
            ComposerAction::RemoveQuote { lane, identity } => {
                next.0.remove_quote(lane, &identity);
            }
            ComposerAction::AddAttachment {
                lane,
                quote,
                attachment,
            } => {
                let id = next.0.issue_image();
                let draft = next.0.draft_mut(lane);
                let attachment_count = draft.attachments.len()
                    + draft
                        .quotes
                        .iter()
                        .map(|quote| quote.attachments.len())
                        .sum::<usize>();
                if attachment_count >= MAX_ATTACHMENTS {
                    next.report("Maximum images attached.".to_owned());
                } else if let Some(attachments) = attachment_state_mut(draft, quote.as_deref()) {
                    attachments.push(DraftImage::new(id, attachment));
                }
            }
            ComposerAction::RemoveAttachment { lane, quote, id } => {
                let draft = next.0.draft_mut(lane);
                if let Some(attachments) = attachment_state_mut(draft, quote.as_deref()) {
                    attachments.retain(|image| image.id != id);
                }
            }
            ComposerAction::ApplyImage {
                lane,
                quote,
                id,
                revision,
                media,
                shapes,
            } => {
                if !next
                    .0
                    .apply_image(&lane, quote.as_deref(), id, revision, media, shapes)
                {
                    return self;
                }
            }
            ComposerAction::Report { issue } => next.report(issue),
            ComposerAction::Clear { lane } => {
                next.0.clear(&lane);
            }
            ComposerAction::ClearPrimary { lane } => {
                next.0.clear_primary(&lane);
            }
            ComposerAction::RestoreAll {
                lane,
                drafts,
                current,
            } => {
                *next.0.draft_mut(lane.clone()) = current;
                next.0.restore_all(lane, drafts.into_iter());
            }
            ComposerAction::KeepDetached {
                lane,
                quote,
                drawn,
                live,
            } => next.0.keep_detached(lane, quote.as_deref(), &drawn, live),
        }
        Rc::new(next)
    }
}

fn attachment_state_mut<'a>(
    draft: &'a mut ComposerDraft,
    quote_identity: Option<&str>,
) -> Option<&'a mut Vec<DraftImage>> {
    match quote_identity {
        None => Some(&mut draft.attachments),
        Some(identity) => draft
            .quotes
            .iter_mut()
            .find(|quote| quote.identity == identity)
            .map(|quote| &mut quote.attachments),
    }
}

impl ComposerDrafts {
    fn report(&mut self, text: String) {
        let serial = self
            .1
            .as_ref()
            .map_or(0, |issue| issue.serial.wrapping_add(1));
        self.1 = Some(ComposerIssue { serial, text });
    }

    fn issue(&self) -> Option<&ComposerIssue> {
        self.1.as_ref()
    }

    pub(in crate::browser) fn image(
        &self,
        lane: &str,
        quote: Option<&str>,
        id: u64,
    ) -> Option<&DraftImage> {
        self.0.image(lane, quote, id)
    }
    pub(super) fn get(&self, lane: &str) -> Option<&ComposerDraft> {
        self.0.get(lane)
    }

    pub(in crate::browser) fn is_unsafe(&self) -> bool {
        self.0.is_unsafe()
    }
}

/// Announce each composer refusal through the root toast, so the refusal
/// fades on its own instead of shifting the composer it concerns.
#[hook]
pub(in crate::browser) fn use_composer_issue_toast(drafts: &UseReducerHandle<ComposerDrafts>) {
    let toaster = crate::browser::toast::use_toaster();
    use_effect_with(drafts.issue().cloned(), move |issue| {
        if let Some(issue) = issue {
            toaster.settled(issue.text.clone());
        }
    });
}
