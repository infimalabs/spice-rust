//! The one lane an operator's Design nudge messages.
//!
//! The operator already chose to push the Design, so no stance floor applies:
//! any free lane may take it, and the TF-IDF matcher Design awareness uses only
//! orders them.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use board::similarity::{FrequencyProfile, GenerationMismatch, Prepared};
use wire::CanonicalUuid;

use super::profile::SessionProfiles;

/// The lane a nudge messages and how well its retained profile matched.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Recipient {
    /// Logical lane the message addresses.
    pub lane: CanonicalUuid,
    /// Best cosine of the lane's retained cards against the Design statement;
    /// zero when no card scores.
    pub score: f64,
    /// Whether an occupant is seated; otherwise the message wakes the lane.
    pub seated: bool,
}

/// The lanes free to take a nudged Design: no active claim but the Design's.
#[derive(Clone, Debug, Default)]
pub struct FreeLanes {
    /// Each free seated lane and its current occupant.
    pub seated: BTreeMap<CanonicalUuid, CanonicalUuid>,
    /// Free vacant lanes the automatic lifecycle may wake.
    pub vacant: BTreeSet<CanonicalUuid>,
}

/// The recipient of one nudge.
///
/// A free seated lane always outranks a vacant one, which is messaged only when
/// no seated lane is free. Within either group the higher score wins and equal
/// scores go to the lowest lane identity. A seated lane scores its occupant's
/// continuing session; a vacant lane scores the best of its retained sessions.
/// `design` is absent when the statement weighs no term, so every lane scores
/// zero.
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
) -> Result<Option<Recipient>, GenerationMismatch> {
    let mut best: Option<Recipient> = None;
    for (&lane, &occupant) in &free.seated {
        let score = match (design, profiles.session(lane, occupant).cloned()) {
            (Some(design), Some(session)) => match profiles.profile_mut(&session) {
                Some(profile) => profile.evidence_mut().best_score(frequency, design)?,
                None => None,
            },
            _ => None,
        };
        offer(&mut best, lane, score, true);
    }
    if best.is_some() {
        return Ok(best);
    }
    let mut retained = BTreeMap::<CanonicalUuid, Option<f64>>::new();
    if let Some(design) = design {
        for (session, profile) in profiles.iter_mut() {
            if !free.vacant.contains(&session.lane) {
                continue;
            }
            let score = profile.evidence_mut().best_score(frequency, design)?;
            let known = retained.entry(session.lane).or_default();
            if score.is_some_and(|score| known.is_none_or(|known| score > known)) {
                *known = score;
            }
        }
    }
    for &lane in &free.vacant {
        offer(
            &mut best,
            lane,
            retained.get(&lane).copied().flatten(),
            false,
        );
    }
    Ok(best)
}

/// Keep `lane` when it strictly outscores the current best. Lanes arrive in
/// ascending identity order, so a tie keeps the lower identity.
fn offer(best: &mut Option<Recipient>, lane: CanonicalUuid, score: Option<f64>, seated: bool) {
    let score = score.unwrap_or(0.0);
    if best
        .as_ref()
        .is_none_or(|known| score.partial_cmp(&known.score) == Some(Ordering::Greater))
    {
        *best = Some(Recipient {
            lane,
            score,
            seated,
        });
    }
}

#[cfg(test)]
mod tests;
