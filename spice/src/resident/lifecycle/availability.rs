use super::*;

/// Select current work, with the task selected, or retain the exact deadline
/// of a temporarily blocked phase handoff so the resident reconciles when its
/// prior owner becomes valid.
#[expect(
    clippy::too_many_arguments,
    reason = "selection reads the snapshot, index, readiness, actor, team, allocation, environment, and clock"
)]
pub(super) fn selected_ready<'a>(
    snapshot: &bus::TaskSnapshot,
    entries: &'a [Entry],
    selection: &board::SelectionIndex,
    ready_at: &BTreeMap<TaskStamp, Instant>,
    actor: &Actor,
    subscription: &wire::TeamSubscription,
    allocation: &board::NextConfig,
    environment: &wire::Environment,
    now: Instant,
) -> CommandResult<(Option<SelectedReady>, Option<&'a Entry>)> {
    let recent_cell = snapshot.recent_claim_cell(actor);
    let satisfied = selection.resolved();
    let selected = board::select_with_index_at(
        entries,
        board::Claimant {
            actor,
            subscription,
            recent_cell: recent_cell.as_ref(),
            environment,
        },
        allocation.band(),
        selection,
        now,
    );
    let Some(selected) = selected else {
        let wait =
            board::blocked_handoff_until(entries, satisfied, actor, subscription, environment, now)
                .map(|at| SelectedReady {
                    at,
                    eligible: 0,
                    handoff: false,
                    handoff_wait: true,
                    last_resort: false,
                });
        return Ok((wait, None));
    };
    let at = ready_instant(selected.entry(), ready_at)?;
    let handoff = selected.entry().phase_handoff().is_some();
    let eligible = entries
        .iter()
        .filter(|entry| {
            board::selectable_entry_at(satisfied, entry, actor, subscription, environment, now)
        })
        .count();
    Ok((
        Some(SelectedReady {
            at,
            eligible,
            handoff,
            handoff_wait: false,
            last_resort: selected.last_resort(),
        }),
        Some(selected.entry()),
    ))
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
