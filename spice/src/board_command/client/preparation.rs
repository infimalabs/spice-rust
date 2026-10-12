use serde::Deserialize;
use serde::de::DeserializeOwned;
use wire::intent::{BoardAnswer, BoardRead, BoardTask};
use wire::rooms::{
    CandidateCoverage, CandidateCoverageRecord, CandidateCritique, CandidateCritiqueRecord,
    CandidateRetain, CoverageAuthority, ExecutionAuthorization, ExecutionAuthorize, OutputAdopt,
    PreparationDecide, PreparationDecision, ProposalRef, RetainedCandidate, RoomSource,
};

use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CoverageInput {
    coverage: wire::rooms::CoverageRef,
    scopes: Vec<Name>,
    source: RoomSource,
}
pub(super) fn dispatch_client(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
    verb: &str,
) -> Outcome<CommandAnswer> {
    match verb {
        "retain" => client_candidate_retain(repository, contract, matches),
        "critique" => client_candidate_critique(repository, contract, matches),
        "cover" => client_candidate_cover(repository, contract, matches),
        "decide" => client_preparation_decide(repository, contract, matches),
        "authorize" => client_execution_authorize(repository, contract, matches),
        "adopt" => client_output_adopt(repository, contract, matches),
        _ => unreachable!("preparation CLI partition is closed"),
    }
}

pub(super) fn client_candidate_retain(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let candidate: RetainedCandidate = match json_argument(matches, "CANDIDATE_JSON") {
        Ok(candidate) => candidate,
        Err(outcome) => return outcome,
    };
    let tasks = std::iter::once(candidate.candidate.controller.clone())
        .chain(candidate.governed_inputs.iter().cloned())
        .chain(candidate.outputs.iter().cloned())
        .collect::<Vec<_>>();
    submit_with_tasks(repository, contract, tasks, false, |board| {
        let proposal = ProposalRef::Candidate {
            candidate: candidate.candidate.clone(),
        };
        let mut expected = task_witnesses(board)?;
        expected.push(Witness::Proposal {
            proposal,
            head: None,
        });
        Ok((
            CommandKind::CandidateRetain,
            expected,
            CandidateRetain { candidate },
        ))
    })
}

pub(super) fn client_candidate_critique(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let critique: CandidateCritique = match json_argument(matches, "CRITIQUE_JSON") {
        Ok(critique) => critique,
        Err(outcome) => return outcome,
    };
    let tasks = vec![critique.critique.candidate.controller.clone()];
    submit_with_tasks(repository, contract, tasks, false, |board| {
        let proposal = ProposalRef::Critique {
            critique: critique.critique.clone(),
        };
        let mut expected = task_witnesses(board)?;
        expected.push(Witness::Proposal {
            proposal,
            head: None,
        });
        Ok((
            CommandKind::CandidateCritiqueRecord,
            expected,
            CandidateCritiqueRecord { critique },
        ))
    })
}

pub(super) fn client_candidate_cover(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let input: CoverageInput = match json_argument(matches, "COVERAGE_JSON") {
        Ok(input) => input,
        Err(outcome) => return outcome,
    };
    let tasks = vec![input.coverage.candidate.controller.clone()];
    submit_with_tasks(repository, contract, tasks, false, |board| {
        let proposal = ProposalRef::Coverage {
            coverage: input.coverage.clone(),
        };
        let mut expected = task_witnesses(board)?;
        expected.push(Witness::Proposal {
            proposal,
            head: None,
        });
        Ok((
            CommandKind::CandidateCoverageRecord,
            expected,
            CandidateCoverageRecord {
                coverage: CandidateCoverage {
                    coverage: input.coverage,
                    authority: CoverageAuthority::Operator {},
                    scopes: input.scopes,
                    source: input.source,
                },
            },
        ))
    })
}

pub(super) fn client_preparation_decide(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let decision: PreparationDecision = match json_argument(matches, "DECISION_JSON") {
        Ok(decision) => decision,
        Err(outcome) => return outcome,
    };
    let tasks = vec![decision.decision.controller.clone()];
    submit_with_tasks(repository, contract, tasks, false, |board| {
        let proposal = ProposalRef::Decision {
            decision: decision.decision.clone(),
        };
        let mut expected = task_witnesses(board)?;
        expected.push(Witness::Proposal {
            proposal,
            head: None,
        });
        Ok((
            CommandKind::PreparationDecide,
            expected,
            PreparationDecide { decision },
        ))
    })
}

pub(super) fn client_execution_authorize(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let authorization: ExecutionAuthorization = match json_argument(matches, "AUTHORIZATION_JSON") {
        Ok(authorization) => authorization,
        Err(outcome) => return outcome,
    };
    let tasks = std::iter::once(authorization.candidate.controller.clone())
        .chain(authorization.governed_inputs.iter().cloned())
        .chain(authorization.selected_outputs.iter().cloned())
        .collect::<Vec<_>>();
    submit_with_tasks(repository, contract, tasks, true, |board| {
        let mut expected = Vec::new();
        let controller = task(board, &authorization.candidate.controller)?;
        expected.extend(transition_witnesses(controller));
        for input in &authorization.governed_inputs {
            expected.push(statement_witness(task(board, input)?));
        }
        for output in &authorization.selected_outputs {
            expected.extend(transition_witnesses(task(board, output)?));
            expected.push(Witness::Authorization {
                output: output.clone(),
                head: None,
            });
        }
        Ok((
            CommandKind::ExecutionAuthorize,
            expected,
            ExecutionAuthorize { authorization },
        ))
    })
}

pub(super) fn client_output_adopt(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let output = required(matches, "OUTPUT")
        .parse::<TaskHandle>()
        .map_err(|error| invalid_json("OUTPUT", &error.to_string()));
    let preparation = required(matches, "PREPARATION")
        .parse::<TaskHandle>()
        .map_err(|error| invalid_json("PREPARATION", &error.to_string()));
    let (output, preparation) = match (output, preparation) {
        (Ok(output), Ok(preparation)) => (output, preparation),
        (Err(outcome), _) | (_, Err(outcome)) => return outcome,
    };
    let stamps = match (output.born(), preparation.born()) {
        (Ok(output), Ok(preparation)) => vec![output, preparation],
        (Err(error), _) | (_, Err(error)) => {
            return invalid_json("task handle", &error.to_string());
        }
    };
    submit_with_tasks(repository, contract, stamps, true, |board| {
        let output_stamp = output.born().expect("validated");
        let preparation_stamp = preparation.born().expect("validated");
        let mut expected = transition_witnesses(task(board, &output_stamp)?);
        expected.extend(transition_witnesses(task(board, &preparation_stamp)?));
        Ok((
            CommandKind::OutputAdopt,
            expected,
            OutputAdopt {
                output,
                preparation,
            },
        ))
    })
}

fn submit_with_tasks<B, F>(
    repository: &Repository,
    contract: &Contract,
    mut tasks: Vec<TaskStamp>,
    claims: bool,
    map: F,
) -> Outcome<CommandAnswer>
where
    B: serde::Serialize,
    F: FnOnce(&BoardAnswer) -> Result<(CommandKind, Vec<Witness>, B), Outcome<CommandAnswer>>,
{
    tasks.sort();
    tasks.dedup();
    let board = match current_board(
        repository,
        BoardRead::Tasks {
            tasks,
            dependencies: false,
            claims,
            notes: false,
        },
    ) {
        Ok(board) => board,
        Err(outcome) => return outcome,
    };
    let (kind, expected, body) = match map(&board) {
        Ok(mapped) => mapped,
        Err(outcome) => return outcome,
    };
    crate::command_client::execute_typed(repository, contract, kind, expected, &body)
}

fn task<'a>(
    board: &'a BoardAnswer,
    stamp: &TaskStamp,
) -> Result<&'a BoardTask, Outcome<CommandAnswer>> {
    board
        .tasks
        .iter()
        .find(|task| task.statement.body.born == *stamp)
        .ok_or_else(|| {
            refusal(
                "preparation::task-unavailable",
                Some(stamp.to_string()),
                "read the current board and name an existing visible task",
            )
        })
}

fn task_witnesses(board: &BoardAnswer) -> Result<Vec<Witness>, Outcome<CommandAnswer>> {
    board
        .tasks
        .iter()
        .map(|task| Ok(statement_witness(task)))
        .collect()
}

fn transition_witnesses(task: &BoardTask) -> Vec<Witness> {
    vec![
        statement_witness(task),
        workflow_witness(task),
        claim_witness(task),
    ]
}

fn json_argument<T: DeserializeOwned>(
    matches: &ArgMatches,
    name: &str,
) -> Result<T, Outcome<CommandAnswer>> {
    wire::decode(required(matches, name).as_bytes())
        .map_err(|error| invalid_json(name, &error.to_string()))
}

fn invalid_json(name: &str, detail: &str) -> Outcome<CommandAnswer> {
    refusal(
        "preparation::argument-invalid",
        Some(name.to_owned()),
        &format!("supply the documented canonical value: {detail}"),
    )
}
