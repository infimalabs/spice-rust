//! How well one lane's retained work matches a task statement: the one ranker
//! every choice of a lane for a task shares, Design nudges and the lifecycle's
//! best-match wake order alike.

use board::similarity::{FrequencyProfile, GenerationMismatch, Prepared};
use wire::{CanonicalUuid, TaskStamp};

use super::profile::{EvidenceKey, SessionProfile, SessionProfiles};

/// One lane's match against a task statement.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Standing {
    /// Best cosine of the lane's retained cards, less the task's own, against
    /// the statement; absent when no card scores.
    pub score: Option<f64>,
    /// Whether a scored profile holds one of the task's own cards: the lane
    /// already worked the task, so it takes it only as a last resort.
    pub worked: bool,
}

/// The standing of `lane` for the task born `task`, whose statement weighs as
/// `statement`; absent when the statement weighs no term, so nothing scores.
///
/// A seated lane, named with its `occupant`, scores its occupant's continuing
/// session; a vacant lane scores the best of its retained sessions. The task's
/// own cards never count toward the score, so it measures only other related
/// work.
///
/// # Errors
///
/// Refuses a `statement` weighted under a generation other than `frequency`'s.
pub fn standing(
    profiles: &mut SessionProfiles,
    frequency: &FrequencyProfile,
    statement: Option<&Prepared>,
    task: &TaskStamp,
    lane: CanonicalUuid,
    occupant: Option<CanonicalUuid>,
) -> Result<Standing, GenerationMismatch> {
    let own =
        |key: &EvidenceKey| matches!(key, EvidenceKey::Task { task: card, .. } if card == task);
    let foreign = |key: &EvidenceKey| !own(key);
    let mut standing = Standing::default();
    let mut weigh = |profile: &mut SessionProfile| -> Result<(), GenerationMismatch> {
        let evidence = profile.evidence_mut();
        standing.worked |= evidence.holds(own);
        if let Some(statement) = statement {
            let score = evidence.best_score_where(frequency, statement, foreign)?;
            if score.is_some_and(|score| standing.score.is_none_or(|known| score > known)) {
                standing.score = score;
            }
        }
        Ok(())
    };
    match occupant {
        Some(occupant) => {
            if let Some(profile) = profiles
                .session(lane, occupant)
                .cloned()
                .and_then(|session| profiles.profile_mut(&session))
            {
                weigh(profile)?;
            }
        }
        None => {
            for (_, profile) in profiles
                .iter_mut()
                .filter(|(session, _)| session.lane == lane)
            {
                weigh(profile)?;
            }
        }
    }
    Ok(standing)
}
