//! How well a running occupant's session matches each in-band candidate:
//! the `task next` supplier of [`board::Claimant::match_score`].
//!
//! The score is [`spice::rooms::standing::standing`] with the running occupant,
//! the one ranker Design nudges and the best-match wake order share. The fit
//! and each candidate's vector come from the shared [`MatchCache`]; a
//! candidate is weighed only when the ladder asks for its score, which the
//! board does only for rows tied on every key above the handle. A cold or
//! absent profile scores zero for every row, leaving the handle order exactly.

use std::cell::RefCell;
use std::sync::Arc;

use board::Entry;
use board::similarity::{FrequencyProfile, MatchCache};
use spice::rooms::profile::SessionProfiles;
use spice::rooms::standing::standing;
use wire::CanonicalUuid;

pub(super) struct OccupantMatch<'a> {
    profiles: RefCell<&'a mut SessionProfiles>,
    frequency: Arc<FrequencyProfile>,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
}

impl<'a> OccupantMatch<'a> {
    pub(super) fn new(
        profiles: &'a mut SessionProfiles,
        entries: &[Entry],
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
    ) -> Self {
        Self {
            profiles: RefCell::new(profiles),
            frequency: MatchCache::shared().fit(entries.iter().map(Entry::statement)),
            lane,
            occupant,
        }
    }

    /// The occupant's standing for `entry`; zero when nothing scores.
    pub(super) fn score(&self, entry: &Entry) -> f64 {
        let statement = MatchCache::shared().prepare(&self.frequency, entry.statement());
        let mut profiles = self.profiles.borrow_mut();
        standing(
            &mut profiles,
            &self.frequency,
            statement.as_deref(),
            entry.born(),
            self.lane,
            Some(self.occupant),
        )
        // The statement was prepared under this same frequency generation.
        .ok()
        .and_then(|standing| standing.score)
        .unwrap_or(0.0)
    }
}
