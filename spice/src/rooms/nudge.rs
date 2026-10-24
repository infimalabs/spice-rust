//! The one lane an operator's Design nudge messages.
//!
//! The operator already chose to push the Design, so no stance floor applies:
//! any free lane may take it, and the shared [`standing`] ranker only orders
//! them.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use board::similarity::{FrequencyProfile, GenerationMismatch, Prepared};
use wire::{CanonicalUuid, TaskStamp};

use super::profile::SessionProfiles;
use super::standing::{Standing, standing};

/// The lane a nudge messages and how well its retained profile matched.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Recipient {
    /// Logical lane the message addresses.
    pub lane: CanonicalUuid,
    /// Best cosine of the lane's retained cards, less the Design's own,
    /// against the Design statement; zero when no card scores.
    pub score: f64,
    /// Whether an occupant is seated; otherwise the message wakes the lane.
    pub seated: bool,
    /// Whether the lane already worked a phase of the Design, so it was chosen
    /// only because no free lane that had not worked it could take it.
    pub last_resort: bool,
}

impl Recipient {
    /// Whether this lane is messaged ahead of `known`: a lane that never
    /// worked the Design first, then seated over vacant, then the higher
    /// score. Equal ranks keep `known`, the lower identity, because lanes
    /// arrive in ascending identity order within each group.
    fn outranks(&self, known: &Self) -> bool {
        known
            .last_resort
            .cmp(&self.last_resort)
            .then_with(|| self.seated.cmp(&known.seated))
            .then_with(|| self.score.total_cmp(&known.score))
            == Ordering::Greater
    }
}

/// The lanes free to take a nudged Design: no active claim but the Design's.
#[derive(Clone, Debug, Default)]
pub struct FreeLanes {
    /// Each free seated lane and its current occupant.
    pub seated: BTreeMap<CanonicalUuid, CanonicalUuid>,
    /// Free vacant lanes the automatic lifecycle may wake.
    pub vacant: BTreeSet<CanonicalUuid>,
}

/// The recipient of one nudge of the Design born `nudged`.
///
/// Every lane scores without the Design's own task cards, so the score
/// measures only other related work; a lane whose scored profile holds such a
/// card has worked the Design and is chosen only as a last resort, after every
/// free lane that has not. Within either group a seated lane outranks a vacant
/// one, then the higher score wins and equal scores go to the lowest lane
/// identity. A seated lane scores its occupant's continuing session; a vacant
/// lane scores the best of its retained sessions. `design` is absent when the
/// statement weighs no term, so every lane scores zero.
///
/// # Errors
///
/// Refuses a `design` vector weighted under a generation other than
/// `frequency`'s.
pub fn choose(
    profiles: &mut SessionProfiles,
    frequency: &FrequencyProfile,
    design: Option<&Prepared>,
    free: &FreeLanes,
    nudged: &TaskStamp,
) -> Result<Option<Recipient>, GenerationMismatch> {
    let mut best: Option<Recipient> = None;
    for (&lane, &occupant) in &free.seated {
        let standing = standing(profiles, frequency, design, nudged, lane, Some(occupant))?;
        offer(&mut best, lane, standing, true);
    }
    for &lane in &free.vacant {
        let standing = standing(profiles, frequency, design, nudged, lane, None)?;
        offer(&mut best, lane, standing, false);
    }
    Ok(best)
}

/// Keep `lane` when it strictly outranks the current best.
fn offer(best: &mut Option<Recipient>, lane: CanonicalUuid, standing: Standing, seated: bool) {
    let candidate = Recipient {
        lane,
        score: standing.score.unwrap_or(0.0),
        seated,
        last_resort: standing.worked,
    };
    if best.as_ref().is_none_or(|known| candidate.outranks(known)) {
        *best = Some(candidate);
    }
}

#[cfg(test)]
mod tests;
