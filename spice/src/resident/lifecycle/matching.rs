use board::similarity::{FrequencyProfile, TermCounts};
use spice::rooms::standing::standing;

use super::super::message_surface;
use super::*;

/// Score each vacant lane's retained work against the task the allocator
/// selected for it, through the ranker Design nudges share. The frequency
/// profile is fitted once from the recent statements; a lane's own cards of
/// its task never count. Scores order lanes only against one another, so
/// fewer than two selected targets leave every target unscored.
pub(super) fn match_targets(
    targets: &mut [Target<'_>],
    entries: &[Entry],
    profiles: &std::sync::Mutex<message_surface::MessageState>,
) -> CommandResult<()> {
    if targets
        .iter()
        .filter(|target| target.task.is_some())
        .count()
        < 2
    {
        return Ok(());
    }
    let frequency = FrequencyProfile::recent(entries.iter().map(Entry::statement));
    let mut state = message_surface::lock_state(profiles)?;
    for target in targets {
        let Some(task) = target.task else {
            continue;
        };
        let statement = frequency.prepare(&TermCounts::task(task.statement()));
        target.matched = standing(
            &mut state.profiles,
            &frequency,
            statement.as_ref(),
            task.born(),
            target.lane,
            None,
        )
        .map_err(|error| {
            store_fault(
                "lifecycle::wake-profile-generation-mismatch",
                &error.to_string(),
            )
        })?
        .score;
    }
    Ok(())
}
