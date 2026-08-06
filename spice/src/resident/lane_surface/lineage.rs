//! Atomic task-subscription and membership changes for one Canvas lineage.

use super::*;

#[cfg(test)]
mod tests;

struct MembershipChange {
    member: CanonicalUuid,
    from_lineage: CanonicalUuid,
    source_rekeyed_to: Option<CanonicalUuid>,
}

/// Commit one task-subscription policy and optional member move.
///
/// `drain` here is a policy spelling. Occupant teardown remains exclusively
/// owned by `lane::state { action: drain }`, so this transaction changes no
/// seat, capability, session, occupant, or disposition field.
pub(in crate::resident) fn configure(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &Seats,
    projects: &board::ProjectPolicy,
    command: &wire::AdmittedCommand,
    request: &LineageConfigure,
) -> Outcome<CommandAnswer> {
    let observed = match current_residency(bus, epoch) {
        Ok(head) => head,
        Err(outcome) => return typed_outcome(outcome),
    };
    let previous = match previous_subscription(observed.body(), request) {
        Ok(previous) => previous,
        Err(outcome) => return outcome,
    };
    let manual_projects = match admit_manual_projects(projects, request.manual_projects.as_deref())
    {
        Ok(projects) => projects,
        Err(outcome) => return outcome,
    };
    let mut next = observed.body().clone();
    let membership = match request.member {
        Some(member) => match move_member(&mut next, member, request.lineage_id) {
            Ok(change) => Some(change),
            Err(outcome) => return outcome,
        },
        None => None,
    };
    let Some(subscription) = next
        .subscriptions
        .iter_mut()
        .find(|subscription| subscription.lineage_id == request.lineage_id)
    else {
        return typed_store_fault(
            "lineage::subscription-missing",
            "the configured lineage has no task-subscription policy after membership moved",
        );
    };
    if subscription.policy != request.life || manual_projects.is_some() {
        subscription.auto_projects.clear();
    }
    subscription.policy = request.life;
    if let Some(manual_projects) = manual_projects {
        subscription.manual_projects = manual_projects;
    }
    let configured_manual_projects = subscription.manual_projects.clone();
    let lane_answer = match answer(&next, None, seats) {
        Ok(answer) => answer,
        Err(outcome) => return typed_outcome(outcome),
    };
    let mut write = match write(
        command.actor().clone(),
        epoch,
        Producer::ResidentRecognition,
    ) {
        Ok(write) => write,
        Err(outcome) => return typed_outcome(outcome),
    };
    guard_residency(&mut write, &observed);
    let fact = match write.fact(
        wire::body::LineageConfigured {
            lineage_id: request.lineage_id,
            member: membership.as_ref().map(|change| change.member),
            from_lineage: membership.as_ref().map(|change| change.from_lineage),
            source_rekeyed_to: membership.and_then(|change| change.source_rekeyed_to),
            from: previous.policy,
            life: request.life,
            from_manual_projects: previous.manual_projects,
            manual_projects: configured_manual_projects,
        },
        FactCause::Existing(observed.position()),
    ) {
        Ok(fact) => fact,
        Err(error) => return typed_store_fault("lineage::fact-invalid", &error.to_string()),
    };
    write.residency_head(fact, move |_| next);
    if let Err(error) = write.command_result(
        Command {
            v: wire::WireVersion::CURRENT,
            id: command.id(),
            epoch: command.epoch(),
            kind: command.kind(),
            expected: command.expected().to_vec(),
            body: request.clone(),
        },
        epoch,
        lane_answer.clone(),
    ) {
        return typed_store_fault("lineage::receipt-invalid", &error.to_string());
    }
    match commit(bus, write) {
        Ok(committed) => Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(committed.first()),
                last: seq(committed.last()),
            }),
            body: CommandAnswer::Lane(Box::new(lane_answer)),
        }),
        Err(outcome) => typed_outcome(outcome),
    }
}

fn previous_subscription(
    head: &wire::ResidencyHead,
    request: &LineageConfigure,
) -> Result<wire::LineageSubscription, Outcome<CommandAnswer>> {
    let target_exists = head
        .lanes
        .iter()
        .any(|row| row.lineage_id == request.lineage_id);
    let baseline = if target_exists {
        request.lineage_id
    } else if let Some(member) = request.member {
        let Some(row) = head.lanes.iter().find(|row| row.lane == member) else {
            return Err(lane_refusal(
                "lineage::member-missing",
                Some("one current Canvas member".to_owned()),
                Some(member.to_string()),
                "read lane state and retry with a current member identity",
            ));
        };
        row.lineage_id
    } else {
        return Err(lane_refusal(
            "lineage::missing",
            Some("one current Canvas lineage".to_owned()),
            Some(request.lineage_id.to_string()),
            "read lane state and retry with a current lineage identity",
        ));
    };
    match subscription_row(head, baseline) {
        Ok(Some(subscription)) => Ok(subscription.clone()),
        Ok(None) => Err(typed_store_fault(
            "lineage::subscription-missing",
            "a current lineage has no task-subscription policy",
        )),
        Err(outcome) => Err(typed_outcome(outcome)),
    }
}

fn move_member(
    head: &mut wire::ResidencyHead,
    member: CanonicalUuid,
    target: CanonicalUuid,
) -> Result<MembershipChange, Outcome<CommandAnswer>> {
    let Some(member_index) = head.lanes.iter().position(|row| row.lane == member) else {
        return Err(lane_refusal(
            "lineage::member-missing",
            Some("one current Canvas member".to_owned()),
            Some(member.to_string()),
            "read lane state and retry with a current member identity",
        ));
    };
    let source = head.lanes[member_index].lineage_id;
    let source_members = head
        .lanes
        .iter()
        .filter(|row| row.lineage_id == source)
        .map(|row| row.lane)
        .collect::<Vec<_>>();
    if source == target {
        if member != target || source_members.len() == 1 {
            return Err(lane_refusal(
                "lineage::member-unchanged",
                Some("a different target lineage".to_owned()),
                Some(target.to_string()),
                "drop the member on another team or on Agents Without Team",
            ));
        }
        return split_host_member(head, member, source, &source_members);
    }
    let source_subscription = match subscription_row(head, source) {
        Ok(Some(subscription)) => subscription.clone(),
        Ok(None) => {
            return Err(typed_store_fault(
                "lineage::subscription-missing",
                "the moved member source has no task-subscription policy",
            ));
        }
        Err(outcome) => return Err(typed_outcome(outcome)),
    };
    let target_exists = head.lanes.iter().any(|row| row.lineage_id == target);
    if target_exists {
        match subscription_row(head, target) {
            Ok(Some(_)) => {}
            Ok(None) => {
                return Err(typed_store_fault(
                    "lineage::subscription-missing",
                    "the target lineage has no task-subscription policy",
                ));
            }
            Err(outcome) => return Err(typed_outcome(outcome)),
        }
    } else {
        let mut subscription = source_subscription;
        subscription.lineage_id = target;
        head.subscriptions.push(subscription);
    }
    head.lanes[member_index].lineage_id = target;
    if !head.lanes.iter().any(|row| row.lineage_id == source) {
        head.subscriptions
            .retain(|subscription| subscription.lineage_id != source);
    }
    head.subscriptions
        .sort_by_key(|subscription| subscription.lineage_id);
    Ok(MembershipChange {
        member,
        from_lineage: source,
        source_rekeyed_to: None,
    })
}

fn split_host_member(
    head: &mut wire::ResidencyHead,
    member: CanonicalUuid,
    source: CanonicalUuid,
    source_members: &[CanonicalUuid],
) -> Result<MembershipChange, Outcome<CommandAnswer>> {
    let rekeyed = source_members
        .iter()
        .copied()
        .find(|candidate| *candidate != member)
        .expect("a multi-member source has one remaining member");
    if head
        .lanes
        .iter()
        .any(|row| row.lineage_id == rekeyed && row.lineage_id != source)
    {
        return Err(typed_store_fault(
            "lineage::rekey-collision",
            "the remaining team member identity already names another lineage",
        ));
    }
    let source_subscription = match subscription_row(head, source) {
        Ok(Some(subscription)) => subscription.clone(),
        Ok(None) => {
            return Err(typed_store_fault(
                "lineage::subscription-missing",
                "the split source has no task-subscription policy",
            ));
        }
        Err(outcome) => return Err(typed_outcome(outcome)),
    };
    for row in &mut head.lanes {
        if row.lineage_id == source && row.lane != member {
            row.lineage_id = rekeyed;
        }
    }
    let mut remainder = source_subscription;
    remainder.lineage_id = rekeyed;
    head.subscriptions.push(remainder);
    head.subscriptions
        .sort_by_key(|subscription| subscription.lineage_id);
    Ok(MembershipChange {
        member,
        from_lineage: source,
        source_rekeyed_to: Some(rekeyed),
    })
}
