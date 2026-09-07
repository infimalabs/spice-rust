//! Authenticated Canvas subscriptions over journal and transcript sources.

use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use bus::{BusReader, CanvasHeads, JournalCommit};
use wire::{
    Accepted, AgentMessage, Fault, FaultClass, OperatorDirection, Packed, Refusal, Seq,
    StreamCursor, StreamFact, StreamFrame, StreamRequest, ViewSubject, WireVersion,
};

mod message;
mod policy;

pub use message::{
    INITIAL_MESSAGE_LIMIT, MESSAGE_CAPACITY, MESSAGE_HISTORY_PAGE_LIMIT, MessageHistoryPage,
    MessageHistoryProvider, MessagePublishError, MessagePublisher,
};
use message::{MessageFeed, message_snapshot_fault};
pub use policy::Failure;
use policy::{
    admit_subject, admit_subjects, canvas_visible_fact, pack_unique, present_facts, protocol_fault,
    refusal, required_horizon, thread_fault, trust_rows,
};

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
            .canvas_thread(cursor.seq, self.rows)
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
            .canvas_facts(start, horizon, self.rows)
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

/// One bounded live-delivery transaction awaiting successful socket sends.
#[derive(Debug)]
pub struct LiveDelivery {
    frames: Vec<StreamFrame>,
    fact_cursor: Option<Seq>,
    complete: bool,
    trust_continuation: Option<Seq>,
}

impl LiveDelivery {
    /// Frames materialized by this one delivery quantum.
    #[must_use]
    pub fn frames(&self) -> &[StreamFrame] {
        &self.frames
    }
}

/// One admitted subscription and its exact continuation/live cursor state.
#[derive(Debug)]
pub struct Subscription {
    id: wire::CanonicalUuid,
    epoch: wire::CanonicalUuid,
    subjects: BTreeSet<ViewSubject>,
    cursor: Option<Seq>,
    pending: BTreeMap<ViewSubject, Seq>,
    barrier: Option<CanvasHeads>,
    catch_up: Option<CanvasHeads>,
    rows: usize,
}

impl Subscription {
    /// Correlation identity of this subscription's live updates.
    #[must_use]
    pub const fn id(&self) -> wire::CanonicalUuid {
        self.id
    }

    /// Last fact cursor this subscription can resume strictly after.
    #[must_use]
    pub const fn cursor(&self) -> Option<Seq> {
        self.cursor
    }

    /// True once every initial packed page has been continued to its barrier.
    #[must_use]
    pub const fn is_live(&self) -> bool {
        self.barrier.is_none()
    }

    /// Continue the exact subject cursor reported by the preceding page.
    pub fn continue_page(
        &mut self,
        source: &Source,
        request: &StreamRequest,
    ) -> Result<StreamFrame, Failure> {
        request.admit(self.epoch).map_err(Failure::admission)?;
        let StreamRequest::Continue {
            id,
            subject,
            cursor,
            ..
        } = request
        else {
            return Err(Failure::Refusal(refusal(
                "stream::continuation-required",
                "the request is not a packed continuation",
            )));
        };
        let expected = self.pending.get(subject).copied().ok_or_else(|| {
            Failure::Refusal(refusal(
                "stream::continuation-unexpected",
                "this subject has no pending packed continuation",
            ))
        })?;
        if cursor.seq != expected {
            return Err(Failure::Refusal(Refusal::bare(
                wire::RefusalDiagnostic::new(
                    "stream::continuation-moved",
                    Some("cursor".to_owned()),
                    Some(expected.to_string()),
                    Some(cursor.seq.to_string()),
                ),
            )));
        }
        let frame = self.page(source, *subject, Some(cursor.seq), *id)?;
        let horizon = required_horizon(
            self.barrier
                .as_ref()
                .expect("pending continuations retain their barrier"),
        )?;
        self.settle_barrier(horizon);
        Ok(frame)
    }

    /// Whether a proved live barrier still has facts to deliver.
    #[must_use]
    pub const fn has_continuation(&self) -> bool {
        self.catch_up.is_some()
    }

    /// Materialize at most one live fact quantum after a durable Bus notification.
    ///
    /// The returned delivery has not yet advanced the resumable cursor. The
    /// socket owner must call [`Self::admit`] only after every frame was sent.
    pub fn advance(
        &mut self,
        source: &Source,
        commit: JournalCommit,
    ) -> Result<Option<LiveDelivery>, Failure> {
        if !self.is_live() {
            return Ok(None);
        }
        let target = self.catch_up.as_ref().map(required_horizon).transpose()?;
        if target.is_none_or(|target| commit.last() > target.get()) {
            if self
                .cursor
                .is_some_and(|cursor| commit.last() <= cursor.get())
            {
                return Ok(None);
            }
            let heads = source.heads()?;
            let horizon = required_horizon(&heads)?;
            if self.cursor.is_some_and(|cursor| horizon <= cursor) {
                return Ok(None);
            }
            self.catch_up = Some(heads);
        }
        self.next_delivery(source).map(Some)
    }

    /// Materialize the next quantum from the retained live barrier.
    pub fn continue_live(&self, source: &Source) -> Result<Option<LiveDelivery>, Failure> {
        self.catch_up
            .as_ref()
            .map(|_| self.next_delivery(source))
            .transpose()
    }

    /// Admit one delivery after every frame in it reached the socket.
    pub fn admit(&mut self, delivery: LiveDelivery) {
        if let Some(cursor) = delivery.fact_cursor {
            self.cursor = Some(cursor);
        }
        if !delivery.complete {
            return;
        }
        let heads = self
            .catch_up
            .take()
            .expect("a complete live delivery retains its proved barrier");
        let horizon = required_horizon(&heads).expect("a retained barrier has a horizon");
        if let Some(continuation) = delivery.trust_continuation {
            self.pending.insert(ViewSubject::Trust, continuation);
            self.barrier = Some(heads);
        } else {
            self.pending.remove(&ViewSubject::Trust);
            self.barrier = None;
        }
        self.cursor = Some(horizon);
    }

    fn next_delivery(&self, source: &Source) -> Result<LiveDelivery, Failure> {
        let heads = self
            .catch_up
            .as_ref()
            .expect("live continuation retains its proved barrier");
        let horizon = required_horizon(heads)?;
        let mut frames = Vec::new();
        let mut fact_cursor = None;
        let mut complete = true;
        if self.subjects.contains(&ViewSubject::Facts) {
            let packed = source.facts(journal::FactPageStart::After(self.cursor), horizon)?;
            fact_cursor = packed.rows.last().map(|fact| fact.seq);
            complete = packed.overflow.is_none();
            frames.extend(
                packed
                    .rows
                    .into_iter()
                    .filter(canvas_visible_fact)
                    .map(StreamFact::from)
                    .map(|fact| StreamFrame::Fact {
                        v: WireVersion::CURRENT,
                        id: self.id,
                        epoch: self.epoch,
                        fact,
                    }),
            );
        }
        let trust_continuation = if complete && self.subjects.contains(&ViewSubject::Trust) {
            let trust = trust_rows(heads);
            let packed = pack_unique(trust, self.rows, horizon, |row| row.position)?;
            let continuation = packed.overflow.map(|overflow| overflow.continuation);
            frames.push(StreamFrame::Trust {
                v: WireVersion::CURRENT,
                id: self.id,
                epoch: self.epoch,
                packed,
            });
            continuation
        } else {
            None
        };
        Ok(LiveDelivery {
            frames,
            fact_cursor,
            complete,
            trust_continuation,
        })
    }

    /// Project one live transcript activity for subscriptions that asked for it.
    #[must_use]
    pub fn message(&self, message: AgentMessage) -> Option<StreamFrame> {
        (self.is_live() && self.subjects.contains(&ViewSubject::Messages)).then_some(
            StreamFrame::Message {
                v: WireVersion::CURRENT,
                id: self.id,
                epoch: self.epoch,
                message,
            },
        )
    }

    /// Terminal bounded-backpressure frame, resumable after the admitted cursor.
    pub fn overflow(&self, dropped: u64) -> Result<StreamFrame, Failure> {
        let cursor = self.cursor.ok_or_else(|| {
            Failure::Fault(protocol_fault(
                "stream::cursor-missing",
                "subscription lagged before it admitted a resumable cursor",
            ))
        })?;
        Ok(StreamFrame::Overflow {
            v: WireVersion::CURRENT,
            id: self.id,
            epoch: self.epoch,
            after: StreamCursor {
                epoch: self.epoch,
                seq: cursor,
            },
            dropped,
        })
    }

    /// Terminal generation shutdown frame at the last admitted cursor.
    pub fn closing(&self) -> Option<StreamFrame> {
        self.cursor.map(|cursor| StreamFrame::Closing {
            v: WireVersion::CURRENT,
            id: self.id,
            epoch: self.epoch,
            after: StreamCursor {
                epoch: self.epoch,
                seq: cursor,
            },
        })
    }

    fn page(
        &mut self,
        source: &Source,
        subject: ViewSubject,
        continuation: Option<Seq>,
        request_id: wire::CanonicalUuid,
    ) -> Result<StreamFrame, Failure> {
        let heads = self.barrier.as_ref().ok_or_else(|| {
            Failure::Refusal(refusal(
                "stream::continuation-stale",
                "the subscription has already crossed into live delivery",
            ))
        })?;
        let horizon = required_horizon(heads)?;
        let frame = match subject {
            ViewSubject::Facts => {
                let start = continuation.map_or(
                    journal::FactPageStart::After(self.cursor),
                    journal::FactPageStart::At,
                );
                let packed = present_facts(source.facts(start, horizon)?);
                self.record_page(subject, &packed, StreamFact::seq);
                StreamFrame::Facts {
                    v: WireVersion::CURRENT,
                    id: request_id,
                    epoch: self.epoch,
                    packed,
                }
            }
            ViewSubject::Trust => {
                let rows = trust_rows(heads)
                    .into_iter()
                    .filter(|row| continuation.is_none_or(|cursor| row.position >= cursor))
                    .collect::<Vec<_>>();
                let packed = pack_unique(rows, self.rows, horizon, |row| row.position)?;
                self.record_page(subject, &packed, |row| row.position);
                StreamFrame::Trust {
                    v: WireVersion::CURRENT,
                    id: request_id,
                    epoch: self.epoch,
                    packed,
                }
            }
            ViewSubject::Messages => {
                return Err(Failure::Refusal(refusal(
                    "stream::message-continuation-invalid",
                    "message snapshots never carry a journal continuation",
                )));
            }
            ViewSubject::Directions => {
                return Err(Failure::Refusal(refusal(
                    "stream::direction-continuation-invalid",
                    "direction snapshots never carry a journal continuation",
                )));
            }
        };
        Ok(frame)
    }

    fn record_page<T>(
        &mut self,
        subject: ViewSubject,
        packed: &Packed<T>,
        position: impl Fn(&T) -> Seq,
    ) {
        if let Some(overflow) = packed.overflow {
            self.pending.insert(subject, overflow.continuation);
        } else {
            self.pending.remove(&subject);
        }
        if subject == ViewSubject::Facts
            && let Some(last) = packed.rows.last()
        {
            self.cursor = Some(position(last));
        }
    }

    fn settle_barrier(&mut self, horizon: Seq) {
        if self.pending.is_empty() {
            self.cursor = Some(horizon);
            self.barrier = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::{Bus, CommitOutcome};
    use journal::{FactCause, Journal, JournalWrite};
    use wire::{Actor, Disposition, Domain, Producer, SteeringHead, TrustHead, TrustState, body};

    mod retention;

    fn uuid(value: &str) -> wire::CanonicalUuid {
        value.parse().expect("canonical fixture uuid")
    }

    fn seq(value: u64) -> Seq {
        Seq::new(value).expect("non-zero fixture position")
    }

    fn accepted(actor: Actor) -> Accepted {
        Accepted {
            wire: WireVersion::CURRENT,
            actor,
            epoch: epoch(),
        }
    }

    fn epoch() -> wire::CanonicalUuid {
        uuid("018f82d4-6f59-7d06-92e3-bce9131163c3")
    }

    fn message(lane: u8, offset: u64) -> AgentMessage {
        AgentMessage {
            lane: uuid(&format!("018f82d4-6f59-7d06-92e3-bce9131163{lane:02x}")),
            author: uuid("018f82d4-6f59-7d06-92e3-bce9131163ef"),
            locus: wire::MessageLocus {
                member: 1,
                offset,
                length: 20,
                event: 0,
            },
            authored: "2030-04-12T18:44:09.271Z".parse().expect("fixture instant"),
            authored_synthetic: false,
            observed: None,
            activity: wire::AgentActivityKind::Prose,
            final_answer: false,
            text: format!("lane {lane:02x} message {offset}"),
            automatic_speech: None,
            unavailable_tool_terminals: Vec::new(),
            media: Vec::new(),
        }
    }

    fn request(last: u8, subjects: Vec<ViewSubject>, after: Option<Seq>) -> StreamRequest {
        StreamRequest::Subscribe {
            v: WireVersion::CURRENT,
            id: uuid(&format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02x}")),
            epoch: epoch(),
            subjects,
            after: after.map(|seq| StreamCursor {
                epoch: epoch(),
                seq,
            }),
        }
    }

    fn fixture(notes: usize) -> Bus {
        let bus = Bus::start(Journal::in_memory().expect("authority opens")).expect("bus starts");
        commit_notes(&bus, notes, true);
        bus
    }

    #[test]
    fn retained_tool_status_does_not_consume_visible_lane_capacity() {
        let bus = fixture(1);
        let source = Source::new(bus.reader(), epoch(), 10);
        let publisher = source.message_publisher();
        for offset in 0..MESSAGE_CAPACITY {
            publisher
                .publish(message(0xe5, u64::try_from(offset).expect("offset fits")))
                .expect("visible backlog publishes");
        }
        for offset in [10_000, 10_001] {
            let mut status = message(0xe5, offset);
            status.activity = wire::AgentActivityKind::Tool;
            publisher.publish(status).expect("tool status publishes");
        }

        let (_, frames) = source
            .begin(
                &accepted(Actor::Operator),
                &request(0xc4, vec![ViewSubject::Messages], None),
            )
            .expect("message subscription begins");
        let StreamFrame::Messages { messages, more, .. } = &frames[0] else {
            panic!("expected retained message snapshot")
        };
        assert_eq!(
            messages
                .iter()
                .filter(|message| message.activity.is_prose())
                .count(),
            INITIAL_MESSAGE_LIMIT,
        );
        assert_eq!(more, &[message(0xe5, 0).lane]);
        assert_eq!(
            messages
                .iter()
                .filter(|message| !message.activity.is_prose())
                .count(),
            1,
            "only the latest non-card status is retained",
        );
        assert_eq!(messages.last().expect("latest status").locus.offset, 10_001);
        drop(source);
        bus.shutdown().expect("bus shuts down");
    }

    fn commit_notes(bus: &Bus, notes: usize, trust: bool) -> JournalCommit {
        let mut write = JournalWrite::new(
            Actor::Operator,
            "task:STREAM-a1".parse().expect("origin"),
            "2030-04-12T18:44:09.271Z".parse().expect("instant"),
            Producer::CommandDoor,
        );
        let task: wire::TaskStamp = "1kKRRsDP".parse().expect("task stamp");
        let mut cause = FactCause::Root;
        for index in 0..notes {
            let fact = write
                .fact(
                    body::TaskNoted {
                        task: task.clone(),
                        note: format!("note {index}"),
                    },
                    cause,
                )
                .expect("fact builds");
            cause = FactCause::Pending(fact);
        }
        if trust {
            let digest = wire::Digest::compute(Domain::ConfigExecutableSurface, b"surface");
            let fact = write
                .fact(
                    body::TrustPended {
                        surface: "config".to_owned(),
                        digest: digest.clone(),
                        worktree: None,
                        previous: None,
                    },
                    FactCause::Root,
                )
                .expect("trust fact builds");
            write.trust_head(fact, move |_| TrustHead {
                digest,
                state: TrustState::Pended,
                worktree: None,
            });
        }
        match bus.commit(write).expect("commit answers") {
            CommitOutcome::Committed(commit) => commit,
            other => panic!("fixture commit failed: {other:?}"),
        }
    }

    fn commit_operator_direction(bus: &Bus) -> SteeringHead {
        let directive = uuid("018f82d4-6f59-7d06-92e3-bce9131163d1");
        let lane = uuid("018f82d4-6f59-7d06-92e3-bce9131163e5");
        let key = "1kQnrzNZ".to_owned();
        let at = "2030-04-12T18:45:09.271Z".parse().expect("fixture instant");
        let expires = "2030-04-12T19:45:09.271Z".parse().expect("fixture expiry");
        let origin: wire::Origin = "steer:1kQnrzNZ".parse().expect("fixture origin");
        let sent = body::SteerSent {
            directive,
            fanout: None,
            key: key.clone(),
            lane,
            text: "direction survives standing changes".to_owned(),
            control: None,
            attachments: Vec::new(),
            expires,
        };
        let mut write =
            JournalWrite::new(Actor::Operator, origin.clone(), at, Producer::CommandDoor);
        let pending = write
            .fact(sent.clone(), FactCause::Root)
            .expect("direction fact builds");
        let expected = sent.clone();
        write.steering_head(pending, move |created| SteeringHead {
            directive,
            fanout: None,
            key,
            lane,
            text: expected.text,
            control: None,
            attachments: Vec::new(),
            published: at,
            expires,
            stage: body::SteeringStage::Published,
            escalations: Vec::new(),
            disposition: Disposition::Live,
            origin,
            created,
            minted_micros: None,
        });
        match bus.commit(write).expect("direction commit answers") {
            CommitOutcome::Committed(_) => {}
            other => panic!("direction fixture failed: {other:?}"),
        }
        bus.read(move |read| read.steering_head(directive))
            .expect("direction head reads")
            .expect("direction head exists")
            .into_body()
    }

    fn cancel_operator_direction(bus: &Bus, current: &SteeringHead) {
        let mut write = JournalWrite::new(
            Actor::Operator,
            current.origin.clone(),
            "2030-04-12T18:46:09.271Z".parse().expect("fixture instant"),
            Producer::CommandDoor,
        );
        let pending = write
            .fact(
                body::SteerCanceled {
                    key: current.key.clone(),
                    lane: current.lane,
                },
                FactCause::Existing(current.created),
            )
            .expect("cancellation fact builds");
        let mut retired = current.clone();
        retired.disposition = Disposition::Canceled;
        write.steering_head(pending, move |_| retired);
        match bus.commit(write).expect("cancellation commit answers") {
            CommitOutcome::Committed(_) => {}
            other => panic!("cancellation fixture failed: {other:?}"),
        }
    }

    fn commit_trust_heads(bus: &Bus, surfaces: &[&[u8]]) -> JournalCommit {
        let mut write = JournalWrite::new(
            Actor::Operator,
            "task:STREAM-a1".parse().expect("origin"),
            "2030-04-12T18:44:09.271Z".parse().expect("instant"),
            Producer::CommandDoor,
        );
        for (index, surface) in surfaces.iter().enumerate() {
            let digest = wire::Digest::compute(Domain::ConfigExecutableSurface, surface);
            let fact = write
                .fact(
                    body::TrustPended {
                        surface: format!("surface-{index}"),
                        digest: digest.clone(),
                        worktree: None,
                        previous: None,
                    },
                    FactCause::Root,
                )
                .expect("trust fact builds");
            write.trust_head(fact, move |_| TrustHead {
                digest,
                state: TrustState::Pended,
                worktree: None,
            });
        }
        match bus.commit(write).expect("commit answers") {
            CommitOutcome::Committed(commit) => commit,
            other => panic!("fixture commit failed: {other:?}"),
        }
    }

    fn commit_ingestion(bus: &Bus) -> JournalCommit {
        let lane = uuid("018f82d4-6f59-7d06-92e3-bce9131163e5");
        let mut write = JournalWrite::new(
            Actor::Spice,
            format!("lane:{lane}").parse().expect("lane origin"),
            "2030-04-12T18:44:10.271Z".parse().expect("instant"),
            Producer::ResidentRecognition,
        );
        write
            .fact(
                body::JournalIngested {
                    lane,
                    vendor: "codex".parse().expect("vendor name"),
                    stream: "transcript".to_owned(),
                    source: uuid("018f82d4-6f59-7d06-92e3-bce9131163e6"),
                    supersedes: None,
                    from: body::TranscriptPosition {
                        member: 0,
                        offset: 0,
                    },
                    to: body::TranscriptPosition {
                        member: 0,
                        offset: 4096,
                    },
                    usage: Vec::new(),
                    non_usage_records: 3,
                    residue_records: 0,
                },
                FactCause::Root,
            )
            .expect("ingestion fact builds");
        match bus.commit(write).expect("commit answers") {
            CommitOutcome::Committed(commit) => commit,
            other => panic!("fixture commit failed: {other:?}"),
        }
    }

    fn facts(frame: &StreamFrame) -> &Packed<StreamFact> {
        let StreamFrame::Facts { packed, .. } = frame else {
            panic!("expected facts, found {frame:?}");
        };
        packed
    }

    fn fact_sequences(delivery: &LiveDelivery) -> Vec<Seq> {
        delivery
            .frames()
            .iter()
            .map(|frame| {
                let StreamFrame::Fact { fact, .. } = frame else {
                    panic!("fact subscription emitted {frame:?}")
                };
                fact.seq()
            })
            .collect()
    }

    #[test]
    fn packed_subscription_continues_exactly_then_becomes_live() {
        let bus = fixture(5);
        let source = Source::new(bus.reader(), epoch(), 2);
        let (mut subscription, frames) = source
            .begin(
                &accepted(Actor::Lane(uuid("018f82d4-6f59-7d06-92e3-bce9131163e5"))),
                &request(0xc4, vec![ViewSubject::Facts], None),
            )
            .expect("subscription begins");

        assert_eq!(
            facts(&frames[0])
                .rows
                .iter()
                .map(StreamFact::seq)
                .collect::<Vec<_>>(),
            [seq(1), seq(2)]
        );
        assert_eq!(
            facts(&frames[0]).overflow.expect("overflow").continuation,
            seq(3)
        );
        assert_eq!(subscription.cursor(), Some(seq(2)));
        assert!(!subscription.is_live());

        let moved = StreamRequest::Continue {
            v: WireVersion::CURRENT,
            id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c5"),
            epoch: epoch(),
            subject: ViewSubject::Facts,
            cursor: StreamCursor {
                epoch: epoch(),
                seq: seq(4),
            },
        };
        assert!(matches!(
            subscription.continue_page(&source, &moved),
            Err(Failure::Refusal(ref refusal))
                if refusal.reason() == "stream::continuation-moved"
        ));
        assert_eq!(subscription.cursor(), Some(seq(2)));

        for (id, cursor) in [(0xc6, 3), (0xc7, 5)] {
            let continuation = StreamRequest::Continue {
                v: WireVersion::CURRENT,
                id: uuid(&format!("018f82d4-6f59-7d06-92e3-bce9131163{id:02x}")),
                epoch: epoch(),
                subject: ViewSubject::Facts,
                cursor: StreamCursor {
                    epoch: epoch(),
                    seq: seq(cursor),
                },
            };
            subscription
                .continue_page(&source, &continuation)
                .expect("exact continuation answers");
        }
        assert!(subscription.is_live());
        assert_eq!(subscription.cursor(), Some(seq(6)));
        drop(source);
        bus.shutdown().expect("bus shuts down");
    }

    #[test]
    fn fresh_operator_fact_subscription_begins_live_without_history() {
        let bus = fixture(5);
        let source = Source::new(bus.reader(), epoch(), 2);
        let (subscription, frames) = source
            .begin(
                &accepted(Actor::Operator),
                &request(0xc4, vec![ViewSubject::Facts], None),
            )
            .expect("operator subscription begins");

        assert!(facts(&frames[0]).rows.is_empty());
        assert!(facts(&frames[0]).overflow.is_none());
        assert_eq!(facts(&frames[0]).horizon, seq(6));
        assert_eq!(subscription.cursor(), Some(seq(6)));
        assert!(subscription.is_live());
        drop(source);
        bus.shutdown().expect("bus shuts down");
    }

    #[test]
    fn fresh_operator_direction_snapshot_restores_retired_identity() {
        let bus = fixture(1);
        let current = commit_operator_direction(&bus);
        cancel_operator_direction(&bus, &current);
        let source = Source::new(bus.reader(), epoch(), 2);
        let (subscription, frames) = source
            .begin(
                &accepted(Actor::Operator),
                &request(0xc4, vec![ViewSubject::Directions], None),
            )
            .expect("direction subscription begins");

        let StreamFrame::Directions {
            limit, directions, ..
        } = &frames[0]
        else {
            panic!("expected direction snapshot")
        };
        assert_eq!(*limit as usize, OPERATOR_DIRECTION_CAPACITY);
        assert_eq!(directions.len(), 1);
        let direction = &directions[0];
        assert_eq!(direction.authored, current.published);
        assert_eq!(direction.directive, current.directive);
        assert_eq!(direction.key, current.key);
        assert_eq!(direction.lane, current.lane);
        assert_eq!(direction.text, current.text);
        assert!(subscription.is_live());
        drop(source);
        bus.shutdown().expect("bus shuts down");
    }

    #[test]
    fn reconnect_is_strictly_after_the_admitted_fact_cursor() {
        let bus = fixture(5);
        let source = Source::new(bus.reader(), epoch(), 10);
        let (subscription, frames) = source
            .begin(
                &accepted(Actor::Lane(uuid("018f82d4-6f59-7d06-92e3-bce9131163e5"))),
                &request(0xc4, vec![ViewSubject::Facts], Some(seq(4))),
            )
            .expect("same-generation resume begins");

        assert_eq!(
            facts(&frames[0])
                .rows
                .iter()
                .map(StreamFact::seq)
                .collect::<Vec<_>>(),
            [seq(5), seq(6)]
        );
        assert_eq!(subscription.cursor(), Some(seq(6)));
        drop(source);
        bus.shutdown().expect("bus shuts down");
    }

    #[test]
    fn operator_reconnect_begins_live_instead_of_draining_missed_history() {
        let bus = fixture(5);
        let source = Source::new(bus.reader(), epoch(), 10);
        let (subscription, frames) = source
            .begin(
                &accepted(Actor::Operator),
                &request(0xc4, vec![ViewSubject::Facts], Some(seq(2))),
            )
            .expect("operator resume begins");

        assert!(facts(&frames[0]).rows.is_empty());
        assert!(facts(&frames[0]).overflow.is_none());
        assert_eq!(facts(&frames[0]).horizon, seq(6));
        assert_eq!(subscription.cursor(), Some(seq(6)));
        assert!(subscription.is_live());
        drop(source);
        bus.shutdown().expect("bus shuts down");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn durable_broadcast_drives_later_updates_without_refresh() {
        let bus = fixture(1);
        let source = Source::new(bus.reader(), epoch(), 10);
        let mut broadcasts = source.subscribe();
        let (mut subscription, _) = source
            .begin(
                &accepted(Actor::Operator),
                &request(0xc4, vec![ViewSubject::Facts], None),
            )
            .expect("subscription begins");

        let committed = commit_notes(&bus, 1, false);
        let noticed = broadcasts.recv().await.expect("commit broadcasts");
        assert_eq!(noticed, committed);
        let delivery = subscription
            .advance(&source, noticed)
            .expect("broadcast advances stream")
            .expect("new fact creates a delivery");
        assert_eq!(delivery.frames().len(), 1);
        let StreamFrame::Fact { fact, .. } = &delivery.frames()[0] else {
            panic!("expected live fact: {delivery:?}");
        };
        assert_eq!(fact.seq(), seq(3));
        assert_eq!(
            subscription.cursor(),
            Some(seq(2)),
            "materialization cannot move the resumable cursor before send"
        );
        subscription.admit(delivery);
        assert_eq!(subscription.cursor(), Some(seq(3)));
        drop(source);
        bus.shutdown().expect("bus shuts down");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn transcript_ingestion_advances_live_cursor_without_crossing_canvas() {
        let bus = fixture(1);
        let source = Source::new(bus.reader(), epoch(), 10);
        let mut broadcasts = source.subscribe();
        let (mut subscription, _) = source
            .begin(
                &accepted(Actor::Operator),
                &request(0xc4, vec![ViewSubject::Facts], None),
            )
            .expect("subscription begins");
        let before = subscription.cursor().expect("operator begins live");

        let committed = commit_ingestion(&bus);
        let noticed = broadcasts.recv().await.expect("commit broadcasts");
        assert_eq!(noticed, committed);
        let delivery = subscription
            .advance(&source, noticed)
            .expect("broadcast advances stream")
            .expect("accounting receipt advances the cursor");
        assert!(
            delivery.frames().is_empty(),
            "resident-only accounting does not cross the Canvas socket"
        );
        assert_eq!(subscription.cursor(), Some(before));
        subscription.admit(delivery);
        assert_eq!(subscription.cursor(), Some(seq(committed.last())));
        drop(source);
        bus.shutdown().expect("bus shuts down");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn one_large_commit_delivers_one_indexed_fact_quantum_per_turn() {
        let bus = fixture(1);
        let source = Source::new(bus.reader(), epoch(), 3);
        let mut broadcasts = source.subscribe();
        let (mut subscription, _) = source
            .begin(
                &accepted(Actor::Operator),
                &request(0xc4, vec![ViewSubject::Facts], None),
            )
            .expect("subscription begins");
        let initial = subscription.cursor().expect("operator begins live");

        let committed = commit_notes(&bus, 11, false);
        let noticed = broadcasts.recv().await.expect("commit broadcasts");
        assert_eq!(noticed, committed);
        let mut delivered = Vec::new();
        let mut first = true;
        loop {
            let before = source.read_counts();
            let delivery = if first {
                first = false;
                subscription
                    .advance(&source, noticed)
                    .expect("notice advances")
                    .expect("notice creates delivery")
            } else {
                subscription
                    .continue_live(&source)
                    .expect("continuation advances")
                    .expect("catch-up retains a delivery")
            };
            let after = source.read_counts();
            assert_eq!(after.1 - before.1, 1, "one fact-page query per turn");
            assert_eq!(
                after.0 - before.0,
                usize::from(delivered.is_empty()),
                "only the notification captures one head barrier"
            );
            assert!(delivery.frames().len() <= 3);
            delivered.extend(fact_sequences(&delivery));
            if delivered.len() == 3 {
                assert_eq!(
                    subscription.cursor(),
                    Some(initial),
                    "materialized frames are not yet resumable"
                );
            }
            subscription.admit(delivery);
            if !subscription.has_continuation() {
                break;
            }
        }

        assert_eq!(
            delivered,
            (initial.get() + 1..=committed.last())
                .map(seq)
                .collect::<Vec<_>>()
        );
        assert_eq!(subscription.cursor(), Some(seq(committed.last())));
        drop(source);
        bus.shutdown().expect("bus shuts down");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn queued_commits_coalesce_to_the_newest_barrier_without_duplicates() {
        let bus = fixture(1);
        let source = Source::new(bus.reader(), epoch(), 2);
        let mut broadcasts = source.subscribe();
        let (mut subscription, _) = source
            .begin(
                &accepted(Actor::Operator),
                &request(0xc4, vec![ViewSubject::Facts], None),
            )
            .expect("subscription begins");
        let initial = subscription.cursor().expect("operator begins live");

        let first = commit_notes(&bus, 5, false);
        let second = commit_notes(&bus, 4, false);
        let first_notice = broadcasts.recv().await.expect("first commit broadcasts");
        let second_notice = broadcasts.recv().await.expect("second commit broadcasts");
        assert_eq!((first_notice, second_notice), (first, second));

        let before = source.read_counts();
        let first_delivery = subscription
            .advance(&source, first_notice)
            .expect("first notice advances")
            .expect("first notice creates delivery");
        let after = source.read_counts();
        assert_eq!((after.0 - before.0, after.1 - before.1), (1, 1));
        let mut delivered = fact_sequences(&first_delivery);
        subscription.admit(first_delivery);

        let before = source.read_counts();
        let second_delivery = subscription
            .advance(&source, second_notice)
            .expect("queued notice advances")
            .expect("queued notice creates one delivery");
        let after = source.read_counts();
        assert_eq!(
            (after.0 - before.0, after.1 - before.1),
            (0, 1),
            "the queued notice reuses the already coalesced newest head"
        );
        delivered.extend(fact_sequences(&second_delivery));
        subscription.admit(second_delivery);

        while subscription.has_continuation() {
            let delivery = subscription
                .continue_live(&source)
                .expect("continuation advances")
                .expect("catch-up retains a delivery");
            delivered.extend(fact_sequences(&delivery));
            subscription.admit(delivery);
        }
        assert_eq!(
            delivered,
            (initial.get() + 1..=second.last())
                .map(seq)
                .collect::<Vec<_>>()
        );
        assert_eq!(subscription.cursor(), Some(seq(second.last())));
        drop(source);
        bus.shutdown().expect("bus shuts down");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn broadcast_lag_during_catch_up_resumes_after_the_last_admitted_quantum() {
        let bus = fixture(1);
        let source = Source::new(bus.reader(), epoch(), 3);
        let mut broadcasts = source.subscribe();
        let (mut subscription, _) = source
            .begin(
                &accepted(Actor::Operator),
                &request(0xc4, vec![ViewSubject::Facts], None),
            )
            .expect("subscription begins");
        let initial = subscription.cursor().expect("operator begins live");

        let large = commit_notes(&bus, 11, false);
        let notice = broadcasts.recv().await.expect("large commit broadcasts");
        assert_eq!(notice, large);
        let delivery = subscription
            .advance(&source, notice)
            .expect("notice advances")
            .expect("large commit creates delivery");
        subscription.admit(delivery);
        let admitted = seq(initial.get() + 3);
        assert_eq!(subscription.cursor(), Some(admitted));
        assert!(subscription.has_continuation());

        for _ in 0..70 {
            commit_notes(&bus, 1, false);
        }
        let tokio::sync::broadcast::error::RecvError::Lagged(dropped) = broadcasts
            .recv()
            .await
            .expect_err("the retained notice channel overflows")
        else {
            panic!("the retained broadcast closed instead of lagging")
        };
        let overflow = subscription.overflow(dropped).expect("lag is resumable");
        assert!(matches!(
            overflow,
            StreamFrame::Overflow { after, dropped: observed, .. }
                if after.seq == admitted && observed == dropped
        ));
        drop(source);
        bus.shutdown().expect("bus shuts down");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn live_trust_overflow_retains_its_snapshot_for_exact_continuation() {
        let bus = fixture(1);
        let source = Source::new(bus.reader(), epoch(), 2);
        let mut broadcasts = source.subscribe();
        let (mut subscription, _) = source
            .begin(
                &accepted(Actor::Operator),
                &request(0xc4, vec![ViewSubject::Trust], None),
            )
            .expect("subscription begins");
        assert!(subscription.is_live());

        let committed = commit_trust_heads(&bus, &[b"second", b"third"]);
        let noticed = broadcasts.recv().await.expect("commit broadcasts");
        assert_eq!(noticed, committed);
        let delivery = subscription
            .advance(&source, noticed)
            .expect("broadcast advances stream")
            .expect("new trust heads create a delivery");
        let StreamFrame::Trust { packed, .. } = &delivery.frames()[0] else {
            panic!("expected trust replacement: {delivery:?}");
        };
        let continuation = packed.overflow.expect("trust page overflows").continuation;
        let first_positions = packed
            .rows
            .iter()
            .map(|row| row.position)
            .collect::<Vec<_>>();
        assert_eq!(packed.rows.len(), 2);
        subscription.admit(delivery);
        assert!(!subscription.is_live());

        let request = StreamRequest::Continue {
            v: WireVersion::CURRENT,
            id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c5"),
            epoch: epoch(),
            subject: ViewSubject::Trust,
            cursor: StreamCursor {
                epoch: epoch(),
                seq: continuation,
            },
        };
        let frame = subscription
            .continue_page(&source, &request)
            .expect("exact trust continuation answers");
        let StreamFrame::Trust { packed, .. } = frame else {
            panic!("expected trust continuation");
        };
        assert_eq!(packed.rows.len(), 1);
        assert!(packed.overflow.is_none());
        let positions = first_positions
            .into_iter()
            .chain(packed.rows.iter().map(|row| row.position))
            .collect::<Vec<_>>();
        assert_eq!(positions.len(), 3);
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(subscription.is_live());
        assert_eq!(subscription.cursor(), Some(seq(committed.last())));
        drop(source);
        bus.shutdown().expect("bus shuts down");
    }

    #[test]
    fn lane_principal_never_admits_operator_only_subjects() {
        let bus = fixture(1);
        let source = Source::new(bus.reader(), epoch(), 10);
        let lane = uuid("018f82d4-6f59-7d06-92e3-bce9131163c2");
        for subject in [
            ViewSubject::Directions,
            ViewSubject::Trust,
            ViewSubject::Messages,
        ] {
            let failure = source
                .begin(
                    &accepted(Actor::Lane(lane)),
                    &request(0xc4, vec![subject], None),
                )
                .expect_err("lane operator-only view refuses");

            assert!(matches!(
                failure,
                Failure::Refusal(ref refusal)
                    if refusal.reason() == "stream::subject-authority"
            ));
        }
        drop(source);
        bus.shutdown().expect("bus shuts down");
    }

    #[test]
    fn thread_request_walks_one_indexed_barrier_and_names_continuation() {
        let bus = fixture(5);
        let source = Source::new(bus.reader(), epoch(), 2);
        let request = StreamRequest::Thread {
            v: WireVersion::CURRENT,
            id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c4"),
            epoch: epoch(),
            cursor: StreamCursor {
                epoch: epoch(),
                seq: seq(5),
            },
        };
        let frame = source
            .thread(&accepted(Actor::Operator), &request)
            .expect("thread answers");
        let StreamFrame::Thread { packed, .. } = frame else {
            panic!("expected thread frame");
        };

        assert_eq!(
            packed.rows.iter().map(StreamFact::seq).collect::<Vec<_>>(),
            [seq(5), seq(4)]
        );
        assert_eq!(packed.overflow.expect("overflow").continuation, seq(3));
        drop(source);
        bus.shutdown().expect("bus shuts down");
    }
}
