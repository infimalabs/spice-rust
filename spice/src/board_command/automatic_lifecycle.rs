//! Resident-owned claims for automatic lifecycle wakes.

use super::*;

/// Claim authority minted by the resident before it opens one vacant lane.
///
/// The occupant identity is chosen before the claim, so a crash after this
/// transaction and before process launch leaves an ordinary held claim for the
/// lifecycle authority to recover. No provisional claim vocabulary or second
/// writer exists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AutomaticClaim {
    pub(crate) task: TaskStamp,
    pub(crate) lane: wire::CanonicalUuid,
    pub(crate) occupant: wire::CanonicalUuid,
}

/// The vacant seat one automatic claim is minted for.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AutomaticSeat {
    pub(crate) epoch: wire::CanonicalUuid,
    pub(crate) lane: wire::CanonicalUuid,
    pub(crate) occupant: wire::CanonicalUuid,
}

/// The board policy an automatic claim selects and admits under.
#[derive(Clone, Copy)]
pub(crate) struct AllocationPolicy<'a> {
    pub(crate) projects: &'a ProjectPolicy,
    pub(crate) allocation: &'a board::NextConfig,
}

/// Select and claim once for a vacant resident-owned lane.
///
/// This is the automatic counterpart of `claim_win`: the same validated
/// snapshot, selector, allocator guard, and mint writer decide the result.
pub(crate) fn claim_automatic(
    bus: &Bus,
    observed: &bus::TaskSnapshot,
    repository: Host<'_>,
    seat: AutomaticSeat,
    policy: AllocationPolicy<'_>,
) -> Result<AutomaticClaim, Outcome<CommandAnswer>> {
    let PreparedClaim {
        write,
        selected,
        entries,
        task,
        born,
        actor,
        git,
    } = prepare_claim(bus, observed, repository, seat, policy)?;
    let answer = automatic_answer(&selected, &entries, task, actor, git.transplant());
    match committed_with_task_git(
        bus,
        write,
        CommandAnswer::Claim(Box::new(answer)),
        PreparedTaskGit::Claim(git),
    ) {
        Outcome::Reply(_) => Ok(AutomaticClaim {
            task: born,
            lane: seat.lane,
            occupant: seat.occupant,
        }),
        Outcome::Refusal(refusal) => Err(Outcome::Refusal(refusal)),
        Outcome::Fault(fault) => Err(Outcome::Fault(fault)),
    }
}

struct PreparedClaim<'a> {
    write: JournalWrite,
    selected: Entry,
    entries: Vec<Entry>,
    task: TaskHandle,
    born: TaskStamp,
    actor: Actor,
    git: spice::task_git::ClaimPreparation<'a>,
}

fn prepare_claim<'a>(
    bus: &Bus,
    observed: &bus::TaskSnapshot,
    repository: Host<'a>,
    seat: AutomaticSeat,
    policy: AllocationPolicy<'_>,
) -> Result<PreparedClaim<'a>, Outcome<CommandAnswer>> {
    let AutomaticSeat {
        epoch,
        lane,
        occupant,
    } = seat;
    let actor = Actor::Lane(lane);
    let snapshot = validated_snapshot_for_automatic(
        observed,
        actor.clone(),
        occupant,
        policy.projects,
        policy.allocation,
    )?;
    let row = available_lane(&snapshot, lane)?;
    let subscription = available_subscription(&snapshot, row)?;
    let selected = select_task(&snapshot, &actor, &subscription)?;
    let task = selected.handle().clone();
    let born = selected.born().clone();
    let (at, expiry, maximum) = claim_deadlines()?;
    let source = claim_source_boundary(bus, &lane)?;
    let origin = task_origin(&born)?;
    let lane_repository = lane_repository(repository, &row.worktree)?;
    // A resident claim inherits the task's parked work exactly as an allocator
    // claim does, so a release followed by an automatic wake loses nothing.
    let git = spice::task_git::prepare_task_claim(
        &lane_repository,
        repository.ambient,
        task_parked_commits(&snapshot, &born),
    )
    .map_err(task_git_outcome)?;
    let mut write = JournalWrite::new(actor.clone(), origin, at, Producer::CommandDoor);
    guard_allocation(
        &mut write,
        Vec::new(),
        AllocationObservation {
            cursor: snapshot.cursor,
            actor: actor.clone(),
            residency_position: snapshot.answer.residency.position,
        },
    );
    if let Err(outcome) = mint_claim(
        &mut write,
        board::allocate::Mint {
            task: &born,
            actor: &actor,
            workflow: selected.workflow(),
            project: selected.project().as_str(),
            lane,
            occupant,
            epoch,
            at,
            expiry,
            maximum,
            git: Some(git.boundary.clone()),
            recovery: None,
            source,
            cause: None,
        },
    ) {
        return Err(match git.rollback() {
            Ok(()) => outcome,
            Err(error) => task_git_outcome(error),
        });
    }
    Ok(PreparedClaim {
        write,
        selected,
        entries: snapshot.entries,
        task,
        born,
        actor,
        git,
    })
}

fn available_lane(
    snapshot: &ValidatedSnapshot,
    lane: wire::CanonicalUuid,
) -> Result<&wire::LaneResidency, Outcome<CommandAnswer>> {
    let row = snapshot
        .answer
        .residency
        .body
        .lanes
        .iter()
        .find(|row| row.lane == lane)
        .ok_or_else(|| {
            refusal(
                "lifecycle::available-work-lane-missing",
                Some(lane.to_string()),
                "reconcile against one current vacant lane",
            )
        })?;
    if row.occupant.is_some() || row.disposition == "occupied" {
        return Err(refusal(
            "lifecycle::available-work-capacity-moved",
            Some(row.disposition.clone()),
            "reconcile again after the current occupant is observed",
        ));
    }
    Ok(row)
}

/// The task-allocation lifetime of one vacant lane's team.
///
/// The team policy alone decides: a lane seated alone follows the life its own
/// team was seeded with, and capacity-following wake refuses only a steered
/// team.
fn available_subscription(
    snapshot: &ValidatedSnapshot,
    row: &wire::LaneResidency,
) -> Result<wire::TeamSubscription, Outcome<CommandAnswer>> {
    let residency = &snapshot.answer.residency.body;
    let mut subscriptions = residency
        .subscriptions
        .iter()
        .filter(|subscription| subscription.team == row.team);
    let subscription = subscriptions.next().ok_or_else(|| {
        store_fault(
            "lifecycle::available-work-subscription-missing",
            "the vacant lane lineage has no durable task-allocation lifetime",
        )
    })?;
    if subscriptions.next().is_some() {
        return Err(store_fault(
            "lifecycle::available-work-subscription-ambiguous",
            "the vacant lane lineage has more than one durable task-allocation lifetime",
        ));
    }
    let subscription = subscription.clone();
    if subscription.policy == wire::intent::TeamLife::Steer {
        return Err(refusal(
            "lifecycle::available-work-scope",
            Some(subscription.policy.as_str().to_owned()),
            "use a drive or drain team policy for capacity-following wake",
        ));
    }
    Ok(subscription)
}

fn select_task(
    snapshot: &ValidatedSnapshot,
    actor: &Actor,
    subscription: &wire::TeamSubscription,
) -> Result<Entry, Outcome<CommandAnswer>> {
    let environment = snapshot.answer.environment();
    let now = current_instant()?;
    board::select_at(
        &snapshot.entries,
        actor,
        subscription,
        snapshot.answer.allocation_band,
        snapshot.answer.recent_cell.as_ref(),
        &environment,
        now,
    )
    .map(|selection| selection.entry().clone())
    .ok_or_else(|| {
        refusal(
            "lifecycle::available-work-empty",
            None,
            "wait for a durable eligible task transition",
        )
    })
}

/// The lane's own repository, refused unless it is still the resident's worktree.
/// A refusal parks only this lane; a fault would end the resident for every lane.
fn lane_repository(
    repository: Host<'_>,
    worktree: &str,
) -> Result<Repository, Outcome<CommandAnswer>> {
    let lane_repository = Repository::discover(worktree)
        .map_err(|error| environment_fault("git::worktree-unresolved", &error.to_string()))?;
    if lane_repository.common_dir() != repository.common_dir()
        || lane_repository.worktree_root() != std::path::Path::new(worktree)
    {
        return Err(refusal(
            "git::worktree-moved",
            Some(worktree.to_owned()),
            "the allocating lane no longer names one worktree in the resident repository",
        ));
    }
    Ok(lane_repository)
}

fn claim_deadlines() -> Result<(Instant, Instant, Instant), Outcome<CommandAnswer>> {
    let at = current_instant()?;
    let expiry = instant_after(at, AUTOMATIC_PROVISIONAL_SECONDS)?;
    let maximum = instant_after(at, AUTOMATIC_MAXIMUM_SECONDS)?;
    Ok((at, expiry, maximum))
}

pub(crate) fn is_automatic_claim(claim: &ClaimHead, facts: &bus::TaskFacts) -> bool {
    facts
        .at(claim.claimed)
        .is_some_and(|envelope| spice::claim_lease::is_automatic_claim_fact(claim, envelope))
}

fn claim_source_boundary(
    bus: &Bus,
    lane: &wire::CanonicalUuid,
) -> Result<body::SourceBoundary, Outcome<CommandAnswer>> {
    bus.source_boundary(lane)
        .map_err(|error| store_fault("claim::boundary-unreadable", &error.to_string()))
}

fn automatic_answer(
    selected: &Entry,
    entries: &[Entry],
    task: TaskHandle,
    actor: Actor,
    transplant: Option<&wire::intent::ClaimTransplant>,
) -> wire::intent::ClaimAnswer {
    wire::intent::ClaimAnswer {
        task,
        statement: None,
        holder: Some(actor),
        expiry: None,
        warnings: board::claim_overlaps(selected, entries)
            .into_iter()
            .map(|overlap| wire::intent::ClaimWarning {
                task: overlap.task,
                lane: overlap.lane,
                surface: overlap.surface,
            })
            .collect::<Vec<_>>()
            .into_boxed_slice(),
        creation_context: None,
        origin_context: None,
        room_context: None,
        next: None,
        phase: None,
        nothing_stolen: false,
        transplant: transplant.cloned(),
    }
}

/// Release exactly the automatic claim whose launch did not become durable.
///
/// The claim may already have replayed parked work into the lane, so the
/// release parks the lane exactly as an ordinary task free does and the next
/// claim replays it again.
pub(crate) fn release_automatic(
    bus: &Bus,
    repository: Host<'_>,
    automatic: &AutomaticClaim,
) -> Result<(), Outcome<CommandAnswer>> {
    let ActiveClaim {
        observed,
        actor,
        handle,
        worktree,
        opening,
    } = active_claim(bus, automatic)?;
    let at = current_instant()?;
    let origin = task_origin(&automatic.task)?;
    let lane_repository = lane_repository(repository, &worktree)?;
    let parked =
        spice::task_git::prepare_free(&lane_repository, repository.ambient, opening.as_ref())
            .map_err(task_git_outcome)?;
    let mut write = JournalWrite::new(actor.clone(), origin, at, Producer::CommandDoor);
    guard(
        &mut write,
        vec![Witness::Claim {
            task: automatic.task.clone(),
            head: Some(observed.position()),
        }],
    );
    let freed = match write.fact(
        body::ClaimFreed {
            task: automatic.task.clone(),
            actor: actor.clone(),
            parked: parked.as_ref().map(|release| release.parked().clone()),
        },
        FactCause::Root,
    ) {
        Ok(freed) => freed,
        Err(error) => {
            return Err(rollback_release(
                parked,
                store_fault("lifecycle::claim-release-build", &error.to_string()),
            ));
        }
    };
    let mut next = observed.body().clone();
    next.disposition = ClaimDisposition::Freed;
    let task = automatic.task.clone();
    write.claim_head(task.clone(), freed, move |fact| ClaimHead { fact, ..next });
    let answer = wire::intent::ClaimAnswer {
        task: handle,
        statement: None,
        holder: None,
        expiry: None,
        warnings: Box::default(),
        creation_context: None,
        origin_context: None,
        room_context: None,
        next: None,
        phase: None,
        nothing_stolen: false,
        transplant: None,
    };
    let body = CommandAnswer::Claim(Box::new(answer));
    let outcome = match parked {
        Some(parked) => committed_with_task_git(bus, write, body, PreparedTaskGit::Release(parked)),
        None => committed(bus, write, body),
    };
    match outcome {
        Outcome::Reply(_) => Ok(()),
        Outcome::Refusal(refusal) => Err(Outcome::Refusal(refusal)),
        Outcome::Fault(fault) => Err(Outcome::Fault(fault)),
    }
}

/// The still-active automatic claim and the lane boundary it opened.
struct ActiveClaim {
    observed: journal::JournalHead<ClaimHead>,
    actor: Actor,
    handle: TaskHandle,
    worktree: String,
    opening: Option<GitBoundary>,
}

fn active_claim(
    bus: &Bus,
    automatic: &AutomaticClaim,
) -> Result<ActiveClaim, Outcome<CommandAnswer>> {
    let snapshot = bus
        .task_snapshot()
        .map_err(|error| store_fault("lifecycle::claim-release-read", &error.to_string()))?;
    let heads = snapshot
        .tasks()
        .iter()
        .find(|heads| heads.statement().body().born == automatic.task)
        .ok_or_else(|| {
            store_fault(
                "lifecycle::claim-release-task-missing",
                "the automatic claim task disappeared before launch compensation",
            )
        })?;
    let observed = heads.claim().cloned().ok_or_else(|| {
        store_fault(
            "lifecycle::claim-release-head-missing",
            "the automatic claim head disappeared before launch compensation",
        )
    })?;
    let claim = observed.body();
    if claim.disposition != ClaimDisposition::Active
        || claim.lane != automatic.lane
        || claim.occupant != automatic.occupant
    {
        return Err(refusal(
            "lifecycle::claim-release-moved",
            Some(format!(
                "{} {} {} {:?}",
                automatic.task, claim.lane, claim.occupant, claim.disposition
            )),
            "leave successor authority intact and reconcile current heads",
        ));
    }
    let actor = Actor::Lane(automatic.lane);
    if claim.actor != actor {
        return Err(store_fault(
            "lifecycle::claim-release-owner-moved",
            "the exact automatic claim is no longer owned by its allocating lane",
        ));
    }
    let handle = TaskHandle::for_project(
        &heads.statement().body().project,
        &heads.statement().body().born,
    );
    let worktree = snapshot
        .residency()
        .and_then(|residency| {
            residency
                .body()
                .lanes
                .iter()
                .find(|row| row.lane == automatic.lane)
        })
        .map(|row| row.worktree.clone())
        .ok_or_else(|| {
            store_fault(
                "lifecycle::claim-release-lane-missing",
                "the automatic claim lane disappeared before launch compensation",
            )
        })?;
    let opening = snapshot
        .facts()
        .at(claim.claimed)
        .and_then(|envelope| envelope.fact.read::<body::ClaimWon>())
        .and_then(|won| won.git);
    Ok(ActiveClaim {
        observed,
        actor,
        handle,
        worktree,
        opening,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_claim_starts_short_with_a_one_hour_reclaim_horizon() {
        let (at, expiry, maximum) = claim_deadlines().expect("automatic deadlines build");

        assert_eq!(
            expiry,
            instant_after(at, AUTOMATIC_PROVISIONAL_SECONDS).expect("provisional deadline builds")
        );
        assert_eq!(
            maximum,
            instant_after(at, AUTOMATIC_MAXIMUM_SECONDS).expect("maximum deadline builds")
        );
        assert!(expiry < maximum);
    }
}
