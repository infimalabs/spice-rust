// Authored-document client operations kept beside the native board client.

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
