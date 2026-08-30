//! Task completion and review client flows.

use super::*;
use notify::Watcher as _;

pub(super) fn client_done(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let board = match current_board(repository) {
        Ok(board) => board,
        Err(outcome) => return outcome,
    };
    let (task, heads) = match sole_active(&board) {
        Ok(active) => active,
        Err(outcome) => return outcome,
    };
    let (lane, occupant) = match actor_seat(&board) {
        Ok(seat) => seat,
        Err(outcome) => return outcome,
    };
    let context = completion_learning_context(&board, heads, &task, lane, occupant);
    let expected = match progress_witnesses(&board, heads, lane, occupant, false) {
        Ok(expected) => expected,
        Err(outcome) => return outcome,
    };
    let validation = matches
        .get_one::<String>("validation")
        .cloned()
        .unwrap_or_else(|| "completed by the authenticated lane".to_owned());
    let body = serde_json::to_value(TaskDone { task, validation }).expect("typed intent encodes");
    let mut outcome = crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::TaskDone,
        expected,
        &body,
    );
    if let Outcome::Reply(reply) = &outcome
        && let CommandAnswer::Task(answer) = &reply.body
        && answer.completion.is_some()
    {
        let claim = heads
            .claim
            .as_ref()
            .expect("a completion operation starts only under one claim")
            .body
            .claimed;
        let terminal = match crate::gate_command::resolution_terminal_path(repository, claim) {
            Ok(path) => path,
            Err(error) => {
                return environment_fault(
                    "task::completion-terminal-unavailable",
                    &error.to_string(),
                );
            }
        };
        if let Err(waited) = wait_for_completion_terminal(&terminal) {
            return waited;
        }
        return client_done(repository, contract, matches);
    }
    if let Outcome::Reply(reply) = &mut outcome
        && let CommandAnswer::Task(answer) = &mut reply.body
        && answer.completion.is_none()
    {
        let learning = match context {
            Ok(mut context) => match wire::Instant::new(jiff::Timestamp::now()) {
                Ok(done_at) => {
                    context.done_at = done_at.to_string();
                    crate::session_command::learnings::best_effort(
                        repository,
                        &crate::session_environment(),
                        context,
                    )
                }
                Err(error) => crate::session_command::learnings::unavailable(
                    "completion_clock_invalid",
                    &error.to_string(),
                ),
            },
            Err(detail) => {
                crate::session_command::learnings::unavailable("missing_claim_metadata", &detail)
            }
        };
        answer.learning = Some(Box::new(learning));
    }
    outcome
}

fn wait_for_completion_terminal(path: &std::path::Path) -> Result<(), Outcome<CommandAnswer>> {
    if completion_terminal_ready(path)? {
        return Ok(());
    }
    let parent = path.parent().ok_or_else(|| {
        environment_fault(
            "task::completion-terminal-unavailable",
            "completion terminal has no parent directory",
        )
    })?;
    let (events, receiver) = std::sync::mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = events.send(event);
    })
    .map_err(|error| environment_fault("task::completion-watch-unavailable", &error.to_string()))?;
    watcher
        .watch(parent, notify::RecursiveMode::NonRecursive)
        .map_err(|error| {
            environment_fault("task::completion-watch-unavailable", &error.to_string())
        })?;
    if completion_terminal_ready(path)? {
        return Ok(());
    }
    loop {
        match receiver.recv() {
            Ok(Ok(_)) if completion_terminal_ready(path)? => return Ok(()),
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                return Err(environment_fault(
                    "task::completion-watch-failed",
                    &error.to_string(),
                ));
            }
            Err(error) => {
                return Err(environment_fault(
                    "task::completion-watch-closed",
                    &error.to_string(),
                ));
            }
        }
    }
}

fn completion_terminal_ready(path: &std::path::Path) -> Result<bool, Outcome<CommandAnswer>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_file() => {
            Err(environment_fault(
                "task::completion-terminal-unreadable",
                "completion terminal is not a real regular file",
            ))
        }
        Ok(metadata) => Ok(metadata.len() != 0),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(environment_fault(
            "task::completion-terminal-unreadable",
            &error.to_string(),
        )),
    }
}

fn completion_learning_context(
    board: &BoardAnswer,
    heads: &BoardTask,
    task: &TaskHandle,
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
) -> Result<crate::session_command::learnings::CompletionContext, String> {
    let lease = heads
        .lease
        .as_ref()
        .ok_or_else(|| "active claim has no lease start".to_owned())?;
    let seat = board
        .residency
        .body
        .lanes
        .iter()
        .find(|seat| seat.lane == lane && seat.occupant == Some(occupant))
        .ok_or_else(|| "active claim has no exact occupied lane session".to_owned())?;
    let session = seat
        .session
        .as_ref()
        .ok_or_else(|| "active lane has no vendor session identity".to_owned())?;
    Ok(crate::session_command::learnings::CompletionContext {
        source_task: task.to_string(),
        project_stem: crate::session_command::learnings::project_stem(
            &heads.statement.body.project,
        )?,
        claim_started_at: lease.body.start.to_string(),
        done_at: String::new(),
        thread_id: session.identity.clone(),
    })
}

pub(super) fn client_review(
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

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "spice-completion-watch-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4().simple(),
            ));
            std::fs::create_dir(&root).expect("create completion-watch scratch");
            Self(root)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).expect("remove completion-watch scratch");
        }
    }

    #[test]
    fn completion_wait_returns_on_the_terminal_publication_event() {
        let scratch = Scratch::new();
        let terminal = scratch.0.join("terminal.bin");
        std::fs::write(&terminal, b"").expect("create empty terminal");
        let published = terminal.clone();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let staged = published.with_extension("staged");
            std::fs::write(&staged, b"terminal").expect("stage terminal");
            std::fs::rename(staged, published).expect("publish terminal");
        });
        wait_for_completion_terminal(&terminal).expect("terminal event wakes completion");
        worker.join().expect("terminal publisher joins");
        assert!(completion_terminal_ready(&terminal).expect("terminal remains readable"));
    }
}
