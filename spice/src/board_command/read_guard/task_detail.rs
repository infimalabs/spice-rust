use super::*;

/// Read one task and its notes from one immutable journal barrier.
pub(in crate::board_command) fn read_task_detail(
    bus: &Bus,
    detail: &wire::intent::TaskDetail,
    principal: wire::Actor,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let observed = match bus.task_snapshot() {
        Ok(observed) => observed,
        Err(error) => return store_fault("board::heads-unreadable", &error.to_string()),
    };
    let selected = std::collections::BTreeSet::from([detail.task.clone()]);
    let snapshot = match validated_snapshot_from_with_occupant(
        &observed,
        principal,
        projects,
        allocation,
        None,
        Some(&selected),
        true,
    ) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(task) = snapshot.answer.tasks.into_iter().next() else {
        return refusal(
            "task::detail-unknown",
            Some(detail.task.to_string()),
            "read the board and request one current durable task identity",
        );
    };
    let notes = snapshot
        .facts
        .iter()
        .filter_map(|envelope| {
            let noted = envelope.fact.read::<body::TaskNoted>()?;
            (noted.task == detail.task).then(|| wire::intent::TaskDetailNote {
                position: envelope.seq,
                at: envelope.at,
                author: envelope.actor.clone(),
                text: noted.note,
            })
        })
        .collect();
    snapshot_reply(
        snapshot.cursor,
        CommandAnswer::TaskDetail(Box::new(wire::intent::TaskDetailAnswer { task, notes })),
    )
}
