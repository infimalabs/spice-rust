//! Room conversation authority: lane deliveries bound to native steering
//! directives, operator items, and correlated ASK results admitted, raced,
//! lapsed, recovered, bounded, and rebuilt from facts.

use wire::body::{
    SteerAcked, SteerDeclined, SteerLapsed, SteerMessaged, SteerSent, SteeringStage,
    TranscriptLocus,
};
use wire::rooms::{
    QuestionClosure, QuestionResultRecord, QuestionResultRef, QuestionResultState,
    ROOM_PENDING_RESULT_LIMIT, RoomConversed, RoomDelivery, RoomDestination, RoomDisposed,
    RoomPosted, RoomPublicationKind, RoomQuestionClosed, RoomQuestionTimedOut,
    RoomReceiptDisposition, RoomReceiptLapsed, RoomReceiptRecord, RoomReceiptRecovered,
    RoomReceiptRef, RoomReceiptState, RoomRoute,
};
use wire::{Disposition, SteeringHead};

use super::*;

fn minute(minute: u32) -> Instant {
    format!("2030-04-12T19:{minute:02}:00.000Z")
        .parse()
        .expect("fixture instant")
}

fn origin() -> wire::Origin {
    "task:rooms".parse().expect("room origin")
}

fn directive(micros: i64) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-{:012x}", 0x1000 + micros)
        .parse()
        .expect("fixture directive")
}

/// A native key for a delivery that answers to no result.
fn message_key(micros: i64) -> String {
    authority(500_000 + micros).to_string()
}

fn item(micros: i64) -> RoomReceiptRef {
    RoomReceiptRef::new(authority(micros))
}

fn lane(head: &TranscriptSourceHead) -> RoomDestination {
    RoomDestination::Lane { lane: head.lane }
}

fn route(destination: RoomDestination, context: Option<&TaskStamp>) -> RoomRoute {
    RoomRoute {
        destination,
        context: context.cloned(),
        direction: None,
    }
}

fn record_locus(head: &TranscriptSourceHead) -> TranscriptLocus {
    TranscriptLocus {
        source: Some(head.source),
        member: 1,
        offset: 0,
        length: 64,
        event: 0,
        line: 1,
        column: 1,
    }
}

/// The peer message `author` tells `to` at the record of its own `hunk`.
fn told(
    author: &TranscriptSourceHead,
    directive: CanonicalUuid,
    key: String,
    to: CanonicalUuid,
    room: Option<&TaskStamp>,
) -> SteerMessaged {
    SteerMessaged {
        directive,
        key,
        lane: to,
        occupant: uuid("018f82d4-6f59-7d06-92e3-bce913116308"),
        from: author.lane,
        sender: uuid("018f82d4-6f59-7d06-92e3-bce913116309"),
        text: "room delivery".to_owned(),
        locus: record_locus(author),
        room: room.cloned(),
        expires: minute(25),
    }
}

fn conversed(
    micros: i64,
    author: &TranscriptSourceHead,
    operation: RoomPublicationKind,
    routes: Vec<RoomRoute>,
    directed: Vec<CanonicalUuid>,
) -> RoomConversed {
    RoomConversed {
        publication: authority(micros),
        hunk: hunk(author),
        operation,
        routes,
        directed,
        expires: minute(20),
    }
}

fn ask_of(result: &QuestionResultRef) -> RoomPublicationKind {
    RoomPublicationKind::Ask {
        result: result.clone(),
        deadline: minute(30),
    }
}

fn answer_to(result: &QuestionResultRef) -> RoomPublicationKind {
    RoomPublicationKind::Ans {
        result: result.clone(),
        context: None,
    }
}

/// One conversation write: the native directives it opens or retires and
/// the room records it admits.
struct Write {
    write: JournalWrite,
    at: Instant,
}

impl Write {
    fn new(actor: Actor, producer: Producer, at: Instant) -> Self {
        Self {
            write: JournalWrite::new(actor, origin(), at, producer),
            at,
        }
    }

    fn lane(author: &TranscriptSourceHead, at: Instant) -> Self {
        Self::new(Actor::Lane(author.lane), Producer::AckScanner, at)
    }

    fn operator(at: Instant) -> Self {
        Self::new(Actor::Operator, Producer::CommandDoor, at)
    }

    fn tell(mut self, message: SteerMessaged) -> Self {
        let created = self
            .write
            .fact(message.clone(), FactCause::Root)
            .expect("peer message builds");
        let at = self.at;
        self.write
            .steering_head(created, move |created| SteeringHead {
                directive: message.directive,
                fanout: None,
                key: message.key,
                lane: message.lane,
                text: message.text,
                from: Some(message.from),
                room: message.room,
                control: None,
                attachments: Vec::new(),
                published: at,
                expires: message.expires,
                stage: SteeringStage::Published,
                escalations: Vec::new(),
                disposition: Disposition::Live,
                origin: origin(),
                created,
                minted_micros: None,
            });
        self
    }

    /// Operator direction to `to` under `key`.
    fn direct(
        self,
        directive: CanonicalUuid,
        key: String,
        to: CanonicalUuid,
        room: Option<&TaskStamp>,
    ) -> Self {
        self.send(SteerSent {
            directive,
            fanout: None,
            key,
            lane: to,
            text: "room delivery".to_owned(),
            from: None,
            room: room.cloned(),
            control: None,
            attachments: Vec::new(),
            expires: minute(25),
        })
    }

    fn send(mut self, sent: SteerSent) -> Self {
        let created = self
            .write
            .fact(sent.clone(), FactCause::Root)
            .expect("operator direction builds");
        let at = self.at;
        self.write
            .steering_head(created, move |created| SteeringHead {
                directive: sent.directive,
                fanout: sent.fanout,
                key: sent.key,
                lane: sent.lane,
                text: sent.text,
                from: None,
                room: sent.room,
                control: None,
                attachments: Vec::new(),
                published: at,
                expires: sent.expires,
                stage: SteeringStage::Published,
                escalations: Vec::new(),
                disposition: Disposition::Live,
                origin: origin(),
                created,
                minted_micros: None,
            });
        self
    }

    /// Retire `observed` natively with `disposition`, guarded, as every
    /// retiring producer is, on its observed live standing.
    fn retire(mut self, observed: &JournalHead<SteeringHead>, disposition: Disposition) -> Self {
        let expected = observed.clone();
        self.write.guard(crate::JournalGuard::new(move |read| {
            let current = read
                .steering_head(expected.body().directive)?
                .ok_or_else(|| crate::GuardError::refused("directive disappeared"))?;
            if current.position() != expected.position()
                || current.body().disposition != Disposition::Live
            {
                return Err(crate::GuardError::refused("directive standing moved"));
            }
            Ok(())
        }));
        let head = observed.body();
        let (directive, key, lane) = (head.directive, head.key.clone(), head.lane);
        let cause = FactCause::Existing(observed.position());
        let epoch = uuid("018f82d4-6f59-7d06-92e3-bce913116305");
        let locus = TranscriptLocus {
            source: Some(uuid("018f82d4-6f59-7d06-92e3-bce913116306")),
            member: 1,
            offset: 0,
            length: 1,
            event: 0,
            line: 1,
            column: 1,
        };
        let pending = match disposition {
            Disposition::Acked => self.write.fact(
                SteerAcked {
                    directive,
                    key,
                    lane,
                    epoch,
                    locus,
                },
                cause,
            ),
            Disposition::Declined => self.write.fact(
                SteerDeclined {
                    directive,
                    key,
                    lane,
                    epoch,
                    locus,
                    reason: "cannot".to_owned(),
                },
                cause,
            ),
            Disposition::Lapsed => self.write.fact(
                SteerLapsed {
                    key,
                    lane,
                    expires: head.expires,
                },
                cause,
            ),
            Disposition::DeadLettered => self.write.fact(
                wire::steering::SteerDeadLettered {
                    directive,
                    key,
                    lane,
                    reason: "vacant".to_owned(),
                    requeue: "spice steer recover".to_owned(),
                },
                cause,
            ),
            Disposition::Canceled | Disposition::Live => {
                panic!("these fixtures retire by ACK, NAK, lapse, or dead letter")
            }
        }
        .expect("retirement builds");
        let mut next = head.clone();
        next.disposition = disposition;
        self.write.steering_head(pending, move |_| next);
        self
    }

    fn record<F: wire::Body>(mut self, body: F) -> Self {
        self.write
            .fact(body, FactCause::Root)
            .expect("room fact builds");
        self
    }

    fn apply(self, journal: &mut Journal) -> Commit {
        journal.apply(self.write)
    }
}

fn land<F: wire::Body>(
    journal: &mut Journal,
    actor: Actor,
    producer: Producer,
    at: Instant,
    body: F,
) -> Commit {
    Write::new(actor, producer, at).record(body).apply(journal)
}

/// One room task and two lanes with accounted transcript text.
struct Conversation {
    journal: Journal,
    room: TaskStamp,
    asked: TranscriptSourceHead,
    other: TranscriptSourceHead,
}

impl Conversation {
    fn new() -> Self {
        let mut journal = Journal::in_memory().expect("journal");
        let room = task(&mut journal, 1, None);
        let origin = TranscriptPosition {
            member: 1,
            offset: 0,
        };
        let asked = transcript_source_head();
        ingest(&mut journal, asked.clone(), origin, None);
        let mut other = asked.clone();
        other.lane = uuid("018f82d4-6f59-7d06-92e3-bce913116306");
        ingest(&mut journal, other.clone(), origin, None);
        Self {
            journal,
            room,
            asked,
            other,
        }
    }

    /// The operator's ASK `micros` of the asked lane at minute 1, delivered
    /// as directive `micros` keyed by its result, deadline at minute 30.
    fn ask(&mut self, micros: i64) -> QuestionResultRef {
        let result = QuestionResultRef::new(authority(micros + 1));
        self.post_ask(micros, &result)
            .expect("the operator's ASK admits");
        result
    }

    fn post_ask(&mut self, micros: i64, result: &QuestionResultRef) -> Commit {
        Write::operator(minute(1))
            .direct(
                directive(micros),
                result.to_string(),
                self.asked.lane,
                Some(&self.room),
            )
            .record(RoomPosted {
                publication: authority(micros),
                text: "which one?".to_owned(),
                operation: ask_of(result),
                routes: vec![route(lane(&self.asked), Some(&self.room))],
                directed: vec![directive(micros)],
                expires: minute(20),
            })
            .apply(&mut self.journal)
    }

    /// The other lane's ASK `micros` of the operator, an operator item keyed
    /// by its result.
    fn ask_operator(&mut self, micros: i64) -> QuestionResultRef {
        let result = QuestionResultRef::new(authority(micros + 1));
        let other = self.other.clone();
        Write::lane(&other, minute(1))
            .record(conversed(
                micros,
                &other,
                ask_of(&result),
                vec![route(RoomDestination::Operator {}, Some(&self.room))],
                Vec::new(),
            ))
            .apply(&mut self.journal)
            .expect("a lane asks the operator");
        result
    }

    /// The other lane's ASK `micros` of the asked lane, told as directive
    /// `micros` keyed by its result.
    fn ask_lane(&mut self, micros: i64) -> QuestionResultRef {
        let result = QuestionResultRef::new(authority(micros + 1));
        let (other, asked) = (self.other.clone(), self.asked.clone());
        Write::lane(&other, minute(1))
            .tell(told(
                &other,
                directive(micros),
                result.to_string(),
                asked.lane,
                Some(&self.room),
            ))
            .record(conversed(
                micros,
                &other,
                ask_of(&result),
                vec![route(lane(&asked), Some(&self.room))],
                vec![directive(micros)],
            ))
            .apply(&mut self.journal)
            .expect("a lane asks another lane");
        result
    }

    /// The live native directive delivering `result`, if any.
    fn live_ask(&self, result: &QuestionResultRef) -> Option<JournalHead<SteeringHead>> {
        let (record, _, _) = self.result(result);
        let micros = (0..1_000)
            .find(|micros| authority(*micros) == *record.question())
            .expect("fixture ASK stamp");
        self.journal
            .read(|read| read.steering_head(directive(micros)))
            .expect("steering head reads")
            .filter(|head| head.body().disposition == Disposition::Live)
    }

    /// `author`'s ANS `micros` to `result`: it acknowledges a live ASK
    /// directive and tells a lane requester its return as directive `micros`.
    fn answer(
        &mut self,
        micros: i64,
        author: &TranscriptSourceHead,
        result: &QuestionResultRef,
        at: Instant,
    ) -> Commit {
        let (record, _, _) = self.result(result);
        let mut write = Write::lane(author, at);
        if let Some(live) = self.live_ask(result) {
            write = write.retire(&live, Disposition::Acked);
        }
        let mut directed = Vec::new();
        if let RoomDestination::Lane { lane } = record.requester() {
            write = write.tell(told(
                author,
                directive(micros),
                message_key(micros),
                *lane,
                record.context(),
            ));
            directed.push(directive(micros));
        }
        write
            .record(RoomConversed {
                expires: minute(40),
                ..conversed(micros, author, answer_to(result), Vec::new(), directed)
            })
            .apply(&mut self.journal)
    }

    /// The operator's ANS `micros`, directing a lane requester its return.
    fn post_answer(&mut self, micros: i64, result: &QuestionResultRef, at: Instant) -> Commit {
        let (record, _, _) = self.result(result);
        let mut write = Write::operator(at);
        let mut directed = Vec::new();
        if let RoomDestination::Lane { lane } = record.requester() {
            write = write.direct(
                directive(micros),
                message_key(micros),
                *lane,
                record.context(),
            );
            directed.push(directive(micros));
        }
        write
            .record(RoomPosted {
                publication: authority(micros),
                text: "this one".to_owned(),
                operation: answer_to(result),
                routes: Vec::new(),
                directed,
                expires: minute(40),
            })
            .apply(&mut self.journal)
    }

    /// The respondent lane's native NAK of `result`'s live ASK directive.
    fn nak(&mut self, result: &QuestionResultRef, at: Instant) -> Commit {
        let live = self.directive_head(self.ask_directive(result));
        Write::new(Actor::Lane(live.body().lane), Producer::AckScanner, at)
            .retire(&live, Disposition::Declined)
            .apply(&mut self.journal)
    }

    fn ask_directive(&self, result: &QuestionResultRef) -> CanonicalUuid {
        let (record, _, _) = self.result(result);
        directive(
            (0..1_000)
                .find(|micros| authority(*micros) == *record.question())
                .expect("fixture ASK stamp"),
        )
    }

    fn directive_head(&self, directive: CanonicalUuid) -> JournalHead<SteeringHead> {
        self.journal
            .read(|read| read.steering_head(directive))
            .expect("steering head reads")
            .expect("directive exists")
    }

    fn dispose(&mut self, disposition: RoomReceiptDisposition, item: RoomReceiptRef) -> Commit {
        land(
            &mut self.journal,
            Actor::Operator,
            Producer::CommandDoor,
            minute(5),
            RoomDisposed {
                disposition,
                keys: vec![item],
                clause: "noted".to_owned(),
            },
        )
    }

    fn close(
        &mut self,
        actor: Actor,
        result: &QuestionResultRef,
        closure: QuestionClosure,
    ) -> Commit {
        land(
            &mut self.journal,
            actor,
            Producer::CommandDoor,
            minute(5),
            RoomQuestionClosed {
                result: result.clone(),
                closure,
                clause: "not now".to_owned(),
            },
        )
    }

    fn time_out(&mut self, result: &QuestionResultRef, at: Instant) -> Commit {
        land(
            &mut self.journal,
            Actor::Spice,
            Producer::BusTimer,
            at,
            RoomQuestionTimedOut {
                result: result.clone(),
            },
        )
    }

    fn lapse(&mut self, receipt: RoomReceiptRef, at: Instant) -> Commit {
        land(
            &mut self.journal,
            Actor::Spice,
            Producer::BusTimer,
            at,
            RoomReceiptLapsed { receipt },
        )
    }

    fn recover(
        &mut self,
        actor: Actor,
        receipt: RoomReceiptRef,
        expires: Instant,
        at: Instant,
    ) -> Commit {
        land(
            &mut self.journal,
            actor,
            Producer::CommandDoor,
            at,
            RoomReceiptRecovered { receipt, expires },
        )
    }

    fn result(
        &self,
        result: &QuestionResultRef,
    ) -> (QuestionResultRecord, Seq, QuestionResultState) {
        let (record, head) = self
            .journal
            .read(|read| read.question_result(result))
            .expect("question result reads")
            .expect("question result exists");
        (record, head.position(), head.body().state().clone())
    }

    fn state(&self, result: &QuestionResultRef) -> QuestionResultState {
        self.result(result).2
    }

    fn item_head(&self, item: &RoomReceiptRef) -> (RoomReceiptRecord, Seq, RoomReceiptState) {
        let (record, head) = self
            .journal
            .read(|read| read.room_item(item))
            .expect("operator item reads")
            .expect("operator item exists");
        (record, head.position(), head.body().state().clone())
    }

    fn items(&self) -> usize {
        projection(&self.journal).conversation.items.len()
    }

    /// A lane route to the asked lane and an operator route, both in the room.
    fn both_routes(&self) -> Vec<RoomRoute> {
        vec![
            route(lane(&self.asked), Some(&self.room)),
            route(RoomDestination::Operator {}, Some(&self.room)),
        ]
    }

    /// The other lane's peer message `micros` to the asked lane in the room.
    fn tell_asked(&self, micros: i64) -> SteerMessaged {
        told(
            &self.other,
            directive(micros),
            message_key(micros),
            self.asked.lane,
            Some(&self.room),
        )
    }

    fn refused(&self, result: Commit) {
        refused(&self.journal, result);
    }
}

/// Every lane route is exactly one native directive the same write opened,
/// told from the author's record to that lane in that route's context, and an
/// ASK's directive carries its result handle as key; only an operator route
/// opens an operator item.
#[test]
fn lane_routes_bind_exactly_the_native_directives_their_write_opened() {
    let mut room = Conversation::new();
    let (asked, other) = (room.asked.clone(), room.other.clone());
    let context = room.room.clone();
    Write::lane(&other, minute(1))
        .tell(room.tell_asked(1))
        .record(conversed(
            1,
            &other,
            RoomPublicationKind::Msg {},
            room.both_routes(),
            vec![directive(1)],
        ))
        .apply(&mut room.journal)
        .expect("a lane MSG binds its lane route to its directive");
    let delivered = room.directive_head(directive(1));
    assert_eq!(
        (
            delivered.body().lane,
            delivered.body().room.as_ref(),
            delivered.body().disposition
        ),
        (asked.lane, Some(&context), Disposition::Live)
    );
    let (record, _, state) = room.item_head(&item(1));
    assert_eq!(
        (record.publication(), record.context()),
        (&authority(1), Some(&context))
    );
    assert_eq!(
        state,
        RoomReceiptState::Pending {
            expires: minute(20)
        }
    );
    assert_eq!(room.items(), 1, "the lane route opens no room head");

    Write::operator(minute(1))
        .direct(directive(2), message_key(2), asked.lane, Some(&context))
        .record(RoomPosted {
            publication: authority(2),
            text: "heads up".to_owned(),
            operation: RoomPublicationKind::Msg {},
            routes: vec![route(lane(&asked), Some(&context))],
            directed: vec![directive(2)],
            expires: minute(20),
        })
        .apply(&mut room.journal)
        .expect("an operator MSG binds its lane route to operator direction");
    assert_eq!(room.items(), 1);
    room.journal.verify().expect("admitted bindings verify");
}

/// An operator route concerns only an operator direction its author lane
/// received: a peer message key, another lane's direction, or an unknown key
/// refuses at commit, and history rewritten to name one fails verification.
#[test]
fn an_operator_route_concerns_only_its_authors_operator_direction() {
    let mut room = Conversation::new();
    let (asked, other) = (room.asked.clone(), room.other.clone());
    Write::lane(&other, minute(1))
        .tell(room.tell_asked(1))
        .record(conversed(
            1,
            &other,
            RoomPublicationKind::Msg {},
            room.both_routes(),
            vec![directive(1)],
        ))
        .apply(&mut room.journal)
        .expect("a peer message reaches the asked lane");
    let operator = |micros: i64, to: CanonicalUuid| SteerSent {
        directive: directive(micros),
        fanout: Some(directive(500 + micros)),
        key: message_key(micros),
        lane: to,
        text: "which first".to_owned(),
        from: None,
        room: None,
        control: None,
        attachments: Vec::new(),
        expires: minute(25),
    };
    Write::operator(minute(1))
        .send(operator(2, asked.lane))
        .send(operator(3, other.lane))
        .apply(&mut room.journal)
        .expect("the operator directs both lanes");
    let about = |micros: i64, key: i64| {
        conversed(
            micros,
            &asked,
            RoomPublicationKind::Msg {},
            vec![RoomRoute {
                direction: Some(message_key(key)),
                ..route(RoomDestination::Operator {}, None)
            }],
            Vec::new(),
        )
    };
    for (micros, key) in [(10, 1), (11, 3), (12, 99)] {
        let result = Write::lane(&asked, minute(2))
            .record(about(micros, key))
            .apply(&mut room.journal);
        room.refused(result);
    }
    Write::lane(&asked, minute(2))
        .record(about(13, 2))
        .apply(&mut room.journal)
        .expect("a lane's operator route concerns its own operator direction");
    room.journal.verify().expect("direction routes verify");

    let seq = cursor(&room.journal).expect("nonempty");
    room.journal
        .connection
        .execute(
            "UPDATE fact_envelopes SET envelope = CAST(json_set(CAST(envelope AS TEXT), \
             '$.fact.routes[0].direction', ?1) AS BLOB) WHERE seq = ?2",
            rusqlite::params![message_key(3), i64::try_from(seq.get()).expect("seq")],
        )
        .expect("name another lane's direction beneath the commit");
    let violations = refused_violations(&room.journal);
    assert!(
        violations
            .iter()
            .any(|violation| violation.subject == format!("room_authority:{}", seq.get())),
        "{violations:?}"
    );
}

/// A lane route without exactly its directive refuses: a missing, unopened,
/// duplicated, misdirected, out-of-context, or foreign-record directive, or
/// an ASK directive not keyed by its result.
#[test]
fn a_lane_route_without_its_exact_directive_refuses() {
    let mut room = Conversation::new();
    let (asked, other) = (room.asked.clone(), room.other.clone());
    let context = room.room.clone();
    let message = |micros: i64| room.tell_asked(micros);
    let mut wrong_lane = message(13);
    wrong_lane.lane = uuid("018f82d4-6f59-7d06-92e3-bce913116399");
    let mut wrong_room = message(14);
    wrong_room.room = None;
    let mut wrong_record = message(15);
    wrong_record.locus.offset = 64;
    let unkeyed = QuestionResultRef::new(authority(17));
    let cases = vec![
        (
            10,
            Some(message(10)),
            Vec::new(),
            RoomPublicationKind::Msg {},
        ),
        (11, None, vec![directive(11)], RoomPublicationKind::Msg {}),
        (
            12,
            Some(message(12)),
            vec![directive(12), directive(12)],
            RoomPublicationKind::Msg {},
        ),
        (
            13,
            Some(wrong_lane),
            vec![directive(13)],
            RoomPublicationKind::Msg {},
        ),
        (
            14,
            Some(wrong_room),
            vec![directive(14)],
            RoomPublicationKind::Msg {},
        ),
        (
            15,
            Some(wrong_record),
            vec![directive(15)],
            RoomPublicationKind::Msg {},
        ),
        (16, Some(message(16)), vec![directive(16)], ask_of(&unkeyed)),
    ];
    for (micros, opened, directed, operation) in cases {
        let routes = match operation {
            RoomPublicationKind::Ask { .. } => vec![route(lane(&asked), Some(&context))],
            _ => room.both_routes(),
        };
        let mut write = Write::lane(&other, minute(1));
        if let Some(opened) = opened {
            write = write.tell(opened);
        }
        let result = write
            .record(conversed(micros, &other, operation, routes, directed))
            .apply(&mut room.journal);
        let Err(CommitError::Refused(refusal)) = &result else {
            panic!("binding case {micros} was admitted: {result:?}");
        };
        assert!(
            refusal.to_string().contains("room publication"),
            "binding case {micros} refused for another reason: {refusal}"
        );
        room.refused(result);
    }
    assert_eq!(room.items(), 0);
}

/// ANS settles its result, retires the live ASK directive, and opens its
/// return item to the operator requester in one write; an ANS that leaves a
/// live ASK directive unretired refuses, and return controls never move the
/// answered result.
#[test]
fn answers_settle_retire_and_return_in_one_write() {
    let mut room = Conversation::new();
    let asked = room.asked.clone();

    let from_operator = room.ask(10);
    let (record, _, state) = room.result(&from_operator);
    assert_eq!(state, QuestionResultState::Pending {});
    assert_eq!(
        (record.requester(), record.respondent(), record.context()),
        (
            &RoomDestination::Operator {},
            &lane(&asked),
            Some(&room.room)
        )
    );
    assert_eq!(
        room.directive_head(directive(10)).body().key,
        from_operator.to_string(),
        "the ASK directive's key is its result handle"
    );
    assert_eq!(room.items(), 0, "a lane respondent opens no operator item");
    let unretired = Write::lane(&asked, minute(3))
        .record(RoomConversed {
            expires: minute(40),
            ..conversed(
                11,
                &asked,
                answer_to(&from_operator),
                Vec::new(),
                Vec::new(),
            )
        })
        .apply(&mut room.journal);
    room.refused(unretired);
    assert_eq!(room.state(&from_operator), QuestionResultState::Pending {});

    room.answer(11, &asked, &from_operator, minute(3))
        .expect("the respondent answers and acknowledges in one write");
    let (_, settled, state) = room.result(&from_operator);
    let QuestionResultState::Answered {
        answer,
        return_delivery,
    } = state
    else {
        panic!("the answer settles the result: {state:?}");
    };
    assert_eq!(answer, authority(11));
    assert_eq!(return_delivery.destination(), &RoomDestination::Operator {});
    assert_eq!(
        return_delivery.delivery(),
        &RoomDelivery::Item { item: item(11) }
    );
    let retired = room.directive_head(directive(10));
    assert_eq!(
        (retired.body().disposition, retired.position().get() + 1),
        (Disposition::Acked, settled.get()),
        "the ASK directive retires in the answer's write"
    );
    let (returned, opened, returned_state) = room.item_head(&item(11));
    assert_eq!(opened, settled, "the return opens with the answer");
    assert_eq!(returned.context(), Some(&room.room));
    assert_eq!(
        returned_state,
        RoomReceiptState::Pending {
            expires: minute(40)
        }
    );
    room.dispose(RoomReceiptDisposition::Nak, item(11))
        .expect("the operator declines the return item");
    assert!(matches!(
        room.state(&from_operator),
        QuestionResultState::Answered { .. }
    ));
}

/// A lane requester's answer returns by one native directive in the
/// question's context, an operator respondent's ASK item retires with its
/// answer, and a lane return opens no operator item.
#[test]
fn answers_return_to_lane_requesters_and_retire_operator_items() {
    let mut room = Conversation::new();
    let (asked, other) = (room.asked.clone(), room.other.clone());

    let between_lanes = room.ask_lane(20);
    let acked = room.directive_head(directive(20));
    assert_eq!(acked.body().key, between_lanes.to_string());
    Write::lane(&asked, minute(2))
        .retire(&acked, Disposition::Acked)
        .apply(&mut room.journal)
        .expect("the respondent acknowledges delivery");
    assert_eq!(room.state(&between_lanes), QuestionResultState::Pending {});
    let misdirected = Write::lane(&asked, minute(3))
        .tell(told(
            &asked,
            directive(21),
            message_key(21),
            uuid("018f82d4-6f59-7d06-92e3-bce913116399"),
            Some(&room.room),
        ))
        .record(conversed(
            21,
            &asked,
            answer_to(&between_lanes),
            Vec::new(),
            vec![directive(21)],
        ))
        .apply(&mut room.journal);
    room.refused(misdirected);
    room.answer(21, &asked, &between_lanes, minute(3))
        .expect("an answer after acknowledgment returns to its requester lane");
    let QuestionResultState::Answered {
        return_delivery, ..
    } = room.state(&between_lanes)
    else {
        panic!("the lane answer settles the result");
    };
    assert_eq!(
        return_delivery.delivery(),
        &RoomDelivery::Directive {
            directive: directive(21)
        }
    );
    let returned = room.directive_head(directive(21));
    assert_eq!(
        (returned.body().lane, returned.body().room.as_ref()),
        (other.lane, Some(&room.room))
    );

    let of_operator = room.ask_operator(30);
    let asked_items = room.items();
    room.post_answer(31, &of_operator, minute(3))
        .expect("the operator answers a lane");
    let (_, settled, _) = room.result(&of_operator);
    let (_, retired, retired_state) = room.item_head(&item(31));
    assert_eq!(
        (
            retired,
            matches!(retired_state, RoomReceiptState::Acked { .. })
        ),
        (settled, true),
        "the operator ASK item retires with the answer"
    );
    assert_eq!(room.items(), asked_items, "a lane return opens no item");
    assert_eq!(room.directive_head(directive(31)).body().lane, other.lane);
    room.journal
        .verify()
        .expect("answered conversations verify");
}

/// The respondent lane's native NAK of a live ASK directive declines its
/// pending result in the same fact, as the operator's NAK of an ASK item does
/// even after acknowledgment; a NAK after the result settled moves only the
/// delivery.
#[test]
fn a_nak_of_a_live_ask_declines_its_pending_result_atomically() {
    let mut room = Conversation::new();
    let declined = room.ask(10);
    room.nak(&declined, minute(2))
        .expect("the respondent declines the ASK directive");
    let (_, settled, state) = room.result(&declined);
    assert_eq!(
        state,
        QuestionResultState::Declined {
            source: journal_source(settled.get())
        }
    );
    assert_eq!(
        room.directive_head(directive(10)).position(),
        settled,
        "the result settles at the NAK itself"
    );

    let timed = room.ask(20);
    let early = room.time_out(&timed, minute(24));
    room.refused(early);
    room.time_out(&timed, minute(30))
        .expect("the result times out");
    let settled = room.result(&timed);
    room.nak(&timed, minute(24))
        .expect("the live directive still takes its NAK");
    assert_eq!(room.result(&timed), settled, "a NAK never reopens a result");

    let of_operator = room.ask_operator(30);
    room.dispose(RoomReceiptDisposition::Ack, item(31))
        .expect("the operator acknowledges the ASK item");
    room.dispose(RoomReceiptDisposition::Nak, item(31))
        .expect("the operator still declines the question");
    assert!(matches!(
        room.state(&of_operator),
        QuestionResultState::Declined { .. }
    ));
    assert!(matches!(
        room.item_head(&item(31)).2,
        RoomReceiptState::Acked { .. }
    ));
    let again = room.dispose(RoomReceiptDisposition::Nak, item(31));
    room.refused(again);
}

/// Whichever of answer, respondent NAK, decline, cancellation, or timeout
/// lands first is the result; every later contender either refuses or moves
/// only native steering, never the result.
#[test]
fn one_terminal_result_wins_every_race() {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Contender {
        Answer,
        Nak,
        Decline,
        Cancel,
        Timeout,
    }
    let contenders = [
        Contender::Answer,
        Contender::Nak,
        Contender::Decline,
        Contender::Cancel,
        Contender::Timeout,
    ];
    let race = |room: &mut Conversation, result: &QuestionResultRef, contender, answer| {
        let asked = room.asked.clone();
        match contender {
            Contender::Answer => room.answer(answer, &asked, result, minute(3)),
            Contender::Nak => room.nak(result, minute(3)),
            Contender::Decline => {
                room.close(Actor::Lane(asked.lane), result, QuestionClosure::Declined)
            }
            Contender::Cancel => room.close(Actor::Operator, result, QuestionClosure::Cancelled),
            Contender::Timeout => room.time_out(result, minute(30)),
        }
    };
    for winner in contenders {
        let mut room = Conversation::new();
        let result = room.ask(10);
        race(&mut room, &result, winner, 11).expect("the first contender wins");
        let won = room.result(&result);
        assert!(won.2.is_terminal(), "{winner:?} settles the result");
        for (offset, loser) in (12..).zip(contenders) {
            let live = room.live_ask(&result).is_some();
            let lost = race(&mut room, &result, loser, offset);
            if loser == Contender::Nak && live {
                lost.expect("a settled ASK's live directive still takes its NAK");
                assert!(room.live_ask(&result).is_none());
            } else {
                room.refused(lost);
            }
            assert_eq!(room.result(&result), won, "{loser:?} after {winner:?}");
        }
    }
}

/// Directive lapse and dead letter and operator item lapse and recovery move
/// only their delivery: none changes, extends, or reopens a result.
#[test]
fn delivery_lapse_and_recovery_never_move_result_authority() {
    let mut room = Conversation::new();
    let asked = room.asked.clone();
    let lapsed = room.ask(10);
    let pending = room.result(&lapsed);
    let live = room.directive_head(directive(10));
    Write::new(Actor::Spice, Producer::BusTimer, minute(25))
        .retire(&live, Disposition::Lapsed)
        .apply(&mut room.journal)
        .expect("the ASK directive lapses");
    assert_eq!(room.result(&lapsed), pending);
    room.answer(11, &asked, &lapsed, minute(26))
        .expect("a lapsed directive needs no retirement");

    let dead = room.ask(20);
    let pending = room.result(&dead);
    let live = room.directive_head(directive(20));
    Write::new(Actor::Spice, Producer::ResidentRecognition, minute(2))
        .retire(&live, Disposition::DeadLettered)
        .apply(&mut room.journal)
        .expect("the ASK directive dead-letters");
    assert_eq!(room.result(&dead), pending);

    let of_operator = room.ask_operator(30);
    let pending = room.result(&of_operator);
    let early = room.lapse(item(31), minute(19));
    room.refused(early);
    room.lapse(item(31), minute(20))
        .expect("the expired item lapses");
    assert_eq!(
        room.item_head(&item(31)).2,
        RoomReceiptState::Expired { at: minute(20) }
    );
    assert_eq!(room.result(&of_operator), pending);
    let by_lane = room.recover(Actor::Lane(asked.lane), item(31), minute(35), minute(21));
    room.refused(by_lane);
    let stale = room.recover(Actor::Operator, item(31), minute(21), minute(21));
    room.refused(stale);
    room.recover(Actor::Operator, item(31), minute(35), minute(21))
        .expect("the operator recovers the item");
    assert_eq!(
        room.item_head(&item(31)).2,
        RoomReceiptState::Pending {
            expires: minute(35)
        }
    );
    assert_eq!(
        room.result(&of_operator),
        pending,
        "recovery never extends the result"
    );
    let late = room.post_answer(32, &of_operator, minute(31));
    room.refused(late);
    room.lapse(item(31), minute(35))
        .expect("the recovered item lapses again");
    assert_eq!(room.result(&of_operator), pending);
}

/// Pending results are bounded per requester; at saturation another ASK
/// refuses while MSG, another requester's ASK, and NAKs still admit, and a
/// settled result frees its slot.
#[test]
fn pending_results_bound_asks_without_blocking_other_authority() {
    let mut room = Conversation::new();
    let mut results = Vec::new();
    for index in 0..ROOM_PENDING_RESULT_LIMIT {
        let micros = 100 + 2 * i64::try_from(index).expect("index fits");
        results.push(room.ask(micros));
    }
    let saturated = room.post_ask(1, &QuestionResultRef::new(authority(2)));
    room.refused(saturated);

    let asked = room.asked.clone();
    Write::operator(minute(2))
        .direct(directive(3), message_key(3), asked.lane, None)
        .record(RoomPosted {
            publication: authority(3),
            text: "heads up".to_owned(),
            operation: RoomPublicationKind::Msg {},
            routes: vec![route(lane(&asked), None)],
            directed: vec![directive(3)],
            expires: minute(20),
        })
        .apply(&mut room.journal)
        .expect("MSG admits at saturation");
    room.ask_operator(4);
    room.nak(&results[0], minute(3))
        .expect("a NAK admits at saturation");
    room.post_ask(1, &QuestionResultRef::new(authority(2)))
        .expect("a declined result frees its slot");
}

/// Only the operator controls items and only issued items resolve; only
/// stored parties answer or close a question; a lane writes only its own
/// conversation; publications and results are write-once.
#[test]
fn authority_binds_to_stored_parties_and_issued_items() {
    let mut room = Conversation::new();
    let (asked, other) = (room.asked.clone(), room.other.clone());
    let of_operator = room.ask_operator(10);
    let by_lane = land(
        &mut room.journal,
        Actor::Lane(other.lane),
        Producer::CommandDoor,
        minute(2),
        RoomDisposed {
            disposition: RoomReceiptDisposition::Ack,
            keys: vec![item(11)],
            clause: "noted".to_owned(),
        },
    );
    room.refused(by_lane);
    let unissued = room.dispose(RoomReceiptDisposition::Ack, item(12));
    room.refused(unissued);
    let lane_answer = room.answer(13, &asked, &of_operator, minute(3));
    room.refused(lane_answer);

    let result = room.ask(20);
    let stranger = room.answer(21, &other, &result, minute(3));
    room.refused(stranger);
    let forged = Write::lane(&other, minute(3))
        .record(RoomConversed {
            expires: minute(40),
            ..conversed(22, &asked, answer_to(&result), Vec::new(), Vec::new())
        })
        .apply(&mut room.journal);
    room.refused(forged);
    let respondent_cancels =
        room.close(Actor::Lane(asked.lane), &result, QuestionClosure::Cancelled);
    room.refused(respondent_cancels);
    let stranger_declines = room.close(Actor::Lane(other.lane), &result, QuestionClosure::Declined);
    room.refused(stranger_declines);
    let duplicate = room.post_ask(20, &QuestionResultRef::new(authority(23)));
    room.refused(duplicate);
    let reused = room.post_ask(24, &result);
    room.refused(reused);
    assert_eq!(room.state(&result), QuestionResultState::Pending {});
    assert_eq!(room.state(&of_operator), QuestionResultState::Pending {});
}

/// Reindex rebuilds every conversation head from facts at their own commit
/// instants, and verification names a corrupted head.
#[test]
fn conversation_heads_rebuild_from_facts() {
    let mut room = Conversation::new();
    let asked = room.asked.clone();
    let answered = room.ask(10);
    room.answer(11, &asked, &answered, minute(3))
        .expect("the respondent answers");
    let declined = room.ask(20);
    room.nak(&declined, minute(3))
        .expect("the respondent declines");
    let timed = room.ask_operator(30);
    room.time_out(&timed, minute(30))
        .expect("the result times out");
    room.lapse(item(31), minute(31)).expect("the item lapses");
    Write::lane(&asked, minute(32))
        .record(RoomConversed {
            expires: minute(40),
            ..conversed(
                32,
                &asked,
                RoomPublicationKind::Msg {},
                vec![route(RoomDestination::Operator {}, None)],
                Vec::new(),
            )
        })
        .apply(&mut room.journal)
        .expect("a retained unnamed operator route remains readable");
    let derived = projection(&room.journal);
    let facts = fact_bytes(&room.journal);

    room.journal
        .connection
        .execute_batch(
            "UPDATE room_results SET pending = 1;
             DELETE FROM room_items WHERE open = 0;",
        )
        .expect("corrupt the conversation heads");
    assert!(refused_classes(&room.journal).contains(&ViolationClass::Head));

    room.journal.reindex().expect("heads rebuild from facts");
    assert_eq!(projection(&room.journal), derived);
    assert_eq!(fact_bytes(&room.journal), facts);
    room.journal.verify().expect("rebuilt heads verify");
}

/// A room snapshot carries every conversation record, the current head of each
/// operator item and result, and both the opening and current steering head of
/// each directive a conversation names, so a reader folds the same state.
#[test]
fn room_snapshots_carry_conversation_and_delivery_heads() {
    let mut room = Conversation::new();
    let asked = room.asked.clone();
    let question = room.ask_lane(20);
    let asking = cursor(&room.journal).expect("asked");
    let opened = room.directive_head(directive(20)).position();
    room.answer(21, &asked, &question, minute(3))
        .expect("the respondent answers");
    let answering = cursor(&room.journal).expect("answered");
    let acknowledged = room.directive_head(directive(20)).position();
    let returned = room.directive_head(directive(21)).position();
    room.ask_operator(30);
    let of_operator = cursor(&room.journal).expect("asked the operator");
    room.dispose(RoomReceiptDisposition::Ack, item(31))
        .expect("the operator acknowledges");
    let disposed = cursor(&room.journal).expect("disposed");
    assert_eq!(
        room.item_head(&item(31)).1,
        disposed,
        "the item head moved past its opening record"
    );

    let horizon = disposed;
    let mut start = crate::FactPageStart::After(None);
    let mut rows = Vec::new();
    loop {
        let page = room
            .journal
            .read(|read| read.room_authority_page(start, horizon, fact_budget(4)))
            .expect("room authority reads");
        rows.extend(page.rows.iter().map(|fact| fact.seq));
        let Some(overflow) = page.overflow else {
            break;
        };
        start = crate::FactPageStart::At(overflow.continuation);
    }
    for (seq, what) in [
        (opened, "the ASK delivery's opening"),
        (
            asking,
            "the ASK record and its pending-then-answered result",
        ),
        (acknowledged, "the ASK delivery's current receipt"),
        (returned, "the answer's return delivery"),
        (answering, "the ANS record and the result head it settled"),
        (
            of_operator,
            "the operator ASK record and its pending result",
        ),
        (disposed, "the operator item's current head"),
    ] {
        assert!(rows.contains(&seq), "{what} at {seq} is in the snapshot");
    }
}

/// An operator item reaches the actor its authority records: the sender of
/// the direction a lane's `#KEY` names, or the asker an answer returns to; a
/// retained route naming no direction reaches the historical operator. The
/// open-item page counts and retains only the items its reader admits.
#[test]
fn operator_items_reach_the_actor_their_authority_records() {
    let mut room = Conversation::new();
    let (asked, other) = (room.asked.clone(), room.other.clone());
    let human = Actor::Human(uuid("018f82d4-6f59-7d06-92e3-bce9131163b0"));
    let result = QuestionResultRef::new(authority(11));
    Write::new(human.clone(), Producer::CommandDoor, minute(1))
        .direct(
            directive(10),
            result.to_string(),
            asked.lane,
            Some(&room.room),
        )
        .record(RoomPosted {
            publication: authority(10),
            text: "which one?".to_owned(),
            operation: ask_of(&result),
            routes: vec![route(lane(&asked), Some(&room.room))],
            directed: vec![directive(10)],
            expires: minute(20),
        })
        .apply(&mut room.journal)
        .expect("a principal asks a lane");
    room.answer(12, &asked, &result, minute(2))
        .expect("the lane answers its asker");
    let direction = |micros: i64| SteerSent {
        directive: directive(micros),
        fanout: Some(directive(500 + micros)),
        key: message_key(micros),
        lane: other.lane,
        text: "which first".to_owned(),
        from: None,
        room: None,
        control: None,
        attachments: Vec::new(),
        expires: minute(25),
    };
    Write::new(human.clone(), Producer::CommandDoor, minute(3))
        .send(direction(20))
        .apply(&mut room.journal)
        .expect("a principal directs a lane");
    Write::operator(minute(3))
        .send(direction(30))
        .apply(&mut room.journal)
        .expect("the historical operator directs a lane");
    for (micros, key) in [(21, 20), (31, 30)] {
        Write::lane(&other, minute(4))
            .record(conversed(
                micros,
                &other,
                RoomPublicationKind::Msg {},
                vec![RoomRoute {
                    direction: Some(message_key(key)),
                    ..route(RoomDestination::Operator {}, None)
                }],
                Vec::new(),
            ))
            .apply(&mut room.journal)
            .expect("a lane replies to its direction");
    }
    room.ask_operator(40);

    let requesters = room
        .journal
        .read(|read| {
            [10, 12, 21, 31, 40]
                .into_iter()
                .map(|micros| {
                    let (_, publication) = read
                        .room_conversation(&authority(micros))?
                        .expect("the publication is admitted");
                    read.room_operator_requester(&publication)
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .expect("requesters read");
    assert_eq!(
        requesters,
        [
            None,
            Some(human.clone()),
            Some(human.clone()),
            Some(Actor::Operator),
            Some(Actor::Operator),
        ]
    );

    let page = |limit: usize, reader: &Actor| {
        room.journal
            .read(|read| read.open_room_items_page(None, limit, |actor| actor == reader))
            .map(|page| {
                let keys = page
                    .rows
                    .iter()
                    .map(|(record, _)| record.receipt().clone())
                    .collect::<Vec<_>>();
                (keys, page.population)
            })
            .expect("open items page")
    };
    assert_eq!(page(1, &human), (vec![item(12)], 2));
    assert_eq!(page(8, &human), (vec![item(12), item(21)], 2));
    assert_eq!(page(8, &Actor::Operator), (vec![item(31), item(41)], 2));
}
