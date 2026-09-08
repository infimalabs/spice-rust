use super::*;

pub(super) fn project_targets(
    snapshot: &bus::TaskSnapshot,
    projection: &Projection,
    now: Instant,
    holds: &Holds,
    allocation: &board::NextConfig,
) -> CommandResult<Vec<Target>> {
    let Some(residency) = snapshot.residency() else {
        return Ok(Vec::new());
    };
    if !residency.body().lanes.iter().any(|row| {
        row.occupant.is_none()
            && row.disposition != "occupied"
            && row.life != "drain"
            && holds.seats(row.lane)
    }) {
        return Ok(Vec::new());
    }
    let entries = projection.entries();
    let environment = snapshot.environment();
    let projected_residency = projection
        .project_residency(residency.body())
        .map_err(|error| store_fault("lifecycle::subscription-history-invalid", &error))?;
    let ready_at = projection.ready_at();
    let mut rows = projected_residency.lanes.iter().collect::<Vec<_>>();
    rows.sort_by_key(|row| row.lane);
    let mut targets = Vec::new();
    for row in rows {
        if row.occupant.is_some()
            || row.disposition == "occupied"
            || row.life == "drain"
            || !holds.seats(row.lane)
        {
            continue;
        }
        let pending_inbox = projection.has_operator_steering(row.lane);
        let (held_claim, trust_blocked) = held_claim(entries, row, &environment, now)?;
        let subscription = subscription(&projected_residency, row)?;
        let first_drain_member = !projected_residency.lanes.iter().any(|member| {
            member.lineage_id == row.lineage_id
                && member.occupant.is_some()
                && member.disposition == "occupied"
        });
        let (drain_queue, available) = if trust_blocked {
            (Available::None, Available::None)
        } else {
            work_availability(
                snapshot,
                entries,
                ready_at,
                row,
                subscription,
                allocation,
                first_drain_member,
            )?
        };
        targets.push(Target {
            lane: row.lane,
            pending_inbox,
            held_claim,
            drain_queue,
            available,
        });
    }
    Ok(targets)
}

fn held_claim(
    entries: &[Entry],
    row: &wire::LaneResidency,
    environment: &wire::Environment,
    now: Instant,
) -> CommandResult<(bool, bool)> {
    let mut held = entries.iter().filter(|entry| {
        entry.claim().is_some_and(|claim| {
            claim.disposition == ClaimDisposition::Active && claim.lane == row.lane
        })
    });
    let Some(entry) = held.next() else {
        return Ok((false, false));
    };
    if held.next().is_some() {
        return Err(store_fault(
            "lifecycle::held-claim-ambiguous",
            "one vacant lane carries more than one active claim",
        ));
    }
    let trust_blocked = entry
        .statement()
        .requirements
        .has_unsatisfied_trust_grant(environment);
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
    Ok((live && !trust_blocked, trust_blocked))
}

fn subscription<'a>(
    residency: &'a wire::ResidencyHead,
    row: &wire::LaneResidency,
) -> CommandResult<Option<&'a wire::LineageSubscription>> {
    let mut subscriptions = residency
        .subscriptions
        .iter()
        .filter(|subscription| subscription.lineage_id == row.lineage_id);
    let subscription = subscriptions.next();
    if subscriptions.next().is_some() {
        return Err(store_fault(
            "lifecycle::subscription-ambiguous",
            "one lifecycle target has more than one durable task subscription",
        ));
    }
    Ok(subscription)
}

fn work_availability(
    snapshot: &bus::TaskSnapshot,
    entries: &[Entry],
    ready_at: &BTreeMap<TaskStamp, Instant>,
    row: &wire::LaneResidency,
    subscription: Option<&wire::LineageSubscription>,
    allocation: &board::NextConfig,
    first_drain_member: bool,
) -> CommandResult<(Available, Available)> {
    let actor = Actor::Lane(row.lane);
    match subscription {
        Some(subscription) if subscription.policy == wire::intent::LineageLife::Drain => Ok((
            drain_available(
                snapshot,
                entries,
                ready_at,
                &actor,
                subscription,
                allocation,
                first_drain_member,
            )?,
            Available::None,
        )),
        Some(subscription) if subscription.policy == wire::intent::LineageLife::Drive => Ok((
            Available::None,
            available_deadline(
                snapshot,
                entries,
                ready_at,
                &actor,
                subscription,
                allocation,
            )?,
        )),
        Some(_) | None => Ok((Available::None, Available::None)),
    }
}
