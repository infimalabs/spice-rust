//! Operator controls settle only the live receipts and results the operator
//! owes; lane closures bind to stored parties; an operator closure for a lane
//! party is an explicit, attributed override. Room reads name each receipt,
//! result, and decision author apart, and change none of them.

use serde_json::{Value, json};
use wire::intent::{RoomClose, RoomControl};
use wire::rooms::{
    QuestionClosure, QuestionResultRef, QuestionResultState, RoomReceiptDisposition,
    RoomReceiptRef, RoomReceiptState, RoomRelevanceVerdict,
};

use super::conversation::asked;
use super::*;
use crate::resident::timers::{Fired, RoomDue};

#[path = "packet_context.rs"]
mod packet_context;

impl Lanes {
    fn admitted(
        &self,
        id: u8,
        actor: Actor,
        kind: CommandKind,
        body: &impl serde::Serialize,
    ) -> wire::AdmittedCommand {
        Command {
            v: wire::WireVersion::CURRENT,
            id: test_uuid(id),
            epoch: self.epoch,
            kind,
            expected: Vec::new(),
            body: serde_json::to_value(body).expect("control encodes"),
        }
        .admit(&wire::Accepted {
            wire: wire::WireVersion::CURRENT,
            actor,
            epoch: self.epoch,
            release: "1.2.3".parse().expect("fixture release"),
        })
        .expect("control admits")
    }

    fn operate(&self, id: u8, control: RoomControl) -> Outcome<CommandAnswer> {
        let command = self.admitted(id, Actor::Operator, CommandKind::RoomControl, &control);
        crate::board_command::room_control(&self.bus, &command, &control, &steer::Config::default())
            .0
    }

    fn respond(&self, id: u8, result: &QuestionResultRef, text: &str) -> Outcome<CommandAnswer> {
        self.operate(
            id,
            RoomControl::Answer {
                result: result.clone(),
                context: None,
                text: text.to_owned(),
            },
        )
    }

    fn dispose(
        &self,
        id: u8,
        disposition: RoomReceiptDisposition,
        keys: &[RoomReceiptRef],
        clause: &str,
    ) -> Outcome<CommandAnswer> {
        self.operate(
            id,
            RoomControl::Dispose {
                disposition,
                keys: keys.to_vec(),
                clause: clause.to_owned(),
            },
        )
    }

    fn close(
        &self,
        id: u8,
        actor: Actor,
        result: &QuestionResultRef,
        closure: QuestionClosure,
        override_party: bool,
    ) -> Outcome<CommandAnswer> {
        let close = RoomClose {
            result: result.clone(),
            closure,
            clause: "not this one".to_owned(),
            override_party,
        };
        let command = self.admitted(id, actor, CommandKind::RoomClose, &close);
        crate::board_command::room_close(&self.bus, &command, &close)
    }

    /// `spice room WORDS...` read in process, as its JSON reply body.
    pub(super) fn room_read(&self, words: &[&str]) -> Value {
        let matches = crate::cli::command()
            .try_get_matches_from(["spice", "room"].iter().chain(words))
            .expect("room read parses");
        let contract = crate::cli::selected_contract(&matches).expect("room read is registered");
        let leaf = crate::cli::selected_matches(&matches);
        let journal =
            journal::JournalReader::open(&self.paths.journal).expect("the journal opens to read");
        let body = match contract.path {
            ["room", "inbox"] => match crate::room_command::inbox::answer(
                &journal,
                &self.paths.tape,
                leaf,
                contract.path,
            ) {
                Outcome::Reply(reply) => serde_json::to_value(&reply.body),
                other => panic!("room inbox answered {other:?}"),
            },
            ["room", "result"] => {
                match crate::room_command::result::answer(&journal, &self.paths.tape, leaf) {
                    Outcome::Reply(reply) => serde_json::to_value(&reply.body),
                    other => panic!("room result answered {other:?}"),
                }
            }
            ["room", path @ ("relevance" | "awareness")] => {
                let projects =
                    board::ProjectPolicy::new(Vec::new(), 2, 3).expect("fixture project policy");
                if *path == "relevance" {
                    match crate::room_command::relevance::answer(
                        &journal,
                        &projects,
                        leaf,
                        contract.path,
                    ) {
                        Outcome::Reply(reply) => serde_json::to_value(&reply.body),
                        other => panic!("room relevance answered {other:?}"),
                    }
                } else {
                    match crate::room_command::awareness::answer(
                        &journal,
                        &projects,
                        leaf,
                        contract.path,
                    ) {
                        Outcome::Reply(reply) => serde_json::to_value(&reply.body),
                        other => panic!("room awareness answered {other:?}"),
                    }
                }
            }
            other => panic!("not a room read: {other:?}"),
        };
        body.expect("the read encodes")
    }

    /// Every open operator item key, following each inbox page's
    /// continuation from `words`.
    fn inbox_items(&self, words: &[&str]) -> Vec<(String, String)> {
        let mut words = words
            .iter()
            .map(|word| (*word).to_owned())
            .collect::<Vec<_>>();
        let mut items = Vec::new();
        for _ in 0..8 {
            let read = ["inbox"]
                .into_iter()
                .chain(words.iter().map(String::as_str))
                .collect::<Vec<_>>();
            let page = self.room_read(&read);
            items.extend(
                page["page"]["rows"]
                    .as_array()
                    .expect("rows")
                    .iter()
                    .map(|row| {
                        (
                            row["receipt"]["item"]
                                .as_str()
                                .expect("item key")
                                .to_owned(),
                            row["verb"].as_str().expect("verb").to_owned(),
                        )
                    }),
            );
            let continuation = &page["page"]["omission"]["continuation"];
            if continuation.is_null() {
                return items;
            }
            words = serde_json::from_value::<Vec<String>>(continuation.clone())
                .expect("a continuation is one argument list")
                .split_off(2);
        }
        panic!("paging did not finish: {items:?}");
    }
}

fn reason(outcome: &Outcome<CommandAnswer>) -> &str {
    match outcome {
        Outcome::Refusal(refusal) => refusal.reason(),
        other => panic!("the control refused: {other:?}"),
    }
}

fn committed(outcome: &Outcome<CommandAnswer>) {
    assert!(
        matches!(
            outcome,
            Outcome::Reply(Reply {
                position: ReplyPosition::Committed(_),
                ..
            })
        ),
        "the control committed: {outcome:?}"
    );
}

fn item_of(result: &QuestionResultRef) -> RoomReceiptRef {
    RoomReceiptRef::new(result.authority().clone())
}

fn actor(actor: Actor) -> Value {
    serde_json::to_value(actor).expect("actor encodes")
}

/// The operator answers only a pending question it is the respondent of;
/// the answer settles the result and its item and returns once to the
/// requester.
#[test]
fn the_operator_answers_only_its_own_pending_questions() {
    let lanes = Lanes::new("control-answer");
    let room = lanes.room("dVrD244Jd");
    let first = lanes.address(FIRST);
    let key = lanes.direct(SENDER, 20, "ship when green");
    let mine = asked(&lanes.say(SENDER, &format!("ASK #{key}: ship it?")));
    let theirs = asked(&lanes.say(SENDER, &format!("ASK {first}#{room}: deploy?")));

    assert_eq!(
        reason(&lanes.respond(60, &theirs, "yes")),
        "room::not-respondent"
    );
    assert_eq!(
        reason(&lanes.respond(61, &mine, "  ")),
        "room::answer-empty"
    );
    assert_eq!(
        lanes.question(&mine).1.state(),
        &QuestionResultState::Pending {}
    );

    committed(&lanes.respond(62, &mine, "ship"));
    assert!(matches!(
        lanes.question(&mine).1.state(),
        QuestionResultState::Answered { .. }
    ));
    assert!(matches!(
        lanes.item_state(&item_of(&mine)),
        RoomReceiptState::Acked { .. }
    ));
    let returned = lanes
        .pending(SENDER)
        .into_iter()
        .filter(|directive| directive.text == format!("ANS {mine}: ship"))
        .collect::<Vec<_>>();
    assert_eq!(
        returned.len(),
        1,
        "the answer returns once to its requester"
    );

    assert_eq!(
        reason(&lanes.respond(63, &mine, "again")),
        "room::result-settled"
    );
    assert_eq!(
        reason(&lanes.dispose(64, RoomReceiptDisposition::Ack, &[item_of(&mine)], "seen")),
        "room::item-settled"
    );

    let read = lanes.room_read(&["result", mine.as_str()]);
    assert_eq!(read["answer"]["text"], "ship");
    assert_eq!(read["result"]["returned"]["kind"], "directive");
    assert_eq!(
        read["result"]["returned"]["receipt"]["lane"],
        json_lane(SENDER)
    );
    assert!(read["result"]["closed_by"].is_null());

    assert!(lanes.inbox_items(&[]).is_empty());
    lanes.finish();
}

fn json_lane(lane: u8) -> Value {
    serde_json::to_value(test_uuid(lane)).expect("lane encodes")
}

/// Only a question's stored party closes it; the operator closes for a lane
/// party only by explicit override, which the result read attributes.
#[test]
fn closures_bind_to_stored_parties_and_attribute_overrides() {
    let lanes = Lanes::new("control-close");
    let room = lanes.room("dVrD244Jd");
    let first = lanes.address(FIRST);
    let lane_asked = asked(&lanes.say(SENDER, &format!("ASK {first}#{room}: deploy?")));
    let key = lanes.direct(SENDER, 20, "ship when green");
    let operator_asked = asked(&lanes.say(SENDER, &format!("ASK #{key}: ship?")));

    let declined = QuestionClosure::Declined;
    let cancelled = QuestionClosure::Cancelled;
    let sender = Actor::Lane(test_uuid(SENDER));
    for (id, actor, closure, override_party, refused) in [
        (
            65,
            Actor::Lane(test_uuid(SECOND)),
            cancelled,
            false,
            "room::not-party",
        ),
        (66, sender.clone(), declined, false, "room::not-party"),
        (
            67,
            Actor::Operator,
            declined,
            false,
            "room::override-required",
        ),
        (
            68,
            sender.clone(),
            cancelled,
            true,
            "room::override-operator-only",
        ),
    ] {
        assert_eq!(
            reason(&lanes.close(id, actor, &lane_asked, closure, override_party)),
            refused
        );
    }
    assert_eq!(
        reason(&lanes.close(69, Actor::Operator, &operator_asked, declined, true)),
        "room::override-unneeded"
    );
    assert_eq!(
        lanes.question(&lane_asked).1.state(),
        &QuestionResultState::Pending {},
        "refused closures change nothing"
    );

    committed(&lanes.close(70, Actor::Operator, &lane_asked, declined, true));
    let read = lanes.room_read(&["result", lane_asked.as_str()]);
    assert_eq!(read["result"]["closed_by"], actor(Actor::Operator));
    assert_eq!(read["result"]["override"], true);

    committed(&lanes.close(71, sender.clone(), &operator_asked, cancelled, false));
    let read = lanes.room_read(&["result", operator_asked.as_str()]);
    assert_eq!(read["result"]["closed_by"], actor(sender));
    assert!(read["result"]["override"].is_null());
    assert!(matches!(
        lanes.item_state(&item_of(&operator_asked)),
        RoomReceiptState::Cancelled { .. }
    ));
    assert_eq!(
        reason(&lanes.close(72, Actor::Operator, &operator_asked, declined, false)),
        "room::result-settled"
    );
    lanes.finish();
}

/// The inbox pages every open operator item whole and resumably; an ACK
/// settles a MSG item, one NAK reports and declines every ASK result it
/// names, and settled items leave the inbox.
#[test]
fn operator_items_page_and_settle_exactly() {
    let lanes = Lanes::new("control-items");
    let room = lanes.room("dVrD244Jd");
    let key = lanes.direct(SENDER, 20, "ship when green");
    lanes
        .say(SENDER, &format!("MSG #{key}: shipped"))
        .expect("the MSG publishes");
    let first = asked(&lanes.say(SENDER, &format!("ASK #{key}: and then?")));
    let second = asked(&lanes.say(SENDER, &format!("ASK #{key}: anything else?")));

    let items = lanes.inbox_items(&["--limit", "1"]);
    assert_eq!(
        items
            .iter()
            .map(|(_, verb)| verb.as_str())
            .collect::<Vec<_>>(),
        ["msg", "ask", "ask"],
        "single-row pages resume at exactly the withheld item"
    );
    assert_eq!(items, lanes.inbox_items(&[]));
    let told: RoomReceiptRef = RoomReceiptRef::new(items[0].0.parse().expect("item key"));
    assert_eq!(lanes.inbox_items(&[]), items, "reading settles nothing");

    let unknown = RoomReceiptRef::new(lanes.post(&room, 1, "aside"));
    assert_eq!(
        reason(&lanes.dispose(73, RoomReceiptDisposition::Ack, &[unknown], "seen")),
        "room::item-unknown"
    );
    assert_eq!(
        reason(&lanes.dispose(74, RoomReceiptDisposition::Ack, &[told.clone()], " ")),
        "room::clause-required"
    );
    let declined = lanes.dispose(
        75,
        RoomReceiptDisposition::Nak,
        &[item_of(&second), item_of(&first)],
        "no",
    );
    let Outcome::Reply(Reply {
        position: ReplyPosition::Committed(_),
        body: CommandAnswer::RoomControl(answer),
    }) = declined
    else {
        panic!("both questions declined: {declined:?}");
    };
    assert_eq!(
        answer
            .results
            .iter()
            .map(|moved| (&moved.result, moved.state.as_str()))
            .collect::<Vec<_>>(),
        [(&second, "declined"), (&first, "declined")],
        "the reply reports every result in input-item order, not key order"
    );
    for question in [&first, &second] {
        assert!(matches!(
            lanes.question(question).1.state(),
            QuestionResultState::Declined { .. }
        ));
    }
    committed(&lanes.dispose(76, RoomReceiptDisposition::Ack, &[told.clone()], "seen"));
    assert!(matches!(
        lanes.item_state(&told),
        RoomReceiptState::Acked { .. }
    ));
    assert!(lanes.inbox_items(&[]).is_empty());
    lanes.finish();
}

/// Settling a room's anchor closes it to new posts only: a receipt or
/// question already open in it still acknowledges, answers, and declines.
#[test]
fn open_room_duties_still_settle_after_the_anchor_settles() {
    let lanes = Lanes::new("control-settled-anchor");
    let room = lanes.room("dVrD244Jd");
    let first = lanes.address(FIRST);
    let key = lanes.direct(SENDER, 20, "ship when green");
    lanes
        .say(SENDER, &format!("MSG #{key} #{room}: shipped"))
        .expect("the MSG publishes");
    let answered = asked(&lanes.say(SENDER, &format!("ASK {first}#{room}: ship it?")));
    let declined = asked(&lanes.say(SENDER, &format!("ASK {first}#{room}: and then?")));
    let items = lanes.inbox_items(&[]);
    let told = RoomReceiptRef::new(items[0].0.parse().expect("item key"));
    lanes.close_room(&room);
    let born = room.born().expect("fixture handle carries its stamp");
    let anchor = lanes
        .bus
        .read(move |read| read.workflow_head(&born))
        .expect("anchor workflow reads")
        .expect("anchor exists");
    assert!(
        anchor.body().completion.is_some() && anchor.body().review == "closed",
        "the anchor is settled before its open duties settle"
    );

    assert!(lanes.say(FIRST, &format!("ANS {answered}: ship")).is_ok());
    assert!(matches!(
        lanes.question(&answered).1.state(),
        QuestionResultState::Answered { .. }
    ));
    lanes.answer(FIRST, &[declined.as_str()], &format!("NAK {declined}: no"));
    assert!(matches!(
        lanes.question(&declined).1.state(),
        QuestionResultState::Declined { .. }
    ));
    committed(&lanes.dispose(62, RoomReceiptDisposition::Ack, &[told.clone()], "seen"));
    assert!(matches!(
        lanes.item_state(&told),
        RoomReceiptState::Acked { .. }
    ));
    lanes.finish();
}

/// A lapsed operator item stays open but refuses settlement until the
/// operator recovers it to pending with a fresh expiry; a timed-out result is
/// terminal for every later answer or closure.
#[test]
fn lapsed_items_recover_and_timed_out_results_stay_terminal() {
    let lanes = Lanes::new("control-lapse");
    let key = lanes.direct(SENDER, 20, "ship when green");
    let result = asked(&lanes.say(SENDER, &format!("ASK #{key}: ship it?")));
    let item = item_of(&result);
    let recover = |id| lanes.operate(id, RoomControl::Recover { item: item.clone() });
    let mut timers = lanes.timers();
    let RoomReceiptState::Pending { expires } = lanes.item_state(&item) else {
        panic!("the operator item opens pending");
    };
    assert_eq!(reason(&recover(80)), "room::item-live");
    assert_eq!(
        timers.fire_room_at(&lanes.bus, &RoomDue::Item(item.clone()), expires),
        Ok(Fired::Room)
    );
    assert_eq!(
        reason(&lanes.dispose(81, RoomReceiptDisposition::Ack, &[item.clone()], "seen")),
        "room::item-lapsed"
    );
    assert_eq!(
        lanes.inbox_items(&[]),
        [(item.to_string(), "ask".to_owned())],
        "a lapsed item still owes a disposition"
    );

    committed(&recover(82));
    let RoomReceiptState::Pending { expires: renewed } = lanes.item_state(&item) else {
        panic!("recovery returns the item to pending");
    };
    assert!(renewed > expires, "recovery re-arms a later expiry");
    assert_eq!(reason(&recover(83)), "room::item-live");

    let deadline = lanes.question(&result).0.deadline();
    assert_eq!(
        timers.fire_room_at(&lanes.bus, &RoomDue::Result(result.clone()), deadline),
        Ok(Fired::Room)
    );
    assert_eq!(
        reason(&lanes.respond(84, &result, "late")),
        "room::result-settled"
    );
    assert_eq!(
        reason(&lanes.close(
            85,
            Actor::Operator,
            &result,
            QuestionClosure::Declined,
            false
        )),
        "room::result-settled"
    );
    let read = lanes.room_read(&["result", result.as_str()]);
    assert_eq!(
        serde_json::to_value(&read["result"]["state"]).expect("state"),
        serde_json::to_value(QuestionResultState::TimedOut {}).expect("state encodes")
    );
    drop(timers);
    lanes.finish();
}

/// Each settled question asked in a room is observed once in shadow; an
/// answer that only agrees is unknown, not progress, and a decline is no
/// progress. Observing moves no delivery or result, and the operator
/// assesses each observation exactly once.
#[test]
fn settled_room_questions_are_observed_once_and_assessed_by_the_operator() {
    let lanes = Lanes::new("control-relevance");
    let room = lanes.room("dVrD244Jd");
    let other = lanes.room("dVrD245Kc");
    let first = lanes.address(FIRST);
    let answered = asked(&lanes.say(SENDER, &format!("ASK {first}#{room}: ship it?")));
    let declined = asked(&lanes.say(SENDER, &format!("ASK {first}#{room}: deploy?")));
    let other_room = asked(&lanes.say(SENDER, &format!("ASK {first}#{other}: lunch?")));
    let pending = asked(&lanes.say(SENDER, &format!("ASK {first}#{room}: and then?")));
    assert!(lanes.say(FIRST, &format!("ANS {answered}: agreed")).is_ok());
    committed(&lanes.close(
        91,
        Actor::Lane(test_uuid(FIRST)),
        &declined,
        QuestionClosure::Declined,
        false,
    ));
    assert!(lanes.say(FIRST, &format!("ANS {other_room}: soup")).is_ok());

    let directives = (lanes.pending(SENDER).len(), lanes.pending(FIRST).len());
    let mut diagnostic = Vec::new();
    for _ in 0..2 {
        crate::resident::room_relevance::reconcile(&lanes.bus, lanes.epoch, &mut diagnostic)
            .expect("observation reconciles");
    }
    assert!(
        diagnostic.is_empty(),
        "{}",
        String::from_utf8_lossy(&diagnostic)
    );
    assert_eq!(
        (lanes.pending(SENDER).len(), lanes.pending(FIRST).len()),
        directives,
        "observing opens and settles no delivery"
    );
    assert_eq!(
        lanes.question(&pending).1.state(),
        &QuestionResultState::Pending {}
    );

    let read = lanes.room_read(&["relevance", room.as_str()]);
    let metrics = &read["metrics"];
    assert_eq!(metrics["observations"], 2, "each settled room result once");
    assert_eq!(metrics["relevant_contributions"], 0);
    assert_eq!(metrics["unknown_observations"], 1);
    assert_eq!(metrics["declines"], 1);
    assert_eq!(metrics["progress"], 0, "an agreeing answer is not progress");
    assert_eq!(metrics["unassessed"], 2);
    let rows = read["page"]["rows"].as_array().expect("rows");
    let classified = |result: &QuestionResultRef| {
        rows.iter()
            .find(|row| row["observation"]["observation"] == result.as_str())
            .map(|row| {
                (
                    row["observation"]["classification"].clone(),
                    row["observation"]["progress"].clone(),
                )
            })
    };
    assert_eq!(
        classified(&answered),
        Some((json!("unknown"), json!("unknown")))
    );
    assert_eq!(
        classified(&declined),
        Some((json!("declined"), json!("no-progress")))
    );

    let assess = |id, observation: &QuestionResultRef, explanation: &str| {
        lanes.operate(
            id,
            RoomControl::Assess {
                observation: observation.authority().clone(),
                verdict: RoomRelevanceVerdict::FalsePositive,
                explanation: explanation.to_owned(),
            },
        )
    };
    assert_eq!(
        reason(&assess(93, &answered, " ")),
        "room::assessment-invalid"
    );
    assert_eq!(
        reason(&assess(94, &pending, "not observed")),
        "room::observation-unknown"
    );
    committed(&assess(
        95,
        &answered,
        "the answer did not settle the criterion",
    ));
    assert_eq!(
        reason(&assess(96, &answered, "again")),
        "room::observation-assessed"
    );

    let read = lanes.room_read(&["relevance", room.as_str(), "--limit", "1"]);
    assert_eq!(read["metrics"]["false_positives"], 1);
    assert_eq!(read["metrics"]["unassessed"], 1);
    assert_eq!(read["page"]["rows"].as_array().map(Vec::len), Some(1));
    let continuation =
        serde_json::from_value::<Vec<String>>(read["page"]["omission"]["continuation"].clone())
            .expect("a withheld observation names its continuation")
            .split_off(2);
    let rest = lanes.room_read(
        &["relevance"]
            .into_iter()
            .chain(continuation.iter().map(String::as_str))
            .collect::<Vec<_>>(),
    );
    let seen = read["page"]["rows"]
        .as_array()
        .into_iter()
        .chain(rest["page"]["rows"].as_array())
        .flatten()
        .map(|row| row["observation"]["observation"].clone())
        .collect::<Vec<_>>();
    assert_eq!(seen.len(), 2, "pages visit each observation once: {seen:?}");
    assert_ne!(seen[0], seen[1]);
    lanes.finish();
}
