//! Conversation authority as room reads state it.
//!
//! Three distinct things are named apart: a publication recorded in a room,
//! the receipt one participant owes for it (an operator item, or a lane's
//! steering directive), and a question's result. Each decided state names
//! who decided it; an operator closing a question for a stored lane party is
//! marked as an override.

use std::fmt;

use serde::Serialize;
use wire::rooms::{
    QuestionResultHead, QuestionResultRecord, QuestionResultRef, QuestionResultState, RoomAnchor,
    RoomAuthorityStamp, RoomDelivery, RoomDestination, RoomPublication, RoomPublicationKind,
    RoomReceiptHead, RoomReceiptRecord, RoomReceiptRef, RoomReceiptState, RoomSource,
};
use wire::{Actor, CanonicalUuid, Instant, Seq};

use super::{Failure, Receipt, fault, store};

/// One operator item: the receipt the operator owes one publication.
#[derive(Debug, Serialize)]
pub(crate) struct ItemView {
    item: RoomReceiptRef,
    publication: RoomAuthorityStamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<RoomAnchor>,
    /// Journal position of the item's latest move.
    position: Seq,
    state: RoomReceiptState,
    /// Who decided an acked, declined, or cancelled item.
    #[serde(skip_serializing_if = "Option::is_none")]
    decided_by: Option<Actor>,
}

/// One question's result and its parties.
#[derive(Debug, Serialize)]
pub(crate) struct ResultView {
    result: QuestionResultRef,
    question: RoomAuthorityStamp,
    requester: RoomDestination,
    respondent: RoomDestination,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<RoomAnchor>,
    deadline: Instant,
    /// Journal position of the result's latest move.
    position: Seq,
    state: QuestionResultState,
    /// Who declined or cancelled the question.
    #[serde(skip_serializing_if = "Option::is_none")]
    closed_by: Option<Actor>,
    /// The operator closed it for a stored lane party.
    #[serde(rename = "override", skip_serializing_if = "std::ops::Not::not")]
    override_party: bool,
    /// The answer's return receipt to its requester.
    #[serde(skip_serializing_if = "Option::is_none")]
    returned: Option<Returned>,
}

/// The receipt an answer's return owes its requester.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum Returned {
    Directive {
        directive: CanonicalUuid,
        receipt: Receipt,
    },
    Item {
        item: Box<ItemView>,
    },
    /// The return item the answer opened is not indexed.
    Missing {
        item: RoomReceiptRef,
    },
}

/// A room publication's conversation verb, operator receipt, and result.
#[derive(Debug, Serialize)]
pub(crate) struct ConversationView {
    verb: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    item: Option<ItemView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<ResultView>,
}

impl ItemView {
    pub(super) const fn item(&self) -> &RoomReceiptRef {
        &self.item
    }
}

impl ResultView {
    pub(super) const fn question(&self) -> &RoomAuthorityStamp {
        &self.question
    }

    pub(super) const fn answer(&self) -> Option<&RoomAuthorityStamp> {
        match &self.state {
            QuestionResultState::Answered { answer, .. } => Some(answer),
            _ => None,
        }
    }
}

/// The conversation `publication` belongs to, at the current heads.
pub(super) fn conversation(
    read: &journal::JournalRead<'_>,
    publication: &RoomPublication,
) -> Result<ConversationView, Failure> {
    let (verb, item, result) = match publication.kind() {
        RoomPublicationKind::Msg {} => {
            ("msg", publication.operator_item().map(|(key, _)| key), None)
        }
        RoomPublicationKind::Ask { result, .. } => (
            "ask",
            publication.operator_item().map(|(key, _)| key),
            Some(result),
        ),
        RoomPublicationKind::Ans { result, .. } => (
            "ans",
            Some(RoomReceiptRef::new(publication.publication().clone())),
            Some(result),
        ),
    };
    let item = match item {
        Some(key) => read
            .room_item(&key)
            .map_err(store)?
            .map(|(record, head)| item_view(read, record, head))
            .transpose()?,
        None => None,
    };
    let result = match result {
        Some(result) => read
            .question_result(result)
            .map_err(store)?
            .map(|(record, head)| result_view(read, record, head))
            .transpose()?,
        None => None,
    };
    Ok(ConversationView { verb, item, result })
}

pub(super) fn item_view(
    read: &journal::JournalRead<'_>,
    record: RoomReceiptRecord,
    head: journal::JournalHead<RoomReceiptHead>,
) -> Result<ItemView, Failure> {
    let position = head.position();
    let state = head.body().state().clone();
    let decided_by = match &state {
        RoomReceiptState::Acked { source }
        | RoomReceiptState::Declined { source }
        | RoomReceiptState::Cancelled { source } => decider(read, source)?,
        RoomReceiptState::Pending { .. } | RoomReceiptState::Expired { .. } => None,
    };
    Ok(ItemView {
        item: record.receipt().clone(),
        publication: record.publication().clone(),
        context: record.context().cloned(),
        position,
        state,
        decided_by,
    })
}

pub(super) fn result_view(
    read: &journal::JournalRead<'_>,
    record: QuestionResultRecord,
    head: journal::JournalHead<QuestionResultHead>,
) -> Result<ResultView, Failure> {
    let position = head.position();
    let state = head.body().state().clone();
    let (closed_by, party) = match &state {
        QuestionResultState::Declined { source } => (decider(read, source)?, record.respondent()),
        QuestionResultState::Cancelled { source } => (decider(read, source)?, record.requester()),
        _ => (None, record.requester()),
    };
    let override_party =
        closed_by == Some(Actor::Operator) && matches!(party, RoomDestination::Lane { .. });
    let returned = match &state {
        QuestionResultState::Answered {
            return_delivery, ..
        } => Some(match return_delivery.delivery() {
            RoomDelivery::Directive { directive } => Returned::Directive {
                directive: *directive,
                receipt: super::receipt(read, *directive)?,
            },
            RoomDelivery::Item { item } => match read.room_item(item).map_err(store)? {
                Some((record, head)) => Returned::Item {
                    item: Box::new(item_view(read, record, head)?),
                },
                None => Returned::Missing { item: item.clone() },
            },
        }),
        _ => None,
    };
    Ok(ResultView {
        result: record.result().clone(),
        question: record.question().clone(),
        requester: record.requester().clone(),
        respondent: record.respondent().clone(),
        context: record.context().cloned(),
        deadline: record.deadline(),
        position,
        state,
        closed_by,
        override_party,
        returned,
    })
}

/// Who wrote the authority a decided state cites.
fn decider(read: &journal::JournalRead<'_>, source: &RoomSource) -> Result<Option<Actor>, Failure> {
    match source {
        RoomSource::Transcript(hunk) => Ok(Some(Actor::Lane(hunk.head.lane))),
        RoomSource::Journal { fact } => {
            let seq = Seq::new(fact.get()).ok_or_else(|| {
                fault(
                    wire::FaultClass::Store,
                    "room::decision-source-invalid",
                    format!("decision cites journal position {fact}"),
                )
            })?;
            Ok(read
                .room_fact(seq)
                .map_err(store)?
                .map(|envelope| envelope.actor))
        }
        RoomSource::Artifact { .. } => Ok(None),
    }
}

/// One party as a line names it.
pub(super) struct Party<'a>(pub(super) &'a RoomDestination);

impl fmt::Display for Party<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            RoomDestination::Operator {} => formatter.write_str("operator"),
            RoomDestination::Lane { lane } => write!(formatter, "lane={lane}"),
            RoomDestination::Task { task } => write!(formatter, "room={task}"),
        }
    }
}

impl fmt::Display for ItemView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "receipt operator item={} {}",
            self.item,
            self.state.name()
        )?;
        match &self.state {
            RoomReceiptState::Pending { expires } => write!(formatter, " expires={expires}")?,
            RoomReceiptState::Expired { at } => write!(formatter, " lapsed={at}")?,
            _ => {}
        }
        if let Some(actor) = &self.decided_by {
            write!(formatter, " by={actor}")?;
        }
        if let Some(context) = &self.context {
            write!(formatter, " context={context}")?;
        }
        Ok(())
    }
}

impl fmt::Display for ResultView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "result {} {} deadline={} requester={} respondent={}",
            self.result,
            self.state.name(),
            self.deadline,
            Party(&self.requester),
            Party(&self.respondent)
        )?;
        if let Some(actor) = &self.closed_by {
            write!(formatter, " by={actor}")?;
        }
        if self.override_party {
            formatter.write_str(" override")?;
        }
        if let QuestionResultState::Answered { answer, .. } = &self.state {
            write!(formatter, " answer={answer}")?;
        }
        match &self.returned {
            Some(Returned::Directive { directive, receipt }) => {
                write!(formatter, "\n  return ")?;
                write_receipt(formatter, *directive, receipt)?;
            }
            Some(Returned::Item { item }) => write!(formatter, "\n  return {item}")?,
            Some(Returned::Missing { item }) => {
                write!(formatter, "\n  return operator item={item} missing")?;
            }
            None => {}
        }
        Ok(())
    }
}

impl fmt::Display for ConversationView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "recorded {}", self.verb)?;
        if let Some(item) = &self.item {
            write!(formatter, "\n  {item}")?;
        }
        if let Some(result) = &self.result {
            write!(formatter, "\n  {result}")?;
        }
        Ok(())
    }
}

/// A lane directive's receipt as one line names it.
pub(super) fn write_receipt(
    formatter: &mut fmt::Formatter<'_>,
    directive: CanonicalUuid,
    receipt: &Receipt,
) -> fmt::Result {
    match receipt {
        Receipt::Steering {
            key,
            lane,
            disposition,
        } => write!(
            formatter,
            "receipt lane={lane} key={key} {}",
            disposition.as_str()
        ),
        Receipt::Missing => write!(formatter, "receipt directive={directive} missing"),
    }
}
