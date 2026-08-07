/// Execute one generated board-family CLI leaf.
pub(crate) fn run(matches: &ArgMatches, contract: &Contract) -> Outcome<CommandAnswer> {
    let current = match std::env::current_dir() {
        Ok(current) => current,
        Err(error) => return environment_fault("board::cwd-unavailable", &error.to_string()),
    };
    let repository = match Repository::discover(current) {
        Ok(repository) => repository,
        Err(error) => {
            return environment_fault("board::repository-unavailable", &error.to_string());
        }
    };
    dispatch_client(&repository, contract, matches)
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
        ["task", "next"] => client_next(repository, contract),
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

fn client_status(repository: &Repository, contract: &Contract) -> Outcome<CommandAnswer> {
    match crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::BoardRead,
        Vec::new(),
        &BoardRead { family: None },
    ) {
        Outcome::Reply(Reply {
            position,
            body: CommandAnswer::Board(board),
        }) => match task_status(&board) {
            Ok(text) => Outcome::Reply(Reply {
                position,
                body: CommandAnswer::Text(TextAnswer { text }),
            }),
            Err(outcome) => outcome,
        },
        Outcome::Reply(_) => protocol_fault(
            "task::status-answer",
            "task status board read returned another answer shape",
        ),
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

fn client_list(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let options = match TaskListOptions::from_matches(matches) {
        Ok(options) => options,
        Err(outcome) => return outcome,
    };
    match crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::BoardRead,
        Vec::new(),
        &BoardRead { family: None },
    ) {
        Outcome::Reply(Reply {
            position,
            body: CommandAnswer::Board(board),
        }) => Outcome::Reply(Reply {
            position,
            body: CommandAnswer::Text(TextAnswer {
                text: task_list(&board, &options),
            }),
        }),
        Outcome::Reply(_) => protocol_fault(
            "task::list-answer",
            "task list board read returned another answer shape",
        ),
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

#[derive(Clone, Copy)]
enum TaskListStatus {
    Pending,
    Waiting,
    Completed,
    Deleted,
}

struct TaskListOptions {
    all: bool,
    project: Option<String>,
    limit: Option<usize>,
    status: Option<TaskListStatus>,
}

impl TaskListOptions {
    fn from_matches(matches: &ArgMatches) -> Result<Self, Outcome<CommandAnswer>> {
        let all = matches.get_flag("all");
        let project = matches.get_one::<String>("project").map(|written| {
            written
                .trim()
                .strip_prefix("project:")
                .unwrap_or(written.trim())
                .to_owned()
        });
        if all && project.is_some() {
            return Err(refusal(
                "task::list-scope",
                None,
                "choose either --all or --project PROJECT",
            ));
        }
        if project.as_ref().is_some_and(String::is_empty) {
            return Err(refusal(
                "task::list-project",
                matches.get_one::<String>("project").cloned(),
                "name a non-empty project after --project",
            ));
        }
        let limit = matches
            .get_one::<String>("limit")
            .map(|written| {
                written.parse::<usize>().map_err(|_| {
                    refusal(
                        "task::list-limit",
                        Some(written.clone()),
                        "name a positive integer after --limit",
                    )
                })
            })
            .transpose()?;
        if limit == Some(0) {
            return Err(refusal(
                "task::list-limit",
                Some("0".to_owned()),
                "name a positive integer after --limit",
            ));
        }
        let status = matches
            .get_one::<String>("status")
            .map(|status| match status.as_str() {
                "pending" => TaskListStatus::Pending,
                "waiting" => TaskListStatus::Waiting,
                "completed" => TaskListStatus::Completed,
                "deleted" => TaskListStatus::Deleted,
                _ => unreachable!("contract parser owns task list status values"),
            });
        Ok(Self {
            all,
            project,
            limit,
            status,
        })
    }
}

fn task_list(board: &BoardAnswer, options: &TaskListOptions) -> String {
    let mut rows = board
        .tasks
        .iter()
        .filter(|task| list_scope_matches(board, task, options))
        .filter(|task| list_status_matches(task, options))
        .collect::<Vec<_>>();
    if let Some(limit) = options.limit {
        rows.sort_by(|left, right| {
            task_newest_position(right)
                .cmp(&task_newest_position(left))
                .then_with(|| {
                    right
                        .statement
                        .body
                        .handle
                        .cmp(&left.statement.body.handle)
                })
        });
        rows.truncate(limit);
    }

    let (scope, empty) = if options.all {
        (
            "scope global --all".to_owned(),
            "no tasks in global scope".to_owned(),
        )
    } else if let Some(project) = &options.project {
        (
            format!("scope explicit-project project:{project}"),
            "no tasks in scope; use --all for global rows or --project PROJECT for one project"
                .to_owned(),
        )
    } else {
        (
            format!("scope actor-route filter {}", actor_route_filter(board)),
            "no tasks in scope; use --all for global rows or --project PROJECT for one project"
                .to_owned(),
        )
    };
    if rows.is_empty() {
        return format!("{scope}\n{empty}");
    }
    let mut output = scope;
    for task in rows {
        output.push('\n');
        output.push_str(&render_task_list_row(task));
    }
    output
}

fn list_scope_matches(board: &BoardAnswer, task: &BoardTask, options: &TaskListOptions) -> bool {
    if let Some(project) = &options.project {
        let candidate = task.statement.body.project.as_str();
        return candidate == project
            || candidate
                .strip_prefix(project)
                .is_some_and(|suffix| suffix.starts_with('.'));
    }
    options.all || actor_route_matches(board, task)
}

fn actor_route_matches(board: &BoardAnswer, task: &BoardTask) -> bool {
    let statement = &task.statement.body;
    statement.project == board.actor_route.project
        || task.origin_thread == board.actor_route.origin_thread
        || (!task.visibility.is_hidden()
            && board.project_policy.public_stems.iter().any(|stem| {
                statement.project == *stem
                    || statement
                        .project
                        .strip_prefix(stem)
                        .is_some_and(|suffix| suffix.starts_with('.'))
            }))
}

fn list_status_matches(task: &BoardTask, options: &TaskListOptions) -> bool {
    let pending = task.workflow.body.status == "pending"
        && task.workflow.body.completion.is_none()
        && task.workflow.body.deferral.is_none();
    let waiting = task.workflow.body.status == "pending"
        && task.workflow.body.completion.is_none()
        && task.workflow.body.deferral.is_some();
    match options.status {
        Some(TaskListStatus::Pending) => pending,
        Some(TaskListStatus::Waiting) => waiting,
        Some(TaskListStatus::Completed) => task.workflow.body.status == "completed",
        // A withdrawal leaves the row readable but off the working board, so the
        // only way to reach one is to ask for it by name or to ask for all.
        Some(TaskListStatus::Deleted) => task.workflow.body.status == "deleted",
        None if options.all => true,
        None => {
            pending
                || waiting
                    && task.claim.as_ref().is_some_and(|claim| {
                        claim.body.disposition == ClaimDisposition::Active
                    })
        }
    }
}

fn task_newest_position(task: &BoardTask) -> Seq {
    let mut newest = task.statement.position.max(task.workflow.position);
    if let Some(claim) = &task.claim {
        newest = newest.max(claim.position);
    }
    if let Some(lease) = &task.lease {
        newest = newest.max(lease.position);
    }
    newest
}

fn render_task_list_row(task: &BoardTask) -> String {
    // A terminal row shows how it ended, not the phase it stopped in. Falling
    // through to the phase would print a withdrawn task as `[todo]`, which is
    // the one thing it is not, and the predecessor prints `[deleted]` here.
    let state = match task.workflow.body.status.as_str() {
        "completed" => "done",
        "deleted" => "deleted",
        _ => task.workflow.body.phase.as_str(),
    };
    let priority = task
        .statement
        .body
        .tags
        .iter()
        .find_map(|tag| tag.strip_prefix(board::PRIORITY_TAG_PREFIX))
        // A spelling the allocator does not score is rendered as written. It
        // ranks as if no priority were declared, and printing the word rather
        // than a letter is how a reader sees which task carries it.
        .map(|priority| {
            board::priority_from_wire(priority).map_or_else(
                || priority.to_owned(),
                |kind| board::priority_label(kind).to_owned(),
            )
        });
    let priority = priority.map_or_else(String::new, |value| format!(" P:{value}"));
    format!(
        "{} [{state}]{priority} {} {}",
        task.statement.body.handle,
        task.statement.body.project,
        task.presentation_title()
    )
}

fn actor_route_filter(board: &BoardAnswer) -> String {
    let projects = board
        .project_policy
        .public_stems
        .iter()
        .map(|stem| format!("project:{stem}"))
        .collect::<Vec<_>>()
        .join(" or ");
    format!(
        "( project:{} or origin_thread.is:{} or ( {projects} ) )",
        board.actor_route.project, board.actor_route.origin_thread
    )
}

fn task_status(board: &BoardAnswer) -> Result<String, Outcome<CommandAnswer>> {
    let entries = actor_entries(board)?;
    let routed = board
        .tasks
        .iter()
        .filter(|task| actor_route_matches(board, task))
        .map(|task| task.statement.body.handle.clone())
        .collect::<std::collections::BTreeSet<_>>();
    // The board owns which prerequisites are satisfied. Deciding it again here
    // is how the same row gets counted ready and blocked at once: `ready` asks
    // the board through `eligible_entry`, so `blocked` has to ask the board too.
    let satisfied = board::satisfied_handles(&entries);
    let pending_actor = |entry: &&Entry| {
        routed.contains(&entry.statement().handle)
            && entry.workflow().status == "pending"
            && entry.workflow().completion.is_none()
    };
    let active = |entry: &&Entry| {
        entry
            .claim()
            .is_some_and(|claim| claim.disposition == ClaimDisposition::Active)
    };
    let rows = entries.iter().filter(pending_actor).collect::<Vec<_>>();
    let claim = rows
        .iter()
        .copied()
        .find(|entry| {
            active(entry)
                && entry
                    .claim()
                    .is_some_and(|claim| claim.actor == board.principal)
        })
        .map_or_else(|| "-".to_owned(), |entry| entry.statement().handle.to_string());
    let active_count = rows.iter().copied().filter(active).count();
    let review = rows
        .iter()
        .copied()
        .filter(|entry| entry.workflow().phase.as_str() == "review" && !active(entry))
        .count();
    let waiting = rows
        .iter()
        .filter(|entry| entry.workflow().deferral.is_some())
        .count();
    let blocked = rows
        .iter()
        .filter(|entry| {
            entry
                .statement()
                .dependencies
                .iter()
                .any(|dependency| !satisfied.contains(dependency))
        })
        .count();
    let ready = rows
        .iter()
        .copied()
        .filter(|entry| {
            entry.workflow().phase.as_str() != "review"
                && board::eligible_entry(&entries, entry, &board.principal)
        })
        .count();
    let stale = rows
        .iter()
        .filter(|entry| {
            entry
                .claim()
                .is_some_and(|claim| claim.disposition == ClaimDisposition::Lapsed)
        })
        .count();
    let oops = entries
        .iter()
        .filter(|entry| {
            entry.project().visibility().hidden_stem() == Some("oops")
                && entry.workflow().status == "pending"
                && entry.workflow().completion.is_none()
        })
        .count();
    Ok(format!(
        "claim {claim}\nactor {}\nfilter {}\npublic task project depth {}..{} dotted segments\n\
         active {active_count}\nready {ready}\n\
         review {review}\nblocked {blocked}\nwaiting {waiting}\n\
         stale {stale}\noops {oops}",
        board.actor_route.actor,
        actor_route_filter(board),
        board.project_policy.public_min_depth,
        board.project_policy.public_max_depth,
    ))
}

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

fn client_next(repository: &Repository, contract: &Contract) -> Outcome<CommandAnswer> {
    mutate_from_board(repository, contract, |board| {
        let entries = actor_entries(&board)?;
        let subscription = allocation_subscription(&board.residency.body, &board.principal)?;
        let selected = board::select(&entries, &board.principal, subscription).ok_or_else(|| {
            refusal(
                "board::allocator-empty",
                None,
                "add or wake eligible work before requesting task next",
            )
        })?;
        let task = selected.entry().statement().handle.clone();
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
            serde_json::to_value(ClaimWin { rule }).expect("typed intent encodes"),
        ))
    })
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
                    refusal("task::handle-invalid", Some(written.clone()), &error.to_string())
                })?;
                let heads = task_heads(&board, &task)?;
                (task, heads)
            }
            None => sole_active(&board)?,
        };
        let seconds = matches
            .get_one::<String>("lease-seconds")
            .map_or(Ok(60 * 60), |written| {
                written.parse::<u64>().map_err(|error| {
                    refusal(
                        "claim::lease-seconds-invalid",
                        Some(written.clone()),
                        &error.to_string(),
                    )
                })
            })?;
        if seconds == 0 {
            return Err(refusal(
                "claim::lease-seconds-zero",
                Some(seconds.to_string()),
                "request a positive bounded lease extension",
            ));
        }
        let claim = heads.claim.as_ref().ok_or_else(|| {
            refusal(
                "claim::head-missing",
                Some(task.to_string()),
                "reclaim only the current lane's active task",
            )
        })?;
        if claim.body.disposition != ClaimDisposition::Active || claim.body.actor != board.principal
        {
            return Err(refusal(
                "claim::ownership-refused",
                Some(task.to_string()),
                "reclaim only an active claim owned by this lane capability",
            ));
        }
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
                    task: task.clone(),
                    head: Some(claim.position),
                },
                Witness::Lease {
                    task: task.clone(),
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
    let origin = match matches.get_one::<String>("origin") {
        Some(written) => match written.parse::<wire::Origin>() {
            Ok(origin) => Some(origin),
            Err(error) => {
                return refusal(
                    "task::origin-invalid",
                    Some(written.clone()),
                    &error.to_string(),
                );
            }
        },
        None => None,
    };
    submit_add(
        repository,
        contract,
        title,
        project,
        description,
        acceptance,
        origin,
    )
}

fn client_capture(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let stealing = matches.get_flag("steal");
    let task = match matches.get_one::<String>("HANDLE") {
        Some(written) => {
            if matches.contains_id("title") || matches.contains_id("project") {
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
            let add_contract = crate::cli::contract(&["task", "add"])
                .expect("task add contract is registered");
            match submit_add(
                repository,
                add_contract,
                title,
                Some(project),
                None,
                Vec::new(),
                None,
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
            serde_json::to_value(ClaimWin { rule }).expect("typed intent encodes"),
        ))
    })
}

fn client_oops(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let text = required(matches, "TEXT");
    submit_oops_add(
        repository,
        contract,
        TaskOopsAdd {
            title: text.clone(),
            description: text,
        },
    )
}

fn client_note(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    mutate_task(repository, contract, matches, |task, _| {
        let note = TaskNote {
            task: task.clone(),
            note: required(matches, "TEXT"),
        };
        Ok((
            CommandKind::TaskNote,
            vec![Witness::Existence { task }],
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
        let body = TaskRef { task: task.clone() };
        Ok((
            CommandKind::TaskWake,
            vec![workflow_witness(heads), deferral_witness(heads)],
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
        Ok((
            CommandKind::ClaimFree,
            vec![
                claim_witness(heads),
                Witness::Lease {
                    task: task.clone(),
                    head: lease.position,
                },
                residency_witness(board, lane, occupant),
            ],
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
        let mut expected = vec![statement_witness(heads), workflow_witness(heads)];
        for dependency in &after {
            let dependency_heads = task_heads(board, dependency)?;
            expected.push(statement_witness(dependency_heads));
            expected.push(workflow_witness(dependency_heads));
            expected.push(Witness::Edge {
                task: task.clone(),
                after: dependency.clone(),
            });
        }
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
                priority,
                project,
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

fn client_done(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    mutate_from_board(repository, contract, |board| {
        let (task, heads) = sole_active(&board)?;
        let (lane, occupant) = actor_seat(&board)?;
        let validation = matches
            .get_one::<String>("validation")
            .cloned()
            .unwrap_or_else(|| "completed by the authenticated lane".to_owned());
        Ok((
            CommandKind::TaskDone,
            progress_witnesses(&board, heads, lane, occupant, false)?,
            serde_json::to_value(TaskDone { task, validation }).expect("typed intent encodes"),
        ))
    })
}

fn client_review(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    mutate_task_from_board(repository, contract, matches, |board, task, heads| {
        let (lane, occupant) = actor_seat(board)?;
        let mut expected = progress_witnesses(board, heads, lane, occupant, true)?;
        let then = plan_followups(board, matches, &mut expected)?;
        let followup = plan_followup_edges(board, matches, &task, &mut expected)?;
        Ok((
            CommandKind::TaskReview,
            expected,
            serde_json::to_value(TaskReview {
                task,
                finding: matches
                    .get_one::<String>("finding")
                    .cloned()
                    .unwrap_or_default(),
                note: matches.get_one::<String>("note").cloned(),
                then,
                followup,
            })
            .expect("typed intent encodes"),
        ))
    })
}

type ClientMapping = (CommandKind, Vec<Witness>, serde_json::Value);

fn submit_add(
    repository: &Repository,
    contract: &Contract,
    title: String,
    project: Option<String>,
    description: Option<String>,
    acceptance: Vec<String>,
    origin: Option<wire::Origin>,
) -> Outcome<CommandAnswer> {
    submit_task_add(
        repository,
        contract,
        TaskAdd {
            title,
            project,
            description,
            acceptance,
            origin,
            flow: None,
            depends: Vec::new(),
        },
    )
}

fn submit_oops_add(
    repository: &Repository,
    contract: &Contract,
    body: TaskOopsAdd,
) -> Outcome<CommandAnswer> {
    mutate_from_board(repository, contract, |board| {
        let handle = mint_handle(Some(".oops"), &body.title);
        if board
            .tasks
            .iter()
            .any(|task| task.statement.body.handle == handle)
        {
            return Err(refusal(
                "task::identity-collision",
                Some(handle.to_string()),
                "retry task oops so the authority can mint a fresh handle",
            ));
        }
        Ok((
            CommandKind::TaskOopsAdd,
            vec![Witness::Absence { task: handle }],
            serde_json::to_value(body).expect("typed intent encodes"),
        ))
    })
}

pub(crate) fn submit_task_add(
    repository: &Repository,
    contract: &Contract,
    body: TaskAdd,
) -> Outcome<CommandAnswer> {
    mutate_from_board(repository, contract, |board| {
        let handle = mint_handle(body.project.as_deref(), &body.title);
        if board
            .tasks
            .iter()
            .any(|task| task.statement.body.handle == handle)
        {
            return Err(refusal(
                "task::identity-collision",
                Some(handle.to_string()),
                "retry task add so the authority can mint a fresh handle",
            ));
        }
        Ok((
            CommandKind::TaskAdd,
            vec![Witness::Absence { task: handle }],
            serde_json::to_value(body).expect("typed intent encodes"),
        ))
    })
}

fn mutate_task<F>(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
    map: F,
) -> Outcome<CommandAnswer>
where
    F: FnOnce(TaskHandle, &BoardTask) -> Result<ClientMapping, Outcome<CommandAnswer>>,
{
    mutate_task_from_board(repository, contract, matches, |_, task, heads| {
        map(task, heads)
    })
}

fn mutate_task_from_board<F>(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
    map: F,
) -> Outcome<CommandAnswer>
where
    F: FnOnce(
        &BoardAnswer,
        TaskHandle,
        &BoardTask,
    ) -> Result<ClientMapping, Outcome<CommandAnswer>>,
{
    mutate_from_board(repository, contract, |board| {
        let written = required(matches, "HANDLE");
        let task = written
            .parse::<TaskHandle>()
            .map_err(|error| refusal("task::handle-invalid", Some(written), &error.to_string()))?;
        let heads = task_heads(&board, &task)?;
        map(&board, task, heads)
    })
}

fn mutate_from_board<F>(
    repository: &Repository,
    contract: &Contract,
    map: F,
) -> Outcome<CommandAnswer>
where
    F: FnOnce(BoardAnswer) -> Result<ClientMapping, Outcome<CommandAnswer>>,
{
    let board = match current_board(repository) {
        Ok(board) => board,
        Err(outcome) => return outcome,
    };
    let (kind, expected, body) = match map(board) {
        Ok(mapped) => mapped,
        Err(outcome) => return outcome,
    };
    crate::command_client::execute_typed(repository, contract, kind, expected, &body)
}

fn current_board(repository: &Repository) -> Result<BoardAnswer, Outcome<CommandAnswer>> {
    let contract = crate::cli::contract(&["board"]).expect("board read contract is registered");
    match crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::BoardRead,
        Vec::new(),
        &BoardRead { family: None },
    ) {
        Outcome::Reply(Reply {
            body: CommandAnswer::Board(board),
            ..
        }) => Ok(*board),
        Outcome::Reply(_) => Err(protocol_fault(
            "board::preflight-answer",
            "board preflight returned a non-board reply",
        )),
        Outcome::Refusal(refusal) => Err(Outcome::Refusal(refusal)),
        Outcome::Fault(fault) => Err(Outcome::Fault(fault)),
    }
}

fn actor_entries(board: &BoardAnswer) -> Result<Vec<Entry>, Outcome<CommandAnswer>> {
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
        })
        .collect()
}

fn task_heads<'a>(
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

fn statement_witness(heads: &BoardTask) -> Witness {
    Witness::Task {
        task: heads.statement.body.handle.clone(),
        head: heads.statement.position,
    }
}

fn workflow_witness(heads: &BoardTask) -> Witness {
    Witness::Workflow {
        task: heads.statement.body.handle.clone(),
        head: heads.workflow.position,
        phase: heads.workflow.body.phase.clone(),
    }
}

fn claim_witness(heads: &BoardTask) -> Witness {
    Witness::Claim {
        task: heads.statement.body.handle.clone(),
        head: heads.claim.as_ref().map(|head| head.position),
    }
}

fn deferral_witness(heads: &BoardTask) -> Witness {
    Witness::Deferral {
        task: heads.statement.body.handle.clone(),
        head: heads.workflow.position,
        deferred: heads.workflow.body.deferral.is_some(),
    }
}

fn residency_witness(
    board: &BoardAnswer,
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
) -> Witness {
    Witness::Residency {
        lane,
        occupant,
        epoch: board.residency.body.epoch,
    }
}

fn actor_seat(
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

fn progress_witnesses(
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
            task: heads.statement.body.handle.clone(),
            head: Some(claim.position),
        },
        Witness::Lease {
            task: heads.statement.body.handle.clone(),
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
            task: heads.statement.body.handle.clone(),
            head: heads.workflow.position,
            reviewer,
        });
    }
    Ok(expected)
}

fn mint_handle(project: Option<&str>, title: &str) -> TaskHandle {
    mint_handle_at(
        project,
        title,
        jiff::Timestamp::now().as_millisecond(),
    )
}

fn mint_handle_at(project: Option<&str>, title: &str, epoch_millis: i64) -> TaskHandle {
    let stem = project
        .and_then(|project| project.rsplit('.').next())
        .map(stem)
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| stem(title));
    let stem = if stem.is_empty() {
        "TASK".to_owned()
    } else {
        stem
    };
    let key = steer::Key::for_millis(epoch_millis)
        .expect("the current epoch millisecond fits the task stamp codec");
    format!("{stem}-{key}")
        .parse()
        .expect("generated handle uses admitted ASCII components")
}

fn stem(text: &str) -> String {
    text.bytes()
        .filter(|byte| byte.is_ascii_alphanumeric())
        .map(|byte| char::from(byte.to_ascii_uppercase()))
        .take(7)
        .collect()
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
    dispatch_with_repository(bus, command, None, &projects)
}

/// Dispatch with the resident repository required by production Git boundaries.
pub(crate) fn dispatch_with_repository(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
) -> Option<Outcome<CommandAnswer>> {
    let outcome = match command.body() {
        wire::intent::Intent::BoardRead(read) => {
            read_board(bus, read, command.actor().clone(), projects)
        }
        wire::intent::Intent::BoardGraph(graph) => {
            read_graph(bus, graph, command.actor().clone(), projects)
        }
        wire::intent::Intent::BoardHandout(handout) => {
            read_handout(bus, handout, command.actor().clone(), projects)
        }
        wire::intent::Intent::DocLedger(ledger) => {
            read_ledger(bus, ledger, command.actor().clone(), projects)
        }
        wire::intent::Intent::TaskAdd(add) => task_add(bus, command, add, projects),
        wire::intent::Intent::TaskOopsAdd(add) => task_oops_add(bus, command, add, projects),
        wire::intent::Intent::TaskNote(note) => task_note(bus, command, note, projects),
        wire::intent::Intent::TaskEdit(edit) => task_edit(bus, command, edit, projects),
        wire::intent::Intent::TaskDelete(delete) => {
            task_delete(bus, command, delete, projects)
        }
        wire::intent::Intent::TaskDefer(defer) => task_defer(bus, command, defer, projects),
        wire::intent::Intent::TaskRef(task) if command.kind() == CommandKind::TaskWake => {
            task_wake(bus, command, task, projects)
        }
        wire::intent::Intent::TaskRef(task) if command.kind() == CommandKind::ClaimFree => {
            claim_free(bus, command, task, projects)
        }
        wire::intent::Intent::TaskDepends(depends) => {
            task_depends(bus, command, depends, projects)
        }
        wire::intent::Intent::ClaimWin(win) => {
            claim_win(bus, command, win, repository, projects)
        }
        wire::intent::Intent::ClaimRenew(renew)
            if command.kind() == CommandKind::ClaimRenew =>
        {
            claim_reclaim(bus, command, renew, projects)
        }
        wire::intent::Intent::TaskDone(done) => {
            task_done(bus, command, done, repository, projects)
        }
        wire::intent::Intent::TaskReview(review) => {
            task_review(bus, command, review, repository, projects)
        }
        wire::intent::Intent::DocApply(apply) => doc_apply(bus, command, apply, projects),
        _ => return None,
    };
    Some(outcome)
}
