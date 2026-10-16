//! Lane-authored conversation text the operator inbox shows. A lane's bytes
//! stay in its transcript, so the runtime reads each open operator item's
//! text once from the resident, with the operator direction the item
//! concerns and the principal it reaches; a publication's bytes never
//! change, so a settled read stands.

use wire::intent::{RoomConcernedDirection, RoomConversationContent, RoomConversationTextAnswer};
use wire::rooms::{RoomAnchor, RoomAuthorityStamp, RoomReceiptRecord};
use wire::{Actor, Outcome, Response};

use super::AppRuntime;
use super::rooms::OperatorInboxItem;
use crate::socket::Admitted;

/// One lane-authored conversation publication's text, read on demand from
/// the transcript that owns its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConversationText {
    /// A read is queued or in flight.
    Loading,
    /// What the owning source supplied at the read barrier, with the
    /// operator direction the publication's operator route concerns and the
    /// principal its operator item reaches.
    Read {
        content: RoomConversationContent,
        direction: Option<RoomConcernedDirection>,
        recipient: Option<Actor>,
    },
    /// The resident refused or failed the read, with its stable reason.
    Failed(String),
}

/// What one operator item concerns, most contextual first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemConcern {
    /// The operator direction a lane's `#KEY` named, with its text once read.
    Direction { key: String, text: Option<String> },
    /// The task a retained pre-cutover `operator#TASK` route named.
    Task(RoomAnchor),
    /// A retained pre-cutover operator route with no concern.
    Unnamed,
}

#[cfg(any(target_arch = "wasm32", test))]
/// One operator-direction route's visible destination. The resolved principal
/// outranks its concern key; retained routes lacking both name neither the
/// invalid context-free selector nor a guessed principal.
pub(crate) fn operator_destination_label(
    recipient: Option<String>,
    direction: Option<&str>,
) -> String {
    recipient.unwrap_or_else(|| {
        direction.map_or_else(|| "retained recipient".to_owned(), |key| format!("#{key}"))
    })
}

impl AppRuntime {
    /// The on-demand text of one lane-authored conversation publication.
    #[must_use]
    pub fn conversation_text(&self, publication: &RoomAuthorityStamp) -> Option<&ConversationText> {
        self.conversation_texts.get(publication)
    }

    /// What one operator item concerns: the direction its publication's
    /// operator route named, else the task its item carries, else nothing.
    #[must_use]
    pub fn item_concern(&self, record: &RoomReceiptRecord) -> ItemConcern {
        let key = self
            .rooms
            .conversation()
            .conversation(record.publication())
            .and_then(|conversation| {
                conversation
                    .publication
                    .routes()
                    .iter()
                    .find_map(|route| route.direction.clone())
            });
        match (key, record.context()) {
            (Some(key), _) => ItemConcern::Direction {
                text: match self.conversation_text(record.publication()) {
                    Some(ConversationText::Read {
                        direction: Some(direction),
                        ..
                    }) if direction.key == key => Some(direction.text.clone()),
                    _ => None,
                },
                key,
            },
            (None, Some(task)) => ItemConcern::Task(task.clone()),
            (None, None) => ItemConcern::Unnamed,
        }
    }

    /// Open operator items that reach this runtime's principal, in the
    /// catalog's admission order.
    ///
    /// An item shows once its read names this principal as its recipient,
    /// or when the read failed and so cannot place it; until its read
    /// answers it reaches no one here. This principal is matched as reads
    /// name it, so the historical operator actor is the bootstrap principal.
    #[must_use]
    pub fn operator_inbox(&self) -> Vec<OperatorInboxItem<'_>> {
        let me = self.rooms.author(self.principal.actor());
        self.rooms
            .conversation()
            .open_items()
            .into_iter()
            .filter(|entry| {
                match self
                    .conversation_texts
                    .get(entry.item.value.record.publication())
                {
                    Some(ConversationText::Read { recipient, .. }) => {
                        recipient.as_ref().is_none_or(|recipient| *recipient == me)
                    }
                    Some(ConversationText::Failed(_)) => true,
                    Some(ConversationText::Loading) | None => false,
                }
            })
            .collect()
    }

    /// Open operator items whose text and recipient this runtime has not yet
    /// asked for, each now marked loading.
    pub fn begin_operator_item_texts(&mut self) -> Vec<RoomAuthorityStamp> {
        let wanted = self
            .rooms
            .conversation()
            .open_items()
            .into_iter()
            .map(|entry| entry.item.value.record.publication())
            .filter(|publication| !self.conversation_texts.contains_key(publication))
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
                ConversationText::Read {
                    content: reply.body.content.clone(),
                    direction: reply.body.direction.clone(),
                    recipient: reply.body.recipient.clone(),
                }
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

#[cfg(test)]
mod tests {
    use super::operator_destination_label;

    #[test]
    fn operator_route_names_its_principal_then_its_concern() {
        assert_eq!(
            operator_destination_label(Some("CA".to_owned()), Some("dWDcRoute")),
            "CA"
        );
        assert_eq!(
            operator_destination_label(None, Some("dWDcRoute")),
            "#dWDcRoute"
        );
        assert_eq!(operator_destination_label(None, None), "retained recipient");
    }
}
