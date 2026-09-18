use super::*;

mod requirements;
use requirements::environment_requirements;

/// Execute one generated board-family CLI leaf.
pub(crate) fn run(matches: &ArgMatches, contract: &Contract) -> Outcome<CommandAnswer> {
    match discover_repository() {
        Ok(repository) => dispatch_client(&repository, contract, matches),
        Err(outcome) => outcome,
    }
}

/// Execute `task next`, keeping the empty-drain readout beside the outcome.
pub(crate) fn run_task_next(contract: &Contract) -> TaskNext {
    match discover_repository() {
        Ok(repository) => client_next(&repository, contract),
        Err(outcome) => TaskNext {
            outcome,
            drain: None,
        },
    }
}

fn discover_repository() -> Result<Repository, Outcome<CommandAnswer>> {
    let current = std::env::current_dir()
        .map_err(|error| environment_fault("board::cwd-unavailable", &error.to_string()))?;
    Repository::discover(current)
        .map_err(|error| environment_fault("board::repository-unavailable", &error.to_string()))
}

fn dispatch_client(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    match contract.path {
        ["board"] => client_board(repository, contract, matches),
        ["board", "graph"] => client_graph(repository, contract, matches),
        ["board", "handout"] => client_handout(repository, contract, matches),
        ["doc"] => run_doc(repository, contract, matches, dialect::emit),
        ["doc", "ledger"] => client_ledger(repository, contract, matches),
        ["task", "list"] => client_list(repository, contract, matches),
        ["task", "status"] => client_status(repository, contract),
        ["task", "show"] => client_show(repository, contract, matches),
        ["task", "next"] => client_next(repository, contract).outcome,
        ["task", "recover"] => client_recover(repository, contract, matches),
        ["task", "reclaim"] => client_reclaim(repository, contract, matches),
        ["task", "add"] => client_add(repository, contract, matches),
        ["task", "capture"] => client_capture(repository, contract, matches),
        ["task", "oops"] => client_oops(repository, contract, matches),
        ["task", "note"] => client_note(repository, contract, matches),
        ["task", "defer"] => client_defer(repository, contract, matches),
        ["task", "wake"] => client_wake(repository, contract, matches),
        ["task", "free"] => client_free(repository, contract, matches),
        ["task", "depends"] => client_depends(repository, contract, matches),
        ["task", "modify"] => client_modify(repository, contract, matches),
        ["task", "require"] => client_require(repository, contract, matches),
        ["task", "delete"] => client_delete(repository, contract, matches),
        ["task", "done"] => client_done(repository, contract, matches),
        ["task", "review"] => client_review(repository, contract, matches),
        _ => refusal(
            "board::client-mapping-missing",
            Some(contract.path.join(" ")),
            "use a registered board, task, or doc leaf",
        ),
    }
}

fn client_board(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::BoardRead,
        Vec::new(),
        &BoardRead {
            family: matches.get_one::<String>("FAMILY").cloned(),
        },
    )
}

mod read;
pub(super) use read::*;
mod drain;
pub(crate) use drain::{DrainReadout, ParkedCounts, TaskNext};
pub(super) use drain::{actor_rows, drain_reply};
mod recovery;
use recovery::client_recover;
#[cfg(test)]
pub(super) use recovery::{RecoveryBoardState, recovery_board_state, recovery_preflight};
mod completion;
use completion::{client_done, client_review};
mod submission;
use submission::{
    artifact_operation_failure, mutate_from_board, mutate_task, mutate_task_from_board,
    repository_state_failure, submit_oops_add,
};
pub(crate) use submission::{current_board, submit_task_add, submit_taste_proposal};

fn client_graph(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::BoardGraph,
        Vec::new(),
        &BoardGraph {
            view: required(matches, "VIEW"),
            at: matches.get_one::<String>("at").cloned(),
        },
    )
}

fn client_handout(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::BoardHandout,
        Vec::new(),
        &BoardHandout {
            output: required(matches, "OUTPUT"),
            at: matches.get_one::<String>("at").cloned(),
        },
    )
}

fn client_ledger(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    match required(matches, "HANDLE").parse() {
        Ok(task) => crate::command_client::execute_typed(
            repository,
            contract,
            CommandKind::DocLedger,
            Vec::new(),
            &DocLedger { task },
        ),
        Err(error) => refusal(
            "doc::handle-invalid",
            Some(required(matches, "HANDLE")),
            &error.to_string(),
        ),
    }
}

fn client_next(repository: &Repository, contract: &Contract) -> TaskNext {
    let mut drain = None;
    let outcome = retry_task_next(|| {
        mutate_from_board(repository, contract, |board| {
            let entries = actor_entries(&board)?;
            let subscription = allocation_subscription(&board.residency.body, &board.principal)?;
            let environment = board.environment();
            let selected = board::select(
                &entries,
                &board.principal,
                &subscription,
                board.allocation_band,
                board.recent_cell.as_ref(),
                &environment,
            )
            .ok_or_else(|| {
                // The refusal stays typed for every caller; the readout is
                // measured from this same snapshot for the rim to speak.
                drain = drain_reply(&board).ok();
                refusal(
                    "board::allocator-empty",
                    None,
                    "add or wake eligible work before requesting task next",
                )
            })?;
            let task = selected.entry().handle().clone();
            let heads = task_heads(&board, &task)?;
            let (lane, occupant) = actor_seat(&board)?;
            let rule = "next".parse::<Name>().expect("next is a wire name");
            Ok((
                CommandKind::ClaimWin,
                vec![
                    statement_witness(heads),
                    workflow_witness(heads),
                    claim_witness(heads),
                    Witness::Eligibility { rule: rule.clone() },
                    residency_witness(&board, lane, occupant),
                ],
                serde_json::to_value(ClaimWin {
                    rule,
                    recovery: None,
                })
                .expect("typed intent encodes"),
            ))
        })
    });
    // The resident can also drain empty after the client selected a row that
    // moved under it; that answer arrives without a snapshot, so read one.
    if drain.is_none() && allocator_empty(&outcome) {
        drain = current_board(repository)
            .ok()
            .and_then(|board| drain_reply(&board).ok());
    }
    TaskNext { outcome, drain }
}

/// Whether an outcome is the typed refusal an exhausted allocator returns.
pub(crate) fn allocator_empty(outcome: &Outcome<CommandAnswer>) -> bool {
    matches!(
        outcome,
        Outcome::Refusal(refusal) if refusal.reason() == "board::allocator-empty"
    )
}

pub(super) const TASK_NEXT_ATTEMPTS: usize = 2;

pub(super) fn retry_task_next(
    mut attempt: impl FnMut() -> Outcome<CommandAnswer>,
) -> Outcome<CommandAnswer> {
    for index in 0..TASK_NEXT_ATTEMPTS {
        let outcome = attempt();
        if index + 1 < TASK_NEXT_ATTEMPTS && retryable_task_next(&outcome) {
            continue;
        }
        return outcome;
    }
    unreachable!("task-next retry loop always returns")
}

fn retryable_task_next(outcome: &Outcome<CommandAnswer>) -> bool {
    matches!(
        outcome,
        Outcome::Refusal(refusal)
            if refusal.reason() == "board::witness-moved"
                && matches!(
                    refusal.diagnostic().and_then(wire::RefusalDiagnostic::observed),
                    Some(
                        "board::witness-moved: allocator projection changed"
                            | "board::witness-moved: allocator subscription head changed"
                    )
                )
    )
}

fn client_reclaim(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    mutate_from_board(repository, contract, |board| {
        let (task, heads) = match matches.get_one::<String>("HANDLE") {
            Some(written) => {
                let task = written.parse::<TaskHandle>().map_err(|error| {
                    refusal(
                        "task::handle-invalid",
                        Some(written.clone()),
                        &error.to_string(),
                    )
                })?;
                let heads = task_heads(&board, &task)?;
                (task, heads)
            }
            None => reclaim_active(&board)?,
        };
        let seconds = matches.get_one::<String>("lease-seconds").map_or(
            Ok(spice::claim_lease::DEFAULT_RECLAIM_SECONDS),
            |written| {
                written.parse::<u64>().map_err(|error| {
                    refusal(
                        "claim::lease-seconds-invalid",
                        Some(written.clone()),
                        &error.to_string(),
                    )
                })
            },
        )?;
        if seconds == 0 {
            return Err(refusal(
                "claim::lease-seconds-zero",
                Some(seconds.to_string()),
                "request a positive bounded lease extension",
            ));
        }
        let claim = reclaim_claim(heads, &task, &board.principal)?;
        let lease = heads.lease.as_ref().ok_or_else(|| {
            protocol_fault(
                "claim::lease-missing",
                "an active claim head has no durable lease head",
            )
        })?;
        let (lane, occupant) = actor_seat(&board)?;
        Ok((
            CommandKind::ClaimRenew,
            vec![
                Witness::Claim {
                    task: heads.statement.body.born.clone(),
                    head: Some(claim.position),
                },
                Witness::Lease {
                    task: heads.statement.body.born.clone(),
                    head: lease.position,
                },
                residency_witness(&board, lane, occupant),
            ],
            serde_json::to_value(ClaimRenew {
                task,
                lease_seconds: seconds,
            })
            .expect("typed intent encodes"),
        ))
    })
}

/// Resolve the implicit reclaim target without losing status-first recovery.
pub(super) fn reclaim_active(
    board: &BoardAnswer,
) -> Result<(TaskHandle, &BoardTask), Outcome<CommandAnswer>> {
    match sole_active(board) {
        Err(Outcome::Refusal(refusal)) if refusal.reason() == "claim::active-missing" => Err(
            reclaim_authority_refusal("claim::active-missing", Some(board.principal.to_string())),
        ),
        outcome => outcome,
    }
}

pub(super) fn reclaim_claim<'a>(
    heads: &'a BoardTask,
    task: &TaskHandle,
    principal: &Actor,
) -> Result<&'a Positioned<ClaimHead>, Outcome<CommandAnswer>> {
    let claim = heads
        .claim
        .as_ref()
        .ok_or_else(|| reclaim_authority_refusal("claim::head-missing", Some(task.to_string())))?;
    if claim.body.disposition != ClaimDisposition::Active || claim.body.actor != *principal {
        return Err(reclaim_authority_refusal(
            "claim::ownership-refused",
            Some(task.to_string()),
        ));
    }
    Ok(claim)
}

fn client_add(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let title = required(matches, "TITLE");
    let project = matches.get_one::<String>("project").cloned();
    let description = matches.get_one::<String>("description").cloned();
    let acceptance = matches
        .get_many::<String>("acceptance")
        .into_iter()
        .flatten()
        .cloned()
        .collect();
    let requirements = match environment_requirements(
        matches
            .get_one::<String>("requires-journal-schema")
            .map(String::as_str),
        matches
            .get_one::<String>("requires-trust-grant")
            .map(String::as_str),
    ) {
        Ok(requirements) => requirements,
        Err(outcome) => return outcome,
    };
    let origin = match parse_origin(matches) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    submit_task_add(
        repository,
        contract,
        TaskAdd {
            title,
            project,
            description,
            acceptance,
            requirements,
            origin,
            flow: None,
            depends: Vec::new(),
        },
    )
}

fn client_capture(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let board = match current_board(repository) {
        Ok(board) => board,
        Err(outcome) => return outcome,
    };
    if let Err(outcome) = require_no_plan_implementation(&board, "task capture") {
        return outcome;
    }
    let stealing = matches.get_flag("steal");
    let task = match matches.get_one::<String>("HANDLE") {
        Some(written) => {
            if matches.contains_id("title")
                || matches.contains_id("project")
                || matches.contains_id("origin")
            {
                return refusal(
                    "task::capture-shape",
                    Some(written.clone()),
                    "name either an existing HANDLE or new-task title/project fields",
                );
            }
            match written.parse::<TaskHandle>() {
                Ok(task) => task,
                Err(error) => {
                    return refusal(
                        "task::handle-invalid",
                        Some(written.clone()),
                        &error.to_string(),
                    );
                }
            }
        }
        None => {
            if stealing {
                return refusal(
                    "task::steal-handle-missing",
                    None,
                    "name the HANDLE of the claim to steal; a new task has no holder",
                );
            }
            let Some(project) = matches.get_one::<String>("project").cloned() else {
                return refusal(
                    "task::capture-project-missing",
                    None,
                    "supply --project when task capture mints a new task",
                );
            };
            let candidate = match spice::task_git::capture(repository) {
                Ok(candidate) => candidate,
                Err(error) => return task_git_outcome(error),
            };
            let title = matches
                .get_one::<String>("title")
                .cloned()
                .unwrap_or(candidate.subject);
            let origin = match parse_origin(matches) {
                Ok(origin) => origin,
                Err(outcome) => return outcome,
            };
            let add_contract =
                crate::cli::contract(&["task", "add"]).expect("task add contract is registered");
            match submit_task_add(
                repository,
                add_contract,
                TaskAdd {
                    title,
                    project: Some(project),
                    description: None,
                    acceptance: Vec::new(),
                    requirements: wire::EnvironmentRequirements::default(),
                    origin,
                    flow: None,
                    depends: Vec::new(),
                },
            ) {
                Outcome::Reply(Reply {
                    body: CommandAnswer::Task(answer),
                    ..
                }) => answer.task,
                Outcome::Reply(_) => {
                    return protocol_fault(
                        "task::capture-add-answer",
                        "task add returned another answer shape",
                    );
                }
                Outcome::Refusal(refusal) => return Outcome::Refusal(refusal),
                Outcome::Fault(fault) => return Outcome::Fault(fault),
            }
        }
    };
    mutate_from_board(repository, contract, |board| {
        require_no_plan_implementation(&board, "task capture")?;
        let heads = task_heads(&board, &task)?;
        let (lane, occupant) = actor_seat(&board)?;
        let rule = if stealing { "steal" } else { "capture" }
            .parse::<Name>()
            .expect("possession rules are wire names");
        Ok((
            CommandKind::ClaimWin,
            vec![
                statement_witness(heads),
                workflow_witness(heads),
                claim_witness(heads),
                Witness::Eligibility { rule: rule.clone() },
                residency_witness(&board, lane, occupant),
            ],
            serde_json::to_value(ClaimWin {
                rule,
                recovery: None,
            })
            .expect("typed intent encodes"),
        ))
    })
}

fn client_oops(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let text = required(matches, "TEXT");
    let origin = match parse_origin(matches) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    submit_oops_add(
        repository,
        contract,
        TaskOopsAdd {
            title: text.clone(),
            description: text,
            origin,
        },
    )
}

fn client_note(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    mutate_task(repository, contract, matches, |task, heads| {
        let note = TaskNote {
            task: task.clone(),
            note: required(matches, "TEXT"),
        };
        Ok((
            CommandKind::TaskNote,
            vec![Witness::Existence {
                task: heads.statement.body.born.clone(),
            }],
            serde_json::to_value(note).expect("typed intent encodes"),
        ))
    })
}

fn client_defer(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    mutate_task(repository, contract, matches, |task, heads| {
        let wait = required(matches, "UNTIL")
            .parse::<wire::Instant>()
            .map_err(|error| {
                refusal(
                    "task::defer-instant-invalid",
                    Some(required(matches, "UNTIL")),
                    &error.to_string(),
                )
            })?;
        let body = TaskDefer {
            task: task.clone(),
            wait,
        };
        Ok((
            CommandKind::TaskDefer,
            vec![workflow_witness(heads), deferral_witness(heads)],
            serde_json::to_value(body).expect("typed intent encodes"),
        ))
    })
}

fn client_wake(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    mutate_task(repository, contract, matches, |task, heads| {
        let into = matches.get_one::<String>("into").cloned();
        let mut expected = vec![workflow_witness(heads), deferral_witness(heads)];
        if into.is_some() {
            expected.push(statement_witness(heads));
        }
        let body = TaskWake {
            task: task.clone(),
            into,
        };
        Ok((
            CommandKind::TaskWake,
            expected,
            serde_json::to_value(body).expect("typed intent encodes"),
        ))
    })
}

fn client_free(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    mutate_task_from_board(repository, contract, matches, |board, task, heads| {
        let (lane, occupant) = actor_seat(board)?;
        let lease = heads.lease.as_ref().ok_or_else(|| {
            refusal(
                "claim::lease-missing",
                Some(task.to_string()),
                "name a task carrying a current live claim",
            )
        })?;
        if let Err(error) = spice::task_git::prepare_free(repository, &task.to_string()) {
            return Err(task_git_outcome(error));
        }
        let mut expected = vec![claim_witness(heads)];
        if heads.workflow.body.phase.as_str() == "review"
            && heads.workflow.body.assignment.as_ref() == Some(&board.principal)
        {
            expected.push(workflow_witness(heads));
        }
        expected.push(Witness::Lease {
            task: heads.statement.body.born.clone(),
            head: lease.position,
        });
        expected.push(residency_witness(board, lane, occupant));
        Ok((
            CommandKind::ClaimFree,
            expected,
            serde_json::to_value(TaskRef { task }).expect("typed intent encodes"),
        ))
    })
}

fn client_depends(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    mutate_task_from_board(repository, contract, matches, |board, task, heads| {
        let after = matches
            .get_many::<String>("AFTER")
            .expect("generated syntax requires dependencies")
            .map(|value| value.parse::<TaskHandle>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| refusal("task::dependency-invalid", None, &error.to_string()))?;
        let mut expected = vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
        ];
        let mut new_edges = Vec::new();
        for dependency in &after {
            let dependency_heads = task_heads(board, dependency)?;
            expected.push(statement_witness(dependency_heads));
            expected.push(workflow_witness(dependency_heads));
            let born = &dependency_heads.statement.body.born;
            if !heads.statement.body.dependencies.contains(born) && !new_edges.contains(born) {
                expected.push(Witness::Edge {
                    task: heads.statement.body.born.clone(),
                    after: born.clone(),
                });
                new_edges.push(born.clone());
            }
        }
        append_dependency_reachability_witnesses(board, &new_edges, &mut expected)?;
        Ok((
            CommandKind::TaskDepends,
            expected,
            serde_json::to_value(TaskDepends { task, after }).expect("typed intent encodes"),
        ))
    })
}

fn client_modify(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    mutate_task_from_board(repository, contract, matches, |_board, task, heads| {
        let title = matches.get_one::<String>("title").cloned();
        let description = matches.get_one::<String>("description").cloned();
        let acceptance = matches
            .get_many::<String>("acceptance")
            .map(|values| values.cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let priority = matches.get_one::<String>("priority").cloned();
        let project = matches.get_one::<String>("project").cloned();
        if title.is_none()
            && description.is_none()
            && acceptance.is_empty()
            && priority.is_none()
            && project.is_none()
        {
            return Err(refusal(
                "task::edit-empty",
                None,
                "name at least one of --title, --description, --acceptance, --priority, or --project",
            ));
        }
        // Law::EDIT names the statement head alone. A workflow witness here is
        // not extra care, it is a refusal: the admission door demands exactly
        // the predicates the law states, and a correction replaces statement
        // text without reading the phase.
        Ok((
            CommandKind::TaskEdit,
            vec![statement_witness(heads)],
            serde_json::to_value(TaskEdit {
                task,
                title,
                description,
                acceptance,
                requirements: None,
                priority,
                project,
            })
            .expect("typed intent encodes"),
        ))
    })
}

fn client_require(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    mutate_task_from_board(repository, contract, matches, |_board, task, heads| {
        let schema = matches.get_one::<String>("journal-schema");
        let trust = matches.get_one::<String>("trust-grant");
        let clear = matches.get_flag("clear");
        let requirements = match (schema, trust, clear) {
            (schema, trust, false) if schema.is_some() || trust.is_some() => {
                wire::intent::TaskRequirementsEdit::Set {
                    requirements: environment_requirements(
                        schema.map(String::as_str),
                        trust.map(String::as_str),
                    )?,
                }
            }
            (None, None, true) => wire::intent::TaskRequirementsEdit::Clear {},
            (_, _, true) => {
                return Err(refusal(
                    "task::require-shape",
                    None,
                    "name environmental requirements or --clear, never both",
                ));
            }
            (None, None, false) => {
                return Err(refusal(
                    "task::require-shape",
                    None,
                    "name --journal-schema SCHEMA and/or --trust-grant DIGEST to replace requirements, or --clear to remove them",
                ));
            }
            _ => unreachable!("requirement shape arms are exhaustive"),
        };
        Ok((
            CommandKind::TaskEdit,
            requirements::witnesses(heads),
            serde_json::to_value(TaskEdit {
                task,
                title: None,
                description: None,
                acceptance: Vec::new(),
                requirements: Some(requirements),
                priority: None,
                project: None,
            })
            .expect("typed intent encodes"),
        ))
    })
}

fn client_delete(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    mutate_task_from_board(repository, contract, matches, |_board, task, heads| {
        // Law::DELETE names the claim head, and claim_witness spells the
        // absence of a claim as `head: None`. That is the point: an unclaimed
        // task witnesses that it was unclaimed, so a claim landing between this
        // board read and the door turns the withdrawal into a refusal instead
        // of silently taking the new holder's work.
        Ok((
            CommandKind::TaskDelete,
            vec![
                statement_witness(heads),
                workflow_witness(heads),
                claim_witness(heads),
            ],
            serde_json::to_value(TaskDelete {
                task,
                reason: required(matches, "REASON"),
                force_claimed: matches.get_flag("force-claimed"),
            })
            .expect("typed intent encodes"),
        ))
    })
}

pub(super) fn actor_entries(board: &BoardAnswer) -> Result<Vec<Entry>, Outcome<CommandAnswer>> {
    board
        .tasks
        .iter()
        .map(|heads| {
            Entry::from_answer(
                heads.client_statement(),
                heads.workflow.body.clone(),
                heads.claim.as_ref().map(|head| head.body.clone()),
                heads.lease.as_ref().map(|head| head.body.clone()),
                heads.visibility.clone(),
            )
            .map_err(|error| {
                protocol_fault(
                    "board::preflight-invalid",
                    &format!(
                        "task {} current heads are invalid: {error}",
                        heads.statement.body.handle
                    ),
                )
            })
            .map(|entry| entry.with_creator_team(heads.creator_team))
        })
        .collect()
}

pub(super) fn task_heads<'a>(
    board: &'a BoardAnswer,
    task: &TaskHandle,
) -> Result<&'a BoardTask, Outcome<CommandAnswer>> {
    board
        .tasks
        .iter()
        .find(|heads| heads.statement.body.handle == *task)
        .ok_or_else(|| {
            refusal(
                "task::unknown",
                Some(task.to_string()),
                "read board and name one current task handle",
            )
        })
}

fn task_heads_by_stamp<'a>(
    board: &'a BoardAnswer,
    task: &wire::TaskStamp,
) -> Result<&'a BoardTask, Outcome<CommandAnswer>> {
    board
        .tasks
        .iter()
        .find(|heads| heads.statement.body.born == *task)
        .ok_or_else(|| {
            protocol_fault(
                "task::board-incoherent",
                "a stored dependency has no visible board task",
            )
        })
}

pub(super) fn statement_witness(heads: &BoardTask) -> Witness {
    Witness::Task {
        task: heads.statement.body.born.clone(),
        head: heads.statement.position,
    }
}

pub(super) fn workflow_witness(heads: &BoardTask) -> Witness {
    Witness::Workflow {
        task: heads.statement.body.born.clone(),
        head: heads.workflow.position,
        phase: heads.workflow.body.phase.clone(),
    }
}

pub(super) fn claim_witness(heads: &BoardTask) -> Witness {
    Witness::Claim {
        task: heads.statement.body.born.clone(),
        head: heads.claim.as_ref().map(|head| head.position),
    }
}

pub(super) fn deferral_witness(heads: &BoardTask) -> Witness {
    Witness::Deferral {
        task: heads.statement.body.born.clone(),
        head: heads.workflow.position,
        deferred: heads.workflow.body.deferral.is_some(),
    }
}

pub(super) fn residency_witness(
    board: &BoardAnswer,
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
) -> Witness {
    Witness::Residency {
        lane,
        occupant: Some(occupant),
        epoch: board.residency.body.epoch,
    }
}

pub(super) fn actor_seat(
    board: &BoardAnswer,
) -> Result<(wire::CanonicalUuid, wire::CanonicalUuid), Outcome<CommandAnswer>> {
    let occupied =
        |lane: &wire::LaneResidency| lane.disposition == "occupied" && lane.occupant.is_some();
    let seat = match &board.principal {
        Actor::Lane(actor) => board
            .residency
            .body
            .lanes
            .iter()
            .find(|seat| seat.lane == *actor && occupied(seat)),
        Actor::Operator => {
            let mut seats = board
                .residency
                .body
                .lanes
                .iter()
                .filter(|seat| occupied(seat));
            let first = seats.next();
            if seats.next().is_some() {
                return Err(refusal(
                    "claim::operator-lane-ambiguous",
                    None,
                    "use a lane capability when more than one occupied lane is seated",
                ));
            }
            first
        }
        Actor::Spice => None,
    }
    .ok_or_else(|| {
        refusal(
            "claim::lane-unavailable",
            Some(board.principal.to_string()),
            "seat one occupied lane before changing claim ownership",
        )
    })?;
    Ok((
        seat.lane,
        seat.occupant.expect("occupied filter requires an occupant"),
    ))
}

fn sole_active(board: &BoardAnswer) -> Result<(TaskHandle, &BoardTask), Outcome<CommandAnswer>> {
    let mut held = board.tasks.iter().filter(|heads| {
        !heads.visibility.is_hidden()
            && heads.claim.as_ref().is_some_and(|claim| {
                claim.body.actor == board.principal
                    && claim.body.disposition == wire::ClaimDisposition::Active
            })
    });
    let Some(heads) = held.next() else {
        return Err(refusal(
            "claim::active-missing",
            Some(board.principal.to_string()),
            "win one task with task next before completing its held phase",
        ));
    };
    if held.next().is_some() {
        return Err(protocol_fault(
            "claim::active-ambiguous",
            "one principal holds more than one active claim",
        ));
    }
    Ok((heads.statement.body.handle.clone(), heads))
}

pub(super) fn progress_witnesses(
    board: &BoardAnswer,
    heads: &BoardTask,
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
    require_assignment: bool,
) -> Result<Vec<Witness>, Outcome<CommandAnswer>> {
    let claim = heads.claim.as_ref().ok_or_else(|| {
        refusal(
            "claim::head-missing",
            Some(heads.statement.body.handle.to_string()),
            "win the task before advancing its workflow",
        )
    })?;
    let lease = heads.lease.as_ref().ok_or_else(|| {
        protocol_fault(
            "claim::lease-missing",
            "an active claim head has no lease head",
        )
    })?;
    let mut expected = vec![
        statement_witness(heads),
        workflow_witness(heads),
        Witness::Claim {
            task: heads.statement.body.born.clone(),
            head: Some(claim.position),
        },
        Witness::Lease {
            task: heads.statement.body.born.clone(),
            head: lease.position,
        },
        residency_witness(board, lane, occupant),
    ];
    if require_assignment {
        let reviewer = heads.workflow.body.assignment.clone().ok_or_else(|| {
            refusal(
                "review::assignment-missing",
                Some(heads.statement.body.handle.to_string()),
                "win the review phase with task next before recording its verdict",
            )
        })?;
        expected.push(Witness::Assignment {
            task: heads.statement.body.born.clone(),
            head: heads.workflow.position,
            reviewer,
        });
    }
    if heads.workflow.body.phase.as_str() == "plan" {
        for dependency in &heads.statement.body.dependencies {
            let child = task_heads_by_stamp(board, dependency)?;
            if child.workflow.body.status == "pending"
                && !child.statement.body.acceptance.is_empty()
            {
                expected.push(statement_witness(child));
                expected.push(workflow_witness(child));
            }
        }
    }
    Ok(expected)
}

#[cfg(test)]
pub(super) fn mint_handle_at(project: Option<&str>, _title: &str, epoch_micros: i64) -> TaskHandle {
    let project = project.unwrap_or("task.private");
    let stamp = wire::TaskStamp::for_micros(epoch_micros)
        .expect("the current epoch microsecond fits the task stamp codec");
    TaskHandle::for_project(project, &stamp)
}

pub(super) fn allocate_task_stamp(
    board: &BoardAnswer,
    epoch_micros: i64,
) -> Result<wire::TaskStamp, Outcome<CommandAnswer>> {
    Ok(allocate_task_stamps(board, epoch_micros, 1)?
        .pop()
        .expect("one requested task stamp is allocated"))
}

pub(super) fn allocate_task_stamps(
    board: &BoardAnswer,
    mut epoch_micros: i64,
    count: usize,
) -> Result<Vec<wire::TaskStamp>, Outcome<CommandAnswer>> {
    let mut planned = Vec::with_capacity(count);
    while planned.len() < count {
        let candidate = wire::TaskStamp::for_micros(epoch_micros)
            .map_err(|error| environment_fault("task::stamp-clock-invalid", &error.to_string()))?;
        if board
            .tasks
            .iter()
            .all(|task| task.statement.body.born != candidate)
            && planned.iter().all(|born| born != &candidate)
        {
            planned.push(candidate);
        }
        if planned.len() < count {
            epoch_micros = epoch_micros
                .checked_sub(1)
                .filter(|micros| *micros >= 0)
                .ok_or_else(|| {
                    environment_fault(
                        "task::stamp-clock-invalid",
                        "task birth collision exhausted the representable clock before zero",
                    )
                })?;
        }
    }
    planned.reverse();
    Ok(planned)
}

/// Dispatch one board-family request after transport admission.
#[cfg(test)]
pub(crate) fn dispatch(
    bus: &Bus,
    command: &wire::AdmittedCommand,
) -> Option<Outcome<CommandAnswer>> {
    let projects = ProjectPolicy::new(
        ["board".to_owned(), "rim".to_owned()],
        ["sandbox".to_owned()],
        dialect::PROJECT_MIN_DEPTH as u64,
        dialect::PROJECT_MAX_DEPTH as u64,
    )
    .expect("board command test project policy");
    dispatch_with_repository(bus, command, None, &projects, &board::NextConfig::default())
}

/// Dispatch with the resident repository required by production Git boundaries.
pub(crate) fn dispatch_with_repository(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Option<Outcome<CommandAnswer>> {
    let outcome = match command.body() {
        wire::intent::Intent::BoardRead(read) => {
            read_board(bus, read, command.actor().clone(), projects, allocation)
        }
        wire::intent::Intent::BoardGraph(graph) => {
            read_graph(bus, graph, command.actor().clone(), projects, allocation)
        }
        wire::intent::Intent::BoardHandout(handout) => {
            read_handout(bus, handout, command.actor().clone(), projects, allocation)
        }
        wire::intent::Intent::DocLedger(ledger) => {
            read_ledger(bus, ledger, command.actor().clone(), projects, allocation)
        }
        wire::intent::Intent::TaskAdd(add) => {
            task_add(bus, command, add, repository, projects, allocation)
        }
        wire::intent::Intent::TaskOopsAdd(add) => {
            task_oops_add(bus, command, add, projects, allocation)
        }
        wire::intent::Intent::TasteProposalAdd(add) => {
            taste_proposal_add(bus, command, add, projects, allocation)
        }
        wire::intent::Intent::TaskNote(note) => task_note(bus, command, note, projects, allocation),
        wire::intent::Intent::TaskEdit(edit) => {
            task_edit(bus, command, edit, repository, projects, allocation)
        }
        wire::intent::Intent::TaskDelete(delete) => {
            task_delete(bus, command, delete, projects, allocation)
        }
        wire::intent::Intent::TaskDefer(defer) => {
            task_defer(bus, command, defer, projects, allocation)
        }
        wire::intent::Intent::TaskWake(wake) => task_wake(bus, command, wake, projects, allocation),
        wire::intent::Intent::TaskRef(task) if command.kind() == CommandKind::ClaimFree => {
            claim_free(bus, command, task, projects, allocation)
        }
        wire::intent::Intent::TaskDepends(depends) => {
            task_depends(bus, command, depends, projects, allocation)
        }
        wire::intent::Intent::ClaimWin(win) => {
            claim_win(bus, command, win, repository, projects, allocation)
        }
        wire::intent::Intent::ClaimRenew(renew) if command.kind() == CommandKind::ClaimRenew => {
            claim_reclaim(bus, command, renew, projects, allocation)
        }
        wire::intent::Intent::TaskDone(done) => {
            task_done(bus, command, done, repository, projects, allocation)
        }
        wire::intent::Intent::TaskReview(review) => {
            task_review(bus, command, review, repository, projects, allocation)
        }
        wire::intent::Intent::DocApply(apply) => {
            doc_apply(bus, command, apply, repository, projects, allocation)
        }
        _ => return None,
    };
    Some(outcome)
}
