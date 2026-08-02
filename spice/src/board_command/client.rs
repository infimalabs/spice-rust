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
        ["doc"] => run_doc(repository, contract, matches),
        ["doc", "ledger"] => client_ledger(repository, contract, matches),
        ["task", "next"] => client_next(repository, contract),
        ["task", "add"] => client_add(repository, contract, matches),
        ["task", "capture"] => client_capture(repository, contract, matches),
        ["task", "oops"] => client_oops(repository, contract, matches),
        ["task", "note"] => client_note(repository, contract, matches),
        ["task", "defer"] => client_defer(repository, contract, matches),
        ["task", "wake"] => client_wake(repository, contract, matches),
        ["task", "free"] => client_free(repository, contract, matches),
        ["task", "depends"] => client_depends(repository, contract, matches),
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
        let selected = board::select(&entries, &board.principal).ok_or_else(|| {
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

fn client_add(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let title = required(matches, "TITLE");
    let project = matches.get_one::<String>("project").cloned();
    submit_add(repository, contract, title, project, None)
}

fn client_capture(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
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
            match submit_add(repository, add_contract, title, Some(project), None) {
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
        let rule = "capture".parse::<Name>().expect("capture is a wire name");
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
    submit_system_add(
        repository,
        contract,
        text.clone(),
        ".oops".to_owned(),
        Some(text),
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
        Ok((
            CommandKind::TaskReview,
            progress_witnesses(board, heads, lane, occupant, true)?,
            serde_json::to_value(TaskReview {
                task,
                finding: matches
                    .get_one::<String>("finding")
                    .cloned()
                    .unwrap_or_default(),
                note: matches.get_one::<String>("note").cloned(),
            })
            .expect("typed intent encodes"),
        ))
    })
}

fn run_doc(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let input = match read_doc_input(matches) {
        Ok(input) => input,
        Err(outcome) => return outcome,
    };
    let preview = match board::Preview::classify(&input) {
        Ok(preview) => preview,
        Err(error) => {
            return refusal(
                "doc::classification-refused",
                Some(error.to_string()),
                "correct the authored task document and preview it again",
            );
        }
    };
    let normalized = dialect::emit(preview.document());
    let normalized = match board::Preview::classify(&normalized) {
        Ok(preview) => preview,
        Err(error) => {
            return protocol_fault(
                "doc::normal-form-invalid",
                &format!("dialect emitted a document it refused: {error}"),
            );
        }
    };
    let board = match current_board(repository) {
        Ok(board) => board,
        Err(outcome) => return outcome,
    };
    let touched = document_handles(normalized.document());
    let answer = wire::intent::DocAnswer {
        preview: normalized.digest().clone(),
        touched,
    };
    let Some(apply) = matches.get_one::<String>("apply") else {
        return Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(board.cursor),
            body: CommandAnswer::Doc(answer),
        });
    };
    let observed = match apply.parse::<Digest>() {
        Ok(digest) => digest,
        Err(error) => {
            return refusal(
                "doc::apply-digest-invalid",
                Some(apply.clone()),
                &error.to_string(),
            );
        }
    };
    if observed != *normalized.digest() {
        return refusal(
            "doc::preview-moved",
            Some(observed.to_string()),
            &format!(
                "apply the exact current preview digest {}",
                normalized.digest()
            ),
        );
    }
    let barrier = board_barrier(&board);
    crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::DocApply,
        vec![
            Witness::Preview {
                digest: normalized.digest().clone(),
            },
            Witness::Barrier {
                name: "board".parse().expect("board is a wire name"),
                digest: barrier,
            },
        ],
        &DocApply {
            preview: normalized.digest().clone(),
            document: dialect::emit(normalized.document()),
        },
    )
}

fn read_doc_input(matches: &ArgMatches) -> Result<String, Outcome<CommandAnswer>> {
    let Some(path) = matches.get_one::<String>("PATH|-") else {
        return Err(refusal(
            "doc::input-missing",
            None,
            "name one task-document PATH or - for standard input",
        ));
    };
    if path == "-" {
        let mut input = String::new();
        std::io::stdin()
            .read_to_string(&mut input)
            .map_err(|error| environment_fault("doc::stdin-unreadable", &error.to_string()))?;
        return Ok(input);
    }
    std::fs::read_to_string(path)
        .map_err(|error| environment_fault("doc::input-unreadable", &error.to_string()))
}

fn document_handles(document: &dialect::Document) -> Vec<TaskHandle> {
    document
        .nodes
        .iter()
        .filter(|node| node.kind != dialect::NodeKind::SyntheticRoot)
        .map(|node| document_handle(document.project.as_deref(), node))
        .collect()
}

fn document_handle(project: Option<&str>, node: &dialect::Node) -> TaskHandle {
    let family = project
        .and_then(|project| project.rsplit('.').next())
        .map(stem)
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| stem(&node.title));
    let family = if family.is_empty() {
        "TASK".to_owned()
    } else {
        family
    };
    let seed = wire::encode(&(project, node.slug.as_str()))
        .expect("canonical document task identity encodes");
    let key = Digest::compute(Domain::BoardTaskIdentity, &seed);
    format!("{family}-{}", &key.as_str()[..8])
        .parse()
        .expect("document handle uses admitted ASCII components")
}

fn board_barrier(board: &BoardAnswer) -> Digest {
    let heads = board
        .tasks
        .iter()
        .map(|task| {
            (
                task.statement.body.handle.clone(),
                task.statement.position,
                task.workflow.position,
            )
        })
        .collect::<Vec<_>>();
    let bytes = wire::encode(&heads).expect("typed board barrier encodes");
    Digest::compute(Domain::BoardBarrier, &bytes)
}

type ClientMapping = (CommandKind, Vec<Witness>, serde_json::Value);

fn submit_add(
    repository: &Repository,
    contract: &Contract,
    title: String,
    project: Option<String>,
    description: Option<String>,
) -> Outcome<CommandAnswer> {
    submit_task_add(
        repository,
        contract,
        TaskAdd {
            title,
            project,
            description,
            acceptance: Vec::new(),
            flow: None,
            depends: Vec::new(),
        },
    )
}

fn submit_system_add(
    repository: &Repository,
    contract: &Contract,
    title: String,
    project: String,
    description: Option<String>,
) -> Outcome<CommandAnswer> {
    submit_task_add_with_kind(
        repository,
        contract,
        CommandKind::TaskSystemAdd,
        TaskAdd {
            title,
            project: Some(project),
            description,
            acceptance: Vec::new(),
            flow: None,
            depends: Vec::new(),
        },
    )
}

pub(crate) fn submit_task_add(
    repository: &Repository,
    contract: &Contract,
    body: TaskAdd,
) -> Outcome<CommandAnswer> {
    submit_task_add_with_kind(repository, contract, CommandKind::TaskAdd, body)
}

fn submit_task_add_with_kind(
    repository: &Repository,
    contract: &Contract,
    kind: CommandKind,
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
            kind,
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
    let key = uuid::Uuid::new_v4().simple().to_string();
    format!("{stem}-{}", &key[..8])
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
        wire::intent::Intent::TaskNote(note) => task_note(bus, command, note),
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
