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
    pub(crate) task: TaskHandle,
    pub(crate) lane: wire::CanonicalUuid,
    pub(crate) occupant: wire::CanonicalUuid,
}

/// Select and claim once for a vacant resident-owned lane.
///
/// This is the automatic counterpart of `claim_win`: the same validated
/// snapshot, selector, allocator guard, and mint writer decide the result.
#[allow(
    clippy::too_many_arguments,
    reason = "one automatic claim carries the complete allocator decision boundary"
)]
pub(crate) fn claim_automatic(
    bus: &Bus,
    observed: &bus::TaskSnapshot,
    repository: &Repository,
    epoch: wire::CanonicalUuid,
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Result<AutomaticClaim, Outcome<CommandAnswer>> {
    let prepared = prepare_claim(
        bus, observed, repository, epoch, lane, occupant, projects, allocation,
    )?;
    let answer = automatic_answer(
        &prepared.selected,
        &prepared.entries,
        prepared.task.clone(),
        prepared.actor,
    );
    match committed(bus, prepared.write, CommandAnswer::Claim(answer)) {
        Outcome::Reply(_) => Ok(AutomaticClaim {
            task: prepared.task,
            lane,
            occupant,
        }),
        Outcome::Refusal(refusal) => Err(Outcome::Refusal(refusal)),
        Outcome::Fault(fault) => Err(Outcome::Fault(fault)),
    }
}

struct PreparedClaim {
    write: JournalWrite,
    selected: Entry,
    entries: Vec<Entry>,
    task: TaskHandle,
    actor: Actor,
}

#[allow(
    clippy::too_many_arguments,
    reason = "the prepared write captures one automatic allocator boundary"
)]
fn prepare_claim(
    bus: &Bus,
    observed: &bus::TaskSnapshot,
    repository: &Repository,
    epoch: wire::CanonicalUuid,
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Result<PreparedClaim, Outcome<CommandAnswer>> {
    let actor = Actor::Lane(lane);
    let snapshot = validated_snapshot_from(observed, actor.clone(), projects, allocation)?;
    let row = available_lane(&snapshot, lane)?;
    let subscription = available_subscription(&snapshot, row)?;
    let selected = select_task(&snapshot, &actor, subscription)?;
    let task = selected.statement().handle.clone();
    let (at, expiry, maximum) = claim_deadlines()?;
    let git = prepare_git(repository, &row.worktree)?;
    let source = claim_source_boundary(bus, &lane)?;
    let origin = task_origin(&task)?;
    let mut write = JournalWrite::new(actor.clone(), origin, at, Producer::CommandDoor);
    guard_allocation(
        &mut write,
        Vec::new(),
        AllocationObservation {
            heads: allocation_heads(&snapshot.answer),
            winner: task.clone(),
            actor: actor.clone(),
            subscription: subscription.clone(),
            residency_position: snapshot.answer.residency.position,
            projects: projects.clone(),
            band: snapshot.answer.allocation_band,
            recent_cell: snapshot.answer.recent_cell.clone(),
        },
    );
    mint_claim(
        &mut write, &selected, &task, &actor, lane, occupant, epoch, at, expiry, maximum, git,
        source,
    )?;
    Ok(PreparedClaim {
        write,
        selected,
        entries: snapshot.entries,
        task,
        actor,
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
    if row.life == "drain" {
        return Err(refusal(
            "lifecycle::available-work-life-ended",
            Some(row.life.clone()),
            "allocate through a lane whose lifecycle still admits re-seating",
        ));
    }
    Ok(row)
}

fn available_subscription<'a>(
    snapshot: &'a ValidatedSnapshot,
    row: &wire::LaneResidency,
) -> Result<&'a wire::LineageSubscription, Outcome<CommandAnswer>> {
    let mut subscriptions = snapshot
        .answer
        .residency
        .body
        .subscriptions
        .iter()
        .filter(|subscription| subscription.lineage_id == row.lineage_id);
    let subscription = subscriptions.next().ok_or_else(|| {
        store_fault(
            "lifecycle::available-work-subscription-missing",
            "the vacant lane lineage has no durable task subscription",
        )
    })?;
    if subscriptions.next().is_some() {
        return Err(store_fault(
            "lifecycle::available-work-subscription-ambiguous",
            "the vacant lane lineage has more than one durable task subscription",
        ));
    }
    if subscription.policy != wire::intent::LineageLife::Drain {
        return Err(refusal(
            "lifecycle::available-work-scope",
            Some(subscription.policy.as_str().to_owned()),
            "use the all-assignable drain subscription for capacity-following wake",
        ));
    }
    Ok(subscription)
}

fn select_task(
    snapshot: &ValidatedSnapshot,
    actor: &Actor,
    subscription: &wire::LineageSubscription,
) -> Result<Entry, Outcome<CommandAnswer>> {
    board::select(
        &snapshot.entries,
        actor,
        subscription,
        snapshot.answer.allocation_band,
        snapshot.answer.recent_cell.as_ref(),
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

fn prepare_git(
    repository: &Repository,
    worktree: &str,
) -> Result<GitBoundary, Outcome<CommandAnswer>> {
    let lane_repository = Repository::discover(worktree)
        .map_err(|error| environment_fault("git::worktree-unresolved", &error.to_string()))?;
    if lane_repository.common_dir() != repository.common_dir()
        || lane_repository.worktree_root() != std::path::Path::new(worktree)
    {
        return Err(environment_fault(
            "git::worktree-moved",
            "the allocating lane no longer names one worktree in the resident repository",
        ));
    }
    spice::task_git::prepare_claim(&lane_repository).map_err(task_git_outcome)
}

fn claim_deadlines() -> Result<(Instant, Instant, Instant), Outcome<CommandAnswer>> {
    let at = current_instant()?;
    let expiry = instant_after(at, 15 * 60)?;
    let maximum = instant_after(at, 60 * 60)?;
    Ok((at, expiry, maximum))
}

fn claim_source_boundary(
    bus: &Bus,
    lane: &wire::CanonicalUuid,
) -> Result<body::SourceBoundary, Outcome<CommandAnswer>> {
    bus.source_boundary(lane)
        .map_err(|error| store_fault("claim::boundary-unreadable", &error.to_string()))
}

#[allow(
    clippy::too_many_arguments,
    reason = "one claim fact and its two heads share one boundary"
)]
fn mint_claim(
    write: &mut JournalWrite,
    selected: &Entry,
    task: &TaskHandle,
    actor: &Actor,
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
    epoch: wire::CanonicalUuid,
    at: Instant,
    expiry: Instant,
    maximum: Instant,
    git: GitBoundary,
    source: body::SourceBoundary,
) -> Result<(), Outcome<CommandAnswer>> {
    board::allocate::mint(
        write,
        board::allocate::Mint {
            task,
            actor,
            workflow: selected.workflow(),
            project: selected.project().as_str(),
            lane,
            occupant,
            epoch,
            at,
            expiry,
            maximum,
            git: Some(git),
            source,
        },
    )
    .map(|_| ())
    .map_err(|error| match error {
        board::allocate::MintError::Assignment(error) => {
            store_fault("review::assignment-build", &error.to_string())
        }
        board::allocate::MintError::Claim(error) => {
            store_fault("claim::win-build", &error.to_string())
        }
    })
}

fn automatic_answer(
    selected: &Entry,
    entries: &[Entry],
    task: TaskHandle,
    actor: Actor,
) -> wire::intent::ClaimAnswer {
    wire::intent::ClaimAnswer {
        task,
        holder: actor,
        warnings: board::claim_overlaps(selected, entries)
            .into_iter()
            .map(|overlap| wire::intent::ClaimWarning {
                task: overlap.task,
                lane: overlap.lane,
                surface: overlap.surface,
            })
            .collect(),
        next: None,
    }
}

/// Release exactly the automatic claim whose launch did not become durable.
pub(crate) fn release_automatic(
    bus: &Bus,
    automatic: &AutomaticClaim,
) -> Result<(), Outcome<CommandAnswer>> {
    let (observed, actor) = active_claim(bus, automatic)?;
    let at = current_instant()?;
    let origin = task_origin(&automatic.task)?;
    let mut write = JournalWrite::new(actor.clone(), origin, at, Producer::CommandDoor);
    guard(
        &mut write,
        vec![Witness::Claim {
            task: automatic.task.clone(),
            head: Some(observed.position()),
        }],
    );
    let freed = write
        .fact(
            body::ClaimFreed {
                task: automatic.task.clone(),
                actor: actor.clone(),
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("lifecycle::claim-release-build", &error.to_string()))?;
    let mut next = observed.body().clone();
    next.disposition = ClaimDisposition::Freed;
    let task = automatic.task.clone();
    write.claim_head(task.clone(), freed, move |fact| ClaimHead { fact, ..next });
    let answer = wire::intent::ClaimAnswer {
        task,
        holder: actor,
        warnings: Vec::new(),
        next: None,
    };
    match committed(bus, write, CommandAnswer::Claim(answer)) {
        Outcome::Reply(_) => Ok(()),
        Outcome::Refusal(refusal) => Err(Outcome::Refusal(refusal)),
        Outcome::Fault(fault) => Err(Outcome::Fault(fault)),
    }
}

fn active_claim(
    bus: &Bus,
    automatic: &AutomaticClaim,
) -> Result<(journal::JournalHead<ClaimHead>, Actor), Outcome<CommandAnswer>> {
    let snapshot = bus
        .task_snapshot()
        .map_err(|error| store_fault("lifecycle::claim-release-read", &error.to_string()))?;
    let heads = snapshot
        .tasks()
        .iter()
        .find(|heads| heads.statement().body().handle == automatic.task)
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
    Ok((observed, actor))
}
