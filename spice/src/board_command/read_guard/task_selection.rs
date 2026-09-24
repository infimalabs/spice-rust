//! Bounded task identity selection for board reads.

use super::*;

pub(super) struct TaskSelection {
    pub(super) tasks: Option<std::collections::BTreeSet<wire::TaskStamp>>,
    pub(super) history: Option<TaskHistoryPage>,
}

/// Select identities before cloning or projecting their task heads.
///
/// Complete command-line reads deliberately remain unbounded. Canvas open
/// reads and terminal pages select only the rows their response can carry.
pub(super) fn selected_tasks(
    observed: &bus::TaskSnapshot,
    read: &BoardRead,
    principal: &wire::Actor,
) -> Result<TaskSelection, Outcome<CommandAnswer>> {
    match read {
        BoardRead::All { .. } => Ok(TaskSelection {
            tasks: None,
            history: None,
        }),
        BoardRead::Open => Ok(TaskSelection {
            tasks: Some(
                observed
                    .tasks()
                    .iter()
                    .filter(|heads| !terminal_task(heads))
                    .map(|heads| heads.statement().body().born.clone())
                    .collect(),
            ),
            history: None,
        }),
        BoardRead::History { before, limit } => history_selection(observed, before, *limit),
        BoardRead::Trust => Ok(TaskSelection {
            tasks: Some(Default::default()),
            history: None,
        }),
        BoardRead::Tasks {
            tasks,
            dependencies,
            claims,
        } => Ok(TaskSelection {
            tasks: Some(context_tasks(
                observed,
                tasks,
                *dependencies,
                *claims,
                principal,
            )),
            history: None,
        }),
    }
}

fn context_tasks(
    observed: &bus::TaskSnapshot,
    tasks: &[wire::TaskStamp],
    dependencies: bool,
    claims: bool,
    principal: &wire::Actor,
) -> std::collections::BTreeSet<wire::TaskStamp> {
    let mut selected: std::collections::BTreeSet<_> = tasks.iter().cloned().collect();
    if claims {
        selected.extend(observed.tasks().iter().filter_map(|heads| {
            heads
                .claim()
                .filter(|claim| {
                    claim.body().actor == *principal
                        && claim.body().disposition == wire::ClaimDisposition::Active
                })
                .map(|_| heads.statement().body().born.clone())
        }));
    }
    if dependencies {
        let mut pending: Vec<_> = selected.iter().cloned().collect();
        while let Some(task) = pending.pop() {
            // The immutable bus projection is ordered by durable task stamp.
            if let Ok(index) = observed
                .tasks()
                .binary_search_by(|heads| heads.statement().body().born.cmp(&task))
            {
                for dependency in &observed.tasks()[index].statement().body().dependencies {
                    if selected.insert(dependency.clone()) {
                        pending.push(dependency.clone());
                    }
                }
            }
        }
    }
    selected
}

fn history_selection(
    observed: &bus::TaskSnapshot,
    before: &Option<TaskHistoryCursor>,
    limit: u16,
) -> Result<TaskSelection, Outcome<CommandAnswer>> {
    if limit == 0 || limit > TASK_HISTORY_PAGE_LIMIT_MAX {
        return Err(refusal(
            "board::history-limit-invalid",
            Some(format!(
                "limit={limit}; maximum={TASK_HISTORY_PAGE_LIMIT_MAX}"
            )),
            "request between one and the advertised maximum history rows",
        ));
    }
    let mut history = observed
        .tasks()
        .iter()
        .filter(|heads| terminal_task(heads))
        .map(|heads| TaskHistoryCursor {
            workflow: heads.workflow().position(),
            task: heads.statement().body().born.clone(),
        })
        .collect::<Vec<_>>();
    history.sort_by(history_cursor_order);
    let start = match before {
        None => 0,
        Some(cursor) => history
            .iter()
            .position(|candidate| candidate == cursor)
            .map(|index| index.saturating_add(1))
            .ok_or_else(|| {
                refusal(
                    "board::history-cursor-invalid",
                    Some(format!(
                        "task={}; workflow={}",
                        cursor.task, cursor.workflow
                    )),
                    "restart terminal history from its first page",
                )
            })?,
    };
    let end = start.saturating_add(usize::from(limit)).min(history.len());
    let selected = history[start..end]
        .iter()
        .map(|cursor| cursor.task.clone())
        .collect();
    let next = (end < history.len()).then(|| history[end.saturating_sub(1)].clone());
    Ok(TaskSelection {
        tasks: Some(selected),
        history: Some(TaskHistoryPage {
            before: before.clone(),
            limit,
            total: u64::try_from(history.len()).expect("task count fits the wire total"),
            next,
        }),
    })
}

pub(in crate::board_command) fn history_cursor_order(
    left: &TaskHistoryCursor,
    right: &TaskHistoryCursor,
) -> std::cmp::Ordering {
    right
        .workflow
        .cmp(&left.workflow)
        .then_with(|| left.task.cmp(&right.task))
}

fn terminal_task(heads: &journal::TaskHeads) -> bool {
    let workflow = heads.workflow().body();
    workflow.status != "pending" || workflow.completion.is_some()
}
