//! The two writes that address Canvas lineages rather than the seats in them.
//!
//! [`configure`] names one lineage and changes it: its task-subscription
//! policy, and at most one member's home. [`arrange`] names no lineage in
//! particular and restates the whole topology at once, rewriting both the order
//! lineages stand in and which lineage each lane belongs to, since dropping a
//! team onto another is the same durable write as reordering two.
//!
//! The asymmetry is deliberate. A partial configuration is a change; a partial
//! arrangement is a loss, because the head's lane order *is* the durable team
//! order, so a lane the operator failed to restate would vanish from every team
//! while still being seated. [`arrange`] therefore admits only a faithful
//! restatement of the current lane table and repairs nothing by inference.
//!
//! `lane::arrange` is deliberately not here. It moves one seated member before
//! another and states only that pair, so it lives beside the seat surface in
//! the parent module even though both laws write the one lane table.

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
    let topology_revision = match next_topology_revision(observed.body(), request.topology_revision)
    {
        Ok(revision) => revision,
        Err(outcome) => return outcome,
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
    next.topology_revision = topology_revision;
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
    // A policy-only change crosses no lane boundary, so an empty read is authoritative.
    let moved: Vec<_> = membership.iter().map(|change| change.member).collect();
    let sources = match bus.lane_source_boundaries(&moved) {
        Ok(sources) => sources,
        Err(error) => {
            return typed_store_fault("lineage::boundary-unreadable", &error.to_string());
        }
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
            sources,
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
        .filter(|candidate| *candidate != member)
        .find(|candidate| {
            !head
                .lanes
                .iter()
                .any(|row| row.lineage_id == *candidate && row.lineage_id != source)
        })
        .unwrap_or_else(|| first_free_derived_lineage(head, source));
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

/// Resolve the first free UUID after `source`, preserving UUID v4/variant bits.
///
/// Member lane identities win in their resident order above. Only a collision
/// on every member reaches this deterministic suffix space, where the first
/// identity absent from lanes, lineages, and subscriptions wins.
fn first_free_derived_lineage(head: &wire::ResidencyHead, source: CanonicalUuid) -> CanonicalUuid {
    let mut ordinal = 1_u128;
    loop {
        let mut bytes = source.get().as_u128().wrapping_add(ordinal).to_be_bytes();
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let candidate = CanonicalUuid::new(uuid::Uuid::from_bytes(bytes));
        let names_lane = head
            .lanes
            .iter()
            .any(|row| row.lane == candidate || row.lineage_id == candidate);
        let names_subscription = head
            .subscriptions
            .iter()
            .any(|subscription| subscription.lineage_id == candidate);
        if !names_lane && !names_subscription {
            return candidate;
        }
        ordinal = ordinal
            .checked_add(1)
            .expect("the finite residency cannot occupy the complete UUID space");
    }
}

/// Group the head's lanes into lineage standings in first-appearance order.
///
/// First appearance is not an arbitrary choice: it is exactly how the Canvas
/// groups the lane answer into team surfaces, so the head's lane order *is* the
/// durable team order. Rewriting that order below is what makes a drag durable
/// without teaching the canvas a second ordering to disagree with.
/// Lanes this arrangement moved between lineages, in arranged order.
///
/// Reordering lanes inside one lineage moves no lane between lineages and so
/// splits no attribution; only a lane whose owning lineage differs across the
/// transaction crosses a boundary a reducer has to account for. Narrowing to
/// those keeps the recorded boundaries the exact set the gesture changed rather
/// than a snapshot of every seated lane.
fn moved_lanes(
    from: &[wire::body::LineageStanding],
    to: &[wire::body::LineageStanding],
) -> Vec<CanonicalUuid> {
    let owner = |standings: &[wire::body::LineageStanding], lane: &CanonicalUuid| {
        standings
            .iter()
            .find(|standing| standing.lanes.contains(lane))
            .map(|standing| standing.lineage_id)
    };
    let mut moved = Vec::new();
    for standing in to {
        for lane in &standing.lanes {
            if owner(from, lane) != Some(standing.lineage_id) {
                moved.push(*lane);
            }
        }
    }
    moved
}

fn standings(lanes: &[wire::LaneResidency]) -> Vec<wire::body::LineageStanding> {
    let mut standings = Vec::<wire::body::LineageStanding>::new();
    for row in lanes {
        if let Some(standing) = standings
            .iter_mut()
            .find(|standing| standing.lineage_id == row.lineage_id)
        {
            standing.lanes.push(row.lane);
        } else {
            standings.push(wire::body::LineageStanding {
                lineage_id: row.lineage_id,
                lanes: vec![row.lane],
            });
        }
    }
    standings
}

/// Refuse an arrangement that is not a faithful restatement of the lane table.
///
/// Every rule here is stated over the arrangement as a whole. A partial
/// arrangement is never repaired by inference: a lane the operator forgot would
/// otherwise be silently dropped from every team, which is a lane that exists
/// in the resident and appears nowhere on the canvas.
fn admit_arrangement(
    head: &wire::ResidencyHead,
    request: &LineageArrange,
) -> Result<(), Outcome<CommandAnswer>> {
    let current = standings(&head.lanes);
    if request.lineages.is_empty() {
        return Err(lane_refusal(
            "lineage::arrangement-empty",
            Some("at least one lineage".to_owned()),
            Some("0".to_owned()),
            "state every surviving lineage; an arrangement never removes a lane",
        ));
    }
    let mut seen_lineages = Vec::<CanonicalUuid>::new();
    let mut seen_lanes = Vec::<CanonicalUuid>::new();
    for lineage in &request.lineages {
        if !current
            .iter()
            .any(|standing| standing.lineage_id == lineage.lineage_id)
        {
            return Err(lane_refusal(
                "lineage::arrangement-unknown-lineage",
                Some("one current Canvas lineage".to_owned()),
                Some(lineage.lineage_id.to_string()),
                "join teams by naming the surviving lineage; a new lineage is created by seating",
            ));
        }
        if seen_lineages.contains(&lineage.lineage_id) {
            return Err(lane_refusal(
                "lineage::arrangement-duplicate-lineage",
                Some("each lineage stated once".to_owned()),
                Some(lineage.lineage_id.to_string()),
                "state one arrangement per lineage",
            ));
        }
        seen_lineages.push(lineage.lineage_id);
        if lineage.lanes.is_empty() {
            return Err(lane_refusal(
                "lineage::arrangement-vacant-lineage",
                Some("at least one lane".to_owned()),
                Some(lineage.lineage_id.to_string()),
                "omit an emptied lineage entirely rather than arranging it with no lanes",
            ));
        }
        for lane in &lineage.lanes {
            if !head.lanes.iter().any(|row| row.lane == *lane) {
                return Err(lane_refusal(
                    "lineage::arrangement-unknown-lane",
                    Some("one current lane".to_owned()),
                    Some(lane.to_string()),
                    "read lane state and retry with the current lane table",
                ));
            }
            if seen_lanes.contains(lane) {
                return Err(lane_refusal(
                    "lineage::arrangement-duplicate-lane",
                    Some("each lane held once".to_owned()),
                    Some(lane.to_string()),
                    "hold every lane in exactly one lineage",
                ));
            }
            seen_lanes.push(*lane);
        }
    }
    if let Some(missing) = head
        .lanes
        .iter()
        .find(|row| !seen_lanes.contains(&row.lane))
    {
        return Err(lane_refusal(
            "lineage::arrangement-incomplete",
            Some(format!("{} arranged lanes", head.lanes.len())),
            Some(missing.lane.to_string()),
            "state every current lane; an arrangement moves lanes, it never drops them",
        ));
    }
    Ok(())
}

/// Commit one complete Canvas lineage arrangement.
///
/// This is the durable end of both drag gestures. Reordering the teams and
/// joining two of them differ only in the arrangement the operator sends, so
/// they commit through one law and one fact rather than racing two.
pub(in crate::resident) fn arrange(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &Seats,
    command: &wire::AdmittedCommand,
    request: &LineageArrange,
) -> Outcome<CommandAnswer> {
    let observed = match current_residency(bus, epoch) {
        Ok(head) => head,
        Err(outcome) => return typed_outcome(outcome),
    };
    let topology_revision = match next_topology_revision(observed.body(), request.topology_revision)
    {
        Ok(revision) => revision,
        Err(outcome) => return outcome,
    };
    if let Err(outcome) = admit_arrangement(observed.body(), request) {
        return outcome;
    }
    let from = standings(&observed.body().lanes);
    let mut next = observed.body().clone();
    next.topology_revision = topology_revision;
    let mut arranged = Vec::with_capacity(next.lanes.len());
    for lineage in &request.lineages {
        for lane in &lineage.lanes {
            let Some(row) = next.lanes.iter().find(|row| row.lane == *lane) else {
                return typed_store_fault(
                    "lineage::arrangement-lane-lost",
                    "an admitted arrangement named a lane the residency head no longer holds",
                );
            };
            let mut row = row.clone();
            row.lineage_id = lineage.lineage_id;
            arranged.push(row);
        }
    }
    next.lanes = arranged;
    // A lineage the arrangement emptied keeps no durable subscription: leaving
    // one behind would silently re-apply that policy to a later lineage seated
    // under the same identity.
    next.subscriptions.retain(|subscription| {
        request
            .lineages
            .iter()
            .any(|lineage| lineage.lineage_id == subscription.lineage_id)
    });
    let lineages = standings(&next.lanes);
    let lane_answer = match answer(&next, None, seats) {
        Ok(answer) => answer,
        Err(outcome) => return typed_outcome(outcome),
    };
    let sources = match bus.lane_source_boundaries(&moved_lanes(&from, &lineages)) {
        Ok(sources) => sources,
        Err(error) => {
            return typed_store_fault("lineage::arrange-boundary-unreadable", &error.to_string());
        }
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
        wire::body::LineageArranged {
            from,
            lineages,
            sources,
        },
        FactCause::Existing(observed.position()),
    ) {
        Ok(fact) => fact,
        Err(error) => {
            return typed_store_fault("lineage::fact-invalid", &error.to_string());
        }
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
