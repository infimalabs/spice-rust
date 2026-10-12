//! Required question evidence read before positive completion.

use super::*;
use wire::rooms::QuestionResultState;

/// Refuse positive completion of `entry` while any result its statement
/// requires is not answered.
///
/// An answer makes evidence available for assessment; it approves nothing by
/// itself. The journal enforces the same rule when the completion commits;
/// reading it here refuses before completion publishes repository history.
pub(super) fn require_answered(bus: &Bus, entry: &Entry) -> Result<(), Outcome<CommandAnswer>> {
    let required = entry.statement().evidence.clone();
    if required.is_empty() {
        return Ok(());
    }
    let states = bus
        .read(move |read| {
            required
                .into_iter()
                .map(|result| {
                    let state = read
                        .question_result(&result)?
                        .map(|(_, head)| head.body().state().clone());
                    Ok((result, state))
                })
                .collect::<Result<Vec<_>, GuardError>>()
        })
        .map_err(|error| store_fault("task::evidence-unreadable", &error.to_string()))?;
    let unsatisfied = states
        .iter()
        .filter(|(_, state)| !matches!(state, Some(QuestionResultState::Answered { .. })))
        .map(|(result, state)| {
            format!(
                "{result} is {}",
                state.as_ref().map_or("missing", QuestionResultState::name)
            )
        })
        .collect::<Vec<_>>();
    if unsatisfied.is_empty() {
        return Ok(());
    }
    Err(refusal(
        "task::evidence-unsatisfied",
        Some(format!("{}: {}", entry.handle(), unsatisfied.join(", "))),
        "positive completion needs every required result answered; ask again, or have the operator replace or waive it with spice task require",
    ))
}
