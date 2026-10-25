//! The resident lapses a pending operator item at its expiry exactly once and
//! only while the head it armed from is unmoved.

use journal::{FactCause, JournalWrite};
use wire::rooms::{RoomReceiptRecovered, RoomReceiptRef, RoomReceiptState};
use wire::{Actor, Instant, Producer};

use super::*;
use crate::resident::timers::{Fired, RoomDue, Runtime};

impl Lanes {
    pub(super) fn timers(&self) -> Runtime {
        let mut timers = Runtime::start(steer::Config::default(), self.root.join("supervisors"))
            .expect("timer runtime starts");
        timers.synchronize(&self.bus).expect("timers arm");
        timers
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

    /// The operator item an MSG about a fresh direction opens, and its expiry.
    fn operator_item(&self) -> (RoomReceiptRef, Instant) {
        let key = self.direct(SENDER, 20, "ship when green");
        self.say(SENDER, &format!("MSG #{key}: ship it?"))
            .expect("the operator MSG publishes");
        let [conversed] = self
            .conversations()
            .try_into()
            .expect("one conversation record");
        let item = RoomReceiptRef::new(conversed.publication);
        let RoomReceiptState::Pending { expires } = self.item_state(&item) else {
            panic!("the operator item opens pending");
        };
        (item, expires)
    }

    /// Commit the operator's recovery of `item` under `origin`, renewing it to
    /// `renewed`.
    fn recover_item(&self, item: &RoomReceiptRef, origin: &str, at: Instant, renewed: Instant) {
        let mut recovery = JournalWrite::new(
            Actor::Operator,
            origin.parse().expect("fixture origin"),
            at,
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
            self.bus.commit(recovery).expect("recovery commits"),
            bus::CommitOutcome::Committed(_)
        ));
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

/// An item is not due before its expiry and lapses once at it, and the
/// operator's recovery re-arms it.
#[test]
fn due_items_lapse_once_and_recovery_rearms_them() {
    let lanes = Lanes::new("deadline-due");
    let (item, expires) = lanes.operator_item();
    let due = RoomDue::Item(item.clone());
    let mut timers = lanes.timers();
    assert_eq!(timers.room_deadline(&due), Some(expires));

    assert_eq!(
        timers.fire_room_at(&lanes.bus, &due, shifted(expires, -1)),
        Ok(Fired::Stale)
    );
    assert!(matches!(
        lanes.item_state(&item),
        RoomReceiptState::Pending { .. }
    ));
    assert_eq!(
        timers.room_deadline(&due),
        Some(expires),
        "an early wake re-arms its expiry"
    );

    assert_eq!(
        timers.fire_room_at(&lanes.bus, &due, expires),
        Ok(Fired::Room)
    );
    assert_eq!(
        lanes.item_state(&item),
        RoomReceiptState::Expired { at: expires }
    );
    timers.synchronize(&lanes.bus).expect("timers re-fold");
    assert_eq!(timers.room_deadline(&due), None);
    assert_eq!(
        timers.fire_room_at(&lanes.bus, &due, expires),
        Ok(Fired::Stale)
    );

    let renewed = shifted(expires, 60_000);
    lanes.recover_item(
        &item,
        &format!("resident:{}", lanes.epoch),
        shifted(expires, 1),
        renewed,
    );
    timers.synchronize(&lanes.bus).expect("timers re-fold");
    assert_eq!(timers.room_deadline(&due), Some(renewed));
    drop(timers);
    lanes.finish();
}

/// An item whose head another origin wrote, as an earlier epoch's resident
/// recovering it, lapses again under the current resident.
#[test]
fn items_written_under_another_origin_still_lapse() {
    let lanes = Lanes::new("deadline-origin");
    let (item, expires) = lanes.operator_item();
    let due = RoomDue::Item(item.clone());
    let mut timers = lanes.timers();
    assert_eq!(
        timers.fire_room_at(&lanes.bus, &due, expires),
        Ok(Fired::Room)
    );
    let renewed = shifted(expires, 60_000);
    lanes.recover_item(
        &item,
        "resident:018f82d4-6f59-7d06-92e3-bce913116400",
        shifted(expires, 1),
        renewed,
    );
    timers.synchronize(&lanes.bus).expect("timers re-fold");
    assert_eq!(timers.room_deadline(&due), Some(renewed));
    assert_eq!(
        timers.fire_room_at(&lanes.bus, &due, renewed),
        Ok(Fired::Room)
    );
    assert_eq!(
        lanes.item_state(&item),
        RoomReceiptState::Expired { at: renewed }
    );
    drop(timers);
    lanes.finish();
}

/// A resident started after an item was opened arms it from durable state
/// alone, and of two residents holding the same captured head only one writes
/// its lapse.
#[test]
fn restarted_residents_catch_up_one_lapse() {
    let lanes = Lanes::new("deadline-restart");
    let (item, expires) = lanes.operator_item();
    let due = RoomDue::Item(item.clone());
    let mut before = lanes.timers();
    let mut after = lanes.timers();
    assert_eq!(after.room_deadline(&due), Some(expires));

    let late = shifted(expires, 5_000);
    assert_eq!(after.fire_room_at(&lanes.bus, &due, late), Ok(Fired::Room));
    assert_eq!(
        before.fire_room_at(&lanes.bus, &due, late),
        Ok(Fired::Stale)
    );
    assert_eq!(
        lanes.item_state(&item),
        RoomReceiptState::Expired { at: late }
    );
    drop((before, after));
    lanes.finish();
}
