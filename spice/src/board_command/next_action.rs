use super::*;

/// One command transition admitted by the task's projected post-transaction state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NextTransition {
    Settled,
    Review,
    Done,
    Propose,
    Wake,
    File,
    Next,
    Inert,
}

impl NextTransition {
    fn command(self, task: &TaskHandle) -> Option<Box<str>> {
        match self {
            Self::Review => Some(format!("spice task review {task}").into_boxed_str()),
            Self::Done => Some("spice task done".into()),
            Self::Propose => Some("spice preparation propose JUSTIFICATION".into()),
            Self::Wake => Some(format!("spice task wake {task}").into_boxed_str()),
            Self::File => Some(format!("spice task file {task}").into_boxed_str()),
            Self::Next => Some("spice task next".into()),
            Self::Settled | Self::Inert => None,
        }
    }
}

/// Derive a next action from the exact task transitions admitted after a write.
///
/// `post` is the changed task as it will exist after the transaction. Replacing
/// that handle in the observed entry set makes dependency and possession checks
/// use one coherent projected board rather than the phase's position in a flow.
pub(super) fn next_action(snapshot: &ValidatedSnapshot, post: &Entry) -> Option<Box<str>> {
    let post = snapshot
        .entries
        .iter()
        .find(|entry| entry.born() == post.born())
        .map_or_else(
            || post.clone(),
            |entry| post.clone().with_creator_team(entry.creator_team()),
        );
    let entries = snapshot
        .entries
        .iter()
        .filter(|entry| entry.born() != post.born())
        .cloned()
        .chain(std::iter::once(post.clone()))
        .collect::<Vec<_>>();
    let environment = snapshot.answer.environment();
    accepted_transition(
        &snapshot.answer.principal,
        &snapshot.answer.residency.body,
        &entries,
        &post,
        &environment,
    )
    .command(post.handle())
}

pub(super) fn accepted_transition(
    principal: &Actor,
    residency: &wire::ResidencyHead,
    entries: &[Entry],
    entry: &Entry,
    environment: &wire::Environment,
) -> NextTransition {
    // This check is deliberately first. A settled workflow retains its final
    // phase, but no command may infer another review merely from that phase.
    if !entry.workflow().status.is_pending() {
        return allocator_continuation(principal, residency, entries, entry, environment)
            .unwrap_or(NextTransition::Settled);
    }

    if entry.is_draft() {
        return NextTransition::File;
    }

    if principal_can_progress(principal, residency, entries, entry) {
        let workflow = entry.workflow();
        if workflow.phase.as_str() == "review"
            && workflow.flow.last() == Some(&workflow.phase)
            && workflow.status.reviewer() == Some(principal)
        {
            return NextTransition::Review;
        }
        // Only the open Design phase stays operator-owned: its holder ends a
        // pass with one proposal. Intake phases use their ordinary
        // plan/review transitions.
        if wire::intent::is_open_design(entry.statement().intent, entry.workflow()) {
            return NextTransition::Propose;
        }
        return NextTransition::Done;
    }

    if entry.workflow().deferral.is_some() && !matches!(principal, Actor::Spice) {
        return NextTransition::Wake;
    }

    allocator_continuation(principal, residency, entries, entry, environment)
        .unwrap_or(NextTransition::Inert)
}

fn allocator_continuation(
    principal: &Actor,
    residency: &wire::ResidencyHead,
    entries: &[Entry],
    entry: &Entry,
    environment: &wire::Environment,
) -> Option<NextTransition> {
    if principal_has_active_claim(entries, principal) {
        return None;
    }
    let subscription = allocation_subscription(residency, principal).ok()?;
    // A drain lane asks the allocator even when this row is no longer
    // selectable; the typed allocator-empty response closes that drain pass.
    let selectable = current_instant().ok().is_some_and(|now| {
        let satisfied = board::resolved_tasks(entries);
        board::selectable_entry_at(
            &satisfied,
            entry,
            principal,
            &subscription,
            environment,
            now,
        )
    });
    (subscription.policy == wire::intent::TeamLife::Drain || selectable)
        .then_some(NextTransition::Next)
}

fn principal_has_active_claim(entries: &[Entry], principal: &Actor) -> bool {
    entries.iter().any(|candidate| {
        candidate.claim().is_some_and(|claim| {
            claim.actor == *principal && claim.disposition == ClaimDisposition::Active
        })
    })
}

fn principal_can_progress(
    principal: &Actor,
    residency: &wire::ResidencyHead,
    entries: &[Entry],
    entry: &Entry,
) -> bool {
    let Actor::Lane(lane) = principal else {
        return false;
    };
    let Some(claim) = entry.claim() else {
        return false;
    };
    let seated = residency.lanes.iter().any(|seat| {
        seat.lane == *lane
            && claim.lane == *lane
            && seat.occupant == Some(claim.occupant)
            && claim.epoch == residency.epoch
            && seat.disposition == "occupied"
    });
    if !seated {
        return false;
    }
    let sole = entries
        .iter()
        .filter(|candidate| {
            candidate.claim().is_some_and(|candidate_claim| {
                candidate_claim.actor == *principal
                    && candidate_claim.disposition == ClaimDisposition::Active
            })
        })
        .count()
        == 1;
    sole && current_instant()
        .ok()
        .and_then(|at| board::Active::admit(entry, principal, at).ok())
        .is_some()
}
