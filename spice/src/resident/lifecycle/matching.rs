use std::cell::RefCell;
use std::sync::{Arc, MutexGuard};

use board::similarity::{FrequencyProfile, MatchCache};

use super::super::message_surface;
use super::*;

/// Every vacant lane's retained work scored against the board, through the
/// ranker Design nudges and `task next` share, under the shared
/// [`MatchCache`] fit. One reconciliation pass takes the profile lock once
/// and fits once; each lane then selects the candidate its own best retained
/// session matches, and its wake rank is that selected task's score. A lane's
/// own cards of a task never count.
pub(super) struct RetainedMatch<'a> {
    state: RefCell<MutexGuard<'a, message_surface::MessageState>>,
    frequency: Arc<FrequencyProfile>,
}

impl<'a> RetainedMatch<'a> {
    /// The pass's matcher; none before message startup restored profiles,
    /// which leaves every lane selecting the unscored ladder exactly.
    pub(super) fn open(
        profiles: Option<&'a std::sync::Mutex<message_surface::MessageState>>,
        entries: &[Entry],
    ) -> CommandResult<Option<Self>> {
        let Some(profiles) = profiles else {
            return Ok(None);
        };
        let state = message_surface::lock_state(profiles)?;
        Ok(Some(Self {
            state: RefCell::new(state),
            frequency: MatchCache::shared().fit(entries.iter().map(Entry::statement)),
        }))
    }

    /// `lane`'s best retained standing for `entry` at or above the wide-reach
    /// floor; absent below it or when nothing scores.
    pub(super) fn score(&self, lane: CanonicalUuid, entry: &Entry) -> Option<f64> {
        let mut state = self.state.borrow_mut();
        crate::board_command::floored_standing(
            &mut state.profiles,
            &self.frequency,
            entry,
            lane,
            None,
        )
    }
}
