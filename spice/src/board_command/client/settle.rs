//! Operator settlement: withdrawal, closure, and Design conclusion.

use super::*;

/// Map the operator settlement leaves to their typed commands.
pub(super) fn client_settle(
    repository: Host<'_>,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    if contract.path == ["task", "conclude"] {
        client_conclude(repository, contract, matches)
    } else {
        client_delete(repository, contract, matches)
    }
}

fn client_delete(
    repository: Host<'_>,
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

/// Conclude a Design done, or collapsed with the stated reason.
fn client_conclude(
    repository: Host<'_>,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let conclusion = match (
        matches.get_flag("done"),
        matches.get_one::<String>("collapse").cloned(),
    ) {
        (true, None) => body::DesignConclusion::Done {},
        (false, Some(reason)) => body::DesignConclusion::Collapsed { reason },
        _ => {
            return refusal(
                "task::conclusion-shape",
                None,
                "name exactly one of --done or --collapse REASON",
            );
        }
    };
    mutate_task_from_board(repository, contract, matches, |_board, task, heads| {
        // Law::CLOSE witnesses the claim head, so a claim landing between
        // this read and the door refuses the conclusion.
        Ok((
            CommandKind::TaskConclude,
            vec![
                statement_witness(heads),
                workflow_witness(heads),
                claim_witness(heads),
            ],
            serde_json::to_value(TaskConclude { task, conclusion }).expect("typed intent encodes"),
        ))
    })
}

/// Dispatch one operator settlement: withdrawal, closure, or Design conclusion.
pub(super) fn dispatch_settlement(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Option<Outcome<CommandAnswer>> {
    Some(match command.body() {
        wire::intent::Intent::TaskDelete(delete) => {
            task_delete(bus, command, delete, projects, allocation)
        }
        wire::intent::Intent::TaskClose(close) => {
            task_close(bus, command, close, projects, allocation)
        }
        wire::intent::Intent::TaskConclude(conclude) => {
            task_conclude(bus, command, conclude, projects, allocation)
        }
        _ => return None,
    })
}
