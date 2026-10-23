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

use wire::grouping::{Arranged, Destination, Group, GroupingError, Partition, Placement};

type Teams = Partition<CanonicalUuid, CanonicalUuid>;
type TeamError = GroupingError<CanonicalUuid, CanonicalUuid>;

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
    if request.fast_mode == Some(true) {
        for row in next.lanes.iter_mut().filter(|row| row.team == request.team) {
            super::retune::forget_fast_refusal(row);
        }
    }
    let levels = match super::super::settings_surface::FastLevels::read(bus) {
        Ok(levels) => levels,
        Err(outcome) => return typed_outcome(outcome),
    };
    let flips = super::retune::fast_flips(observed.body(), &next, &levels, levels.global());
    // A policy-only change crosses no lane boundary, so an empty read is authoritative.
    let moved: Vec<_> = membership.iter().map(|change| change.member).collect();
    let sources = match bus.lane_source_boundaries(&moved) {
        Ok(sources) => sources,
        Err(error) => {
            return typed_store_fault("team::boundary-unreadable", &error.to_string());
        }
    };
    let write = match write(
        command.actor().clone(),
        epoch,
        Producer::ResidentRecognition,
    ) {
        Ok(write) => write,
        Err(outcome) => return typed_outcome(outcome),
    };
    reconcile_formations(observed.body(), &mut next, write.at());
    let lane_answer = match answer(&next, None, seats, paths) {
        Ok(answer) => answer,
        Err(outcome) => return typed_outcome(outcome),
    };
    commit_configured(
        bus,
        epoch,
        command,
        request,
        effects,
        &observed,
        write,
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
    mut write: journal::JournalWrite,
    configured: ConfiguredTeam,
) -> Outcome<CommandAnswer> {
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

/// Preserve a team's formation instant while its identity stays
/// [`wire::head::teamed`], mint it at the commit that makes the identity
/// teamed, and clear it while the identity is unteamed.
pub(super) fn reconcile_formations(
    observed: &wire::ResidencyHead,
    next: &mut wire::ResidencyHead,
    at: wire::Instant,
) {
    let teamed = |head: &wire::ResidencyHead, team| {
        wire::head::teamed(
            team,
            head.lanes
                .iter()
                .filter(|row| row.team == team)
                .map(|row| row.lane),
        )
    };
    for index in 0..next.subscriptions.len() {
        let team = next.subscriptions[index].team;
        next.subscriptions[index].formed_at = if !teamed(next, team) {
            None
        } else if teamed(observed, team) {
            observed
                .subscriptions
                .iter()
                .find(|current| current.team == team)
                .and_then(|current| current.formed_at)
        } else {
            Some(at)
        };
    }
}

/// The app teams one residency head states: its lanes, in durable order.
pub(super) fn partition(head: &wire::ResidencyHead) -> Result<Teams, Outcome<CommandAnswer>> {
    Partition::new(head.lanes.iter().map(|row| Placement {
        member: row.lane,
        group: row.team,
    }))
    .map_err(|_| {
        typed_store_fault(
            "team::partition-invalid",
            "the residency head seats one lane twice",
        )
    })
}

/// The lane table one partition states: each row in its placement's order
/// and team, with every other field unchanged.
pub(super) fn placed(
    head: &wire::ResidencyHead,
    teams: &Teams,
) -> Result<Vec<wire::LaneResidency>, Outcome<CommandAnswer>> {
    teams
        .placements()
        .iter()
        .map(|placement| {
            let mut row = head
                .lanes
                .iter()
                .find(|row| row.lane == placement.member)
                .cloned()
                .ok_or_else(|| {
                    typed_store_fault(
                        "team::placement-lost",
                        "a team placement names a lane the residency head no longer holds",
                    )
                })?;
            row.team = placement.group;
            Ok(row)
        })
        .collect()
}

/// The refusal for a grouping change no team command words for itself.
pub(super) fn grouping_refusal(error: TeamError) -> Outcome<CommandAnswer> {
    match error {
        GroupingError::MemberLimit { members } => lane_refusal(
            "team::member-limit",
            Some(format!("at most {} agents", wire::head::TEAM_MEMBER_LIMIT)),
            Some(members.to_string()),
            "move an agent out of the target team or choose another team",
        ),
        other => typed_store_fault("team::grouping-invalid", &format!("{other:?}")),
    }
}

/// Teams as a grouping kind: members are seated lanes, and a lane in no
/// formation stands alone in the team carrying its own identity.
struct TeamKind<'a>(&'a wire::ResidencyHead);

impl wire::grouping::Kind for TeamKind<'_> {
    type Group = CanonicalUuid;
    type Member = CanonicalUuid;

    fn admits(&self, member: CanonicalUuid) -> bool {
        self.0.lanes.iter().any(|row| row.lane == member)
    }

    fn ungrouped(&self, member: CanonicalUuid) -> Option<CanonicalUuid> {
        Some(member)
    }

    /// The first remaining member whose identity names no other team, else a
    /// derived identity free across lanes, teams, and subscriptions.
    fn rekey(&self, teams: &Teams, group: CanonicalUuid, leaving: CanonicalUuid) -> CanonicalUuid {
        teams
            .members(group)
            .filter(|candidate| *candidate != leaving)
            .find(|candidate| *candidate == group || !teams.holds(*candidate))
            .unwrap_or_else(|| first_free_derived_team(self.0, group))
    }
}

fn member_missing(member: CanonicalUuid) -> Outcome<CommandAnswer> {
    lane_refusal(
        "team::member-missing",
        Some("one current team member".to_owned()),
        Some(member.to_string()),
        "read lane state and retry with a current member identity",
    )
}

fn move_member(
    head: &mut wire::ResidencyHead,
    member: CanonicalUuid,
    target: CanonicalUuid,
) -> Result<MembershipChange, Outcome<CommandAnswer>> {
    let teams = partition(head)?;
    let target_exists = teams.holds(target);
    let (next, moved) = teams
        .apply_move(&TeamKind(head), member, Destination::Group(target))
        .map_err(|error| match error {
            GroupingError::NotAdmitted(_) => member_missing(member),
            GroupingError::Unchanged => lane_refusal(
                "team::member-unchanged",
                Some("a different target team".to_owned()),
                Some(target.to_string()),
                "drop the member on another team or on Agents Without Team",
            ),
            other => grouping_refusal(other),
        })?;
    let Some(source) = moved.from else {
        return Err(member_missing(member));
    };
    let source_subscription = match subscription_row(head, source) {
        Ok(Some(subscription)) => subscription.clone(),
        Ok(None) => {
            return Err(typed_store_fault(
                "team::subscription-missing",
                if moved.rekeyed.is_some() {
                    "the split source has no task-allocation lifetime"
                } else {
                    "the moved member source has no task-allocation lifetime"
                },
            ));
        }
        Err(outcome) => return Err(typed_outcome(outcome)),
    };
    let added = if let Some(rekeyed) = moved.rekeyed {
        Some((source_subscription, rekeyed))
    } else if target_exists {
        match subscription_row(head, target) {
            Ok(Some(_)) => None,
            Ok(None) => {
                return Err(typed_store_fault(
                    "team::subscription-missing",
                    "the target team has no task-allocation lifetime",
                ));
            }
            Err(outcome) => return Err(typed_outcome(outcome)),
        }
    } else {
        Some((source_subscription, target))
    };
    head.lanes = placed(head, &next)?;
    if let Some((mut subscription, team)) = added {
        subscription.team = team;
        head.subscriptions.push(subscription);
    }
    if !next.holds(source) {
        head.subscriptions
            .retain(|subscription| subscription.team != source);
    }
    head.subscriptions
        .sort_by_key(|subscription| subscription.team);
    Ok(MembershipChange {
        member,
        from_team: source,
        source_rekeyed_to: moved.rekeyed,
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

/// Group one partition into team standings in first-appearance order.
///
/// First appearance is not an arbitrary choice: it is exactly how the app
/// groups the lane answer into team surfaces, so the head's lane order *is* the
/// durable team order. Rewriting that order below is what makes a drag durable
/// without teaching the app a second ordering to disagree with.
fn standings(teams: &Teams) -> Vec<wire::body::TeamStanding> {
    teams
        .groups()
        .into_iter()
        .map(|group| wire::body::TeamStanding {
            team: group.group,
            lanes: group.members,
        })
        .collect()
}

/// Admit an arrangement only as a faithful restatement of the lane table.
///
/// Every rule here is stated over the arrangement as a whole. A partial
/// arrangement is never repaired by inference: a lane the operator forgot would
/// otherwise be silently dropped from every team, which is a lane that exists
/// in the resident and appears nowhere in the app.
fn admit_arrangement(
    head: &wire::ResidencyHead,
    request: &TeamArrange,
) -> Result<Arranged<CanonicalUuid, CanonicalUuid>, Outcome<CommandAnswer>> {
    let proposed = request
        .teams
        .iter()
        .map(|team| Group {
            group: team.team,
            members: team.lanes.clone(),
        })
        .collect::<Vec<_>>();
    partition(head)?
        .arrange(&proposed)
        .map_err(arrangement_refusal)
}

fn arrangement_refusal(error: TeamError) -> Outcome<CommandAnswer> {
    let (reason, expected, observed, detail) = match error {
        GroupingError::ArrangeEmpty => (
            "team::arrangement-empty",
            "at least one team".to_owned(),
            "0".to_owned(),
            "state every surviving team; an arrangement never removes a lane",
        ),
        GroupingError::UnknownGroup(team) => (
            "team::arrangement-unknown-team",
            "one current team or a fresh singleton split".to_owned(),
            team.to_string(),
            "a fresh team must hold one lane from a current multi-agent team",
        ),
        GroupingError::DuplicateGroup(team) => (
            "team::arrangement-duplicate-team",
            "each team stated once".to_owned(),
            team.to_string(),
            "state one arrangement per team",
        ),
        GroupingError::VacantGroup(team) => (
            "team::arrangement-vacant-team",
            "at least one lane".to_owned(),
            team.to_string(),
            "omit an emptied team entirely rather than arranging it with no lanes",
        ),
        GroupingError::UnknownMember(lane) => (
            "team::arrangement-unknown-lane",
            "one current lane".to_owned(),
            lane.to_string(),
            "read lane state and retry with the current lane table",
        ),
        GroupingError::DuplicateMember(lane) => (
            "team::arrangement-duplicate-lane",
            "each lane held once".to_owned(),
            lane.to_string(),
            "hold every lane in exactly one team",
        ),
        GroupingError::Incomplete { expected, missing } => (
            "team::arrangement-incomplete",
            format!("{expected} arranged lanes"),
            missing.to_string(),
            "state every current lane; an arrangement moves lanes, it never drops them",
        ),
        other => return grouping_refusal(other),
    };
    lane_refusal(reason, Some(expected), Some(observed), detail)
}

/// The residency head one admitted arrangement produces.
fn arranged_head(
    observed: &wire::ResidencyHead,
    arranged: &Arranged<CanonicalUuid, CanonicalUuid>,
    topology_revision: u64,
) -> Result<wire::ResidencyHead, Outcome<CommandAnswer>> {
    let mut inherited = Vec::with_capacity(arranged.splits.len());
    for (team, source) in &arranged.splits {
        let Some(subscription) = observed
            .subscriptions
            .iter()
            .find(|subscription| subscription.team == *source)
        else {
            return Err(typed_store_fault(
                "team::subscription-missing",
                "a split source team has no task-allocation lifetime",
            ));
        };
        inherited.push(wire::TeamSubscription {
            team: *team,
            ..subscription.clone()
        });
    }
    let mut next = observed.clone();
    next.topology_revision = topology_revision;
    next.lanes = placed(observed, &arranged.partition)?;
    // A team the arrangement emptied keeps no durable subscription: leaving
    // one behind would silently re-apply that policy to a later lineage seated
    // under the same identity.
    next.subscriptions
        .retain(|subscription| arranged.partition.holds(subscription.team));
    next.subscriptions.extend(inherited);
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
    let arranged = match admit_arrangement(observed.body(), request) {
        Ok(arranged) => arranged,
        Err(outcome) => return outcome,
    };
    let from = match partition(observed.body()) {
        Ok(current) => standings(&current),
        Err(outcome) => return outcome,
    };
    let mut next = match arranged_head(observed.body(), &arranged, topology_revision) {
        Ok(next) => next,
        Err(outcome) => return outcome,
    };
    let levels = match super::super::settings_surface::FastLevels::read(bus) {
        Ok(levels) => levels,
        Err(outcome) => return typed_outcome(outcome),
    };
    let flips = super::retune::fast_flips(observed.body(), &next, &levels, levels.global());
    let teams = standings(&arranged.partition);
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
    reconcile_formations(observed.body(), &mut next, write.at());
    let lane_answer = match answer(&next, None, seats, paths) {
        Ok(answer) => answer,
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
