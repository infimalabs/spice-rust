//! Room conversation authority: each substantive MSG and the receipt each
//! destination owes it.
//!
//! A room snapshot carries only the facts behind current heads, so every move
//! sets the state its fact names instead of replaying a guarded transition.

use std::collections::{BTreeMap, BTreeSet};

use wire::body::{
    SteerAcked, SteerCanceled, SteerDeclined, SteerLapsed, SteerMessaged, SteerQueued,
    SteerRecovered, SteerSent,
};
use wire::rooms::{
    RoomAnchor, RoomAuthorityStamp, RoomDisposed, RoomPublication, RoomReceiptControl,
    RoomReceiptLapsed, RoomReceiptRecord, RoomReceiptRecovered, RoomReceiptRef, RoomReceiptState,
};
use wire::steering::SteerDeadLettered;
use wire::{CanonicalUuid, Disposition, FactKind, Instant, Seq, SequenceCode, StreamFact};

use super::RoomPositioned;

/// One admitted publication with the directives its write opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conversation {
    /// Journal position of the admitting fact.
    pub position: Seq,
    /// Author, verb, source, and frozen routes.
    pub publication: RoomPublication,
    /// Directives delivering its lane routes, or an answer's lane return.
    pub directed: Vec<CanonicalUuid>,
    /// Operator-composed text; a lane's bytes stay in its transcript.
    pub text: Option<String>,
}

/// The receipt the operator owes one publication.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperatorItem {
    /// Immutable journal position that admitted this item.
    pub admitted: Seq,
    /// Immutable publication, context, and source of the item.
    pub record: RoomReceiptRecord,
    /// Current disposition.
    pub state: RoomReceiptState,
}

/// One catalog entry while its operator receipt or answer duty remains open.
#[derive(Clone, Copy, Debug)]
pub struct OperatorInboxItem<'a> {
    /// Stable operator-item identity.
    pub key: &'a RoomReceiptRef,
    /// Immutable record and current receipt state.
    pub item: &'a RoomPositioned<OperatorItem>,
}

/// One lane delivery's native steering standing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delivery {
    /// Steering key the recipient acknowledges.
    pub key: String,
    /// Logical lane that owes the receipt.
    pub lane: CanonicalUuid,
    /// Queued for a vacant lane, unheard by any seat.
    pub queued: bool,
    /// Current steering disposition.
    pub disposition: Disposition,
    /// Text the delivery carried.
    pub text: String,
}

/// Capability-local conversation heads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConversationProjection {
    publications: BTreeMap<RoomAuthorityStamp, Conversation>,
    items: BTreeMap<RoomReceiptRef, RoomPositioned<OperatorItem>>,
    deliveries: BTreeMap<CanonicalUuid, RoomPositioned<Delivery>>,
    keys: BTreeMap<String, CanonicalUuid>,
    /// Directives some room record or conversation names; a directive opens
    /// in the write that names it, so an unnamed one never joins later.
    named: BTreeSet<CanonicalUuid>,
}

impl ConversationProjection {
    /// One admitted publication.
    pub fn conversation(&self, publication: &RoomAuthorityStamp) -> Option<&Conversation> {
        self.publications.get(publication)
    }

    /// One operator item.
    pub fn item(&self, item: &RoomReceiptRef) -> Option<&RoomPositioned<OperatorItem>> {
        self.items.get(item)
    }

    /// Operator items whose receipt remains open, in immutable admission order.
    /// Receipt head moves never reorder this catalog.
    pub fn open_items(&self) -> Vec<OperatorInboxItem<'_>> {
        let mut open = self
            .items
            .iter()
            .filter_map(|(key, item)| {
                (!item.value.state.is_terminal()).then_some(OperatorInboxItem { key, item })
            })
            .collect::<Vec<_>>();
        open.sort_by(|left, right| {
            left.item
                .value
                .admitted
                .cmp(&right.item.value.admitted)
                .then_with(|| left.key.cmp(right.key))
        });
        open
    }

    /// One named lane delivery.
    pub fn delivery(&self, directive: &CanonicalUuid) -> Option<&RoomPositioned<Delivery>> {
        self.deliveries.get(directive)
    }

    /// Mark directives a room record names as deliveries to keep.
    pub(super) fn name(&mut self, directed: &[CanonicalUuid]) {
        self.named.extend(directed.iter().copied());
    }

    /// Admit one publication and the operator item it opens, if any.
    pub(super) fn admit(
        &mut self,
        position: Seq,
        _at: Instant,
        publication: RoomPublication,
        directed: Vec<CanonicalUuid>,
        expires: Instant,
        text: Option<String>,
    ) -> bool {
        let stamp = publication.publication().clone();
        if self.publications.contains_key(&stamp) {
            return false;
        }
        self.name(&directed);
        if let Some((item, context)) = publication.operator_item() {
            let context = context.cloned();
            self.open(position, item, &publication, context, expires);
        }
        self.publications.insert(
            stamp,
            Conversation {
                position,
                publication,
                directed,
                text,
            },
        );
        true
    }

    fn open(
        &mut self,
        position: Seq,
        item: RoomReceiptRef,
        publication: &RoomPublication,
        context: Option<RoomAnchor>,
        expires: Instant,
    ) {
        let record = RoomReceiptRecord::new(
            item.clone(),
            publication.publication().clone(),
            context,
            publication.source().clone(),
        );
        self.items.insert(
            item,
            RoomPositioned {
                position,
                value: OperatorItem {
                    admitted: position,
                    record,
                    state: RoomReceiptState::Pending { expires },
                },
            },
        );
    }

    /// Fold one operator-item move, or `None` when `fact` is unrelated.
    pub(super) fn apply(&mut self, fact: &StreamFact) -> Option<bool> {
        let position = fact.seq();
        let code = SequenceCode::new(position.get());
        Some(match fact.kind() {
            FactKind::RoomDisposed => fact
                .read::<RoomDisposed>()
                .and_then(|body| body.control(code).ok())
                .is_some_and(|control| self.disposed(position, &control)),
            FactKind::RoomReceiptLapsed => fact.read::<RoomReceiptLapsed>().is_some_and(|body| {
                self.settle_item(
                    position,
                    &body.receipt,
                    RoomReceiptState::Expired { at: *fact.at() },
                )
            }),
            FactKind::RoomReceiptRecovered => {
                fact.read::<RoomReceiptRecovered>().is_some_and(|body| {
                    self.move_item(
                        position,
                        &body.receipt,
                        RoomReceiptState::Pending {
                            expires: body.expires,
                        },
                    )
                })
            }
            _ => return None,
        })
    }

    /// Settle every named pending item.
    fn disposed(&mut self, position: Seq, control: &RoomReceiptControl) -> bool {
        let mut changed = false;
        for key in control.keys() {
            changed |= self.settle_item(position, key, control.receipt_state());
        }
        changed
    }

    /// A decision moves only a pending item.
    fn settle_item(
        &mut self,
        position: Seq,
        item: &RoomReceiptRef,
        state: RoomReceiptState,
    ) -> bool {
        let pending = self
            .items
            .get(item)
            .is_some_and(|item| matches!(item.value.state, RoomReceiptState::Pending { .. }));
        pending && self.move_item(position, item, state)
    }

    fn move_item(&mut self, position: Seq, item: &RoomReceiptRef, state: RoomReceiptState) -> bool {
        self.items.get_mut(item).is_some_and(|item| {
            let changed = item.value.state != state;
            item.value.state = state;
            item.position = position;
            changed
        })
    }

    /// Fold one lane delivery's opening or steering move.
    pub(super) fn apply_delivery(&mut self, fact: &StreamFact) -> bool {
        let position = fact.seq();
        match fact.kind() {
            FactKind::SteerSent => fact.read::<SteerSent>().is_some_and(|body| {
                self.deliver(
                    position,
                    body.directive,
                    body.key,
                    body.lane,
                    false,
                    body.text,
                )
            }),
            FactKind::SteerMessaged => fact.read::<SteerMessaged>().is_some_and(|body| {
                self.deliver(
                    position,
                    body.directive,
                    body.key,
                    body.lane,
                    false,
                    body.text,
                )
            }),
            FactKind::SteerQueued => fact.read::<SteerQueued>().is_some_and(|body| {
                self.deliver(
                    position,
                    body.directive,
                    body.key,
                    body.lane,
                    true,
                    body.text,
                )
            }),
            FactKind::SteerAcked => fact
                .read::<SteerAcked>()
                .is_some_and(|body| self.dispose(position, &body.key, Disposition::Acked)),
            FactKind::SteerDeclined => fact
                .read::<SteerDeclined>()
                .is_some_and(|body| self.dispose(position, &body.key, Disposition::Declined)),
            FactKind::SteerLapsed => fact
                .read::<SteerLapsed>()
                .is_some_and(|body| self.dispose(position, &body.key, Disposition::Lapsed)),
            FactKind::SteerDeadLettered => fact
                .read::<SteerDeadLettered>()
                .is_some_and(|body| self.dispose(position, &body.key, Disposition::DeadLettered)),
            FactKind::SteerCanceled => fact
                .read::<SteerCanceled>()
                .is_some_and(|body| self.dispose(position, &body.key, Disposition::Canceled)),
            FactKind::SteerRecovered => fact
                .read::<SteerRecovered>()
                .is_some_and(|body| self.dispose(position, &body.key, Disposition::Live)),
            _ => false,
        }
    }

    fn deliver(
        &mut self,
        position: Seq,
        directive: CanonicalUuid,
        key: String,
        lane: CanonicalUuid,
        queued: bool,
        text: String,
    ) -> bool {
        self.keys.insert(key.clone(), directive);
        self.deliveries.insert(
            directive,
            RoomPositioned {
                position,
                value: Delivery {
                    key,
                    lane,
                    queued,
                    disposition: Disposition::Live,
                    text,
                },
            },
        );
        false
    }

    /// Move one delivery; an unnamed delivery that retires is dropped, since
    /// no later record can name it.
    fn dispose(&mut self, position: Seq, key: &str, disposition: Disposition) -> bool {
        let Some(directive) = self.keys.get(key).copied() else {
            return false;
        };
        if disposition != Disposition::Live && !self.named.contains(&directive) {
            self.keys.remove(key);
            self.deliveries.remove(&directive);
            return false;
        }
        self.deliveries.get_mut(&directive).is_some_and(|delivery| {
            let changed = delivery.value.disposition != disposition;
            delivery.value.disposition = disposition;
            delivery.position = position;
            changed
        })
    }
}
