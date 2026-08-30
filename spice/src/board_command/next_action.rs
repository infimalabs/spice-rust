use super::*;

/// One command transition admitted by the task's projected post-transaction state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NextTransition {
    Settled,
    Review,
    Done,
    Wake,
    Next,
    Inert,
}

impl NextTransition {
    fn command(self, task: &TaskHandle) -> Option<Box<str>> {
        match self {
            Self::Review => Some(format!("spice task review {task}").into_boxed_str()),
            Self::Done => Some("spice task done".into()),
            Self::Wake => Some(format!("spice task wake {task}").into_boxed_str()),
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
pub(super) fn next_action(
    snapshot: &ValidatedSnapshot,
    post: &Entry,
    learn_created_project: bool,
) -> Option<Box<str>> {
    let entries = snapshot
        .entries
        .iter()
        .filter(|entry| entry.born() != post.born())
        .cloned()
        .chain(std::iter::once(post.clone()))
        .collect::<Vec<_>>();
    let mut residency = snapshot.answer.residency.body.clone();
    if learn_created_project {
        learn_project_created_by_principal(&mut residency, &snapshot.answer.principal, post);
    }
    accepted_transition(&snapshot.answer.principal, &residency, &entries, post)
        .command(post.handle())
}

pub(super) fn accepted_transition(
    principal: &Actor,
    residency: &wire::ResidencyHead,
    entries: &[Entry],
    entry: &Entry,
) -> NextTransition {
    // This check is deliberately first. A completed workflow retains its final
    // phase, but no command may infer another review merely from that phase.
    if entry.workflow().status == "completed" || entry.workflow().completion.is_some() {
        return allocator_continuation(principal, residency, entries, entry)
            .unwrap_or(NextTransition::Settled);
    }

    if principal_can_progress(principal, residency, entries, entry) {
        let workflow = entry.workflow();
        if workflow.phase.as_str() == "review"
            && workflow.flow.last() == Some(&workflow.phase)
            && workflow.assignment.as_ref() == Some(principal)
        {
            return NextTransition::Review;
        }
        return NextTransition::Done;
    }

    if entry.workflow().deferral.is_some() && !matches!(principal, Actor::Spice) {
        return NextTransition::Wake;
    }

    allocator_continuation(principal, residency, entries, entry).unwrap_or(NextTransition::Inert)
}

fn allocator_continuation(
    principal: &Actor,
    residency: &wire::ResidencyHead,
    entries: &[Entry],
    entry: &Entry,
) -> Option<NextTransition> {
    if principal_has_active_claim(entries, principal) {
        return None;
    }
    let subscription = allocation_subscription(residency, principal).ok()?;
    // A drain lane asks the allocator even when this row is no longer
    // selectable; the typed allocator-empty response closes that drain pass.
    (subscription.policy == wire::intent::LineageLife::Drain
        || board::selectable_entry(entries, entry, principal, subscription))
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
            !candidate.project().visibility().is_hidden()
                && candidate.claim().is_some_and(|candidate_claim| {
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

fn learn_project_created_by_principal(
    residency: &mut wire::ResidencyHead,
    principal: &Actor,
    entry: &Entry,
) {
    if entry.project().visibility().is_hidden() {
        return;
    }
    let Actor::Lane(lane) = principal else {
        return;
    };
    let Some(lineage) = residency
        .lanes
        .iter()
        .find(|seat| seat.lane == *lane && seat.disposition == "occupied")
        .map(|seat| seat.lineage_id)
    else {
        return;
    };
    let mut subscriptions = residency
        .subscriptions
        .iter_mut()
        .filter(|subscription| subscription.lineage_id == lineage);
    let Some(subscription) = subscriptions.next() else {
        return;
    };
    if subscriptions.next().is_none() {
        subscription.learn_project(entry.project().as_str());
    }
}
