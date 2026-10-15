//! Task mutation submission and steering-origin artifact attachment.

use super::*;
use base64::Engine as _;

pub(super) fn client_add(
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
    repository: &Repository,
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
            notes: false,
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
    let pending = match spice::task_git::pending_recovery(repository) {
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
    repository: &Repository,
    contract: &Contract,
    task: TaskHandle,
    stealing: bool,
) -> Outcome<CommandAnswer> {
    let dirty = match spice::task_git::has_dirty_work(repository) {
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
        notes: false,
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
    repository: &Repository,
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
    let dirty = match spice::task_git::has_dirty_work(repository) {
        Ok(dirty) => dirty,
        Err(error) => return task_git_outcome(error),
    };
    let written_title = matches.get_one::<String>("title").cloned();
    let candidate = if dirty && written_title.is_some() {
        None
    } else {
        match spice::task_git::capture(repository) {
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
    let body = TaskAdd {
        preparation: None,
        intent: None,
        title,
        project: Some(project),
        description: matches.get_one::<String>("description").cloned(),
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
            project: matches.get_one::<String>("project").cloned(),
        },
    )
}

type ClientMapping = (CommandKind, Vec<Witness>, serde_json::Value);

pub(super) fn submit_oops_add(
    repository: &Repository,
    contract: &Contract,
    body: TaskOopsAdd,
) -> Outcome<CommandAnswer> {
    mutate_from_board(
        repository,
        contract,
        BoardRead::All { family: None },
        |board| {
            let stamp = allocate_task_stamp(&board, jiff::Timestamp::now().as_microsecond())?;
            Ok((
                CommandKind::TaskOopsAdd,
                vec![Witness::Absence { task: stamp }],
                serde_json::to_value(body).expect("typed intent encodes"),
            ))
        },
    )
}

pub(crate) fn submit_task_add(
    repository: &Repository,
    contract: &Contract,
    body: TaskAdd,
) -> Outcome<CommandAnswer> {
    submit_task_add_with_rule(repository, contract, body, "capture")
}

pub(super) fn submit_recovery_task_add(
    repository: &Repository,
    contract: &Contract,
    body: TaskAdd,
) -> Outcome<CommandAnswer> {
    submit_task_add_with_rule(repository, contract, body, "recover")
}

fn submit_captured_task(
    repository: &Repository,
    contract: &Contract,
    body: TaskAdd,
) -> Outcome<CommandAnswer> {
    submit_task_add_with_rule(repository, contract, body, CAPTURE_WORK_RULE)
}

fn submit_task_add_with_rule(
    repository: &Repository,
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
    repository: &Repository,
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
    let text = origin.as_str();
    if let Some(key) = text
        .strip_prefix("steering:")
        .or_else(|| text.strip_prefix("steer:"))
    {
        return Err(refusal(
            "task::steering-origin-noncanonical",
            Some(text.to_owned()),
            &format!("use --origin ack:{key}"),
        ));
    }
    text.strip_prefix("ack:")
        .map(|key| {
            key.parse::<steer::Key>().map(Some).map_err(|error| {
                refusal(
                    "task::steering-origin-invalid",
                    Some(text.to_owned()),
                    &error.to_string(),
                )
            })
        })
        .unwrap_or(Ok(None))
}

fn steering_attachments(
    repository: &Repository,
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
    repository: &Repository,
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

pub(super) fn mutate_task_from_board<F>(
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
    repository: &Repository,
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
    repository: &Repository,
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
    // Every handle an option names joins the scoped read; a typed OOPS report
    // or grooming controller is otherwise absent from it.
    for (argument, invalid, graph) in [
        ("AFTER", "task::dependency-invalid", true),
        ("followup", "task::handle-invalid", true),
        ("report", "room::oops-report-invalid", false),
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
    Ok((
        task,
        BoardRead::Tasks {
            claims: tasks.is_empty(),
            tasks,
            dependencies,
            notes: false,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steering_origin_admission_refuses_aliases_and_accepts_ack() {
        for written in ["steering:1kLGp2NJ", "steer:1kLGp2NJ"] {
            let origin: wire::Origin = written.parse().expect("fixture origin");
            let failure =
                steering_origin_key(Some(&origin)).expect_err("noncanonical origin refuses");
            assert!(matches!(
                &failure,
                Outcome::Refusal(refusal)
                    if refusal.reason() == "task::steering-origin-noncanonical"
                        && refusal.explanation().contains("ack:1kLGp2NJ")
            ));
        }

        let canonical: wire::Origin = "ack:1kLGp2NJ".parse().expect("canonical origin");
        let key = steering_origin_key(Some(&canonical))
            .expect("canonical origin is admitted")
            .expect("canonical origin carries a steering key");
        assert_eq!(key.as_str(), "1kLGp2NJ");
    }
}
