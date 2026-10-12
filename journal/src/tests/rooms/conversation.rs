//! Room conversation authority: correlated ASK results and per-route receipts
//! admitted, raced, lapsed, recovered, bounded, and rebuilt from facts.

use wire::rooms::{
    QuestionClosure, QuestionResultRecord, QuestionResultRef, QuestionResultState,
    ROOM_PENDING_RESULT_LIMIT, RoomConversed, RoomDeliveryKey, RoomDestination, RoomDisposed,
    RoomPosted, RoomPublicationKind, RoomQuestionClosed, RoomQuestionTimedOut,
    RoomReceiptDisposition, RoomReceiptLapsed, RoomReceiptRecovered, RoomReceiptRef,
    RoomReceiptState, RoomReceiptUnavailable, RoomReceipted, RoomRoute,
};

use super::*;

fn minute(minute: u32) -> Instant {
    format!("2030-04-12T19:{minute:02}:00.000Z")
        .parse()
        .expect("fixture instant")
}

fn land<F: wire::Body>(
    journal: &mut Journal,
    actor: Actor,
    producer: Producer,
    at: Instant,
    body: F,
) -> Commit {
    let mut write = JournalWrite::new(
        actor,
        "task:rooms".parse().expect("room origin"),
        at,
        producer,
    );
    write.fact(body, FactCause::Root).expect("room fact builds");
    journal.apply(write)
}

fn lane(head: &TranscriptSourceHead) -> RoomDestination {
    RoomDestination::Lane { lane: head.lane }
}

fn route(publication: i64, index: u64) -> RoomReceiptRef {
    RoomReceiptRef {
        publication: authority(publication),
        route: SequenceCode::new(index),
    }
}

fn key(result: &QuestionResultRef) -> RoomDeliveryKey {
    RoomDeliveryKey::Result {
        result: result.clone(),
    }
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

    /// The operator's ASK `micros` of the asked lane, admitted at minute 1
    /// with receipts expiring at minute 20 and its deadline at minute 30.
    fn ask(&mut self, micros: i64) -> QuestionResultRef {
        let result = QuestionResultRef::new(authority(micros + 1));
        self.post_ask(micros, &result)
            .expect("the operator's ASK admits");
        result
    }

    fn post_ask(&mut self, micros: i64, result: &QuestionResultRef) -> Commit {
        land(
            &mut self.journal,
            Actor::Operator,
            Producer::CommandDoor,
            minute(1),
            RoomPosted {
                publication: authority(micros),
                text: "which one?".to_owned(),
                operation: RoomPublicationKind::Ask {
                    result: result.clone(),
                    deadline: minute(30),
                },
                routes: vec![RoomRoute {
                    destination: lane(&self.asked),
                    context: Some(self.room.clone()),
                }],
                expires: minute(20),
            },
        )
    }

    fn answer(
        &mut self,
        micros: i64,
        author: &TranscriptSourceHead,
        result: &QuestionResultRef,
        at: Instant,
    ) -> Commit {
        land(
            &mut self.journal,
            Actor::Lane(author.lane),
            Producer::AckScanner,
            at,
            RoomConversed {
                publication: authority(micros),
                hunk: hunk(author),
                operation: RoomPublicationKind::Ans {
                    result: result.clone(),
                    context: None,
                },
                routes: Vec::new(),
                expires: minute(40),
            },
        )
    }

    fn receipt(
        &mut self,
        by: &TranscriptSourceHead,
        disposition: RoomReceiptDisposition,
        key: RoomDeliveryKey,
        at: Instant,
    ) -> Commit {
        land(
            &mut self.journal,
            Actor::Lane(by.lane),
            Producer::AckScanner,
            at,
            RoomReceipted {
                disposition,
                keys: vec![key],
                hunk: hunk(by),
            },
        )
    }

    fn dispose(&mut self, disposition: RoomReceiptDisposition, key: RoomDeliveryKey) -> Commit {
        land(
            &mut self.journal,
            Actor::Operator,
            Producer::CommandDoor,
            minute(5),
            RoomDisposed {
                disposition,
                keys: vec![key],
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

    fn receipt_head(&self, receipt: &RoomReceiptRef) -> (RoomRoute, Seq, RoomReceiptState) {
        let (record, head) = self
            .journal
            .read(|read| read.room_receipt(receipt))
            .expect("room receipt reads")
            .expect("room receipt exists");
        (
            record.route().clone(),
            head.position(),
            head.body().state().clone(),
        )
    }

    fn refused(&self, result: Commit) {
        refused(&self.journal, result);
    }
}

/// An ASK freezes its parties, context, and deadline; ACK settles only the
/// delivery; ANS settles the result, retires a still-live ASK receipt, and
/// opens its return receipt to the stored requester in one write; return
/// controls settle that receipt without reopening anything.
#[test]
fn answers_settle_results_and_open_their_return_in_one_write() {
    let mut room = Conversation::new();
    let acked = room.ask(10);
    let (record, _, state) = room.result(&acked);
    assert_eq!(state, QuestionResultState::Pending {});
    assert_eq!(record.requester(), &RoomDestination::Operator {});
    assert_eq!(record.respondent(), &lane(&room.asked));
    assert_eq!(record.context(), Some(&room.room));
    assert_eq!(record.deadline(), minute(30));
    assert_eq!(
        room.receipt_head(&route(10, 0)).2,
        RoomReceiptState::Pending {
            expires: minute(20)
        }
    );

    let asked = room.asked.clone();
    room.receipt(&asked, RoomReceiptDisposition::Ack, key(&acked), minute(2))
        .expect("the respondent acknowledges delivery");
    assert!(matches!(
        room.receipt_head(&route(10, 0)).2,
        RoomReceiptState::Acked { .. }
    ));
    assert_eq!(room.state(&acked), QuestionResultState::Pending {});

    room.answer(11, &asked, &acked, minute(3))
        .expect("the respondent answers");
    let (_, settled, state) = room.result(&acked);
    let QuestionResultState::Answered {
        answer,
        return_delivery,
    } = state
    else {
        panic!("the answer settles the result: {state:?}");
    };
    assert_eq!(answer, authority(11));
    assert_eq!(return_delivery.destination(), &RoomDestination::Operator {});
    let (returned, opened, returned_state) = room.receipt_head(&route(11, 0));
    assert_eq!(opened, settled, "the return opens with the answer");
    assert_eq!(
        returned,
        RoomRoute {
            destination: RoomDestination::Operator {},
            context: Some(room.room.clone()),
        }
    );
    assert_eq!(
        returned_state,
        RoomReceiptState::Pending {
            expires: minute(40)
        }
    );

    let live = room.ask(20);
    room.answer(21, &asked, &live, minute(4))
        .expect("an answer without a prior ACK admits");
    let (_, answered, _) = room.result(&live);
    let (_, retired, retired_state) = room.receipt_head(&route(20, 0));
    assert_eq!(
        retired, answered,
        "the live ASK receipt retires with the answer"
    );
    assert!(matches!(retired_state, RoomReceiptState::Acked { .. }));

    let receipts = projection(&room.journal).conversation.receipts.len();
    room.dispose(
        RoomReceiptDisposition::Nak,
        RoomDeliveryKey::Route(route(11, 0)),
    )
    .expect("the requester declines the return delivery");
    assert!(matches!(
        room.receipt_head(&route(11, 0)).2,
        RoomReceiptState::Declined { .. }
    ));
    assert!(matches!(
        room.state(&acked),
        QuestionResultState::Answered { .. }
    ));
    assert_eq!(
        projection(&room.journal).conversation.receipts.len(),
        receipts,
        "controls open no receipt"
    );
}

/// The commit instant decides lateness: an ANS at or after the deadline
/// refuses before any timeout is recorded, and timeout itself waits for the
/// deadline.
#[test]
fn deadlines_refuse_late_answers_before_any_timeout_lands() {
    let mut room = Conversation::new();
    let result = room.ask(10);
    let asked = room.asked.clone();
    let late = room.answer(11, &asked, &result, minute(30));
    room.refused(late);
    assert_eq!(room.state(&result), QuestionResultState::Pending {});
    assert!(matches!(
        room.receipt_head(&route(10, 0)).2,
        RoomReceiptState::Pending { .. }
    ));

    let early = room.time_out(&result, minute(29));
    room.refused(early);
    room.time_out(&result, minute(30))
        .expect("the deadline times the result out");
    assert_eq!(room.state(&result), QuestionResultState::TimedOut {});
    assert!(
        matches!(
            room.receipt_head(&route(10, 0)).2,
            RoomReceiptState::Pending { .. }
        ),
        "timeout leaves delivery to its own receipt"
    );
}

/// Whichever of answer, respondent NAK, decline, cancellation, or timeout
/// lands first is the result; every later contender refuses and moves
/// neither the result nor its receipt.
#[test]
fn one_terminal_result_wins_every_race() {
    #[derive(Clone, Copy, Debug)]
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
            Contender::Nak => {
                room.receipt(&asked, RoomReceiptDisposition::Nak, key(result), minute(3))
            }
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
        let mut receipt = room.receipt_head(&route(10, 0));
        for (offset, loser) in (12..).zip(contenders) {
            let lost = race(&mut room, &result, loser, offset);
            if matches!((winner, loser), (Contender::Timeout, Contender::Nak)) {
                lost.expect("a timed-out ASK's live delivery still takes its NAK");
                receipt = room.receipt_head(&route(10, 0));
                assert!(matches!(receipt.2, RoomReceiptState::Declined { .. }));
            } else {
                room.refused(lost);
            }
            assert_eq!(room.result(&result), won, "{loser:?} after {winner:?}");
            assert_eq!(room.receipt_head(&route(10, 0)), receipt);
        }
    }
}

/// NAK of an ASK's public key declines its pending result even after that
/// delivery was acknowledged.
#[test]
fn a_nak_after_acknowledgment_still_declines_the_question() {
    let mut room = Conversation::new();
    let result = room.ask(10);
    let asked = room.asked.clone();
    room.receipt(&asked, RoomReceiptDisposition::Ack, key(&result), minute(2))
        .expect("delivery is acknowledged");
    let acked = room.receipt_head(&route(10, 0));
    room.receipt(&asked, RoomReceiptDisposition::Nak, key(&result), minute(3))
        .expect("the respondent declines the question");
    assert!(matches!(
        room.state(&result),
        QuestionResultState::Declined { .. }
    ));
    assert_eq!(room.receipt_head(&route(10, 0)), acked);
    let again = room.receipt(&asked, RoomReceiptDisposition::Nak, key(&result), minute(4));
    room.refused(again);
}

/// Lapse waits for expiry and recovery returns only the receipt to pending:
/// neither moves the result, and no return-receipt outcome retracts an
/// answer.
#[test]
fn receipt_lapse_and_recovery_never_move_result_authority() {
    let mut room = Conversation::new();
    let result = room.ask(10);
    let pending = room.result(&result);
    let early = room.lapse(route(10, 0), minute(19));
    room.refused(early);
    room.lapse(route(10, 0), minute(20))
        .expect("the expired receipt lapses");
    assert_eq!(
        room.receipt_head(&route(10, 0)).2,
        RoomReceiptState::Expired { at: minute(20) }
    );
    assert_eq!(room.result(&result), pending);

    let asked = room.asked.clone();
    let by_lane = room.recover(
        Actor::Lane(asked.lane),
        route(10, 0),
        minute(25),
        minute(21),
    );
    room.refused(by_lane);
    let stale = room.recover(Actor::Operator, route(10, 0), minute(21), minute(21));
    room.refused(stale);
    room.recover(Actor::Operator, route(10, 0), minute(25), minute(21))
        .expect("the operator recovers the receipt");
    assert_eq!(
        room.receipt_head(&route(10, 0)).2,
        RoomReceiptState::Pending {
            expires: minute(25)
        }
    );
    assert_eq!(
        room.result(&result),
        pending,
        "recovery never extends the result"
    );
    let late = room.answer(11, &asked, &result, minute(31));
    room.refused(late);

    let answered = room.ask(20);
    room.answer(21, &asked, &answered, minute(3))
        .expect("the respondent answers");
    let settled = room.result(&answered);
    land(
        &mut room.journal,
        Actor::Spice,
        Producer::ResidentRecognition,
        minute(4),
        RoomReceiptUnavailable {
            receipt: route(21, 0),
        },
    )
    .expect("the return delivery is unavailable");
    assert_eq!(room.result(&answered), settled);
    room.recover(Actor::Operator, route(21, 0), minute(45), minute(5))
        .expect("the return receipt recovers");
    room.lapse(route(21, 0), minute(45))
        .expect("the recovered return lapses again");
    assert_eq!(room.result(&answered), settled, "a lapse never retracts");
}

/// Pending results are bounded per requester; at saturation another ASK
/// refuses while controls, MSG, and other requesters still admit, and a
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
    land(
        &mut room.journal,
        Actor::Operator,
        Producer::CommandDoor,
        minute(2),
        RoomPosted {
            publication: authority(3),
            text: "heads up".to_owned(),
            operation: RoomPublicationKind::Msg {},
            routes: vec![RoomRoute {
                destination: lane(&asked),
                context: None,
            }],
            expires: minute(20),
        },
    )
    .expect("MSG admits at saturation");
    land(
        &mut room.journal,
        Actor::Lane(asked.lane),
        Producer::AckScanner,
        minute(2),
        RoomConversed {
            publication: authority(4),
            hunk: hunk(&asked),
            operation: RoomPublicationKind::Ask {
                result: QuestionResultRef::new(authority(5)),
                deadline: minute(30),
            },
            routes: vec![RoomRoute {
                destination: RoomDestination::Operator {},
                context: None,
            }],
            expires: minute(20),
        },
    )
    .expect("another requester asks at the operator's saturation");
    room.receipt(
        &asked,
        RoomReceiptDisposition::Nak,
        key(&results[0]),
        minute(3),
    )
    .expect("controls admit at saturation");
    room.post_ask(1, &QuestionResultRef::new(authority(2)))
        .expect("a settled result frees its slot");
}

/// Only a key's stored destination may control it, only issued keys
/// resolve, only stored parties close a question, and a lane writes only
/// its own conversation.
#[test]
fn authority_binds_to_stored_parties_and_issued_keys() {
    let mut room = Conversation::new();
    let asked = room.asked.clone();
    let other = room.other.clone();
    land(
        &mut room.journal,
        Actor::Operator,
        Producer::CommandDoor,
        minute(1),
        RoomPosted {
            publication: authority(1),
            text: "heads up".to_owned(),
            operation: RoomPublicationKind::Msg {},
            routes: vec![
                RoomRoute {
                    destination: lane(&asked),
                    context: None,
                },
                RoomRoute {
                    destination: RoomDestination::Task {
                        task: room.room.clone(),
                    },
                    context: None,
                },
            ],
            expires: minute(20),
        },
    )
    .expect("MSG admits");
    let wrong_lane = room.receipt(
        &other,
        RoomReceiptDisposition::Ack,
        RoomDeliveryKey::Route(route(1, 0)),
        minute(2),
    );
    room.refused(wrong_lane);
    let unissued = room.receipt(
        &asked,
        RoomReceiptDisposition::Ack,
        RoomDeliveryKey::Route(route(1, 1)),
        minute(2),
    );
    room.refused(unissued);
    room.receipt(
        &asked,
        RoomReceiptDisposition::Ack,
        RoomDeliveryKey::Route(route(1, 0)),
        minute(2),
    )
    .expect("the destination acknowledges");

    let result = room.ask(10);
    let internal = room.receipt(
        &asked,
        RoomReceiptDisposition::Ack,
        RoomDeliveryKey::Route(route(10, 0)),
        minute(2),
    );
    room.refused(internal);
    let stranger = room.answer(11, &other, &result, minute(3));
    room.refused(stranger);
    let forged = land(
        &mut room.journal,
        Actor::Lane(other.lane),
        Producer::AckScanner,
        minute(3),
        RoomConversed {
            publication: authority(12),
            hunk: hunk(&asked),
            operation: RoomPublicationKind::Ans {
                result: result.clone(),
                context: None,
            },
            routes: Vec::new(),
            expires: minute(40),
        },
    );
    room.refused(forged);
    let respondent_cancels =
        room.close(Actor::Lane(asked.lane), &result, QuestionClosure::Cancelled);
    room.refused(respondent_cancels);
    let stranger_declines = room.close(Actor::Lane(other.lane), &result, QuestionClosure::Declined);
    room.refused(stranger_declines);
    let duplicate = room.post_ask(10, &QuestionResultRef::new(authority(13)));
    room.refused(duplicate);
    let reused = room.post_ask(14, &result);
    room.refused(reused);
    assert_eq!(room.state(&result), QuestionResultState::Pending {});
}

/// Reindex rebuilds every conversation head from facts at their own commit
/// instants, and verification names a corrupted head.
#[test]
fn conversation_heads_rebuild_from_facts() {
    let mut room = Conversation::new();
    let answered = room.ask(10);
    let asked = room.asked.clone();
    room.answer(11, &asked, &answered, minute(3))
        .expect("the respondent answers");
    let declined = room.ask(20);
    room.close(
        Actor::Lane(asked.lane),
        &declined,
        QuestionClosure::Declined,
    )
    .expect("the respondent declines");
    let timed = room.ask(30);
    room.time_out(&timed, minute(30))
        .expect("the result times out");
    room.lapse(route(30, 0), minute(31))
        .expect("the receipt lapses");
    let derived = projection(&room.journal);
    let facts = fact_bytes(&room.journal);

    room.journal
        .connection
        .execute_batch(
            "UPDATE room_results SET pending = 1;
             DELETE FROM room_receipts WHERE open = 0;",
        )
        .expect("corrupt the conversation heads");
    assert!(refused_classes(&room.journal).contains(&ViolationClass::Head));

    room.journal.reindex().expect("heads rebuild from facts");
    assert_eq!(projection(&room.journal), derived);
    assert_eq!(fact_bytes(&room.journal), facts);
    room.journal.verify().expect("rebuilt heads verify");
}
