//! ASK opens one correlated result whose lane delivery is keyed by it, ANS
//! settles it with its return and the respondent's ACK in one write, and an
//! operator route opens an explicit pending item.

use steer::ack::scan::Verb;
use steering::{Conversed, Published, Unpublished};
use wire::rooms::{
    QuestionResultHead, QuestionResultRecord, QuestionResultRef, QuestionResultState,
    RoomConversed, RoomDestination, RoomReceiptRef,
};

use super::*;

impl Lanes {
    pub(super) fn question(
        &self,
        result: &QuestionResultRef,
    ) -> (QuestionResultRecord, QuestionResultHead) {
        let result = result.clone();
        let (record, head) = self
            .bus
            .read(move |read| read.question_result(&result))
            .expect("question reads")
            .expect("the ASK opened its result");
        (record, head.into_body())
    }

    fn conversations(&self) -> Vec<RoomConversed> {
        self.bus
            .fact_snapshot()
            .expect("facts read")
            .facts()
            .iter()
            .filter_map(|envelope| envelope.fact.read::<RoomConversed>())
            .collect()
    }

    /// Author `text` on `lane`, account it, and hear its newest packet.
    pub(super) fn say(&self, lane: u8, text: &str) -> Result<Published, Unpublished> {
        self.author(lane, text);
        self.account(lane);
        let (_, outcomes) = self.hear_newest(lane);
        let [outcome] = outcomes.try_into().expect("one packet per block");
        outcome
    }
}

/// The result an ASK reports opening.
pub(super) fn asked(outcome: &Result<Published, Unpublished>) -> QuestionResultRef {
    match outcome {
        Ok(Published::Sent {
            conversed:
                Some(Conversed {
                    verb: Verb::Ask,
                    result: Some(result),
                    ..
                }),
            ..
        }) => result.clone(),
        other => panic!("the ASK opened a result: {other:?}"),
    }
}

fn refused(outcome: &Result<Published, Unpublished>) -> &str {
    match outcome {
        Err(Unpublished::Refused { reason, .. }) => reason,
        other => panic!("the packet refused: {other:?}"),
    }
}

/// An ASK's one lane delivery carries its result handle as its key and its
/// context as its room; the respondent's ANS returns to the requester and
/// retires its ASK directive in the same write; rereading either packet
/// publishes nothing more.
#[test]
fn an_answer_returns_and_retires_its_question_once() {
    let lanes = Lanes::new("conv-answer");
    let room = lanes.room("dVrD244Jd");
    let born = room.born().expect("fixture handle carries its birth");
    let first = lanes.address(FIRST);

    let asking = lanes.say(SENDER, &format!("ASK {first}#{room}: why?"));
    let result = asked(&asking);
    assert_eq!(
        asking,
        Ok(Published::Sent {
            lanes: vec![test_uuid(FIRST)],
            queued: Vec::new(),
            unreached: Vec::new(),
            rooms: vec![room.clone()],
            conversed: Some(Conversed {
                verb: Verb::Ask,
                operator: false,
                result: Some(result.clone()),
            }),
            held_room: false,
        })
    );
    let [delivery] = lanes
        .pending(FIRST)
        .try_into()
        .expect("the respondent holds one delivery");
    assert_eq!(delivery.key.as_str(), result.as_str());
    assert_eq!(delivery.room.as_ref(), Some(&room));
    let (record, head) = lanes.question(&result);
    assert_eq!(
        record.requester(),
        &RoomDestination::Lane {
            lane: test_uuid(SENDER)
        }
    );
    assert_eq!(
        record.respondent(),
        &RoomDestination::Lane {
            lane: test_uuid(FIRST)
        }
    );
    assert_eq!(record.context(), Some(&born));
    assert_eq!(head.state(), &QuestionResultState::Pending {});

    let answering = lanes.say(FIRST, &format!("ANS {result}#{room}: because"));
    assert_eq!(
        answering,
        Ok(Published::Sent {
            lanes: vec![test_uuid(SENDER)],
            queued: Vec::new(),
            unreached: Vec::new(),
            rooms: vec![room.clone()],
            conversed: Some(Conversed {
                verb: Verb::Ans,
                operator: false,
                result: Some(result.clone()),
            }),
            held_room: false,
        })
    );
    assert!(
        matches!(
            lanes.question(&result).1.state(),
            QuestionResultState::Answered { .. }
        ),
        "the answer settles the result"
    );
    assert!(
        lanes.pending(FIRST).is_empty(),
        "the answer retires the respondent's ASK directive"
    );
    let [returned] = lanes
        .pending(SENDER)
        .try_into()
        .expect("the requester holds the answer");
    assert_eq!(returned.text, "because");
    assert_eq!(returned.room.as_ref(), Some(&room));

    let (_, reread) = lanes.hear_newest(FIRST);
    assert_eq!(reread, [Ok(Published::Already)]);
    let (outcomes, _) = lanes.hear(SENDER, test_uuid(SENDER + 1));
    assert_eq!(outcomes, [Ok(Published::Already)]);
    assert_eq!(lanes.conversations().len(), 2);
    lanes.finish();
}

/// ASK uses its public result handle as the one queued steering key when the
/// named historical address proves a currently vacant logical lane.
#[test]
fn an_ask_to_a_vacant_lane_keeps_its_result_key_and_deadline_until_seated() {
    let lanes = Lanes::new("conv-queued");
    let room = lanes.room("dVrD244Jd");
    let first = lanes.address(FIRST);
    lanes.vacate(FIRST);

    let asking = lanes.say(SENDER, &format!("ASK {first}#{room}: why?"));
    let result = asked(&asking);
    assert!(
        matches!(
            &asking,
            Ok(Published::Sent {
                lanes,
                queued,
                unreached,
                conversed: Some(Conversed { verb: Verb::Ask, .. }),
                ..
            }) if lanes.is_empty() && queued == &[first] && unreached.is_empty()
        ),
        "{asking:?}"
    );
    let pending = lanes.pending(FIRST);
    let [before] = pending.as_slice() else {
        panic!("the vacant respondent lane owns one pending ASK")
    };
    assert_eq!(before.key.as_str(), result.as_str());
    assert!(!before.wakes);
    let before = before.clone();
    let queued_messages = lanes.queued_messages();
    let [(_, queued)] = queued_messages.as_slice() else {
        panic!("the ASK opens exactly one queued native fact")
    };
    assert_eq!(queued.key.as_str(), result.as_str());
    assert_eq!(queued.expires, before.expires);

    lanes.renew(FIRST, test_uuid(40));
    assert_eq!(lanes.pending(FIRST), [before]);
    lanes.finish();
}

/// Only the named respondent answers, only while the result is pending, and
/// an asserted context must equal the stored one; a refused ANS changes
/// nothing.
#[test]
fn only_the_named_respondent_answers_a_pending_result_in_its_context() {
    let lanes = Lanes::new("conv-respondent");
    let room = lanes.room("dVrD244Jd");
    let other = lanes.room("dVrD255Kf");
    let first = lanes.address(FIRST);
    let result = asked(&lanes.say(SENDER, &format!("ASK {first}#{room}: why?")));

    let stranger = lanes.say(SECOND, &format!("ANS {result}: not mine"));
    assert_eq!(refused(&stranger), "ans::respondent-other");
    let elsewhere = lanes.say(FIRST, &format!("ANS {result}#{other}: wrong room"));
    assert_eq!(refused(&elsewhere), "ans::context-mismatch");
    let unknown = lanes.say(FIRST, "ANS dVrD266Lg: no such question");
    assert_eq!(refused(&unknown), "ans::result-unknown");
    assert_eq!(
        lanes.question(&result).1.state(),
        &QuestionResultState::Pending {}
    );
    assert_eq!(lanes.pending(FIRST).len(), 1);
    assert_eq!(lanes.conversations().len(), 1);

    assert!(lanes.say(FIRST, &format!("ANS {result}: because")).is_ok());
    let again = lanes.say(FIRST, &format!("ANS {result}: because"));
    assert_eq!(refused(&again), "ans::result-settled");
    assert_eq!(lanes.pending(SENDER).len(), 1, "one answer returns");
    lanes.finish();
}

/// The respondent's NAK of its live ASK delivery declines the result, after
/// which no answer is admitted.
#[test]
fn a_nak_of_the_live_ask_declines_its_result() {
    let lanes = Lanes::new("conv-nak");
    let first = lanes.address(FIRST);
    let result = asked(&lanes.say(SENDER, &format!("ASK {first}: may I?")));

    lanes.answer(
        FIRST,
        &[result.as_str()],
        &format!("NAK {result}: not this one"),
    );
    assert!(
        matches!(
            lanes.question(&result).1.state(),
            QuestionResultState::Declined { .. }
        ),
        "the NAK declines the result"
    );
    let late = lanes.say(FIRST, &format!("ANS {result}: after all"));
    assert_eq!(refused(&late), "ans::result-settled");
    lanes.finish();
}

/// An operator route opens one pending operator item and wakes no lane; an
/// operator-routed MSG still reaches its seated lane routes.
#[test]
fn operator_routes_open_pending_items() {
    let lanes = Lanes::new("conv-operator");
    let room = lanes.room("dVrD244Jd");
    let first = lanes.address(FIRST);

    let asking = lanes.say(SENDER, &format!("ASK operator#{room}: ship it?"));
    let result = asked(&asking);
    let Ok(Published::Sent {
        lanes: reached,
        conversed: Some(Conversed { operator: true, .. }),
        ..
    }) = &asking
    else {
        panic!("the operator ASK opens an item: {asking:?}");
    };
    assert!(reached.is_empty());
    assert_eq!(
        lanes.question(&result).0.respondent(),
        &RoomDestination::Operator {}
    );

    let telling = lanes.say(SENDER, &format!("MSG operator {first}: shipped"));
    assert!(
        matches!(
            &telling,
            Ok(Published::Sent { lanes: reached, conversed: Some(Conversed { verb: Verb::Msg, operator: true, result: None }), .. })
                if reached == &[test_uuid(FIRST)]
        ),
        "{telling:?}"
    );
    let [_, told] = lanes
        .conversations()
        .try_into()
        .expect("two conversation records");
    for item in [
        RoomReceiptRef::new(result.authority().clone()),
        RoomReceiptRef::new(told.publication),
    ] {
        let opened = lanes
            .bus
            .read(move |read| read.room_item(&item))
            .expect("items read");
        assert!(
            matches!(
                opened.as_ref().map(|(_, head)| head.body().state()),
                Some(wire::rooms::RoomReceiptState::Pending { .. })
            ),
            "each operator route opens one pending item: {opened:?}"
        );
    }
    assert!(lanes.pending(SENDER).is_empty());
    lanes.finish();
}

/// The operator reads a lane's operator ASK body from the author's transcript,
/// the bytes `spice room inbox` prints, and an unknown publication reads as
/// missing rather than as some other text.
#[test]
fn an_operator_item_body_reads_back_from_its_authoring_transcript() {
    let lanes = Lanes::new("conv-operator-text");
    let room = lanes.room("dVrD244Jd");
    let asking = lanes.say(SENDER, &format!("ASK operator#{room}: ship it?"));
    asked(&asking);
    let [asked] = lanes
        .conversations()
        .try_into()
        .expect("one conversation record");
    let read = |publication| {
        let outcome = crate::room_command::conversation_text(
            &lanes.bus,
            &lanes.paths.tape,
            &wire::intent::RoomConversationText { publication },
        );
        let wire::Outcome::Reply(reply) = outcome else {
            panic!("the conversation text read answers: {outcome:?}");
        };
        let wire::intent::CommandAnswer::RoomConversationText(answer) = reply.body else {
            panic!("another answer shape: {:?}", reply.body);
        };
        answer.content
    };
    assert_eq!(
        read(asked.publication.clone()),
        wire::intent::RoomConversationContent::Text {
            text: "ship it?".to_owned()
        }
    );
    assert_eq!(
        read("dVrD244Jz".parse().expect("fixture publication")),
        wire::intent::RoomConversationContent::Unavailable {
            unavailable: wire::rooms::RoomSourceUnavailable::Missing {}
        }
    );
    lanes.finish();
}

/// An ASK whose one address proves no occupant publishes nothing.
#[test]
fn an_ask_to_an_unproved_audience_publishes_nothing() {
    let lanes = Lanes::new("conv-unproved");
    let unproved =
        wire::OccupantAddress::from(wire::Seq::new(9_999_999).expect("nonzero position"));

    let asking = lanes.say(SENDER, &format!("ASK {unproved}: anyone?"));
    assert_eq!(refused(&asking), "msg::recipient-unavailable");
    assert!(lanes.conversations().is_empty());
    assert!(lanes.messages().is_empty());
    assert!(lanes.queued_messages().is_empty());
    lanes.finish();
}
