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
    let validation = matches
        .get_one::<String>("validation")
        .cloned()
        .unwrap_or_else(|| "completed by the authenticated lane".to_owned());
    let board = match current_board(repository, read.clone()) {
        Ok(board) => board,
        Err(outcome) => return outcome,
    };
    let first = match completion_plan(
        &board,
        requested_task.as_ref(),
        operation.clone(),
        validation.clone(),
    ) {
        Ok(plan) => plan,
        Err(outcome) => return outcome,
    };
    let outcome = submit_completion(repository, contract, first.expected.clone(), &first.done);
    let refusal_reason = match &outcome {
        Outcome::Refusal(refusal)
            if matches!(
                refusal.reason(),
                "board::witness-moved" | "claim::generation-stale"
            ) =>
        {
            refusal.reason()
        }
        _ => return outcome,
    };

    let refreshed = match current_board(repository, read) {
        Ok(board) => board,
        Err(_) => return outcome,
    };
    let retry = match completion_plan(&refreshed, requested_task.as_ref(), operation, validation) {
        Ok(plan) => plan,
        Err(_) => return outcome,
    };
    if own_edit_is_only_completion_movement(&first, &retry) {
        return submit_completion(repository, contract, retry.expected, &retry.done);
    }
    if refusal_reason == "claim::generation-stale"
        && !retry.holder_acknowledged
        && task_witness_moved(&first, &retry)
    {
        return refusal(
            "board::witness-moved",
            Some(retry.done.task.to_string()),
            "read the current task before retrying completion",
        );
    }
    outcome
}

struct CompletionPlan {
    task: wire::TaskStamp,
    expected: Vec<Witness>,
    done: TaskDone,
    holder_acknowledged: bool,
}

fn completion_plan(
    board: &BoardAnswer,
    requested_task: Option<&TaskHandle>,
    operation: Option<wire::CanonicalUuid>,
    validation: String,
) -> Result<CompletionPlan, Outcome<CommandAnswer>> {
    let implicit = requested_task.is_none();
    let (task, heads) = match requested_task {
        Some(task) => {
            let heads = named_task_heads(board, task)?;
            (heads.statement.body.handle.clone(), heads)
        }
        None => sole_active(board)?,
    };
    let claim = heads.claim.as_ref();
    let active = claim.is_some_and(|claim| {
        claim.body.actor == board.principal
            && claim.body.disposition == wire::ClaimDisposition::Active
    });
    if !active && implicit {
        return Err(refusal(
            "claim::active-missing",
            Some(board.principal.to_string()),
            "name the completed task to replay its terminal completion",
        ));
    }
    let (lane, occupant) = actor_seat(board)?;
    let expected = progress_witnesses(board, heads, lane, occupant, false)?;
    let operation = match (operation, claim) {
        (Some(operation), _) => operation,
        (None, Some(claim)) => TaskDone::operation_for_claim(claim.body.claimed),
        (None, None) => {
            return Err(refusal(
                "task::completion-operation-unavailable",
                Some(task.to_string()),
                "name an explicit operation when the task no longer retains a claim head",
            ));
        }
    };
    let holder_acknowledged = claim.is_some_and(|claim| {
        claim.body.actor == board.principal
            && claim.body.disposition == wire::ClaimDisposition::Active
            && claim.body.acknowledged == heads.workflow.body.generation
    });
    Ok(CompletionPlan {
        task: heads.statement.body.born.clone(),
        expected,
        done: TaskDone {
            task,
            validation,
            operation: Some(operation),
        },
        holder_acknowledged,
    })
}

fn submit_completion(
    repository: &Repository,
    contract: &Contract,
    expected: Vec<Witness>,
    done: &TaskDone,
) -> Outcome<CommandAnswer> {
    let body = serde_json::to_value(done).expect("typed intent encodes");
    crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::TaskDone,
        expected,
        &body,
    )
}

/// A holder's own edit is the only stale completion read safe to refresh.
///
/// The journal advances that holder's acknowledgement with the edit. A foreign
/// edit leaves it behind. Every witness outside the edited task's statement
/// and workflow must remain exact, so a dependency, lease, residency, or other
/// concurrent movement still returns the first `board::witness-moved`.
fn own_edit_is_only_completion_movement(before: &CompletionPlan, after: &CompletionPlan) -> bool {
    after.holder_acknowledged && only_task_witnesses_moved(before, after)
}

fn only_task_witnesses_moved(before: &CompletionPlan, after: &CompletionPlan) -> bool {
    task_witness_moved(before, after)
        && stable_completion_witnesses(&before.expected, &before.task)
            .eq(stable_completion_witnesses(&after.expected, &after.task))
}

fn task_witness_moved(before: &CompletionPlan, after: &CompletionPlan) -> bool {
    if before.task != after.task
        || before.done.validation != after.done.validation
        || before.done.operation != after.done.operation
    {
        return false;
    }
    let before_head = task_head(&before.expected, &before.task);
    let after_head = task_head(&after.expected, &after.task);
    before_head.is_some() && after_head.is_some() && before_head != after_head
}
#[cfg(test)]
pub(in crate::board_command) fn completion_retryable_after_task_change(
    before: &BoardAnswer,
    after: &BoardAnswer,
    task: &TaskHandle,
) -> bool {
    let before = completion_plan(before, Some(task), None, "fixture validation".to_owned());
    let after = completion_plan(after, Some(task), None, "fixture validation".to_owned());
    matches!(
        (before, after),
        (Ok(before), Ok(after)) if own_edit_is_only_completion_movement(&before, &after)
    )
}
#[cfg(test)]
pub(in crate::board_command) fn completion_foreign_task_change(
    before: &BoardAnswer,
    after: &BoardAnswer,
    task: &TaskHandle,
) -> bool {
    let before = completion_plan(before, Some(task), None, "fixture validation".to_owned());
    let after = completion_plan(after, Some(task), None, "fixture validation".to_owned());
    matches!(
        (before, after),
        (Ok(before), Ok(after))
            if !after.holder_acknowledged && task_witness_moved(&before, &after)
    )
}

fn task_head(expected: &[Witness], task: &wire::TaskStamp) -> Option<wire::Seq> {
    expected.iter().find_map(|witness| match witness {
        Witness::Task {
            task: subject,
            head,
        } if subject == task => Some(*head),
        _ => None,
    })
}

fn stable_completion_witnesses<'a>(
    expected: &'a [Witness],
    task: &'a wire::TaskStamp,
) -> impl Iterator<Item = &'a Witness> + 'a {
    expected.iter().filter(move |witness| {
        !matches!(
            witness,
            Witness::Task { task: subject, .. } | Witness::Workflow { task: subject, .. }
                if subject == task
        )
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
