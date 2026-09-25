//! Bounded task identity selection for board reads.

use super::*;

pub(super) struct TaskSelection {
    pub(super) tasks: Option<std::collections::BTreeSet<wire::TaskStamp>>,
    /// Rows read only as allocation evidence and dropped from the answer.
    pub(super) evidence: std::collections::BTreeSet<wire::TaskStamp>,
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
            evidence: Default::default(),
            history: None,
        }),
        BoardRead::Open => {
            let open = observed
                .tasks()
                .iter()
                .filter(|heads| !terminal_task(heads));
            let mut tasks: std::collections::BTreeSet<_> = open
                .clone()
                .map(|heads| heads.statement().body().born.clone())
                .collect();
            // Readiness reads each open row's direct prerequisites: a settled
            // one is terminal, so it is carried only as allocation evidence.
            let evidence = open
                .flat_map(|heads| &heads.statement().body().dependencies)
                .filter(|dependency| !tasks.contains(*dependency))
                .cloned()
                .collect::<std::collections::BTreeSet<_>>();
            tasks.extend(evidence.iter().cloned());
            Ok(TaskSelection {
                tasks: Some(tasks),
                evidence,
                history: None,
            })
        }
        BoardRead::History {
            before,
            query,
            completed_before,
            limit,
        } => history_selection(observed, before, query, completed_before.as_ref(), *limit),
        BoardRead::Trust => Ok(TaskSelection {
            tasks: Some(Default::default()),
            evidence: Default::default(),
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
            evidence: Default::default(),
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
    query: &Option<String>,
    completed_before: Option<&wire::Instant>,
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
    let query = normalized_history_query(query)?;
    let mut history = observed
        .tasks()
        .iter()
        .filter(|heads| terminal_task(heads))
        .filter(|heads| {
            completed_before.is_none_or(|cutoff| {
                heads
                    .workflow()
                    .body()
                    .completion
                    .as_ref()
                    .is_some_and(|completed| completed <= cutoff)
            })
        })
        .filter(|heads| {
            query
                .as_deref()
                .is_none_or(|query| history_query_matches(heads, query))
        })
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
        evidence: Default::default(),
        history: Some(TaskHistoryPage {
            before: before.clone(),
            limit,
            total: u64::try_from(history.len()).expect("task count fits the wire total"),
            next,
        }),
    })
}

fn normalized_history_query(
    query: &Option<String>,
) -> Result<Option<String>, Outcome<CommandAnswer>> {
    let Some(query) = query.as_deref() else {
        return Ok(None);
    };
    if query.len() > TASK_HISTORY_QUERY_BYTES_MAX {
        return Err(refusal(
            "board::history-query-invalid",
            Some(format!(
                "bytes={}; maximum={TASK_HISTORY_QUERY_BYTES_MAX}",
                query.len()
            )),
            "shorten the terminal history query",
        ));
    }
    let query = query.trim();
    if query.is_empty() {
        return Ok(None);
    }
    if query.chars().any(char::is_control) {
        return Err(refusal(
            "board::history-query-invalid",
            Some("query contains a control character".to_owned()),
            "search terminal history with printable text",
        ));
    }
    Ok(Some(query.to_lowercase()))
}

fn history_query_matches(heads: &journal::TaskHeads, query: &str) -> bool {
    let statement = heads.statement().body();
    contains_case_insensitive(&statement.title, query)
        || contains_case_insensitive(&statement.project, query)
        || statement
            .description
            .as_deref()
            .is_some_and(|description| contains_case_insensitive(description, query))
        || contains_case_insensitive(
            wire::TaskHandle::for_project(&statement.project, &statement.born).as_str(),
            query,
        )
}

fn contains_case_insensitive(text: &str, lowercase_query: &str) -> bool {
    if text.is_ascii() && lowercase_query.is_ascii() {
        return text
            .as_bytes()
            .windows(lowercase_query.len())
            .any(|window| window.eq_ignore_ascii_case(lowercase_query.as_bytes()));
    }
    text.to_lowercase().contains(lowercase_query)
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
