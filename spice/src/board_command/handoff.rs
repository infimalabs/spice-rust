use super::*;

/// The commit instant of the fact that first queued `statement`'s task:
/// what the allocator's overdue tier measures a row's wait from.
pub(crate) fn queued_at(
    facts: &bus::TaskFacts,
    statement: &wire::TaskHead,
) -> Option<wire::Instant> {
    facts.at(statement.queue_origin).map(|envelope| envelope.at)
}

/// Project the current allocation hold from durable transition evidence.
///
/// A completion that published repository work asks for independent ownership
/// of the phase it entered. A completion that carried its claim into the next
/// phase needs no hold: the claim itself is the assignment. Task birth, an
/// unrecorded transition into the final declared phase, and an edit that
/// withdrew a taste hold's plan phase carry no inferred handoff preference.
pub(crate) fn phase_handoff(
    facts: &bus::TaskFacts,
    heads: &journal::TaskHeads,
) -> Result<Option<board::PhaseHandoff>, Outcome<CommandAnswer>> {
    let workflow = facts
        .at(heads.workflow().position())
        .filter(|envelope| envelope.fact.kind() == wire::FactKind::TaskPhased);
    let Some(workflow) = workflow else {
        return Ok(None);
    };
    let cause = workflow.cause.and_then(|cause| facts.at(cause));
    if cause.is_some_and(|envelope| envelope.fact.kind() == wire::FactKind::TaskEdited) {
        return Ok(None);
    }
    let completed = cause
        .filter(|envelope| envelope.fact.kind() == wire::FactKind::TaskDone)
        .ok_or_else(|| {
            protocol_fault(
                "board::phase-handoff-cause-missing",
                "a task phase head has no causal task completion",
            )
        })?;
    let done = completed.fact.read::<body::TaskDone>().ok_or_else(|| {
        protocol_fault(
            "board::phase-handoff-completion-invalid",
            "a task phase cause does not decode as its registered completion fact",
        )
    })?;
    let claim = heads.claim().ok_or_else(|| {
        protocol_fault(
            "board::phase-handoff-claim-missing",
            "a task phase head has no prior claimant",
        )
    })?;
    if !publishes(&done) {
        return Ok(None);
    }
    Ok(Some(board::PhaseHandoff::new(
        claim.body().actor.clone(),
        hold_until(workflow.at)?,
    )))
}

fn hold_until(at: Instant) -> Result<Instant, Outcome<CommandAnswer>> {
    let until = at
        .get()
        .checked_add(jiff::SignedDuration::from_secs(board::HANDOFF_SECONDS))
        .map_err(|error| environment_fault("board::phase-handoff-range", &error.to_string()))?;
    Instant::new(until)
        .map_err(|error| environment_fault("board::phase-handoff-invalid", &error.to_string()))
}

/// Whether a completion published repository history, which asks the phase it
/// entered to go to a peer.
fn publishes(done: &body::TaskDone) -> bool {
    done.git
        .as_ref()
        .is_some_and(|boundary| boundary.local_commits > 0)
}
