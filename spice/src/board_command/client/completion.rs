//! Task completion and review client flows.

use super::*;
pub(super) fn client_done(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let (requested_task, read) = match mutation_read(contract, matches) {
        Ok(read) => read,
        Err(outcome) => return outcome,
    };
    let board = match current_board(repository, read) {
        Ok(board) => board,
        Err(outcome) => return outcome,
    };
    let written_operation = matches.get_one::<String>("operation");
    let operation = match written_operation {
        Some(written) => match written.parse::<wire::CanonicalUuid>() {
            Ok(operation) => Some(operation),
            Err(error) => {
                return refusal(
                    "task::completion-operation-invalid",
                    Some(written.clone()),
                    &error.to_string(),
                );
            }
        },
        None => None,
    };
    let implicit = requested_task.is_none();
    let (task, heads) = match requested_task {
        Some(task) => {
            let heads = match named_task_heads(&board, &task) {
                Ok(heads) => heads,
                Err(outcome) => return outcome,
            };
            (heads.statement.body.handle.clone(), heads)
        }
        None => match sole_active(&board) {
            Ok(active) => active,
            Err(outcome) => return outcome,
        },
    };
    let active = heads.claim.as_ref().is_some_and(|claim| {
        claim.body.actor == board.principal
            && claim.body.disposition == wire::ClaimDisposition::Active
    });
    if !active && implicit {
        return refusal(
            "claim::active-missing",
            Some(board.principal.to_string()),
            "name the completed task to replay its terminal completion",
        );
    }
    let (lane, occupant) = match actor_seat(&board) {
        Ok(seat) => seat,
        Err(outcome) => return outcome,
    };
    let expected = match progress_witnesses(&board, heads, lane, occupant, false) {
        Ok(expected) => expected,
        Err(outcome) => return outcome,
    };
    let validation = matches
        .get_one::<String>("validation")
        .cloned()
        .unwrap_or_else(|| "completed by the authenticated lane".to_owned());
    let operation = match (operation, heads.claim.as_ref()) {
        (Some(operation), _) => operation,
        (None, Some(claim)) => TaskDone::operation_for_claim(claim.body.claimed),
        (None, None) => {
            return refusal(
                "task::completion-operation-unavailable",
                Some(task.to_string()),
                "name an explicit operation when the task no longer retains a claim head",
            );
        }
    };
    let body = serde_json::to_value(TaskDone {
        task,
        validation,
        operation: Some(operation),
    })
    .expect("typed intent encodes");
    crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::TaskDone,
        expected,
        &body,
    )
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
