//! How well a running occupant's session matches each candidate: the
//! supplier of [`board::Claimant::match_score`] for `task next`'s read and
//! for the claim that follows it, so both rank the same board the same way.
//!
//! The score is [`spice::rooms::standing::standing`] with the running occupant,
//! the one ranker Design nudges and the best-match wake order share. The fit
//! and each candidate's vector come from the shared [`MatchCache`]; the board
//! asks for every row in the winning priority and weight group. A score below
//! the lowest Design invitation floor counts as zero, so incidental vocabulary
//! overlap never outranks age; a cold or absent profile scores zero for every
//! row, leaving the ladder exactly as it stands without a score.

use std::cell::RefCell;
use std::sync::{Arc, MutexGuard};

use board::Entry;
use board::similarity::{FrequencyProfile, MatchCache};
use spice::rooms::profile::SessionProfiles;
use spice::rooms::standing::standing;
use wire::CanonicalUuid;
use wire::rooms::WorkingGroupStance;

use super::OccupantProfiles;
use crate::resident::design_awareness::{command_answer_outcome, match_score_floor};
use crate::resident::message_surface::{self, MessageState};

/// The running occupant's matcher, holding the profile lock while it scores.
pub(crate) struct OccupantMatch<'a> {
    state: RefCell<MutexGuard<'a, MessageState>>,
    frequency: Arc<FrequencyProfile>,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
}

impl<'a> OccupantMatch<'a> {
    /// The matcher for `principal` over `entries`: none when the resident
    /// retains no profiles or the principal is not a seated, occupied lane.
    pub(crate) fn open(
        profiles: OccupantProfiles<'a>,
        residency: &wire::ResidencyHead,
        principal: &wire::Actor,
        entries: &[Entry],
    ) -> Result<Option<Self>, wire::Outcome<wire::intent::CommandAnswer>> {
        let (Some(profiles), Some((lane, occupant))) =
            (profiles, running_occupant(residency, principal))
        else {
            return Ok(None);
        };
        let state = message_surface::lock_state(profiles).map_err(command_answer_outcome)?;
        Ok(Some(Self {
            state: RefCell::new(state),
            frequency: MatchCache::shared().fit(entries.iter().map(Entry::statement)),
            lane,
            occupant,
        }))
    }

    /// The occupant's standing for `entry` at or above the Explore floor;
    /// zero below it or when nothing scores.
    pub(crate) fn score(&self, entry: &Entry) -> f64 {
        let mut state = self.state.borrow_mut();
        floored_standing(
            &mut state.profiles,
            &self.frequency,
            entry,
            self.lane,
            Some(self.occupant),
        )
        .unwrap_or(0.0)
    }
}

/// One lane's standing for `entry` under `frequency`: the running occupant's
/// session when `occupant` is given, else the lane's best retained session.
/// Absent when nothing scores or the score sits below the Explore floor, the
/// lowest Design invitation floor, so incidental vocabulary overlap never
/// counts as a match.
pub(crate) fn floored_standing(
    profiles: &mut SessionProfiles,
    frequency: &Arc<FrequencyProfile>,
    entry: &Entry,
    lane: CanonicalUuid,
    occupant: Option<CanonicalUuid>,
) -> Option<f64> {
    let statement = MatchCache::shared().prepare(frequency, entry.statement());
    standing(
        profiles,
        frequency,
        statement.as_deref(),
        entry.born(),
        lane,
        occupant,
    )
    // The statement was prepared under this same frequency generation.
    .ok()
    .and_then(|standing| standing.score)
    .filter(|score| *score >= match_score_floor(WorkingGroupStance::Explore))
}

/// The lane and occupant seated for `principal`, when it is an occupied lane.
pub(super) fn running_occupant(
    residency: &wire::ResidencyHead,
    principal: &wire::Actor,
) -> Option<(CanonicalUuid, CanonicalUuid)> {
    let wire::Actor::Lane(lane) = principal else {
        return None;
    };
    residency
        .lanes
        .iter()
        .find(|row| row.lane == *lane && row.disposition == "occupied")
        .and_then(|row| row.occupant)
        .map(|occupant| (*lane, occupant))
}
