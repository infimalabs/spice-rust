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
/// Complete command-line reads deliberately remain unbounded. App-open
/// reads and terminal pages select only the rows their response can carry.
pub(super) fn selected_tasks(
    observed: &bus::TaskSnapshot,
    read: &BoardRead,
    principal: &wire::Actor,
) -> Result<TaskSelection, Outcome<CommandAnswer>> {
    match read {
        BoardRead::Next => Ok(TaskSelection {
            tasks: Some(Default::default()),
            evidence: Default::default(),
            history: None,
        }),
        BoardRead::All { .. } => Ok(TaskSelection {
            tasks: None,
            evidence: Default::default(),
            history: None,
        }),
        BoardRead::Open => {
            let mut tasks: std::collections::BTreeSet<_> = observed
                .allocation_frontier()
                .map(|heads| heads.statement().body().born.clone())
                .collect();
            // Saved group anchors stay present after settlement so the app can
            // distinguish a concluded room from an identity absent from task
            // authority. This compact current-head index does not admit any
            // unrelated terminal history.
            tasks.extend(observed.room_anchor_tasks().cloned());
            // Readiness reads each open row's direct prerequisites and, for a
            // controlled output, its preparation parent and controller: a
            // settled one is terminal, so it is carried only as allocation
            // evidence.
            let evidence = observed
                .allocation_frontier()
                .flat_map(|heads| {
                    let statement = heads.statement().body();
                    statement.dependencies.iter().chain(
                        statement
                            .preparation
                            .iter()
                            .flat_map(|preparation| [&preparation.parent, &preparation.controller]),
                    )
                })
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
            team,
            limit,
        } => history_selection(
            observed,
            before,
            query,
            completed_before.as_ref(),
            team.as_ref(),
            *limit,
        ),
        BoardRead::Trust => Ok(TaskSelection {
            tasks: Some(Default::default()),
            evidence: Default::default(),
            history: None,
        }),
        BoardRead::Tasks {
            tasks,
            dependencies,
            claims,
            history: _,
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

/// Keep only one upper-case task family's rows, refusing any other family name.
pub(super) fn retain_family(
    tasks: &mut Vec<wire::intent::BoardTask>,
    family: &str,
) -> Result<(), Outcome<CommandAnswer>> {
    validate_family(family)?;
    tasks.retain(|task| in_family(&task.statement.body.handle, family));
    Ok(())
}

/// Refuse anything but one upper-case task family name.
pub(in crate::board_command) fn validate_family(
    family: &str,
) -> Result<(), Outcome<CommandAnswer>> {
    if family.is_empty()
        || !family
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(refusal(
            "board::family-invalid",
            Some(family.to_owned()),
            "name one upper-case task family",
        ));
    }
    Ok(())
}

/// Whether a handle's stem names this family.
pub(in crate::board_command) fn in_family(handle: &wire::TaskHandle, family: &str) -> bool {
    handle
        .as_str()
        .split_once('-')
        .is_some_and(|(stem, _)| stem == family)
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
        let construction_owners = selected
            .iter()
            .filter_map(|task| {
                observed
                    .tasks()
                    .binary_search_by(|heads| heads.statement().body().born.cmp(task))
                    .ok()
                    .and_then(|index| {
                        observed.tasks()[index]
                            .statement()
                            .body()
                            .preparation
                            .as_ref()
                    })
                    .map(|preparation| preparation.construction.clone())
            })
            .collect::<Vec<_>>();
        selected.extend(construction_owners);
        let mut selected_plans = std::collections::BTreeSet::new();
        let mut open_designs = std::collections::BTreeSet::new();
        for task in &selected {
            let Ok(index) = observed
                .tasks()
                .binary_search_by(|heads| heads.statement().body().born.cmp(task))
            else {
                continue;
            };
            let heads = &observed.tasks()[index];
            let workflow = heads.workflow().body();
            if workflow.status.is_pending()
                && (workflow.phase.as_str() == "plan"
                    || planning::requires_construction(
                        heads.statement().body().intent,
                        &workflow.flow,
                    ))
            {
                selected_plans.insert(task.clone());
            }
            if wire::intent::is_open_design(heads.statement().body().intent, workflow) {
                open_designs.insert(task.clone());
            }
        }
        // A Plan's children are the outputs still gated by it; an open
        // Design also carries the pending construction roots it released and
        // now waits on as prerequisites.
        if !selected_plans.is_empty() || !open_designs.is_empty() {
            selected.extend(
                observed
                    .tasks()
                    .iter()
                    .filter(|child| {
                        let statement = child.statement().body();
                        let workflow = child.workflow().body();
                        !workflow.status.is_deleted()
                            && substantive_acceptance(&statement.acceptance)
                            && (statement
                                .dependencies
                                .iter()
                                .any(|dependency| selected_plans.contains(dependency))
                                || (workflow.status.is_pending()
                                    && wire::intent::released_construction_root(
                                        &statement.born,
                                        &statement.dependencies,
                                        statement.preparation.as_ref(),
                                    )
                                    .is_some_and(|parent| open_designs.contains(parent))))
                    })
                    .map(|child| child.statement().body().born.clone()),
            );
        }

        let mut pending: Vec<_> = selected.iter().cloned().collect();
        while let Some(task) = pending.pop() {
            // The immutable bus projection is ordered by durable task stamp.
            if let Ok(index) = observed
                .tasks()
                .binary_search_by(|heads| heads.statement().body().born.cmp(&task))
            {
                if let Some(preparation) = &observed.tasks()[index].statement().body().preparation {
                    for context in [
                        &preparation.parent,
                        &preparation.controller,
                        &preparation.construction,
                    ] {
                        if selected.insert(context.clone()) {
                            pending.push(context.clone());
                        }
                    }
                }
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

/// The terminal tasks one app team may call its own history: those it
/// created, and those whose most recent winning claim was bound to a lane now
/// in the team, whichever principal won it. A team the residency does not
/// know keeps only what it created.
struct TeamHistoryScope {
    lanes: std::collections::BTreeSet<wire::CanonicalUuid>,
    created: std::collections::BTreeSet<wire::TaskStamp>,
}

impl TeamHistoryScope {
    fn admits(&self, heads: &journal::TaskHeads) -> bool {
        self.created.contains(&heads.statement().body().born)
            || heads
                .claim()
                .is_some_and(|claim| self.lanes.contains(&claim.body().lane))
    }
}

fn team_history_scope(
    observed: &bus::TaskSnapshot,
    team: &wire::CanonicalUuid,
) -> Result<TeamHistoryScope, Outcome<CommandAnswer>> {
    let lanes = observed
        .residency()
        .map(|residency| {
            residency
                .body()
                .lanes
                .iter()
                .filter(|lane| lane.team == *team)
                .map(|lane| lane.lane)
                .collect()
        })
        .unwrap_or_default();
    let created = task_creator_teams(observed.facts().iter(), None)?
        .into_iter()
        .filter_map(|(task, creator)| (creator == Some(*team)).then_some(task))
        .collect();
    Ok(TeamHistoryScope { lanes, created })
}

fn history_selection(
    observed: &bus::TaskSnapshot,
    before: &Option<TaskHistoryCursor>,
    query: &Option<String>,
    completed_before: Option<&wire::Instant>,
    team: Option<&wire::CanonicalUuid>,
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
    let team = team
        .map(|team| team_history_scope(observed, team))
        .transpose()?;
    let mut history = observed
        .tasks()
        .iter()
        .filter(|heads| terminal_task(heads))
        .filter(|heads| {
            completed_before.is_none_or(|cutoff| {
                heads
                    .workflow()
                    .body()
                    .status
                    .completion()
                    .is_some_and(|completed| completed <= *cutoff)
            })
        })
        .filter(|heads| team.as_ref().is_none_or(|team| team.admits(heads)))
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
            wire::TaskHandle::for_filing(&statement.project, &statement.born, statement.filing)
                .as_str(),
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
    !heads.workflow().body().status.is_pending()
}
