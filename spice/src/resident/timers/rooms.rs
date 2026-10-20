//! Resident lapse of operator items at their expiry.
//!
//! Each armed wait captures the head position it read the expiry from, and its
//! transition commits only while that head is unmoved, so a disposition or
//! recovery that commits first leaves the timer nothing to write.

use std::collections::BTreeMap;

use bus::timer::TimerKind;
use bus::{Bus, CommitOutcome};
use journal::{FactCause, GuardError, JournalGuard, JournalHead, JournalWrite};
use wire::rooms::{
    RoomReceiptHead, RoomReceiptLapsed, RoomReceiptRecord, RoomReceiptRef, RoomReceiptState,
};
use wire::{Actor, CanonicalUuid, Instant, Producer, ResidentState};

use super::{CommandResult, resident_origin, store_fault};

/// Refusal of a room transition whose captured head has since moved.
const MOVED: &str = "room::timer-head-moved";

/// One room item the resident settles at its expiry.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(in crate::resident) enum RoomDue {
    Item(RoomReceiptRef),
}

impl RoomDue {
    pub(super) const fn kind(&self) -> TimerKind {
        TimerKind::RoomLapse
    }

    pub(super) fn owner(&self) -> String {
        match self {
            Self::Item(item) => item.to_string(),
        }
    }
}

/// The exact head a room deadline was read from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RoomAuthority {
    pub(super) due: RoomDue,
    pub(super) position: wire::Seq,
    pub(super) deadline: Instant,
    pub(super) epoch: CanonicalUuid,
}

/// Every pending operator item's expiry while a resident is seated to own it.
pub(super) fn desired(
    items: &[(RoomReceiptRecord, JournalHead<RoomReceiptHead>)],
    residency: Option<&JournalHead<wire::ResidencyHead>>,
) -> BTreeMap<RoomDue, RoomAuthority> {
    let Some(residency) = residency else {
        return BTreeMap::new();
    };
    if residency.body().state != ResidentState::Seated {
        return BTreeMap::new();
    }
    let epoch = residency.body().epoch;
    let items = items
        .iter()
        .filter_map(|(record, head)| match head.body().state() {
            RoomReceiptState::Pending { expires } => Some(RoomAuthority {
                due: RoomDue::Item(record.receipt().clone()),
                position: head.position(),
                deadline: *expires,
                epoch,
            }),
            _ => None,
        });
    items
        .map(|authority| (authority.due.clone(), authority))
        .collect()
}

/// Lapse the captured item at `at`.
///
/// Returns whether the transition committed; a moved head writes nothing.
pub(super) fn settle(bus: &Bus, authority: &RoomAuthority, at: Instant) -> CommandResult<bool> {
    let origin = resident_origin(authority.epoch)?;
    let mut proposal = JournalWrite::new(Actor::Spice, origin, at, Producer::BusTimer);
    let (due, position) = (authority.due.clone(), authority.position);
    proposal.guard(JournalGuard::new(move |read| {
        let RoomDue::Item(item) = &due;
        let current = read.room_item(item)?.map(|(_, head)| head.position());
        if current == Some(position) {
            Ok(())
        } else {
            Err(GuardError::refused(MOVED))
        }
    }));
    // The settled head may come from any origin (an earlier epoch's resident,
    // or a room's task), and a cause never crosses origins; the guard above
    // pins the head instead.
    let cause = FactCause::Root;
    let RoomDue::Item(item) = &authority.due;
    let staged = proposal.fact(
        RoomReceiptLapsed {
            receipt: item.clone(),
        },
        cause,
    );
    staged.map_err(|error| store_fault("room::timer-fact-invalid", &error.to_string()))?;
    match bus
        .commit(proposal)
        .map_err(|error| store_fault("bus::commit-failed", &error.to_string()))?
    {
        CommitOutcome::Committed(_) => Ok(true),
        CommitOutcome::Refused(refusal) if refusal.reason() == MOVED => Ok(false),
        CommitOutcome::Refused(refusal) => {
            Err(store_fault("room::timer-commit-refused", refusal.reason()))
        }
        CommitOutcome::Fault(fault) => Err(store_fault("journal::commit-failed", fault.detail())),
    }
}
