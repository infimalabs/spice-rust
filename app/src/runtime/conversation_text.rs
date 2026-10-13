//! Lane-authored conversation text the operator inbox shows. A lane's bytes
//! stay in its transcript, so the runtime reads each open operator item's
//! text once from the resident; a publication's bytes never change, so a
//! settled read stands.

use wire::intent::{RoomConversationContent, RoomConversationTextAnswer};
use wire::rooms::RoomAuthorityStamp;
use wire::{Outcome, Response};

use super::AppRuntime;
use crate::socket::Admitted;

/// One lane-authored conversation publication's text, read on demand from
/// the transcript that owns its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConversationText {
    /// A read is queued or in flight.
    Loading,
    /// What the owning source supplied at the read barrier.
    Read(RoomConversationContent),
    /// The resident refused or failed the read, with its stable reason.
    Failed(String),
}

impl AppRuntime {
    /// The on-demand text of one lane-authored conversation publication.
    #[must_use]
    pub fn conversation_text(&self, publication: &RoomAuthorityStamp) -> Option<&ConversationText> {
        self.conversation_texts.get(publication)
    }

    /// Open operator items whose lane-authored text this runtime has not yet
    /// asked for, each now marked loading. Operator-authored text arrives
    /// with its fact and needs no read.
    pub fn begin_operator_item_texts(&mut self) -> Vec<RoomAuthorityStamp> {
        let projection = self.rooms.conversation();
        let wanted = projection
            .inbox_items()
            .into_iter()
            .map(|entry| entry.item.value.record.publication())
            .filter(|publication| {
                !self.conversation_texts.contains_key(publication)
                    && projection
                        .conversation(publication)
                        .is_some_and(|conversation| conversation.text.is_none())
            })
            .cloned()
            .collect::<Vec<_>>();
        for publication in &wanted {
            self.conversation_texts
                .insert(publication.clone(), ConversationText::Loading);
        }
        wanted
    }

    /// Apply one correlated conversation text read without crossing
    /// capability scope or publication identity.
    pub fn apply_conversation_text(
        &mut self,
        publication: &RoomAuthorityStamp,
        admitted: &Admitted<Response<RoomConversationTextAnswer>>,
    ) {
        if admitted.scope() != self.accepted.scope() {
            return;
        }
        let text = match &admitted.get().outcome {
            Outcome::Reply(reply) if reply.body.publication == *publication => {
                ConversationText::Read(reply.body.content.clone())
            }
            Outcome::Reply(_) => {
                ConversationText::Failed("room::conversation-text-identity-mismatch".to_owned())
            }
            Outcome::Refusal(refusal) => ConversationText::Failed(
                refusal
                    .diagnostic()
                    .map_or(
                        "room::conversation-text-refused",
                        wire::RefusalDiagnostic::reason,
                    )
                    .to_owned(),
            ),
            Outcome::Fault(fault) => ConversationText::Failed(fault.reason.clone()),
        };
        self.conversation_texts.insert(publication.clone(), text);
    }

    /// A read in flight at a dropped connection never answers; ask again.
    pub(super) fn forget_conversation_text_reads(&mut self) {
        self.conversation_texts
            .retain(|_, text| *text != ConversationText::Loading);
    }
}
