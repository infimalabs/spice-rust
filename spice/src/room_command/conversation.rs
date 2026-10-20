//! Current MSG conversation authority as room reads state it.
//!
//! A publication and the receipt one participant owes for it are named apart.
//! Each decided item state names who decided it.

use std::fmt;

use serde::Serialize;
use wire::principal::PrincipalDirectory;
use wire::rooms::{
    RoomAnchor, RoomAuthorityStamp, RoomDestination, RoomPublication, RoomReceiptHead,
    RoomReceiptRecord, RoomReceiptRef, RoomReceiptState, RoomSource,
};
use wire::{Actor, CanonicalUuid, Seq};

use super::{Failure, Named, Receipt, fault, store};

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
    decided_by: Option<Named>,
}

/// A room publication's operator receipt.
#[derive(Debug, Serialize)]
pub(crate) struct ConversationView {
    verb: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    item: Option<ItemView>,
}

impl ItemView {
    pub(super) const fn item(&self) -> &RoomReceiptRef {
        &self.item
    }
}

/// The conversation `publication` belongs to, at the current heads.
pub(super) fn conversation(
    read: &journal::JournalRead<'_>,
    principals: &PrincipalDirectory,
    publication: &RoomPublication,
) -> Result<ConversationView, Failure> {
    let item = publication.operator_item().map(|(key, _)| key);
    let item = match item {
        Some(key) => read
            .room_item(&key)
            .map_err(store)?
            .map(|(record, head)| item_view(read, principals, record, head))
            .transpose()?,
        None => None,
    };
    Ok(ConversationView { verb: "msg", item })
}

pub(super) fn item_view(
    read: &journal::JournalRead<'_>,
    principals: &PrincipalDirectory,
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
        decided_by: decided_by.map(|actor| Named::of(principals, actor)),
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
        RoomSource::Artifact { .. } | RoomSource::Contribution { .. } => Ok(None),
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

impl fmt::Display for ConversationView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "recorded {}", self.verb)?;
        if let Some(item) = &self.item {
            write!(formatter, "\n  {item}")?;
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
