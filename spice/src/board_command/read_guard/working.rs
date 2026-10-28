use super::*;

/// Project the live board and only the settled rows reachable from it or the
/// command's named roots.
///
/// Mutations still decide against the complete working graph, but their cost
/// does not grow with unrelated terminal history.
pub(in crate::board_command) fn validated_working_snapshot(
    bus: &Bus,
    principal: wire::Actor,
    roots: &[wire::TaskHandle],
) -> Result<ValidatedSnapshot, Outcome<CommandAnswer>> {
    let roots = roots
        .iter()
        .map(|handle| {
            handle
                .born()
                .expect("an admitted task handle carries a canonical task stamp")
        })
        .collect::<Vec<_>>();
    validated_working_snapshot_for_tasks(bus, principal, &roots)
}

pub(in crate::board_command) fn command_task_snapshot(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    task: &wire::TaskHandle,
) -> Result<ValidatedSnapshot, Outcome<CommandAnswer>> {
    validated_working_snapshot(bus, command.actor().clone(), std::slice::from_ref(task))
}

pub(in crate::board_command) fn validated_working_snapshot_for_tasks(
    bus: &Bus,
    principal: wire::Actor,
    roots: &[wire::TaskStamp],
) -> Result<ValidatedSnapshot, Outcome<CommandAnswer>> {
    let observed = bus
        .task_snapshot()
        .map_err(|error| store_fault("board::heads-unreadable", &error.to_string()))?;
    let selected = working_task_selection(&observed, roots);
    validated_snapshot_from_with_occupant(&observed, principal, None, Some(&selected), true)
}

pub(in crate::board_command) fn task_ancestry_selection(
    observed: &bus::TaskSnapshot,
    roots: &[wire::TaskStamp],
) -> std::collections::BTreeSet<wire::TaskStamp> {
    expand_task_selection(observed, roots.to_vec())
}

fn working_task_selection(
    observed: &bus::TaskSnapshot,
    roots: &[wire::TaskStamp],
) -> std::collections::BTreeSet<wire::TaskStamp> {
    // A root without its own acceptance may borrow it from work depending on
    // it. The live graph holds the pending lenders; a positively concluded one
    // is settled, so only this scan projects it.
    let borrowers = roots
        .iter()
        .filter(|root| {
            observed
                .task_heads(root)
                .is_some_and(|heads| !substantive_acceptance(&heads.statement().body().acceptance))
        })
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let candidates = if borrowers.is_empty() {
        &[][..]
    } else {
        observed.tasks()
    };
    let lenders = candidates.iter().filter(|heads| {
        let statement = heads.statement().body();
        lends_across(
            &statement.acceptance,
            heads.workflow().body(),
            &statement.dependencies,
            &borrowers,
        )
    });
    let pending = observed
        .allocation_frontier()
        .chain(lenders)
        .map(|heads| heads.statement().body().born.clone())
        .chain(roots.iter().cloned())
        .collect::<Vec<_>>();
    expand_task_selection(observed, pending)
}

fn expand_task_selection(
    observed: &bus::TaskSnapshot,
    mut pending: Vec<wire::TaskStamp>,
) -> std::collections::BTreeSet<wire::TaskStamp> {
    let mut selected = std::collections::BTreeSet::new();
    while let Some(task) = pending.pop() {
        if !selected.insert(task.clone()) {
            continue;
        }
        if let Some(heads) = observed.task_heads(&task) {
            pending.extend(heads.statement().body().dependencies.iter().cloned());
            if let Some(preparation) = &heads.statement().body().preparation {
                pending.extend([
                    preparation.parent.clone(),
                    preparation.controller.clone(),
                    preparation.construction.clone(),
                ]);
            }
        }
    }
    selected
}

pub(in crate::board_command) fn validated_snapshot_for_automatic(
    observed: &bus::TaskSnapshot,
    principal: wire::Actor,
    occupant: wire::CanonicalUuid,
) -> Result<ValidatedSnapshot, Outcome<CommandAnswer>> {
    let selected = working_task_selection(observed, &[]);
    validated_snapshot_from_with_occupant(
        observed,
        principal,
        Some(occupant),
        Some(&selected),
        true,
    )
}
