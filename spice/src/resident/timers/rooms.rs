//! Resident settlement of room question deadlines and operator item expiry.
//!
//! A pending question result times out at its immutable deadline, and a
//! pending operator item lapses at its expiry. Each armed wait captures the
//! head position it read the deadline from, and its transition commits only
//! while that head is unmoved, so an answer, closure, disposition, or recovery
//! that commits first leaves the timer nothing to write.

use std::collections::BTreeMap;

use bus::timer::TimerKind;
use bus::{Bus, CommitOutcome};
use journal::{FactCause, GuardError, JournalGuard, JournalHead, JournalWrite};
use wire::rooms::{
    QuestionResultHead, QuestionResultRecord, QuestionResultRef, RoomQuestionTimedOut,
    RoomReceiptHead, RoomReceiptLapsed, RoomReceiptRecord, RoomReceiptRef, RoomReceiptState,
};
use wire::{Actor, CanonicalUuid, Instant, Producer, ResidentState};

use super::{CommandResult, resident_origin, store_fault};

/// Refusal of a room transition whose captured head has since moved.
const MOVED: &str = "room::timer-head-moved";

/// One room head the resident settles at its own deadline.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(in crate::resident) enum RoomDue {
    /// A pending question result times out.
    Result(QuestionResultRef),
    /// A pending operator item lapses.
    Item(RoomReceiptRef),
}

impl RoomDue {
    pub(super) const fn kind(&self) -> TimerKind {
        match self {
            Self::Result(_) => TimerKind::RoomTimeout,
            Self::Item(_) => TimerKind::RoomLapse,
        }
    }

    pub(super) fn owner(&self) -> String {
        match self {
            Self::Result(result) => result.to_string(),
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

/// Every pending result's deadline and every pending operator item's expiry,
/// while a resident is seated to own them.
pub(super) fn desired(
    results: &[(QuestionResultRecord, JournalHead<QuestionResultHead>)],
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
    let results = results.iter().map(|(record, head)| RoomAuthority {
        due: RoomDue::Result(record.result().clone()),
        position: head.position(),
        deadline: record.deadline(),
        epoch,
    });
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
    results
        .chain(items)
        .map(|authority| (authority.due.clone(), authority))
        .collect()
}

/// Time out the captured result or lapse the captured item at `at`.
///
/// Returns whether the transition committed; a head that moved since it was
/// captured, including by this transition's own earlier commit, writes
/// nothing.
pub(super) fn settle(bus: &Bus, authority: &RoomAuthority, at: Instant) -> CommandResult<bool> {
    let origin = resident_origin(authority.epoch)?;
    let mut proposal = JournalWrite::new(Actor::Spice, origin, at, Producer::BusTimer);
    let (due, position) = (authority.due.clone(), authority.position);
    proposal.guard(JournalGuard::new(move |read| {
        let current = match &due {
            RoomDue::Result(result) => read
                .question_result(result)?
                .map(|(_, head)| head.position()),
            RoomDue::Item(item) => read.room_item(item)?.map(|(_, head)| head.position()),
        };
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
    let staged = match &authority.due {
        RoomDue::Result(result) => proposal.fact(
            RoomQuestionTimedOut {
                result: result.clone(),
            },
            cause,
        ),
        RoomDue::Item(item) => proposal.fact(
            RoomReceiptLapsed {
                receipt: item.clone(),
            },
            cause,
        ),
    };
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
