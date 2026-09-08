//! Task mutation submission and steering-origin artifact attachment.

use super::*;
use base64::Engine as _;

type ClientMapping = (CommandKind, Vec<Witness>, serde_json::Value);

pub(super) fn submit_oops_add(
    repository: &Repository,
    contract: &Contract,
    body: TaskOopsAdd,
) -> Outcome<CommandAnswer> {
    mutate_from_board(repository, contract, |board| {
        let stamp = allocate_task_stamp(&board, jiff::Timestamp::now().as_microsecond());
        Ok((
            CommandKind::TaskOopsAdd,
            vec![Witness::Absence { task: stamp }],
            serde_json::to_value(body).expect("typed intent encodes"),
        ))
    })
}

pub(crate) fn submit_task_add(
    repository: &Repository,
    contract: &Contract,
    body: TaskAdd,
) -> Outcome<CommandAnswer> {
    let steering_origin = match steering_origin_key(body.origin.as_ref()) {
        Ok(key) => key,
        Err(outcome) => return outcome,
    };
    let outcome = mutate_from_board(repository, contract, |board| {
        let stamp = allocate_task_stamp(&board, jiff::Timestamp::now().as_microsecond());
        Ok((
            CommandKind::TaskAdd,
            vec![Witness::Absence { task: stamp }],
            serde_json::to_value(body).expect("typed intent encodes"),
        ))
    });
    attach_steering_origin_artifacts(repository, steering_origin.as_ref(), outcome)
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
        let name = steering_attachment_name(key, index, attachment);
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
    let key = text
        .strip_prefix("ack:")
        .or_else(|| text.strip_prefix("steer:"));
    key.map(|key| {
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
    let snapshot = reader.fact_snapshot().map_err(|error| {
        store_fault(
            "task::steering-snapshot-unreadable",
            &format!("cannot inspect steering origin {key}: {error}"),
        )
    })?;
    let ledger = steer::Ledger::from_snapshot(&snapshot).map_err(|error| {
        store_fault(
            "task::steering-ledger-invalid",
            &format!("cannot inspect steering origin {key}: {error}"),
        )
    })?;
    Ok(ledger
        .directives()
        .into_iter()
        .find(|directive| directive.key() == key)
        .map_or_else(Vec::new, |directive| directive.attachments().to_vec()))
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

fn steering_attachment_name(
    key: &steer::Key,
    index: usize,
    attachment: &wire::body::DirectiveAttachment,
) -> String {
    if attachment.name.is_empty() {
        format!(
            "steering-{key}-attachment-{}.{}",
            index + 1,
            attachment_extension(&attachment.content_type)
        )
    } else {
        attachment.name.clone()
    }
}

fn attachment_extension(content_type: &str) -> &'static str {
    match content_type {
        artifact::MediaType::IMAGE_JPEG => "jpg",
        artifact::MediaType::IMAGE_GIF => "gif",
        artifact::MediaType::IMAGE_WEBP => "webp",
        _ => "png",
    }
}

pub(super) fn artifact_operation_failure(error: artifact::Error) -> Outcome<CommandAnswer> {
    match error.class() {
        artifact::ErrorClass::Refusal => refusal(
            "artifact::operation-refused",
            Some(error.to_string()),
            "correct the task artifact request and retry",
        ),
        artifact::ErrorClass::Corrupt => store_fault("artifact::store-corrupt", &error.to_string()),
        artifact::ErrorClass::Environment => {
            environment_fault("artifact::store-unavailable", &error.to_string())
        }
    }
}

pub(super) fn repository_state_failure(
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
        let outcome = mutate_from_board(repository, contract, |board| {
            let task = allocate_task_stamp(&board, jiff::Timestamp::now().as_microsecond());
            Ok((
                CommandKind::TasteProposalAdd,
                vec![Witness::Absence { task }],
                serde_json::to_value(body.clone()).expect("typed intent encodes"),
            ))
        });
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
    mutate_from_board(repository, contract, |board| {
        let written = required(matches, "HANDLE");
        let task = written
            .parse::<TaskHandle>()
            .map_err(|error| refusal("task::handle-invalid", Some(written), &error.to_string()))?;
        let heads = task_heads(&board, &task)?;
        map(&board, task, heads)
    })
}

pub(super) fn mutate_from_board<F>(
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

pub(crate) fn current_board(
    repository: &Repository,
) -> Result<BoardAnswer, Outcome<CommandAnswer>> {
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
