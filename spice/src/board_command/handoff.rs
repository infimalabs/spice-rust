use super::*;

/// Project the current phase's ownership policy from durable transition evidence.
///
/// Published repository work asks for independent ownership. A transition with
/// another declared phase still ahead stays with its prior claimant. Task birth,
/// an unrecorded transition into the final declared phase, and an edit that
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
    let current = heads.workflow().body();
    let continues = current
        .flow
        .iter()
        .position(|phase| phase == &current.phase)
        .is_some_and(|index| index + 1 < current.flow.len());
    let Some(kind) = handoff_kind(&done, continues) else {
        return Ok(None);
    };
    let until = workflow
        .at
        .get()
        .checked_add(jiff::SignedDuration::from_secs(board::HANDOFF_SECONDS))
        .map_err(|error| environment_fault("board::phase-handoff-range", &error.to_string()))?;
    let until = Instant::new(until)
        .map_err(|error| environment_fault("board::phase-handoff-invalid", &error.to_string()))?;
    Ok(Some(board::PhaseHandoff::new(
        kind,
        claim.body().actor.clone(),
        until,
    )))
}

fn handoff_kind(done: &body::TaskDone, continues: bool) -> Option<board::PhaseHandoffKind> {
    if done
        .git
        .as_ref()
        .is_some_and(|boundary| boundary.local_commits > 0)
    {
        Some(board::PhaseHandoffKind::Independent)
    } else {
        continues.then_some(board::PhaseHandoffKind::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completed(git: Option<body::GitBoundary>) -> body::TaskDone {
        body::TaskDone {
            task: "1kRKw9Qj".parse().expect("fixture task stamp"),
            validation: "fixture validation".to_owned(),
            terminal: None,
            git,
            source: body::SourceBoundary::unrecorded(),
        }
    }

    #[test]
    fn published_work_requests_independence_without_consulting_a_phase_name() {
        let published = completed(Some(body::GitBoundary {
            head: "lane-head".to_owned(),
            integrated: "integrated-head".to_owned(),
            upstream: None,
            upstream_head: None,
            local_commits: 1,
        }));
        assert_eq!(
            handoff_kind(&published, false),
            Some(board::PhaseHandoffKind::Independent)
        );
        assert_eq!(
            handoff_kind(&completed(None), true),
            Some(board::PhaseHandoffKind::Continue)
        );
        assert_eq!(handoff_kind(&completed(None), false), None);
        let unchanged = completed(Some(body::GitBoundary {
            head: "same-head".to_owned(),
            integrated: "same-head".to_owned(),
            upstream: None,
            upstream_head: None,
            local_commits: 0,
        }));
        assert_eq!(
            handoff_kind(&unchanged, true),
            Some(board::PhaseHandoffKind::Continue)
        );
        assert_eq!(handoff_kind(&unchanged, false), None);
    }
}
