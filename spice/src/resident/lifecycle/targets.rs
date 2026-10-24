use super::*;

pub(super) fn project_targets<'a>(
    snapshot: &bus::TaskSnapshot,
    projection: &'a Projection,
    now: Instant,
    holds: &Holds,
    allocation: &board::NextConfig,
) -> CommandResult<Vec<Target<'a>>> {
    let Some(residency) = snapshot.residency() else {
        return Ok(Vec::new());
    };
    if !residency
        .body()
        .lanes
        .iter()
        .any(|row| row.occupant.is_none() && row.disposition != "occupied" && holds.seats(row.lane))
    {
        return Ok(Vec::new());
    }
    let selection = projection.selection();
    let satisfied = selection.resolved();
    let appliance = spice::version::running().map_err(Outcome::Fault)?;
    let environment = snapshot
        .environment()
        .with_appliance(appliance.product.clone());
    let projected_residency = residency.body();
    let view = BoardView {
        entries: projection.entries(),
        selection,
        ready_at: projection.ready_at(),
        allocation,
        environment: &environment,
        now,
    };
    let mut targets = Vec::new();
    for row in &projected_residency.lanes {
        if row.occupant.is_some() || row.disposition == "occupied" || !holds.seats(row.lane) {
            continue;
        }
        let pending_inbox = projection.has_operator_steering(row.lane);
        let (held_claim, held_parked) = held_claim(
            view.entries,
            satisfied,
            view.ready_at,
            row,
            &environment,
            now,
        )?;
        let subscription = subscription(projected_residency, row)?;
        let first_drain_member = !projected_residency.lanes.iter().any(|member| {
            member.team == row.team && member.occupant.is_some() && member.disposition == "occupied"
        });
        let work = if held_parked {
            Work::default()
        } else {
            work_availability(&view, row, subscription.as_ref(), first_drain_member)?
        };
        targets.push(Target {
            lane: row.lane,
            last_claim: snapshot
                .facts()
                .latest_claim(&Actor::Lane(row.lane))
                .map(|(claim, _)| claim),
            pending_inbox,
            held_claim,
            drain_queue: work.drain_queue,
            available: work.available,
            last_resort: work.last_resort,
            task: work.task,
            matched: None,
        });
    }
    Ok(targets)
}

/// The held-claim trigger, and whether the held work is parked on authority no
/// lane holds: an unsatisfied trust grant, or a Design that can conclude only
/// through the operator's selection. Parked held work wakes nothing; the lane
/// also takes no other work while it holds that claim, so pending operator
/// steering is its only wake.
fn held_claim(
    entries: &[Entry],
    satisfied: &board::ResolvedTasks,
    ready_at: &BTreeMap<TaskStamp, Instant>,
    row: &wire::LaneResidency,
    environment: &wire::Environment,
    now: Instant,
) -> CommandResult<(Available, bool)> {
    let mut held = entries.iter().filter(|entry| {
        entry.claim().is_some_and(|claim| {
            claim.disposition == ClaimDisposition::Active && claim.lane == row.lane
        })
    });
    let Some(entry) = held.next() else {
        return Ok((Available::None, false));
    };
    if held.next().is_some() {
        return Err(store_fault(
            "lifecycle::held-claim-ambiguous",
            "one vacant lane carries more than one active claim",
        ));
    }
    let parked = entry
        .statement()
        .requirements
        .has_unsatisfied_trust_grant(environment)
        || board::design_held(entry, satisfied);
    let live = entry
        .lease()
        .ok_or_else(|| {
            store_fault(
                "lifecycle::held-claim-lease-missing",
                "an active held claim has no durable lease head",
            )
        })?
        .expiry
        > now;
    let available = if live && !parked {
        Available::At(after(ready_instant(entry, ready_at)?, SETTLE_SECONDS)?)
    } else {
        Available::None
    };
    Ok((available, parked))
}

/// Leave a vacant lane whose live held claim waits on its own unfinished
/// deadline-bounded jobs to the job keeper: the keeper's wake on completion,
/// or the jobs' admitted expiry (`delivered`) when no keeper speaks, causes
/// the next seat. Until then the held-claim trigger waits on that expiry,
/// which also keeps drain-queue and available-work from seating the lane;
/// pending operator steering still outranks it in `resolve`. Services and
/// ended jobs bound nothing, so `delivered` is absent for them.
pub(super) fn await_regular_jobs(
    targets: &mut [Target],
    now: Instant,
    delivered: impl Fn(CanonicalUuid) -> Option<Instant>,
) {
    for target in targets {
        if target.held_claim == Available::None {
            continue;
        }
        let Some(delivered) = delivered(target.lane).filter(|delivered| *delivered > now) else {
            continue;
        };
        target.held_claim = match target.held_claim {
            Available::At(settled) if settled > delivered => Available::At(settled),
            _ => Available::At(delivered),
        };
        target.drain_queue = Available::None;
        target.available = Available::None;
    }
}

/// The task-allocation lifetime of one vacant lane's team.
///
/// The team policy alone decides whether the lane follows capacity; the row's
/// own life is the seed its initial team took, not a lifecycle of the lane.
fn subscription(
    residency: &wire::ResidencyHead,
    row: &wire::LaneResidency,
) -> CommandResult<Option<wire::TeamSubscription>> {
    let mut subscriptions = residency
        .subscriptions
        .iter()
        .filter(|subscription| subscription.team == row.team);
    let subscription = subscriptions.next();
    if subscriptions.next().is_some() {
        return Err(store_fault(
            "lifecycle::subscription-ambiguous",
            "one lifecycle target has more than one durable task-allocation lifetime",
        ));
    }
    Ok(subscription.cloned())
}

/// What one lane's work triggers would claim: the drain-queue and
/// available-work availability, whether that work is the lane's own last
/// resort, and the task the allocator selected.
struct Work<'a> {
    drain_queue: Available,
    available: Available,
    last_resort: bool,
    task: Option<&'a Entry>,
}

impl Default for Work<'_> {
    fn default() -> Self {
        Self {
            drain_queue: Available::None,
            available: Available::None,
            last_resort: false,
            task: None,
        }
    }
}

fn work_availability<'e>(
    view: &BoardView<'_, 'e>,
    row: &wire::LaneResidency,
    subscription: Option<&wire::TeamSubscription>,
    first_drain_member: bool,
) -> CommandResult<Work<'e>> {
    let Some(subscription) = subscription.filter(|subscription| {
        matches!(
            subscription.policy,
            wire::intent::TeamLife::Drain | wire::intent::TeamLife::Drive
        )
    }) else {
        return Ok(Work::default());
    };
    let (selected, task) = selected_ready(view, &Actor::Lane(row.lane), subscription)?;
    let last_resort = selected.is_some_and(|selected| selected.last_resort);
    let (drain_queue, available) = if subscription.policy == wire::intent::TeamLife::Drain {
        (
            drain_from_selected(selected, first_drain_member)?,
            Available::None,
        )
    } else {
        (Available::None, available_from_selected(selected)?)
    };
    Ok(Work {
        drain_queue,
        available,
        last_resort,
        task,
    })
}
