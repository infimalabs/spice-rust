//! Authenticated Canvas subscriptions over journal and transcript sources.

use std::collections::BTreeMap;
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use bus::{BusReader, CanvasHeads};
use wire::{
    Accepted, AgentMessage, Fault, FaultClass, OperatorDirection, Packed, Seq, StreamFrame,
    StreamRequest, ViewSubject, WireVersion,
};

mod budget;
mod message;
mod policy;
mod subscription;
#[cfg(test)]
mod tests;

pub use message::{
    MESSAGE_CAPACITY, MESSAGE_HISTORY_PAGE_LIMIT, MessageHistoryPage, MessageHistoryProvider,
    MessagePublishError, MessagePublisher,
};
use message::{MessageFeed, message_snapshot_fault};
pub use policy::Failure;
use policy::{
    admit_subject, admit_subjects, present_facts, protocol_fault, refusal, required_horizon,
    thread_fault,
};
pub use subscription::{LiveDelivery, Subscription};

/// Maximum newest durable operator directions restored for each lane.
pub const OPERATOR_DIRECTION_CAPACITY: usize = 256;

/// Generation-owned source for every streamed canvas answer.
#[derive(Clone, Debug)]
pub struct Source {
    bus: BusReader,
    epoch: wire::CanonicalUuid,
    rows: usize,
    messages: MessageFeed,
    #[cfg(test)]
    reads: Arc<ReadCounts>,
}

#[cfg(test)]
#[derive(Debug, Default)]
struct ReadCounts {
    facts: AtomicUsize,
    heads: AtomicUsize,
}

impl Source {
    /// Bind canvas reads to one resident Bus generation and one packing bound.
    #[must_use]
    pub fn new(bus: BusReader, epoch: wire::CanonicalUuid, rows: usize) -> Self {
        Self {
            bus,
            epoch,
            rows,
            messages: MessageFeed::new(),
            #[cfg(test)]
            reads: Arc::new(ReadCounts::default()),
        }
    }

    /// Reader for the one bounded post-durability Bus cursor broadcast.
    #[must_use]
    pub fn subscribe(&self) -> bus::BroadcastSubscriber {
        self.bus.subscribe()
    }

    /// Reader for live agent-authored transcript activities.
    #[must_use]
    pub fn subscribe_messages(&self) -> tokio::sync::broadcast::Receiver<AgentMessage> {
        self.messages.updates.subscribe()
    }

    /// Generation-scoped writer for visible activity accepted from lane capture.
    #[must_use]
    pub fn message_publisher(&self) -> MessagePublisher {
        MessagePublisher {
            feed: self.messages.clone(),
        }
    }

    /// Start one authenticated subscription from one authority snapshot.
    pub fn begin(
        &self,
        accepted: &Accepted,
        request: &StreamRequest,
    ) -> Result<(Subscription, Vec<StreamFrame>), Failure> {
        request.admit(self.epoch).map_err(Failure::admission)?;
        let StreamRequest::Subscribe {
            id,
            subjects,
            after,
            ..
        } = request
        else {
            return Err(Failure::Refusal(refusal(
                "stream::subscription-required",
                "send one subscribe request before continuation",
            )));
        };
        if self.rows == 0 {
            return Err(Failure::Fault(protocol_fault(
                "stream::pack-limit-invalid",
                "canvas stream packing requires at least one row",
            )));
        }
        let subjects = admit_subjects(&accepted.actor, subjects)?;
        let heads = self.heads()?;
        let horizon = required_horizon(&heads)?;
        let after_seq = after.map(|cursor| cursor.seq);
        if after_seq.is_some_and(|cursor| cursor > horizon) {
            return Err(Failure::Refusal(refusal(
                "stream::cursor-ahead",
                &format!("resume cursor {after_seq:?} is beyond snapshot {horizon}"),
            )));
        }
        let cursor = if matches!(&accepted.actor, wire::Actor::Operator) {
            Some(horizon)
        } else {
            after_seq
        };

        let mut subscription = Subscription {
            id: *id,
            epoch: self.epoch,
            subjects,
            cursor,
            pending: BTreeMap::new(),
            barrier: Some(heads),
            catch_up: None,
            rows: self.rows,
        };
        let mut frames = Vec::new();
        for subject in subscription.subjects.iter().copied().collect::<Vec<_>>() {
            let frame = match subject {
                ViewSubject::Directions => StreamFrame::Directions {
                    v: WireVersion::CURRENT,
                    id: *id,
                    epoch: self.epoch,
                    limit: u32::try_from(OPERATOR_DIRECTION_CAPACITY)
                        .expect("the fixed direction capacity fits in the wire field"),
                    directions: self.directions()?,
                },
                ViewSubject::Messages => {
                    let snapshot = self
                        .messages
                        .initial_snapshot()
                        .map_err(message_snapshot_fault)?;
                    StreamFrame::Messages {
                        v: WireVersion::CURRENT,
                        id: *id,
                        epoch: self.epoch,
                        limit: u32::try_from(MESSAGE_CAPACITY)
                            .expect("the fixed message capacity fits in the wire field"),
                        messages: snapshot.messages,
                        more: snapshot.more,
                    }
                }
                ViewSubject::Facts | ViewSubject::Trust => {
                    subscription.page(self, subject, None, *id)?
                }
            };
            frames.push(frame);
        }
        subscription.settle_barrier(horizon);
        Ok((subscription, frames))
    }

    /// Answer one causal thread request from one immutable Bus snapshot.
    pub fn thread(
        &self,
        accepted: &Accepted,
        request: &StreamRequest,
    ) -> Result<StreamFrame, Failure> {
        request.admit(self.epoch).map_err(Failure::admission)?;
        let StreamRequest::Thread { id, cursor, .. } = request else {
            return Err(Failure::Refusal(refusal(
                "stream::thread-required",
                "the request is not a causal thread request",
            )));
        };
        admit_subject(&accepted.actor, ViewSubject::Facts)?;
        let packed = self
            .bus
            .canvas_thread(cursor.seq, self.fact_budget()?)
            .map_err(thread_fault)?
            .ok_or_else(|| {
                Failure::Refusal(refusal(
                    "stream::cursor-unknown",
                    &format!("causal position {} is absent at this barrier", cursor.seq),
                ))
            })?;
        Ok(StreamFrame::Thread {
            v: WireVersion::CURRENT,
            id: *id,
            epoch: self.epoch,
            packed: present_facts(packed),
        })
    }

    /// Answer one member-local retained transcript history request.
    pub fn message_history(
        &self,
        accepted: &Accepted,
        request: &StreamRequest,
    ) -> Result<StreamFrame, Failure> {
        request.admit(self.epoch).map_err(Failure::admission)?;
        let StreamRequest::MessageHistory {
            id, lane, before, ..
        } = request
        else {
            return Err(Failure::Refusal(refusal(
                "stream::message-history-required",
                "send one message-history request",
            )));
        };
        admit_subject(&accepted.actor, ViewSubject::Messages)?;
        let page = self
            .messages
            .history(*lane, *before)
            .map_err(message_snapshot_fault)?
            .ok_or_else(|| {
                Failure::Refusal(refusal(
                    "stream::message-cursor-unknown",
                    "the lane does not retain the requested oldest message",
                ))
            })?;
        Ok(StreamFrame::MessageHistory {
            v: WireVersion::CURRENT,
            id: *id,
            epoch: self.epoch,
            lane: *lane,
            before: *before,
            more: page.more,
            messages: page.messages,
        })
    }

    fn facts(
        &self,
        start: journal::FactPageStart,
        horizon: Seq,
    ) -> Result<Packed<wire::Envelope<wire::AnyFact>>, Failure> {
        #[cfg(test)]
        self.reads.facts.fetch_add(1, Ordering::Relaxed);
        self.bus
            .canvas_facts(start, horizon, self.fact_budget()?)
            .map_err(|error| {
                Failure::Fault(Fault {
                    class: FaultClass::Store,
                    reason: "stream::fact-page-failed".to_owned(),
                    detail: error.to_string(),
                })
            })
    }

    fn directions(&self) -> Result<Vec<OperatorDirection>, Failure> {
        self.bus
            .canvas_directions(OPERATOR_DIRECTION_CAPACITY)
            .map_err(|error| {
                Failure::Fault(Fault {
                    class: FaultClass::Store,
                    reason: "stream::direction-snapshot-failed".to_owned(),
                    detail: error.to_string(),
                })
            })
            .map(|heads| {
                heads
                    .into_iter()
                    .map(|head| OperatorDirection {
                        authored: head.body().published,
                        directive: head.body().directive,
                        key: head.body().key.clone(),
                        lane: head.body().lane,
                        text: head.body().text.clone(),
                        attachments: head.body().attachments.clone(),
                    })
                    .collect()
            })
    }

    fn heads(&self) -> Result<CanvasHeads, Failure> {
        #[cfg(test)]
        self.reads.heads.fetch_add(1, Ordering::Relaxed);
        self.bus.canvas_heads().map_err(|error| {
            Failure::Fault(Fault {
                class: FaultClass::Store,
                reason: "stream::heads-failed".to_owned(),
                detail: error.to_string(),
            })
        })
    }

    #[cfg(test)]
    fn read_counts(&self) -> (usize, usize) {
        (
            self.reads.heads.load(Ordering::Relaxed),
            self.reads.facts.load(Ordering::Relaxed),
        )
    }
}
