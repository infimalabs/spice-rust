//! Task mutation submission and steering-origin artifact attachment.

use super::*;
use base64::Engine as _;

pub(super) fn client_add(
    repository: Host<'_>,
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
    let depends = match matches
        .get_many::<String>("depends")
        .into_iter()
        .flatten()
        .map(|written| written.parse::<TaskHandle>())
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(depends) => depends,
        Err(error) => {
            return refusal("task::dependency-invalid", None, &error.to_string());
        }
    };
    let requirements = match environment_requirements(
        matches
            .get_one::<String>("requires-journal-schema")
            .map(String::as_str),
        matches
            .get_one::<String>("requires-trust-grant")
            .map(String::as_str),
        None,
    ) {
        Ok(requirements) => requirements,
        Err(outcome) => return outcome,
    };
    let origin = match parse_origin(matches) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let intent = match contract.path {
        ["task", "plan"] => Some(wire::TaskIntent::Plan),
        ["task", "design"] => Some(wire::TaskIntent::Design),
        _ => match super::declared_intent(matches) {
            Ok(intent) => intent,
            Err(outcome) => return outcome,
        },
    };
    let preparation = match matches
        .get_one::<String>("preparation")
        .map(|written| written.parse::<TaskHandle>())
        .transpose()
    {
        Ok(preparation) => preparation,
        Err(error) => return refusal("task::preparation-invalid", None, &error.to_string()),
    };
    let claim = matches.get_flag("claim");
    let body = TaskAdd {
        preparation,
        intent,
        title,
        pitch: matches
            .get_one::<String>("pitch")
            .cloned()
            .unwrap_or_default(),
        project,
        description,
        acceptance,
        requirements,
        origin,
        flow: matches.get_one::<String>("flow").cloned(),
        depends,
        claim,
        draft: matches.get_flag("draft"),
    };
    submit_task_add(repository, contract, body)
}

pub(super) fn client_capture(
    repository: Host<'_>,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    if matches.get_flag("restore") {
        if matches.get_one::<String>("HANDLE").is_some()
            || capture_new_task_fields(matches)
            || matches.get_flag("steal")
        {
            return refusal(
                "git::recovery-restore-shape",
                None,
                "run `spice task capture --restore` without a task handle or task fields",
            );
        }
        return restore_client_recovery(repository);
    }
    let board = match current_board(
        repository,
        BoardRead::Tasks {
            tasks: Vec::new(),
            dependencies: false,
            claims: true,
            history: false,
        },
    ) {
        Ok(board) => board,
        Err(outcome) => return outcome,
    };
    if let Err(outcome) = require_no_planning_implementation(&board, "task capture") {
        return outcome;
    }
    let task = match matches.get_one::<String>("HANDLE") {
        Some(written) => match written.parse::<TaskHandle>() {
            Ok(task) => Some(task),
            Err(error) => {
                return refusal(
                    "task::handle-invalid",
                    Some(written.clone()),
                    &error.to_string(),
                );
            }
        },
        None => None,
    };
    let pending = match spice::task_git::pending_recovery(&repository) {
        Ok(pending) => pending,
        Err(error) => return task_git_outcome(error),
    };
    if let Some(recovery) = pending {
        let exact_shape = task.is_some() && !capture_new_task_fields(matches);
        return resume_pending_client_capture(
            repository,
            task.as_ref().filter(|_| exact_shape),
            recovery,
        );
    }
    let stealing = matches.get_flag("steal");
    match task {
        Some(task) => {
            if capture_new_task_fields(matches) {
                return refusal(
                    "task::capture-shape",
                    Some(task.to_string()),
                    spice::task_git::UNOWNED_WORK_REMEDY,
                );
            }
            capture_existing_task(repository, contract, task, stealing)
        }
        None => capture_new_task(repository, matches, stealing),
    }
}

fn capture_new_task_fields(matches: &ArgMatches) -> bool {
    matches.contains_id("title")
        || matches.contains_id("project")
        || matches.contains_id("origin")
        || matches.contains_id("description")
        || matches.contains_id("acceptance")
}

fn capture_existing_task(
    repository: Host<'_>,
    contract: &Contract,
    task: TaskHandle,
    stealing: bool,
) -> Outcome<CommandAnswer> {
    let dirty = match spice::task_git::has_dirty_work(&repository, repository.ambient) {
        Ok(dirty) => dirty,
        Err(error) => return task_git_outcome(error),
    };
    if dirty {
        return client_capture_recovery(repository, contract, &task, stealing);
    }
    let read = BoardRead::Tasks {
        tasks: vec![
            task.born()
                .expect("a parsed handle carries a canonical task stamp"),
        ],
        dependencies: false,
        claims: true,
        history: false,
    };
    mutate_from_board(repository, contract, read, |board| {
        require_no_planning_implementation(&board, "task capture")?;
        let heads = named_task_heads(&board, &task)?;
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

fn capture_new_task(
    repository: Host<'_>,
    matches: &ArgMatches,
    stealing: bool,
) -> Outcome<CommandAnswer> {
    if stealing {
        return refusal(
            "task::steal-handle-missing",
            None,
            spice::task_git::UNOWNED_WORK_REMEDY,
        );
    }
    let Some(project) = matches.get_one::<String>("project").cloned() else {
        return refusal(
            "task::capture-project-missing",
            None,
            spice::task_git::UNOWNED_WORK_REMEDY,
        );
    };
    let dirty = match spice::task_git::has_dirty_work(&repository, repository.ambient) {
        Ok(dirty) => dirty,
        Err(error) => return task_git_outcome(error),
    };
    let written_title = matches.get_one::<String>("title").cloned();
    let candidate = if dirty && written_title.is_some() {
        None
    } else {
        match spice::task_git::capture(&repository, repository.ambient) {
            Ok(candidate) => Some(candidate),
            Err(error) if error.reason() == "git::nothing-to-capture" && dirty => None,
            Err(error) if error.reason() == "git::nothing-to-capture" => {
                return refusal(
                    "git::nothing-to-capture",
                    None,
                    &format!(
                        "run `spice task add {} --project {project} --claim` for new work with no paths or commits to adopt",
                        written_title.as_deref().unwrap_or("TITLE"),
                    ),
                );
            }
            Err(error) => return task_git_outcome(error),
        }
    };
    let title = match written_title.or_else(|| candidate.map(|candidate| candidate.subject)) {
        Some(title) => title,
        None => {
            return refusal(
                "task::capture-title-missing",
                None,
                spice::task_git::UNOWNED_WORK_REMEDY,
            );
        }
    };
    let origin = match parse_origin(matches) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let description = matches.get_one::<String>("description").cloned();
    // A capture names work that already exists, so an unwritten pitch derives
    // from the title and description rather than refusing the capture.
    let pitch = matches
        .get_one::<String>("pitch")
        .cloned()
        .unwrap_or_else(|| wire::pitch::derive(&title, description.as_deref()));
    let body = TaskAdd {
        preparation: None,
        intent: None,
        title,
        pitch,
        project: Some(project),
        description,
        acceptance: matches
            .get_many::<String>("acceptance")
            .into_iter()
            .flatten()
            .cloned()
            .collect(),
        requirements: wire::EnvironmentRequirements::default(),
        origin,
        flow: None,
        depends: Vec::new(),
        claim: true,
        draft: false,
    };
    let add_contract =
        crate::cli::contract(&["task", "add"]).expect("task add contract is registered");
    let outcome = if dirty {
        client_add_recovery(repository, add_contract, body)
    } else {
        submit_captured_task(repository, add_contract, body)
    };
    capture_claim_answer(outcome)
}

fn capture_claim_answer(outcome: Outcome<CommandAnswer>) -> Outcome<CommandAnswer> {
    match outcome {
        Outcome::Reply(Reply {
            position,
            body: CommandAnswer::Task(mut answer),
        }) => match answer.claim.take() {
            Some(claim) => Outcome::Reply(Reply {
                position,
                body: CommandAnswer::Claim(claim),
            }),
            None => protocol_fault(
                "task::capture-add-claim-missing",
                "atomic capture task creation returned no claim",
            ),
        },
        reply @ Outcome::Reply(Reply {
            body: CommandAnswer::Claim(_),
            ..
        }) => reply,
        Outcome::Reply(_) => protocol_fault(
            "task::capture-add-answer",
            "atomic capture task creation returned another answer shape",
        ),
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

pub(super) fn client_oops(
    repository: Host<'_>,
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
        text,
        origin,
        matches.get_one::<String>("project").cloned(),
    )
}

type ClientMapping = (CommandKind, Vec<Witness>, serde_json::Value);

pub(super) fn submit_oops_add(
    repository: Host<'_>,
    contract: &Contract,
    title: String,
    origin: Option<wire::Origin>,
    project: Option<String>,
) -> Outcome<CommandAnswer> {
    for attempt in 0..2 {
        let outcome = mutate_from_board(
            repository,
            contract,
            BoardRead::All { family: None },
            |board| {
                let generation = board.oops_generation.as_ref();
                let current = generation.and_then(|head| head.body.design.as_ref());
                let now = jiff::Timestamp::now().as_microsecond();
                let (design, mut expected) = match current {
                    Some(design) => {
                        let design = board
                            .tasks
                            .iter()
                            .find(|task| task.statement.body.born == *design)
                            .ok_or_else(|| {
                                protocol_fault(
                                    "task::oops-generation-task-missing",
                                    "the OOPS generation names a task absent from the board",
                                )
                            })?;
                        if design.statement.body.intent
                            != Some(wire::TaskIntent::OpportunisticDesign)
                            || !design
                                .design_standing()
                                .is_some_and(|standing| standing.is_open())
                        {
                            return Err(protocol_fault(
                                "task::oops-generation-invalid",
                                "the OOPS generation does not name one open opportunistic Design",
                            ));
                        }
                        let report = allocate_task_stamp(&board, now)?;
                        (
                            design.statement.body.born.clone(),
                            vec![
                                Witness::Absence { task: report },
                                Witness::Task {
                                    task: design.statement.body.born.clone(),
                                    head: design.statement.position,
                                },
                                Witness::Workflow {
                                    task: design.statement.body.born.clone(),
                                    head: design.workflow.position,
                                    phase: design.workflow.body.phase.clone(),
                                },
                            ],
                        )
                    }
                    None => {
                        let mut stamps = allocate_task_stamps(&board, now, 2)?;
                        let design = stamps.remove(0);
                        let report = stamps.remove(0);
                        (
                            design.clone(),
                            vec![
                                Witness::Absence { task: report },
                                Witness::Absence {
                                    task: design.clone(),
                                },
                            ],
                        )
                    }
                };
                expected.push(Witness::OopsGeneration {
                    head: generation.map(|head| head.position),
                    design: current.cloned(),
                });
                Ok((
                    CommandKind::TaskOopsAdd,
                    expected,
                    serde_json::to_value(TaskOopsAdd {
                        design,
                        title: title.clone(),
                        origin: origin.clone(),
                        project: project.clone(),
                    })
                    .expect("typed intent encodes"),
                ))
            },
        );
        if attempt == 0
            && matches!(
                &outcome,
                Outcome::Refusal(refusal) if refusal.reason() == "board::witness-moved"
            )
        {
            continue;
        }
        return outcome;
    }
    unreachable!("OOPS generation retry loop always returns")
}

pub(crate) fn submit_task_add(
    repository: Host<'_>,
    contract: &Contract,
    body: TaskAdd,
) -> Outcome<CommandAnswer> {
    submit_task_add_with_rule(repository, contract, body, "capture")
}

pub(super) fn submit_recovery_task_add(
    repository: Host<'_>,
    contract: &Contract,
    body: TaskAdd,
) -> Outcome<CommandAnswer> {
    submit_task_add_with_rule(repository, contract, body, "recover")
}

fn submit_captured_task(
    repository: Host<'_>,
    contract: &Contract,
    body: TaskAdd,
) -> Outcome<CommandAnswer> {
    submit_task_add_with_rule(repository, contract, body, CAPTURE_WORK_RULE)
}

fn submit_task_add_with_rule(
    repository: Host<'_>,
    contract: &Contract,
    body: TaskAdd,
    claim_rule: &str,
) -> Outcome<CommandAnswer> {
    let steering_origin = match steering_origin_key(body.origin.as_ref()) {
        Ok(key) => key,
        Err(outcome) => return outcome,
    };
    let outcome = mutate_from_board(
        repository,
        contract,
        BoardRead::All { family: None },
        |board| {
            let stamp = allocate_task_stamp(&board, jiff::Timestamp::now().as_microsecond())?;
            let mut expected = vec![Witness::Absence { task: stamp }];
            append_preparation_witnesses(&board, &body, &mut expected)?;
            if body.claim {
                let (lane, occupant) = actor_seat(&board)?;
                expected.push(Witness::Eligibility {
                    rule: claim_rule.parse().expect("claim rule is a wire name"),
                });
                expected.push(residency_witness(&board, lane, occupant));
            }
            Ok((
                CommandKind::TaskAdd,
                expected,
                serde_json::to_value(body).expect("typed intent encodes"),
            ))
        },
    );
    attach_steering_origin_artifacts(repository, steering_origin.as_ref(), outcome)
}

pub(in crate::board_command) fn append_preparation_witnesses(
    board: &BoardAnswer,
    add: &TaskAdd,
    expected: &mut Vec<Witness>,
) -> Result<(), Outcome<CommandAnswer>> {
    if add.draft {
        // A Draft binds preparation only when it is filed.
        return Ok(());
    }
    if let Some(heads) = planning::preparation_parent(board, add)? {
        expected.push(statement_witness(heads));
        expected.push(workflow_witness(heads));
        if add.preparation.is_none() {
            expected.push(claim_witness(heads));
        }
    }
    Ok(())
}

fn attach_steering_origin_artifacts(
    repository: Host<'_>,
    origin: Option<&steer::Key>,
    outcome: Outcome<CommandAnswer>,
) -> Outcome<CommandAnswer> {
    let Some(key) = origin else {
        return outcome;
    };
    let task = match &outcome {
        Outcome::Reply(Reply {
            body: CommandAnswer::Task(answer),
            ..
        }) => match answer.task.born() {
            Ok(task) => task,
            Err(error) => {
                return protocol_fault(
                    "task::answer-handle-invalid",
                    &format!("task add returned an invalid task handle: {error}"),
                );
            }
        },
        _ => return outcome,
    };
    let attachments = match steering_attachments(repository, key) {
        Ok(attachments) => attachments,
        Err(failure) => return failure,
    };
    if attachments.is_empty() {
        return outcome;
    }
    let created_at = match current_instant() {
        Ok(at) => at,
        Err(failure) => return failure,
    };
    let state = match repository.admit_state() {
        Ok(state) => state,
        Err(error) => return repository_state_failure(&error),
    };
    let store = artifact::Store::new(state.artifacts(), state.locks().join("task-artifacts.lock"));
    for (index, attachment) in attachments.iter().enumerate() {
        let bytes = match decode_attachment(attachment) {
            Ok(bytes) => bytes,
            Err(failure) => return failure,
        };
        let name = task_steering_attachment_name(key.as_str(), index, attachment);
        if let Err(error) = store.add_bytes(artifact::AddBytesRequest {
            task: &task,
            bytes: &bytes,
            name: &name,
            media_type: Some(&attachment.content_type),
            retention: artifact::Retention::Permanent,
            created_at,
        }) {
            return artifact_operation_failure(error);
        }
    }
    outcome
}

fn steering_origin_key(
    origin: Option<&wire::Origin>,
) -> Result<Option<steer::Key>, Outcome<CommandAnswer>> {
    let Some(origin) = origin else {
        return Ok(None);
    };
    authored_provenance(origin)?
        .steering_key()
        .map(|key| {
            key.parse::<steer::Key>().map_err(|error| {
                refusal(
                    "task::steering-origin-invalid",
                    Some(origin.to_string()),
                    &error.to_string(),
                )
            })
        })
        .transpose()
}

fn steering_attachments(
    repository: Host<'_>,
    key: &steer::Key,
) -> Result<Vec<wire::body::DirectiveAttachment>, Outcome<CommandAnswer>> {
    let state = repository
        .admit_state()
        .map_err(|error| repository_state_failure(&error))?;
    let reader = journal::JournalReader::open(state.journal()).map_err(|error| {
        store_fault(
            "task::steering-journal-unreadable",
            &format!("cannot inspect steering origin {key}: {error}"),
        )
    })?;
    let head = reader
        .read(|read| read.steering_key(key.as_str()))
        .map_err(|error| {
            store_fault(
                "task::steering-index-unreadable",
                &format!("cannot inspect steering origin {key}: {error}"),
            )
        })?;
    Ok(head.map_or_else(Vec::new, |head| head.body().attachments.clone()))
}

fn decode_attachment(
    attachment: &wire::body::DirectiveAttachment,
) -> Result<Vec<u8>, Outcome<CommandAnswer>> {
    let prefix = format!("data:{};base64,", attachment.content_type);
    let Some(payload) = attachment.data_url.strip_prefix(&prefix) else {
        return Err(protocol_fault(
            "task::steering-attachment-invalid",
            &format!(
                "{} does not carry its declared data URL prefix",
                attachment.name
            ),
        ));
    };
    base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|error| {
            protocol_fault(
                "task::steering-attachment-invalid",
                &format!("{} does not decode as base64: {error}", attachment.name),
            )
        })
}

pub(in crate::board_command) fn artifact_operation_failure(
    error: artifact::Error,
) -> Outcome<CommandAnswer> {
    match error.class() {
        artifact::ErrorClass::Refusal => refusal(
            "artifact::operation-refused",
            Some(error.to_string()),
            &crate::task_artifact_command::artifact_refusal_detail(&error),
        ),
        artifact::ErrorClass::Corrupt => store_fault("artifact::store-corrupt", &error.to_string()),
        artifact::ErrorClass::Environment => {
            environment_fault("artifact::store-unavailable", &error.to_string())
        }
    }
}

pub(in crate::board_command) fn repository_state_failure(
    error: &spice::repository::StateError,
) -> Outcome<CommandAnswer> {
    match error {
        spice::repository::StateError::Filesystem { .. } => {
            environment_fault("repository::state-path-unavailable", &error.to_string())
        }
        spice::repository::StateError::Linked { .. }
        | spice::repository::StateError::NotDirectory { .. }
        | spice::repository::StateError::NotRegularFile { .. } => refusal(
            "repository::state-path-refused",
            Some(error.to_string()),
            &format!(
                "repair the malformed state component at {}",
                error.path().display()
            ),
        ),
    }
}

pub(crate) fn submit_taste_proposal(
    repository: Host<'_>,
    contract: &Contract,
    body: TasteProposalAdd,
) -> Outcome<CommandAnswer> {
    for attempt in 0..2 {
        let outcome = mutate_from_board(
            repository,
            contract,
            BoardRead::All { family: None },
            |board| {
                let task = allocate_task_stamp(&board, jiff::Timestamp::now().as_microsecond())?;
                Ok((
                    CommandKind::TasteProposalAdd,
                    vec![Witness::Absence { task }],
                    serde_json::to_value(body.clone()).expect("typed intent encodes"),
                ))
            },
        );
        if attempt == 0
            && matches!(
                &outcome,
                Outcome::Refusal(refusal)
                    if matches!(
                        refusal.reason(),
                        "board::witness-moved" | "taste::proposal-predecessor-moved"
                    )
            )
        {
            continue;
        }
        return outcome;
    }
    unreachable!("taste proposal retry loop always returns")
}

pub(super) fn mutate_task<F>(
    repository: Host<'_>,
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

pub(super) fn mutate_task_from_board<F>(
    repository: Host<'_>,
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
    let (task, read) = match mutation_read(contract, matches) {
        Ok(read) => read,
        Err(outcome) => return outcome,
    };
    let task = task.expect("generated mutation syntax requires a task handle");
    mutate_from_board(repository, contract, read, |board| {
        let heads = named_task_heads(&board, &task)?;
        map(&board, heads.statement.body.handle.clone(), heads)
    })
}

pub(in crate::board_command) fn mutate_from_board<F>(
    repository: Host<'_>,
    contract: &Contract,
    read: BoardRead,
    map: F,
) -> Outcome<CommandAnswer>
where
    F: FnOnce(BoardAnswer) -> Result<ClientMapping, Outcome<CommandAnswer>>,
{
    let board = match current_board(repository, read) {
        Ok(board) => board,
        Err(outcome) => return outcome,
    };
    let (kind, expected, body) = match map(board) {
        Ok(mapped) => mapped,
        Err(outcome) => return outcome,
    };
    crate::command_client::execute_typed(repository, contract, kind, expected, &body)
}

pub(crate) fn current_board(
    repository: Host<'_>,
    read: BoardRead,
) -> Result<BoardAnswer, Outcome<CommandAnswer>> {
    let contract = crate::cli::contract(&["board"]).expect("board read contract is registered");
    match crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::BoardRead,
        Vec::new(),
        &read,
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

/// Read only the mutation's named identities and required graph context.
/// New followups still need the complete retained birth-stamp inventory.
pub(in crate::board_command) fn mutation_read(
    contract: &Contract,
    matches: &ArgMatches,
) -> Result<(Option<TaskHandle>, BoardRead), Outcome<CommandAnswer>> {
    let task = matches
        .get_one::<String>("HANDLE")
        .map(|written| {
            written.parse::<TaskHandle>().map_err(|error| {
                refusal(
                    "task::handle-invalid",
                    Some(written.clone()),
                    &error.to_string(),
                )
            })
        })
        .transpose()?;
    if matches
        .try_get_many::<String>("then")
        .ok()
        .flatten()
        .is_some()
    {
        return Ok((task, BoardRead::All { family: None }));
    }
    let mut tasks: Vec<_> = task
        .iter()
        .map(|task| {
            task.born()
                .expect("a parsed handle carries a canonical task stamp")
        })
        .collect();
    // Plan progress witnesses inspect prerequisite heads, including settled ones.
    let mut dependencies = matches!(contract.path, ["task", "done" | "review"]);
    // Every handle an option names joins the scoped read; a preparation
    // controller is otherwise absent from it.
    for (argument, invalid, graph) in [
        ("AFTER", "task::dependency-invalid", true),
        ("followup", "task::handle-invalid", true),
        ("preparation", "preparation::controller-invalid", false),
    ] {
        for written in matches
            .try_get_many::<String>(argument)
            .ok()
            .flatten()
            .into_iter()
            .flatten()
        {
            let named = written.parse::<TaskHandle>().map_err(|error| {
                refusal(
                    invalid,
                    (argument != "AFTER").then(|| written.clone()),
                    &error.to_string(),
                )
            })?;
            tasks.push(
                named
                    .born()
                    .expect("a parsed handle carries a canonical task stamp"),
            );
            dependencies |= graph;
        }
    }
    // A contribution's cited task handle or bare stamp joins the read so its
    // statement head resolves; a stamp no task carries names a publication.
    for written in matches
        .try_get_many::<String>("cite")
        .ok()
        .flatten()
        .into_iter()
        .flatten()
    {
        if let Ok(named) = written.parse::<TaskHandle>() {
            tasks.push(
                named
                    .born()
                    .expect("a parsed handle carries a canonical task stamp"),
            );
        } else if let Ok(stamp) = written.parse::<wire::TaskStamp>() {
            tasks.push(stamp);
        }
    }
    let claims = tasks.is_empty();
    // A disposition's covering task joins the read so its statement can be
    // witnessed; a cover that is not a handle names a commit instead.
    if let Some(cover) = matches
        .try_get_one::<String>("cover")
        .ok()
        .flatten()
        .and_then(|written| written.parse::<TaskHandle>().ok())
    {
        tasks.push(
            cover
                .born()
                .expect("a parsed handle carries a canonical task stamp"),
        );
    }
    Ok((
        task,
        BoardRead::Tasks {
            claims,
            tasks,
            dependencies,
            history: false,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authored_origins_refuse_every_other_spelling_naming_the_canonical_form() {
        for (written, reason, canonical) in [
            (
                "steering:1kLGp2NJ",
                "task::origin-noncanonical",
                Some("ack:1kLGp2NJ"),
            ),
            (
                "steer:1kLGp2NJ",
                "task::origin-noncanonical",
                Some("ack:1kLGp2NJ"),
            ),
            ("gate:complexity", "task::origin-invalid", None),
            ("operator:draft", "task::origin-invalid", None),
        ] {
            let origin: wire::Origin = written.parse().expect("fixture origin");
            let failure = steering_origin_key(Some(&origin)).expect_err("origin refuses");
            let Outcome::Refusal(refusal) = &failure else {
                panic!("{written} did not refuse: {failure:?}")
            };
            assert_eq!(refusal.reason(), reason, "{written}");
            if let Some(canonical) = canonical {
                assert!(refusal.explanation().contains(canonical), "{written}");
            }
        }

        let canonical: wire::Origin = "ack:1kLGp2NJ".parse().expect("canonical origin");
        let key = steering_origin_key(Some(&canonical))
            .expect("canonical origin is admitted")
            .expect("canonical origin carries a steering key");
        assert_eq!(key.as_str(), "1kLGp2NJ");
        let parent: wire::Origin = "task:CARDS-1kRYkKfc".parse().expect("parent origin");
        assert_eq!(
            steering_origin_key(Some(&parent)).expect("parent admitted"),
            None
        );
    }
}
