use super::*;

/// Select current work or retain the exact deadline of a temporarily blocked
/// phase handoff so the resident reconciles when its prior owner becomes valid.
pub(super) fn selected_ready(
    snapshot: &bus::TaskSnapshot,
    entries: &[Entry],
    ready_at: &BTreeMap<TaskStamp, Instant>,
    actor: &Actor,
    subscription: &wire::TeamSubscription,
    allocation: &board::NextConfig,
    now: Instant,
) -> CommandResult<Option<SelectedReady>> {
    let recent_cell =
        board::recent_claim_cell(entries, snapshot.facts().latest_claim(actor), actor);
    let appliance = spice::version::running().map_err(Outcome::Fault)?;
    let environment = snapshot
        .environment()
        .with_appliance(appliance.product.clone());
    let satisfied = board::resolved_tasks(entries);
    let selected = board::select_with_satisfied_at(
        entries,
        actor,
        subscription,
        allocation.band(),
        recent_cell.as_ref(),
        &environment,
        &satisfied,
        now,
    );
    let Some(selected) = selected else {
        return Ok(board::blocked_handoff_until(
            entries,
            &satisfied,
            actor,
            subscription,
            &environment,
            now,
        )
        .map(|at| SelectedReady {
            at,
            eligible: 0,
            handoff: false,
            handoff_wait: true,
            last_resort: false,
        }));
    };
    let at = ready_instant(selected.entry(), ready_at)?;
    let handoff = selected.entry().phase_handoff().is_some();
    let eligible = entries
        .iter()
        .filter(|entry| {
            board::selectable_entry_at(&satisfied, entry, actor, subscription, &environment, now)
        })
        .count();
    Ok(Some(SelectedReady {
        at,
        eligible,
        handoff,
        handoff_wait: false,
        last_resort: selected.last_resort(),
    }))
}

pub(super) fn ready_instant(
    entry: &Entry,
    ready_at: &BTreeMap<TaskStamp, Instant>,
) -> CommandResult<Instant> {
    ready_at.get(entry.born()).copied().ok_or_else(|| {
        store_fault(
            "lifecycle::task-ready-fact-missing",
            &format!(
                "task {} has no durable READY transition instant",
                entry.handle()
            ),
        )
    })
}
