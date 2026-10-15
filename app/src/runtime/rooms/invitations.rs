//! Organic Design invitations for one lane. An invitation is background
//! information only. Following it opens the Design and its contribution
//! affordance; it never joins a working group, acquires the Design, or wakes
//! anyone.

use wire::rooms::DesignNoticed;
use wire::{CanonicalUuid, FactKind, StreamFact};

use super::{RoomPositioned, RoomProjection};
use crate::runtime::BoardView;

impl RoomProjection {
    /// Fold one Design notice, or `None` when `fact` is not one.
    pub(super) fn apply_notice(&mut self, fact: &StreamFact) -> Option<bool> {
        (fact.kind() == FactKind::DesignNoticed).then(|| {
            if self.invitations.contains_key(&fact.seq()) {
                return false;
            }
            fact.read::<DesignNoticed>().is_some_and(|value| {
                self.maturity
                    .entry(value.design.clone())
                    .or_default()
                    .observe_notice(&value);
                self.invitations.insert(
                    fact.seq(),
                    RoomPositioned {
                        position: fact.seq(),
                        value,
                    },
                );
                true
            })
        })
    }

    /// The invitations told to `lane` whose Design the board shows open,
    /// newest first. Without a board nothing proves a Design open, so none
    /// shows.
    #[must_use]
    pub fn invitations(
        &self,
        lane: CanonicalUuid,
        board: Option<&BoardView>,
    ) -> Vec<&RoomPositioned<DesignNoticed>> {
        let Some(board) = board else {
            return Vec::new();
        };
        self.invitations
            .values()
            .rev()
            .filter(|notice| notice.value.lane == lane)
            .filter(|notice| {
                board
                    .row(&notice.value.design)
                    .and_then(wire::intent::BoardTask::design_standing)
                    .is_some_and(|standing| standing.is_open())
            })
            .collect()
    }

    /// Every durable Design notice retained by this projection.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn design_notices(&self) -> impl Iterator<Item = &DesignNoticed> {
        self.invitations.values().map(|notice| &notice.value)
    }
}
