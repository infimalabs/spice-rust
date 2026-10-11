//! The two writes that address app teams rather than the seats in them.
//!
//! [`configure`] names one team and changes it: its task-allocation
//! lifetime, its fast-mode flag, and at most one member's home. [`arrange`] names no team in
//! particular and restates the whole topology at once, rewriting both the order
//! teams stand in and which team each lane belongs to, since dropping a
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
    from_team: CanonicalUuid,
    source_rekeyed_to: Option<CanonicalUuid>,
}

struct ConfiguredTeam {
    membership: Option<MembershipChange>,
    previous: wire::TeamSubscription,
    life: wire::intent::TeamLife,
    fast_mode: bool,
    sources: Vec<wire::body::LaneSourceBoundary>,
    next: wire::ResidencyHead,
    answer: LaneAnswer,
    flips: Vec<super::retune::FastFlip>,
}

/// Commit one task-allocation lifetime, fast-mode flag, and optional member
/// move, then ask running OMP members whose effective fast mode flipped to
/// follow it.
///
/// `drain` here is a policy spelling. Occupant teardown remains exclusively
/// owned by `lane::state { action: drain }`, so this transaction changes no
/// seat, capability, session, occupant, or disposition field.
pub(in crate::resident) fn configure(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &Seats,
    command: &wire::AdmittedCommand,
    request: &TeamConfigure,
    effects: &mut CommandEffects,
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
    let mut next = observed.body().clone();
    next.topology_revision = topology_revision;
    let membership = match request.member {
        Some(member) => match move_member(&mut next, member, request.team) {
            Ok(change) => Some(change),
            Err(outcome) => return outcome,
        },
        None => None,
    };
    let Some(subscription) = next
        .subscriptions
        .iter_mut()
        .find(|subscription| subscription.team == request.team)
    else {
        return typed_store_fault(
            "team::subscription-missing",
            "the configured team has no task-allocation lifetime after membership moved",
        );
    };
    if let Some(life) = request.life {
        subscription.policy = life;
    }
    if let Some(fast_mode) = request.fast_mode {
        subscription.fast_mode = fast_mode;
    }
    let configured_life = subscription.policy;
    let configured_fast = subscription.fast_mode;
    let global_fast = match super::super::settings_surface::fast_mode(bus) {
        Ok(fast_mode) => fast_mode,
        Err(outcome) => return typed_outcome(outcome),
    };
    let flips = super::retune::fast_flips(observed.body(), &next, global_fast);
    let lane_answer = match answer(&next, None, seats, paths) {
        Ok(answer) => answer,
        Err(outcome) => return typed_outcome(outcome),
    };
    // A policy-only change crosses no lane boundary, so an empty read is authoritative.
    let moved: Vec<_> = membership.iter().map(|change| change.member).collect();
    let sources = match bus.lane_source_boundaries(&moved) {
        Ok(sources) => sources,
        Err(error) => {
            return typed_store_fault("team::boundary-unreadable", &error.to_string());
        }
    };
    commit_configured(
        bus,
        epoch,
        command,
        request,
        effects,
        &observed,
        ConfiguredTeam {
            membership,
            previous,
            life: configured_life,
            fast_mode: configured_fast,
            sources,
            next,
            answer: lane_answer,
            flips,
        },
    )
}

fn commit_configured(
    bus: &Bus,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    request: &TeamConfigure,
    effects: &mut CommandEffects,
    observed: &journal::JournalHead<wire::ResidencyHead>,
    configured: ConfiguredTeam,
) -> Outcome<CommandAnswer> {
    let mut write = match write(
        command.actor().clone(),
        epoch,
        Producer::ResidentRecognition,
    ) {
        Ok(write) => write,
        Err(outcome) => return typed_outcome(outcome),
    };
    guard_residency(&mut write, observed);
    let fact = match write.fact(
        wire::body::TeamConfigured {
            team: request.team,
            member: configured.membership.as_ref().map(|change| change.member),
            from_team: configured
                .membership
                .as_ref()
                .map(|change| change.from_team),
            source_rekeyed_to: configured
                .membership
                .and_then(|change| change.source_rekeyed_to),
            from: configured.previous.policy,
            life: configured.life,
            from_fast_mode: configured.previous.fast_mode,
            fast_mode: configured.fast_mode,
            sources: configured.sources,
        },
        FactCause::Existing(observed.position()),
    ) {
        Ok(fact) => fact,
        Err(error) => return typed_store_fault("team::fact-invalid", &error.to_string()),
    };
    write.residency_head(fact, move |_| configured.next);
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
        configured.answer.clone(),
    ) {
        return typed_store_fault("team::receipt-invalid", &error.to_string());
    }
    let committed = match commit(bus, write) {
        Ok(committed) => committed,
        Err(outcome) => return typed_outcome(outcome),
    };
    super::retune::owe_fast_flips(effects, command, configured.flips);
    Outcome::Reply(Reply {
        position: ReplyPosition::Committed(SeqRange {
            first: seq(committed.first()),
            last: seq(committed.last()),
        }),
        body: CommandAnswer::Lane(Box::new(configured.answer)),
    })
}

fn previous_subscription(
    head: &wire::ResidencyHead,
    request: &TeamConfigure,
) -> Result<wire::TeamSubscription, Outcome<CommandAnswer>> {
    let target_exists = head.lanes.iter().any(|row| row.team == request.team);
    let baseline = if target_exists {
        request.team
    } else if let Some(member) = request.member {
        let Some(row) = head.lanes.iter().find(|row| row.lane == member) else {
            return Err(lane_refusal(
                "team::member-missing",
                Some("one current team member".to_owned()),
                Some(member.to_string()),
                "read lane state and retry with a current member identity",
            ));
        };
        row.team
    } else {
        return Err(lane_refusal(
            "team::missing",
            Some("one current team".to_owned()),
            Some(request.team.to_string()),
            "read lane state and retry with a current team identity",
        ));
    };
    match subscription_row(head, baseline) {
        Ok(Some(subscription)) => Ok(subscription.clone()),
        Ok(None) => Err(typed_store_fault(
            "team::subscription-missing",
            "a current team has no task-allocation lifetime",
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
            "team::member-missing",
            Some("one current team member".to_owned()),
            Some(member.to_string()),
            "read lane state and retry with a current member identity",
        ));
    };
    let source = head.lanes[member_index].team;
    let source_members = head
        .lanes
        .iter()
        .filter(|row| row.team == source)
        .map(|row| row.lane)
        .collect::<Vec<_>>();
    if source == target {
        if member != target || source_members.len() == 1 {
            return Err(lane_refusal(
                "team::member-unchanged",
                Some("a different target team".to_owned()),
                Some(target.to_string()),
                "drop the member on another team or on Agents Without Team",
            ));
        }
        return split_host_member(head, member, source, &source_members);
    }
    let current_members = head.team_members(target).count();
    admit_team_size(current_members, current_members + 1)?;
    let source_subscription = match subscription_row(head, source) {
        Ok(Some(subscription)) => subscription.clone(),
        Ok(None) => {
            return Err(typed_store_fault(
                "team::subscription-missing",
                "the moved member source has no task-allocation lifetime",
            ));
        }
        Err(outcome) => return Err(typed_outcome(outcome)),
    };
    let target_exists = head.lanes.iter().any(|row| row.team == target);
    if target_exists {
        match subscription_row(head, target) {
            Ok(Some(_)) => {}
            Ok(None) => {
                return Err(typed_store_fault(
                    "team::subscription-missing",
                    "the target team has no task-allocation lifetime",
                ));
            }
            Err(outcome) => return Err(typed_outcome(outcome)),
        }
    } else {
        let mut subscription = source_subscription;
        subscription.team = target;
        head.subscriptions.push(subscription);
    }
    head.lanes[member_index].team = target;
    if target_exists {
        // A joining member lands at the tail of its new team. Member order,
        // and the accents it drives, belong to the team, so a member created
        // earlier elsewhere must not jump ahead of the team's current members.
        let row = head.lanes.remove(member_index);
        let tail = head
            .lanes
            .iter()
            .rposition(|row| row.team == target)
            .map_or(head.lanes.len(), |last| last + 1);
        head.lanes.insert(tail, row);
    }
    if !head.lanes.iter().any(|row| row.team == source) {
        head.subscriptions
            .retain(|subscription| subscription.team != source);
    }
    head.subscriptions
        .sort_by_key(|subscription| subscription.team);
    Ok(MembershipChange {
        member,
        from_team: source,
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
                .any(|row| row.team == *candidate && row.team != source)
        })
        .unwrap_or_else(|| first_free_derived_team(head, source));
    let source_subscription = match subscription_row(head, source) {
        Ok(Some(subscription)) => subscription.clone(),
        Ok(None) => {
            return Err(typed_store_fault(
                "team::subscription-missing",
                "the split source has no task-allocation lifetime",
            ));
        }
        Err(outcome) => return Err(typed_outcome(outcome)),
    };
    for row in &mut head.lanes {
        if row.team == source && row.lane != member {
            row.team = rekeyed;
        }
    }
    let mut remainder = source_subscription;
    remainder.team = rekeyed;
    head.subscriptions.push(remainder);
    head.subscriptions
        .sort_by_key(|subscription| subscription.team);
    Ok(MembershipChange {
        member,
        from_team: source,
        source_rekeyed_to: Some(rekeyed),
    })
}

/// Resolve the first free UUID after `source`, preserving UUID v4/variant bits.
///
/// Member lane identities win in their resident order above. Only a collision
/// on every member reaches this deterministic suffix space, where the first
/// identity absent from lanes, teams, and subscriptions wins.
fn first_free_derived_team(head: &wire::ResidencyHead, source: CanonicalUuid) -> CanonicalUuid {
    let mut ordinal = 1_u128;
    loop {
        let mut bytes = source.get().as_u128().wrapping_add(ordinal).to_be_bytes();
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let candidate = CanonicalUuid::new(uuid::Uuid::from_bytes(bytes));
        let names_lane = head
            .lanes
            .iter()
            .any(|row| row.lane == candidate || row.team == candidate);
        let names_subscription = head
            .subscriptions
            .iter()
            .any(|subscription| subscription.team == candidate);
        if !names_lane && !names_subscription {
            return candidate;
        }
        ordinal = ordinal
            .checked_add(1)
            .expect("the finite residency cannot occupy the complete UUID space");
    }
}

/// Group the head's lanes into team standings in first-appearance order.
///
/// First appearance is not an arbitrary choice: it is exactly how the app
/// groups the lane answer into team surfaces, so the head's lane order *is* the
/// durable team order. Rewriting that order below is what makes a drag durable
/// without teaching the app a second ordering to disagree with.
/// Lanes this arrangement moved between teams, in arranged order.
///
/// Reordering lanes inside one team moves no lane between teams and so
/// splits no attribution; only a lane whose owning team differs across the
/// transaction crosses a boundary a reducer has to account for. Narrowing to
/// those keeps the recorded boundaries the exact set the gesture changed rather
/// than a snapshot of every seated lane.
fn moved_lanes(
    from: &[wire::body::TeamStanding],
    to: &[wire::body::TeamStanding],
) -> Vec<CanonicalUuid> {
    let owner = |standings: &[wire::body::TeamStanding], lane: &CanonicalUuid| {
        standings
            .iter()
            .find(|standing| standing.lanes.contains(lane))
            .map(|standing| standing.team)
    };
    let mut moved = Vec::new();
    for standing in to {
        for lane in &standing.lanes {
            if owner(from, lane) != Some(standing.team) {
                moved.push(*lane);
            }
        }
    }
    moved
}

fn standings(lanes: &[wire::LaneResidency]) -> Vec<wire::body::TeamStanding> {
    let mut standings = Vec::<wire::body::TeamStanding>::new();
    for row in lanes {
        if let Some(standing) = standings
            .iter_mut()
            .find(|standing| standing.team == row.team)
        {
            standing.lanes.push(row.lane);
        } else {
            standings.push(wire::body::TeamStanding {
                team: row.team,
                lanes: vec![row.lane],
            });
        }
    }
    standings
}

/// The multi-agent source from which one newly named singleton team splits.
fn split_source_team(
    head: &wire::ResidencyHead,
    team: &wire::intent::TeamArrangement,
) -> Option<CanonicalUuid> {
    let [lane] = team.lanes.as_slice() else {
        return None;
    };
    let source = head.lanes.iter().find(|row| row.lane == *lane)?.team;
    head.lanes
        .iter()
        .filter(|row| row.team == source)
        .nth(1)
        .map(|_| source)
}

fn inherited_split_subscriptions(
    head: &wire::ResidencyHead,
    request: &TeamArrange,
) -> Result<Vec<wire::TeamSubscription>, Outcome<CommandAnswer>> {
    let mut inherited = Vec::new();
    for team in &request.teams {
        if head.lanes.iter().any(|row| row.team == team.team) {
            continue;
        }
        let Some(source) = split_source_team(head, team) else {
            return Err(typed_store_fault(
                "team::arrangement-split-lost",
                "an admitted fresh team no longer resolves to its source team",
            ));
        };
        let Some(subscription) = head
            .subscriptions
            .iter()
            .find(|subscription| subscription.team == source)
        else {
            return Err(typed_store_fault(
                "team::subscription-missing",
                "a split source team has no task-allocation lifetime",
            ));
        };
        inherited.push(wire::TeamSubscription {
            team: team.team,
            ..subscription.clone()
        });
    }
    Ok(inherited)
}

/// Refuse an arrangement that is not a faithful restatement of the lane table.
///
/// Every rule here is stated over the arrangement as a whole. A partial
/// arrangement is never repaired by inference: a lane the operator forgot would
/// otherwise be silently dropped from every team, which is a lane that exists
/// in the resident and appears nowhere in the app.
fn admit_arrangement(
    head: &wire::ResidencyHead,
    request: &TeamArrange,
) -> Result<(), Outcome<CommandAnswer>> {
    let current = standings(&head.lanes);
    if request.teams.is_empty() {
        return Err(lane_refusal(
            "team::arrangement-empty",
            Some("at least one team".to_owned()),
            Some("0".to_owned()),
            "state every surviving team; an arrangement never removes a lane",
        ));
    }
    let mut seen_teams = Vec::<CanonicalUuid>::new();
    let mut seen_lanes = Vec::<CanonicalUuid>::new();
    for team in &request.teams {
        if !current.iter().any(|standing| standing.team == team.team)
            && split_source_team(head, team).is_none()
        {
            return Err(lane_refusal(
                "team::arrangement-unknown-team",
                Some("one current team or a fresh singleton split".to_owned()),
                Some(team.team.to_string()),
                "a fresh team must hold one lane from a current multi-agent team",
            ));
        }
        if seen_teams.contains(&team.team) {
            return Err(lane_refusal(
                "team::arrangement-duplicate-team",
                Some("each team stated once".to_owned()),
                Some(team.team.to_string()),
                "state one arrangement per team",
            ));
        }
        seen_teams.push(team.team);
        if team.lanes.is_empty() {
            return Err(lane_refusal(
                "team::arrangement-vacant-team",
                Some("at least one lane".to_owned()),
                Some(team.team.to_string()),
                "omit an emptied team entirely rather than arranging it with no lanes",
            ));
        }
        admit_team_size(head.team_members(team.team).count(), team.lanes.len())?;
        for lane in &team.lanes {
            if !head.lanes.iter().any(|row| row.lane == *lane) {
                return Err(lane_refusal(
                    "team::arrangement-unknown-lane",
                    Some("one current lane".to_owned()),
                    Some(lane.to_string()),
                    "read lane state and retry with the current lane table",
                ));
            }
            if seen_lanes.contains(lane) {
                return Err(lane_refusal(
                    "team::arrangement-duplicate-lane",
                    Some("each lane held once".to_owned()),
                    Some(lane.to_string()),
                    "hold every lane in exactly one team",
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
            "team::arrangement-incomplete",
            Some(format!("{} arranged lanes", head.lanes.len())),
            Some(missing.lane.to_string()),
            "state every current lane; an arrangement moves lanes, it never drops them",
        ));
    }
    Ok(())
}

/// The residency head one admitted arrangement produces.
fn arranged_head(
    observed: &wire::ResidencyHead,
    request: &TeamArrange,
    topology_revision: u64,
    inherited_subscriptions: Vec<wire::TeamSubscription>,
) -> Result<wire::ResidencyHead, Outcome<CommandAnswer>> {
    let mut next = observed.clone();
    next.topology_revision = topology_revision;
    let mut arranged = Vec::with_capacity(next.lanes.len());
    for team in &request.teams {
        for lane in &team.lanes {
            let Some(row) = next.lanes.iter().find(|row| row.lane == *lane) else {
                return Err(typed_store_fault(
                    "team::arrangement-lane-lost",
                    "an admitted arrangement named a lane the residency head no longer holds",
                ));
            };
            let mut row = row.clone();
            row.team = team.team;
            arranged.push(row);
        }
    }
    next.lanes = arranged;
    // A team the arrangement emptied keeps no durable subscription: leaving
    // one behind would silently re-apply that policy to a later lineage seated
    // under the same identity.
    next.subscriptions.retain(|subscription| {
        request
            .teams
            .iter()
            .any(|team| team.team == subscription.team)
    });
    next.subscriptions.extend(inherited_subscriptions);
    next.subscriptions
        .sort_by_key(|subscription| subscription.team);
    Ok(next)
}

/// Commit one complete app team arrangement.
///
/// This is the durable end of both drag gestures. Reordering the teams and
/// joining two of them differ only in the arrangement the operator sends, so
/// they commit through one law and one fact rather than racing two. A lane
/// that lands in a team with another fast-mode flag follows it live.
pub(in crate::resident) fn arrange(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &Seats,
    command: &wire::AdmittedCommand,
    request: &TeamArrange,
    effects: &mut CommandEffects,
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
    let inherited_subscriptions = match inherited_split_subscriptions(observed.body(), request) {
        Ok(subscriptions) => subscriptions,
        Err(outcome) => return outcome,
    };
    let next = match arranged_head(
        observed.body(),
        request,
        topology_revision,
        inherited_subscriptions,
    ) {
        Ok(next) => next,
        Err(outcome) => return outcome,
    };
    let global_fast = match super::super::settings_surface::fast_mode(bus) {
        Ok(fast_mode) => fast_mode,
        Err(outcome) => return typed_outcome(outcome),
    };
    let flips = super::retune::fast_flips(observed.body(), &next, global_fast);
    let teams = standings(&next.lanes);
    let lane_answer = match answer(&next, None, seats, paths) {
        Ok(answer) => answer,
        Err(outcome) => return typed_outcome(outcome),
    };
    let sources = match bus.lane_source_boundaries(&moved_lanes(&from, &teams)) {
        Ok(sources) => sources,
        Err(error) => {
            return typed_store_fault("team::arrange-boundary-unreadable", &error.to_string());
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
        wire::body::TeamArranged {
            from,
            teams,
            sources,
        },
        FactCause::Existing(observed.position()),
    ) {
        Ok(fact) => fact,
        Err(error) => {
            return typed_store_fault("team::fact-invalid", &error.to_string());
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
        return typed_store_fault("team::receipt-invalid", &error.to_string());
    }
    let committed = match commit(bus, write) {
        Ok(committed) => committed,
        Err(outcome) => return typed_outcome(outcome),
    };
    super::retune::owe_fast_flips(effects, command, flips);
    Outcome::Reply(Reply {
        position: ReplyPosition::Committed(SeqRange {
            first: seq(committed.first()),
            last: seq(committed.last()),
        }),
        body: CommandAnswer::Lane(Box::new(lane_answer)),
    })
}
