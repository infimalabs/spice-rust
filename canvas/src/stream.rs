//! Bus-backed authenticated canvas subscription state.

use std::collections::{BTreeMap, BTreeSet};

use bus::{BusReader, CanvasSnapshot, JournalCommit};
use wire::{
    Accepted, Actor, Fault, FaultClass, Overflow, Packed, Refusal, Seq, StreamCursor, StreamFrame,
    StreamRequest, TrustView, ViewSubject, WireVersion,
};

use crate::thread::{ThreadError, ThreadIndex};

/// Generation-owned source for every streamed canvas answer.
#[derive(Clone, Debug)]
pub struct Source {
    bus: BusReader,
    epoch: wire::CanonicalUuid,
    rows: usize,
}

impl Source {
    /// Bind canvas reads to one resident Bus generation and one packing bound.
    #[must_use]
    pub const fn new(bus: BusReader, epoch: wire::CanonicalUuid, rows: usize) -> Self {
        Self { bus, epoch, rows }
    }

    /// Reader for the one bounded post-durability Bus cursor broadcast.
    #[must_use]
    pub fn subscribe(&self) -> bus::BroadcastSubscriber {
        self.bus.subscribe()
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
        let snapshot = self.snapshot()?;
        let horizon = required_horizon(&snapshot)?;
        let after_seq = after.map(|cursor| cursor.seq);
        if after_seq.is_some_and(|cursor| cursor > horizon) {
            return Err(Failure::Refusal(refusal(
                "stream::cursor-ahead",
                &format!("resume cursor {after_seq:?} is beyond snapshot {horizon}"),
            )));
        }

        let mut subscription = Subscription {
            id: *id,
            epoch: self.epoch,
            subjects,
            cursor: after_seq,
            pending: BTreeMap::new(),
            barrier: Some(snapshot),
            rows: self.rows,
        };
        let mut frames = Vec::new();
        for subject in subscription.subjects.iter().copied().collect::<Vec<_>>() {
            let frame = subscription.page(subject, None, *id)?;
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
        let snapshot = self.snapshot()?;
        let index = ThreadIndex::from_snapshot(snapshot.facts()).map_err(thread_fault)?;
        let packed = index
            .walk(cursor.seq, self.rows)
            .map_err(|error| match error {
                ThreadError::UnknownPosition(position) => Failure::Refusal(refusal(
                    "stream::cursor-unknown",
                    &format!("causal position {position} is absent at this barrier"),
                )),
                other => thread_fault(other),
            })?;
        Ok(StreamFrame::Thread {
            v: WireVersion::CURRENT,
            id: *id,
            epoch: self.epoch,
            packed,
        })
    }

    fn snapshot(&self) -> Result<CanvasSnapshot, Failure> {
        self.bus.canvas_snapshot().map_err(|error| {
            Failure::Fault(Fault {
                class: FaultClass::Store,
                reason: "stream::snapshot-failed".to_owned(),
                detail: error.to_string(),
            })
        })
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
    barrier: Option<CanvasSnapshot>,
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
    pub fn continue_page(&mut self, request: &StreamRequest) -> Result<StreamFrame, Failure> {
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
            return Err(Failure::Refusal(Refusal {
                reason: "stream::continuation-moved".to_owned(),
                witness: Some("cursor".to_owned()),
                expected: Some(expected.to_string()),
                observed: Some(cursor.seq.to_string()),
                remedy: "continue from the exact cursor in the preceding overflow".to_owned(),
            }));
        }
        let frame = self.page(*subject, Some(cursor.seq), *id)?;
        let horizon = required_horizon(
            self.barrier
                .as_ref()
                .expect("pending continuations retain their barrier"),
        )?;
        self.settle_barrier(horizon);
        Ok(frame)
    }

    /// Derive later fact and trust updates after a durable Bus notification.
    pub fn advance(
        &mut self,
        source: &Source,
        commit: JournalCommit,
    ) -> Result<Vec<StreamFrame>, Failure> {
        if !self.is_live()
            || self
                .cursor
                .is_some_and(|cursor| commit.last() <= cursor.get())
        {
            return Ok(Vec::new());
        }
        let snapshot = source.snapshot()?;
        let horizon = required_horizon(&snapshot)?;
        let after = self.cursor.map_or(0, Seq::get);
        let mut frames = Vec::new();
        if self.subjects.contains(&ViewSubject::Facts) {
            frames.extend(
                snapshot
                    .facts()
                    .facts()
                    .iter()
                    .filter(|fact| fact.seq.get() > after)
                    .cloned()
                    .map(|fact| StreamFrame::Fact {
                        v: WireVersion::CURRENT,
                        id: self.id,
                        epoch: self.epoch,
                        fact,
                    }),
            );
        }
        if self.subjects.contains(&ViewSubject::Trust) {
            let trust = trust_rows(&snapshot);
            let packed = pack_unique(trust, self.rows, horizon, |row| row.position)?;
            self.record_page(ViewSubject::Trust, &packed, |row| row.position);
            frames.push(StreamFrame::Trust {
                v: WireVersion::CURRENT,
                id: self.id,
                epoch: self.epoch,
                packed,
            });
        }
        self.cursor = Some(horizon);
        if !self.pending.is_empty() {
            self.barrier = Some(snapshot);
        }
        Ok(frames)
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
        subject: ViewSubject,
        continuation: Option<Seq>,
        request_id: wire::CanonicalUuid,
    ) -> Result<StreamFrame, Failure> {
        let snapshot = self.barrier.as_ref().ok_or_else(|| {
            Failure::Refusal(refusal(
                "stream::continuation-stale",
                "the subscription has already crossed into live delivery",
            ))
        })?;
        let horizon = required_horizon(snapshot)?;
        let frame = match subject {
            ViewSubject::Facts => {
                let start = continuation.or(self.cursor);
                let facts = snapshot
                    .facts()
                    .facts()
                    .iter()
                    .filter(|fact| start.is_none_or(|cursor| fact.seq >= cursor))
                    .filter(|fact| {
                        continuation.is_some() || self.cursor.is_none_or(|cursor| fact.seq > cursor)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let packed = pack_unique(facts, self.rows, horizon, |fact| fact.seq)?;
                self.record_page(subject, &packed, |fact| fact.seq);
                StreamFrame::Facts {
                    v: WireVersion::CURRENT,
                    id: request_id,
                    epoch: self.epoch,
                    packed,
                }
            }
            ViewSubject::Trust => {
                let rows = trust_rows(snapshot)
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

/// Typed refusal or fault returned before a stream frame changes client state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Failure {
    Refusal(Refusal),
    Fault(Fault),
}

impl Failure {
    fn admission(error: wire::StreamAdmissionError) -> Self {
        match error {
            wire::StreamAdmissionError::Wire(refusal) => Self::Refusal(refusal),
            wire::StreamAdmissionError::Epoch(fault)
            | wire::StreamAdmissionError::Cursor(fault) => Self::Fault(fault),
        }
    }

    /// Encode this failure as one current stream response.
    #[must_use]
    pub fn frame(self, epoch: wire::CanonicalUuid, id: Option<wire::CanonicalUuid>) -> StreamFrame {
        match self {
            Self::Refusal(refusal) => StreamFrame::Refusal {
                v: WireVersion::CURRENT,
                id: id.expect("decoded stream refusals retain their request id"),
                epoch,
                refusal,
            },
            Self::Fault(fault) => StreamFrame::Fault {
                v: WireVersion::CURRENT,
                epoch,
                id,
                fault,
            },
        }
    }
}

fn admit_subjects(
    actor: &Actor,
    subjects: &[ViewSubject],
) -> Result<BTreeSet<ViewSubject>, Failure> {
    if subjects.is_empty() {
        return Err(Failure::Refusal(refusal(
            "stream::subjects-empty",
            "subscribe to at least one visible subject",
        )));
    }
    let mut admitted = BTreeSet::new();
    for subject in subjects {
        admit_subject(actor, *subject)?;
        if !admitted.insert(*subject) {
            return Err(Failure::Refusal(refusal(
                "stream::subject-duplicate",
                "each view subject may appear once",
            )));
        }
    }
    Ok(admitted)
}

fn admit_subject(actor: &Actor, subject: ViewSubject) -> Result<(), Failure> {
    if subject == ViewSubject::Trust && !matches!(actor, Actor::Operator) {
        return Err(Failure::Refusal(Refusal {
            reason: "stream::subject-authority".to_owned(),
            witness: Some("principal".to_owned()),
            expected: Some("operator".to_owned()),
            observed: Some(actor.to_string()),
            remedy: "subscribe only to subjects visible to this authenticated principal".to_owned(),
        }));
    }
    Ok(())
}

fn required_horizon(snapshot: &CanvasSnapshot) -> Result<Seq, Failure> {
    snapshot.cursor().ok_or_else(|| {
        Failure::Fault(protocol_fault(
            "stream::cursor-missing",
            "resident authority has no durable stream cursor",
        ))
    })
}

fn trust_rows(snapshot: &CanvasSnapshot) -> Vec<TrustView> {
    let mut rows = snapshot
        .trust()
        .iter()
        .map(|head| TrustView {
            position: head.position(),
            head: head.body().clone(),
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        left.position
            .cmp(&right.position)
            .then_with(|| left.head.digest.cmp(&right.head.digest))
    });
    rows
}

fn pack_unique<T>(
    rows: Vec<T>,
    limit: usize,
    horizon: Seq,
    position: impl Fn(&T) -> Seq,
) -> Result<Packed<T>, Failure> {
    if rows.len() <= limit {
        return Ok(Packed {
            rows,
            overflow: None,
            horizon,
        });
    }
    let mut cut = limit;
    while cut > 0 && position(&rows[cut - 1]) == position(&rows[cut]) {
        cut -= 1;
    }
    if cut == 0 {
        return Err(Failure::Fault(protocol_fault(
            "stream::cursor-ambiguous",
            "one packed cursor group exceeds the configured row bound",
        )));
    }
    let omitted = u64::try_from(rows.len() - cut).expect("a vector length fits in u64");
    let continuation = position(&rows[cut]);
    let mut rows = rows;
    rows.truncate(cut);
    Ok(Packed {
        rows,
        overflow: Some(Overflow {
            omitted,
            continuation,
        }),
        horizon,
    })
}

fn refusal(reason: &str, detail: &str) -> Refusal {
    Refusal {
        reason: reason.to_owned(),
        witness: None,
        expected: None,
        observed: Some(detail.to_owned()),
        remedy: "use the exact current stream contract and retry".to_owned(),
    }
}

fn protocol_fault(reason: &str, detail: &str) -> Fault {
    Fault {
        class: FaultClass::Protocol,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    }
}

fn thread_fault(error: ThreadError) -> Failure {
    Failure::Fault(Fault {
        class: FaultClass::Store,
        reason: "stream::thread-invalid".to_owned(),
        detail: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::{Bus, CommitOutcome};
    use journal::{FactCause, Journal, JournalWrite};
    use wire::{Domain, Producer, TrustHead, TrustState, body};

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

    fn commit_notes(bus: &Bus, notes: usize, trust: bool) -> JournalCommit {
        let mut write = JournalWrite::new(
            Actor::Operator,
            "task:STREAM-a1".parse().expect("origin"),
            "2030-04-12T18:44:09.271Z".parse().expect("instant"),
            Producer::CommandDoor,
        );
        let task: wire::TaskHandle = "STREAM-a1".parse().expect("task handle");
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
                        previous: None,
                    },
                    FactCause::Root,
                )
                .expect("trust fact builds");
            write.trust_head(fact, move |_| TrustHead {
                digest,
                state: TrustState::Pended,
            });
        }
        match bus.commit(write).expect("commit answers") {
            CommitOutcome::Committed(commit) => commit,
            other => panic!("fixture commit failed: {other:?}"),
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
                        previous: None,
                    },
                    FactCause::Root,
                )
                .expect("trust fact builds");
            write.trust_head(fact, move |_| TrustHead {
                digest,
                state: TrustState::Pended,
            });
        }
        match bus.commit(write).expect("commit answers") {
            CommitOutcome::Committed(commit) => commit,
            other => panic!("fixture commit failed: {other:?}"),
        }
    }

    fn facts(frame: &StreamFrame) -> &Packed<wire::Envelope<wire::AnyFact>> {
        let StreamFrame::Facts { packed, .. } = frame else {
            panic!("expected facts, found {frame:?}");
        };
        packed
    }

    #[test]
    fn packed_subscription_continues_exactly_then_becomes_live() {
        let bus = fixture(5);
        let source = Source::new(bus.reader(), epoch(), 2);
        let (mut subscription, frames) = source
            .begin(
                &accepted(Actor::Operator),
                &request(0xc4, vec![ViewSubject::Facts], None),
            )
            .expect("subscription begins");

        assert_eq!(
            facts(&frames[0])
                .rows
                .iter()
                .map(|fact| fact.seq)
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
            subscription.continue_page(&moved),
            Err(Failure::Refusal(Refusal { ref reason, .. }))
                if reason == "stream::continuation-moved"
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
                .continue_page(&continuation)
                .expect("exact continuation answers");
        }
        assert!(subscription.is_live());
        assert_eq!(subscription.cursor(), Some(seq(6)));
        drop(source);
        bus.shutdown().expect("bus shuts down");
    }

    #[test]
    fn reconnect_is_strictly_after_the_admitted_fact_cursor() {
        let bus = fixture(5);
        let source = Source::new(bus.reader(), epoch(), 10);
        let (subscription, frames) = source
            .begin(
                &accepted(Actor::Operator),
                &request(0xc4, vec![ViewSubject::Facts], Some(seq(4))),
            )
            .expect("same-generation resume begins");

        assert_eq!(
            facts(&frames[0])
                .rows
                .iter()
                .map(|fact| fact.seq)
                .collect::<Vec<_>>(),
            [seq(5), seq(6)]
        );
        assert_eq!(subscription.cursor(), Some(seq(6)));
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
        let frames = subscription
            .advance(&source, noticed)
            .expect("broadcast advances stream");
        assert_eq!(frames.len(), 1);
        let StreamFrame::Fact { fact, .. } = &frames[0] else {
            panic!("expected live fact: {frames:?}");
        };
        assert_eq!(fact.seq, seq(3));
        assert_eq!(subscription.cursor(), Some(seq(3)));
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
        let frames = subscription
            .advance(&source, noticed)
            .expect("broadcast advances stream");
        let StreamFrame::Trust { packed, .. } = &frames[0] else {
            panic!("expected trust replacement: {frames:?}");
        };
        let continuation = packed.overflow.expect("trust page overflows").continuation;
        let first_positions = packed
            .rows
            .iter()
            .map(|row| row.position)
            .collect::<Vec<_>>();
        assert_eq!(packed.rows.len(), 2);
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
            .continue_page(&request)
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
    fn lane_principal_never_admits_operator_trust_subject() {
        let bus = fixture(1);
        let source = Source::new(bus.reader(), epoch(), 10);
        let lane = uuid("018f82d4-6f59-7d06-92e3-bce9131163c2");
        let failure = source
            .begin(
                &accepted(Actor::Lane(lane)),
                &request(0xc4, vec![ViewSubject::Trust], None),
            )
            .expect_err("lane trust view refuses");

        assert!(matches!(
            failure,
            Failure::Refusal(Refusal { ref reason, .. })
                if reason == "stream::subject-authority"
        ));
        drop(source);
        bus.shutdown().expect("bus shuts down");
    }

    #[test]
    fn thread_request_walks_the_same_snapshot_and_names_continuation() {
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
            packed.rows.iter().map(|fact| fact.seq).collect::<Vec<_>>(),
            [seq(5), seq(4)]
        );
        assert_eq!(packed.overflow.expect("overflow").continuation, seq(3));
        drop(source);
        bus.shutdown().expect("bus shuts down");
    }
}
