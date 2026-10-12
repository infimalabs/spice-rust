//! The resident times out a pending question result at its deadline and lapses
//! a pending operator item at its expiry, each exactly once and only while the
//! head it armed from is unmoved.

use board::ProjectPolicy;
use journal::{FactCause, JournalWrite};
use wire::rooms::{
    QuestionResultRef, QuestionResultState, RoomQuestionTimedOut, RoomReceiptRecovered,
    RoomReceiptRef, RoomReceiptState,
};
use wire::{Actor, Instant, Producer};

use super::conversation::asked;
use super::*;
use crate::resident::timers::{Fired, RoomDue, Runtime};

impl Lanes {
    pub(super) fn timers(&self) -> Runtime {
        let mut timers = Runtime::start(
            ProjectPolicy::new(
                ["timer".to_owned()],
                Vec::new(),
                dialect::PROJECT_MIN_DEPTH as u64,
                dialect::PROJECT_MAX_DEPTH as u64,
            )
            .expect("fixture projects"),
            steer::Config::default(),
            self.root.join("supervisors"),
        )
        .expect("timer runtime starts");
        timers.synchronize(&self.bus).expect("timers arm");
        timers
    }

    fn result_state(&self, result: &QuestionResultRef) -> (wire::Seq, QuestionResultState) {
        let result = result.clone();
        let (_, head) = self
            .bus
            .read(move |read| read.question_result(&result))
            .expect("question reads")
            .expect("the ASK opened its result");
        (head.position(), head.body().state().clone())
    }

    pub(super) fn item_state(&self, item: &RoomReceiptRef) -> RoomReceiptState {
        let item = item.clone();
        let (_, head) = self
            .bus
            .read(move |read| read.room_item(&item))
            .expect("items read")
            .expect("the operator route opened its item");
        head.body().state().clone()
    }

    fn timeouts(&self) -> usize {
        self.bus
            .fact_snapshot()
            .expect("facts read")
            .facts()
            .iter()
            .filter(|envelope| envelope.fact.read::<RoomQuestionTimedOut>().is_some())
            .count()
    }
}

fn shifted(at: Instant, millis: i64) -> Instant {
    Instant::new(
        at.get()
            .checked_add(jiff::SignedDuration::from_millis(millis))
            .expect("fixture instant shifts"),
    )
    .expect("fixture instant is canonical")
}

/// A result is not due before its deadline and times out once at it, without
/// moving its deadline or its operator item; the item lapses on its own at its
/// expiry without moving the result, and the operator's recovery re-arms it.
#[test]
fn due_results_time_out_and_due_items_lapse_once_each() {
    let lanes = Lanes::new("deadline-due");
    let result = asked(&lanes.say(SENDER, "ASK operator: ship it?"));
    let item = RoomReceiptRef::new(result.authority().clone());
    let (asked_due, item_due) = (RoomDue::Result(result.clone()), RoomDue::Item(item.clone()));
    let mut timers = lanes.timers();
    let deadline = lanes.question(&result).0.deadline();
    assert_eq!(timers.room_deadline(&asked_due), Some(deadline));
    let RoomReceiptState::Pending { expires } = lanes.item_state(&item) else {
        panic!("the operator item opens pending");
    };
    assert_eq!(timers.room_deadline(&item_due), Some(expires));

    let early = timers.fire_room_at(&lanes.bus, &asked_due, shifted(deadline, -1));
    assert_eq!(early, Ok(Fired::Stale));
    assert_eq!(
        lanes.result_state(&result).1,
        QuestionResultState::Pending {}
    );
    assert_eq!(
        timers.room_deadline(&asked_due),
        Some(deadline),
        "an early wake re-arms its deadline"
    );

    assert_eq!(
        timers.fire_room_at(&lanes.bus, &asked_due, deadline),
        Ok(Fired::Room)
    );
    let (settled, state) = lanes.result_state(&result);
    assert_eq!(state, QuestionResultState::TimedOut {});
    assert_eq!(lanes.question(&result).0.deadline(), deadline);
    assert!(matches!(
        lanes.item_state(&item),
        RoomReceiptState::Pending { .. }
    ));
    timers.synchronize(&lanes.bus).expect("timers re-fold");
    assert_eq!(timers.room_deadline(&asked_due), None);
    assert_eq!(
        timers.fire_room_at(&lanes.bus, &asked_due, deadline),
        Ok(Fired::Stale)
    );
    assert_eq!(lanes.timeouts(), 1);

    assert_eq!(
        timers.fire_room_at(&lanes.bus, &item_due, expires),
        Ok(Fired::Room)
    );
    assert_eq!(
        lanes.item_state(&item),
        RoomReceiptState::Expired { at: expires }
    );
    assert_eq!(
        lanes.result_state(&result).0,
        settled,
        "a lapse never moves the result"
    );
    timers.synchronize(&lanes.bus).expect("timers re-fold");
    assert_eq!(timers.room_deadline(&item_due), None);

    let renewed = shifted(expires, 60_000);
    let mut recovery = JournalWrite::new(
        Actor::Operator,
        format!("resident:{}", lanes.epoch)
            .parse()
            .expect("fixture origin"),
        shifted(expires, 1),
        Producer::CommandDoor,
    );
    recovery
        .fact(
            RoomReceiptRecovered {
                receipt: item.clone(),
                expires: renewed,
            },
            FactCause::Root,
        )
        .expect("recovery stages");
    assert!(matches!(
        lanes.bus.commit(recovery).expect("recovery commits"),
        bus::CommitOutcome::Committed(_)
    ));
    timers.synchronize(&lanes.bus).expect("timers re-fold");
    assert_eq!(timers.room_deadline(&item_due), Some(renewed));
    drop(timers);
    lanes.finish();
}

/// An answer that commits before the fired timeout wins, and the timer writes
/// nothing for its result.
#[test]
fn an_answer_committed_first_wins_over_the_timeout() {
    let lanes = Lanes::new("deadline-race");
    let first = lanes.address(FIRST);
    let result = asked(&lanes.say(SENDER, &format!("ASK {first}: why?")));
    let due = RoomDue::Result(result.clone());
    let mut timers = lanes.timers();
    let deadline = timers.room_deadline(&due).expect("the result arms");

    assert!(lanes.say(FIRST, &format!("ANS {result}: because")).is_ok());
    assert_eq!(
        timers.fire_room_at(&lanes.bus, &due, deadline),
        Ok(Fired::Stale)
    );
    assert!(matches!(
        lanes.result_state(&result).1,
        QuestionResultState::Answered { .. }
    ));
    assert_eq!(lanes.timeouts(), 0);
    drop(timers);
    lanes.finish();
}

/// A resident started after a result was opened arms it from durable state
/// alone, and of two residents holding the same captured head only one writes
/// its timeout.
#[test]
fn restarted_residents_catch_up_one_timeout() {
    let lanes = Lanes::new("deadline-restart");
    let result = asked(&lanes.say(SENDER, "ASK operator: ship it?"));
    let due = RoomDue::Result(result.clone());
    let deadline = lanes.question(&result).0.deadline();
    let mut before = lanes.timers();
    let mut after = lanes.timers();
    assert_eq!(after.room_deadline(&due), Some(deadline));

    assert_eq!(
        after.fire_room_at(&lanes.bus, &due, shifted(deadline, 5_000)),
        Ok(Fired::Room)
    );
    assert_eq!(
        before.fire_room_at(&lanes.bus, &due, shifted(deadline, 5_000)),
        Ok(Fired::Stale)
    );
    assert_eq!(
        lanes.result_state(&result).1,
        QuestionResultState::TimedOut {}
    );
    assert_eq!(lanes.timeouts(), 1);
    drop((before, after));
    lanes.finish();
}
