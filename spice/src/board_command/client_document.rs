use super::*;

// Authored-document client operations kept beside the native board client.

pub(super) fn run_doc(
    repository: Host<'_>,
    contract: &Contract,
    matches: &ArgMatches,
    emit: fn(&dialect::Document) -> String,
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
    let normalized = emit(preview.document());
    let normalized = match board::Preview::classify(&normalized) {
        Ok(preview) => preview,
        Err(error) => {
            return protocol_fault(
                "doc::normal-form-invalid",
                &format!("dialect emitted a document it refused: {error}"),
            );
        }
    };
    // Document application witnesses a global barrier and allocates fresh births.
    let board = match current_board(repository, BoardRead::All { family: None }) {
        Ok(board) => board,
        Err(outcome) => return outcome,
    };
    let touched = match document_handles(
        normalized.document(),
        &board,
        jiff::Timestamp::now().as_microsecond(),
    ) {
        Ok(touched) => touched,
        Err(outcome) => return outcome,
    };
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

pub(super) fn document_handles(
    document: &dialect::Document,
    board: &BoardAnswer,
    epoch_micros: i64,
) -> Result<Vec<TaskHandle>, Outcome<CommandAnswer>> {
    let project = document.project.as_deref().unwrap_or("task.document");
    let existing = document
        .nodes
        .iter()
        .filter(|node| node.kind != dialect::NodeKind::SyntheticRoot)
        .map(|node| {
            board.tasks.iter().find_map(|task| {
                (task.statement.body.project == project
                    && task.statement.body.document_slug.as_deref() == Some(node.slug.as_str()))
                .then(|| task.statement.body.handle.clone())
            })
        })
        .collect::<Vec<_>>();
    let mut allocated = allocate_task_stamps(
        board,
        epoch_micros,
        existing.iter().filter(|handle| handle.is_none()).count(),
    )?
    .into_iter();
    Ok(existing
        .into_iter()
        .map(|handle| {
            handle.unwrap_or_else(|| {
                TaskHandle::for_project(
                    project,
                    &allocated
                        .next()
                        .expect("every new document node owns one stamp"),
                )
            })
        })
        .collect())
}

pub(super) fn board_barrier(board: &BoardAnswer) -> Digest {
    let cursor = board
        .tasks
        .iter()
        .flat_map(|task| [task.statement.position, task.workflow.position])
        .max();
    task_projection_barrier(cursor)
}

pub(super) fn task_projection_barrier(cursor: Option<wire::Seq>) -> Digest {
    let bytes = wire::encode(&cursor).expect("typed board projection cursor encodes");
    Digest::compute(Domain::BoardBarrier, &bytes)
}
