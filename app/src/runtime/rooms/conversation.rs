//! Room conversation authority: each substantive MSG, ASK, or ANS, the
//! receipt each destination owes it, and each question's result.
//!
//! A room snapshot carries only the facts behind current heads, so every move
//! sets the state its fact names instead of replaying a guarded transition;
//! the journal already admitted each one.

use std::collections::{BTreeMap, BTreeSet};

use wire::body::{
    SteerAcked, SteerCanceled, SteerDeclined, SteerLapsed, SteerMessaged, SteerQueued,
    SteerRecovered, SteerSent,
};
use wire::rooms::{
    QuestionClosure, QuestionParties, QuestionResultRecord, QuestionResultRef, QuestionResultState,
    RoomAnchor, RoomAuthorityStamp, RoomDelivery, RoomDestination, RoomDisposed, RoomPublication,
    RoomPublicationKind, RoomQuestionClosed, RoomQuestionTimedOut, RoomReceiptControl,
    RoomReceiptLapsed, RoomReceiptRecord, RoomReceiptRecovered, RoomReceiptRef, RoomReceiptState,
    RoomSource,
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
    /// Question result the operator owes for an ASK, if this item delivered one.
    pub result: Option<QuestionResultRef>,
    /// Current disposition.
    pub state: RoomReceiptState,
}

/// One question's substantive result, separate from its delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionResult {
    /// Frozen parties, context, and deadline.
    pub record: QuestionResultRecord,
    /// Current result.
    pub state: QuestionResultState,
}

/// One catalog entry while its operator receipt or answer duty remains open.
#[derive(Clone, Copy, Debug)]
pub struct OperatorInboxItem<'a> {
    /// Stable operator-item identity.
    pub key: &'a RoomReceiptRef,
    /// Immutable record and current receipt state.
    pub item: &'a RoomPositioned<OperatorItem>,
    /// Substantive result owed by the operator, when this item is an ASK.
    pub result: Option<&'a RoomPositioned<QuestionResult>>,
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
    results: BTreeMap<QuestionResultRef, RoomPositioned<QuestionResult>>,
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

    /// Operator items whose receipt or substantive answer duty remains open,
    /// in immutable admission order. Receipt/result head moves never reorder
    /// this catalog.
    pub fn inbox_items(&self) -> Vec<OperatorInboxItem<'_>> {
        let mut open = self
            .items
            .iter()
            .filter_map(|(key, item)| {
                let result = item
                    .value
                    .result
                    .as_ref()
                    .and_then(|result| self.results.get(result));
                (!item.value.state.is_terminal()
                    || result.is_some_and(|result| !result.value.state.is_terminal()))
                .then_some(OperatorInboxItem { key, item, result })
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

    /// One question result.
    pub fn result(&self, result: &QuestionResultRef) -> Option<&RoomPositioned<QuestionResult>> {
        self.results.get(result)
    }

    /// One named lane delivery.
    pub fn delivery(&self, directive: &CanonicalUuid) -> Option<&RoomPositioned<Delivery>> {
        self.deliveries.get(directive)
    }

    /// Pending results `requester` holds, against the admission bound.
    pub fn pending_results(&self, requester: &RoomDestination) -> usize {
        self.results
            .values()
            .filter(|result| {
                result.value.record.requester() == requester && !result.value.state.is_terminal()
            })
            .count()
    }

    /// Mark directives a room record names as deliveries to keep.
    pub(super) fn name(&mut self, directed: &[CanonicalUuid]) {
        self.named.extend(directed.iter().copied());
    }

    /// Admit one publication with the operator item and result it opens.
    pub(super) fn admit(
        &mut self,
        position: Seq,
        at: Instant,
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
        match publication.kind() {
            RoomPublicationKind::Msg {} => {}
            RoomPublicationKind::Ask { result, deadline } => {
                self.ask(position, &publication, result, *deadline);
            }
            RoomPublicationKind::Ans { result, .. } => {
                self.answer(position, at, &publication, result, &directed, expires);
            }
        }
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

    fn ask(
        &mut self,
        position: Seq,
        publication: &RoomPublication,
        result: &QuestionResultRef,
        deadline: Instant,
    ) {
        let Some(route) = publication.routes().first() else {
            return;
        };
        let Ok(record) = QuestionResultRecord::try_new(
            result.clone(),
            publication.publication().clone(),
            QuestionParties {
                requester: publication.author().clone(),
                respondent: route.destination.clone(),
                context: route.context.clone(),
            },
            publication.source().clone(),
            deadline,
        ) else {
            return;
        };
        self.results.insert(
            result.clone(),
            RoomPositioned {
                position,
                value: QuestionResult {
                    record,
                    state: QuestionResultState::Pending {},
                },
            },
        );
    }

    /// Settle the answered result, retire a live operator ASK item, and open
    /// the answer's return item to an operator requester.
    fn answer(
        &mut self,
        position: Seq,
        at: Instant,
        publication: &RoomPublication,
        result: &QuestionResultRef,
        directed: &[CanonicalUuid],
        expires: Instant,
    ) {
        let Some(question) = self.results.get_mut(result) else {
            return;
        };
        let Ok(state) = question
            .value
            .record
            .answer_state(publication, at, directed)
        else {
            return;
        };
        let QuestionResultState::Answered {
            return_delivery, ..
        } = &state
        else {
            return;
        };
        let returned = return_delivery.delivery().clone();
        question.value.state = state;
        question.position = position;
        let context = question.value.record.context().cloned();
        if let Some(asked) = question
            .value
            .record
            .item()
            .and_then(|item| self.items.get_mut(&item))
            .filter(|asked| matches!(asked.value.state, RoomReceiptState::Pending { .. }))
        {
            asked.value.state = RoomReceiptState::Acked {
                source: publication.source().clone(),
            };
            asked.position = position;
        }
        if let RoomDelivery::Item { item } = returned {
            self.open(position, item, publication, context, expires);
        }
    }

    fn open(
        &mut self,
        position: Seq,
        item: RoomReceiptRef,
        publication: &RoomPublication,
        context: Option<RoomAnchor>,
        expires: Instant,
    ) {
        let result = match publication.kind() {
            RoomPublicationKind::Ask { result, .. } => Some(result.clone()),
            RoomPublicationKind::Msg {} | RoomPublicationKind::Ans { .. } => None,
        };
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
                    result,
                    state: RoomReceiptState::Pending { expires },
                },
            },
        );
    }

    /// Fold one operator-item or result move, or `None` when `fact` is not one.
    pub(super) fn apply(&mut self, fact: &StreamFact) -> Option<bool> {
        let position = fact.seq();
        let code = SequenceCode::new(position.get());
        Some(match fact.kind() {
            FactKind::RoomDisposed => fact
                .read::<RoomDisposed>()
                .and_then(|body| body.control(code).ok())
                .is_some_and(|control| self.disposed(position, &control)),
            FactKind::RoomQuestionClosed => fact
                .read::<RoomQuestionClosed>()
                .is_some_and(|body| self.closed(position, &body)),
            FactKind::RoomQuestionTimedOut => {
                fact.read::<RoomQuestionTimedOut>().is_some_and(|body| {
                    self.settle_result(position, &body.result, QuestionResultState::TimedOut {})
                })
            }
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

    /// Settle every named pending item, and decline an operator ASK's pending
    /// result on NAK even after its item retired.
    fn disposed(&mut self, position: Seq, control: &RoomReceiptControl) -> bool {
        let mut changed = false;
        for key in control.keys() {
            changed |= self.settle_item(position, key, control.receipt_state());
            let result = QuestionResultRef::new(key.authority().clone());
            let declined = self
                .results
                .get(&result)
                .filter(|question| !question.value.state.is_terminal())
                .and_then(|question| question.value.record.control_result(control));
            if let Some(state) = declined {
                changed |= self.settle_result(position, &result, state);
            }
        }
        changed
    }

    /// A decline or cancellation settles the result and retires a pending
    /// operator ASK item the same way.
    fn closed(&mut self, position: Seq, body: &RoomQuestionClosed) -> bool {
        let source = RoomSource::Journal {
            fact: SequenceCode::new(position.get()),
        };
        let decided = match body.closure {
            QuestionClosure::Declined => RoomReceiptState::Declined { source },
            QuestionClosure::Cancelled => RoomReceiptState::Cancelled { source },
        };
        let item = self
            .results
            .get(&body.result)
            .and_then(|question| question.value.record.item());
        let settled = self.settle_result(
            position,
            &body.result,
            body.state(SequenceCode::new(position.get())),
        );
        settled | item.is_some_and(|item| self.settle_item(position, &item, decided))
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

    fn settle_result(
        &mut self,
        position: Seq,
        result: &QuestionResultRef,
        state: QuestionResultState,
    ) -> bool {
        self.results
            .get_mut(result)
            .filter(|result| !result.value.state.is_terminal())
            .is_some_and(|result| {
                result.value.state = state;
                result.position = position;
                true
            })
    }

    /// Fold one lane delivery's opening or steering move. A respondent lane's
    /// NAK of a live ASK directive also declines that ASK's pending result.
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
            FactKind::SteerDeclined => fact.read::<SteerDeclined>().is_some_and(|body| {
                let moved = self.dispose(position, &body.key, Disposition::Declined);
                moved | self.directive_declined(position, &body)
            }),
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

    /// A respondent lane's NAK of its ASK directive, keyed by the result
    /// handle, declines that pending result.
    fn directive_declined(&mut self, position: Seq, body: &SteerDeclined) -> bool {
        let Ok(result) = body.key.parse().map(QuestionResultRef::new) else {
            return false;
        };
        let respondent = RoomDestination::Lane { lane: body.lane };
        let asked = self
            .results
            .get(&result)
            .is_some_and(|question| question.value.record.respondent() == &respondent);
        let source = RoomSource::Journal {
            fact: SequenceCode::new(position.get()),
        };
        asked && self.settle_result(position, &result, QuestionResultState::Declined { source })
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
