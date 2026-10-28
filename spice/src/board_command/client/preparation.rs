use std::marker::PhantomData;

use serde::de::DeserializeOwned;
use wire::intent::{BoardAnswer, BoardRead, BoardTask};
use wire::rooms::{
    CandidateRef, CandidateReference, CandidateWithdraw, CandidateWithdrawal, DesignPassBody,
    DesignPassProposal, DesignPassRecommendation, DesignPropose, OutputAdopt, PreparationDecide,
    PreparationDecision, ProposalRef, RoomAuthorityStamp,
};

use super::*;
use crate::board_command::preparation::claim_design_remedy;

pub(in crate::board_command) mod assemble;
#[cfg(test)]
mod tests;

use assemble::{PassRecord, assemble, pass_record};

/// One canonical-JSON command operand: its argument name and a complete body
/// the command accepts. A malformed body refuses with that body beside the
/// parse detail, so the shape is learnable from the refusal alone.
struct JsonOperand<T> {
    name: &'static str,
    example: &'static str,
    body: PhantomData<fn() -> T>,
}

impl<T: DeserializeOwned> JsonOperand<T> {
    const fn new(name: &'static str, example: &'static str) -> Self {
        Self {
            name,
            example,
            body: PhantomData,
        }
    }

    fn decode(&self, text: &str) -> Result<T, Outcome<CommandAnswer>> {
        wire::decode(text.as_bytes()).map_err(|error| {
            refusal(
                "preparation::argument-invalid",
                Some(self.name.to_owned()),
                &format!(
                    "{error}; a complete {} this command accepts: {}",
                    self.name, self.example
                ),
            )
        })
    }

    fn argument(&self, matches: &ArgMatches) -> Result<T, Outcome<CommandAnswer>> {
        self.decode(&required(matches, self.name))
    }
}

const PROPOSE_DRY_RUN: &str =
    "dry run: nothing recorded; the write mints its own candidate identity";
const DECISION_JSON: JsonOperand<PreparationDecision> = JsonOperand::new(
    "DECISION_JSON",
    r#"{"decision":{"controller":"dVzdhh8Nx","decision":"1"},"source":{"kind":"journal","fact":"6dth"},"outcome":{"kind":"resolved","candidate":{"controller":"dVzdhh8Nx","candidate":"dVzfC0000"},"outputs":[{"output":"dVzfwWPSh","verdict":"accepted","comment":"This boundary is ready."},{"output":"dVzfwgs7d","verdict":"rejected","comment":"Keep the retry contract explicit."}],"comment":"Proceed with the accepted output."}}"#,
);
const WITHDRAWAL_JSON: JsonOperand<CandidateWithdrawal> = JsonOperand::new(
    "WITHDRAWAL_JSON",
    r#"{"candidate":{"controller":"dVzdhh8Nx","candidate":"dVzfC0000"},"reason":"cites another Design's facts; candidate dVzfC0001 corrects it"}"#,
);

pub(super) fn dispatch_client(
    repository: Host<'_>,
    contract: &Contract,
    matches: &ArgMatches,
    verb: &str,
) -> Outcome<CommandAnswer> {
    match verb {
        "propose" => client_design_propose(repository, contract, matches),
        "decide" => client_preparation_decide(repository, contract, matches),
        "withdraw" => client_candidate_withdraw(repository, contract, matches),
        _ => unreachable!("design CLI partition is closed"),
    }
}

pub(super) fn client_design_propose(
    repository: Host<'_>,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let named = match matches.get_one::<String>("design") {
        Some(written) => match written.parse::<TaskHandle>() {
            Ok(handle) => Some(handle),
            Err(error) => return invalid_handle("--design", &error.to_string()),
        },
        None => None,
    };
    let recommend = matches
        .get_one::<String>("recommend")
        .map(|written| match written.as_str() {
            "conclude" => DesignPassRecommendation::Conclude,
            "collapse" => DesignPassRecommendation::Collapse,
            _ => unreachable!("clap admits only conclude or collapse"),
        });
    let mut written_cites = Vec::new();
    for written in matches.get_many::<String>("cite").into_iter().flatten() {
        match cite_pair(written) {
            Ok(pair) => written_cites.push(pair),
            Err(outcome) => return outcome,
        }
    }
    let board = match current_board(repository, BoardRead::Open) {
        Ok(board) => board,
        Err(outcome) => return outcome,
    };
    let design = match held_design(&board, named.as_ref()) {
        Ok(design) => design,
        Err(outcome) => return outcome,
    };
    let born = design.statement.body.born.clone();
    let mut cites = Vec::new();
    for (relation, key) in written_cites {
        match super::room::cite(&board, &born, key) {
            Ok(source) => cites.push(CandidateReference { relation, source }),
            Err(outcome) => return outcome,
        }
    }
    let (record, room_head) = match read_pass_record(repository, &born, &mut cites) {
        Ok(read) => read,
        Err(outcome) => return outcome,
    };
    let proposal = match assemble(
        design,
        &board.tasks,
        &record,
        recommend,
        cites,
        required(matches, "JUSTIFICATION"),
    )
    .and_then(|pass| derive_proposal(design, pass, mint_candidate()?))
    {
        Ok(proposal) => proposal,
        Err(outcome) => return outcome,
    };
    if matches.get_flag("dry-run") {
        let json = serde_json::to_string_pretty(&proposal).expect("a proposal encodes");
        return Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(board.cursor),
            body: CommandAnswer::Text(TextAnswer {
                text: format!("{PROPOSE_DRY_RUN}\n{json}"),
            }),
        });
    }
    // The proposal releases the holder's claim at its task Git boundary,
    // which resolves the holder's worktree from this witness.
    let (lane, occupant) = match actor_seat(&board) {
        Ok(seat) => seat,
        Err(outcome) => return outcome,
    };
    let mut expected = vec![
        statement_witness(design),
        workflow_witness(design),
        claim_witness(design),
    ];
    for stamp in proposal.governed_inputs.iter().chain(&proposal.outputs) {
        match task(&board, stamp) {
            Ok(heads) => {
                expected.push(statement_witness(heads));
                expected.push(workflow_witness(heads));
            }
            Err(outcome) => return outcome,
        }
    }
    if let Err(outcome) = borrowed_acceptance_witnesses(repository, design, &mut expected) {
        return outcome;
    }
    expected.push(Witness::Proposal {
        proposal: ProposalRef::Candidate {
            candidate: proposal.candidate.clone(),
        },
        head: None,
    });
    expected.push(Witness::Room {
        room: born.clone(),
        head: room_head,
    });
    expected.push(residency_witness(&board, lane, occupant));
    let outcome = crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::DesignPropose,
        expected,
        &DesignPropose {
            candidate: proposal,
        },
    );
    with_cycle(repository, &born, outcome)
}

/// Push the witnesses for the linked work a Design without its own
/// acceptance borrows one from. The open board carries settled prerequisites
/// only as readiness evidence and no settled dependents, so the lenders come
/// from a read of the Design's own dependency scope.
fn borrowed_acceptance_witnesses(
    repository: Host<'_>,
    design: &BoardTask,
    expected: &mut Vec<Witness>,
) -> Result<(), Outcome<CommandAnswer>> {
    if substantive_acceptance(&design.statement.body.acceptance) {
        return Ok(());
    }
    let born = &design.statement.body.born;
    let scope = current_board(
        repository,
        BoardRead::Tasks {
            tasks: vec![born.clone()],
            dependencies: true,
            claims: false,
            history: false,
        },
    )?;
    acceptance_witnesses(&scope, task(&scope, born)?, expected)
}

/// One `--cite RELATION=KEY` split into its relation word and its key, which
/// resolves the way `room contribute --cite` resolves it.
fn cite_pair(written: &str) -> Result<(wire::Name, &str), Outcome<CommandAnswer>> {
    let Some((relation, key)) = written.split_once('=') else {
        return Err(refusal(
            "preparation::cite-invalid",
            Some(written.to_owned()),
            "cite as RELATION=KEY, such as evidence=dWcNrYRX0",
        ));
    };
    let relation = relation.parse::<wire::Name>().map_err(|error| {
        refusal(
            "preparation::cite-invalid",
            Some(written.to_owned()),
            &error.to_string(),
        )
    })?;
    Ok((relation, key))
}

/// [`pass_record`] for `design`, read from the repository journal.
fn read_pass_record(
    repository: Host<'_>,
    design: &TaskStamp,
    cites: &mut [CandidateReference],
) -> Result<(PassRecord, Option<Seq>), Outcome<CommandAnswer>> {
    journal_reader(repository, &format!("Design {design}"))?
        .read(|read| pass_record(read, design, cites))
        .map_err(|error| {
            store_fault(
                "preparation::authority-unreadable",
                &format!("cannot read Design {design}'s room and decisions: {error}"),
            )
        })
}

/// The open Design whose pass the caller is ending: the one it names, or the
/// sole open Design it holds an active claim on.
pub(in crate::board_command) fn held_design<'a>(
    board: &'a BoardAnswer,
    named: Option<&TaskHandle>,
) -> Result<&'a BoardTask, Outcome<CommandAnswer>> {
    let held_by_caller = |heads: &BoardTask| {
        heads.claim.as_ref().is_some_and(|claim| {
            claim.body.actor == board.principal
                && claim.body.disposition == wire::ClaimDisposition::Active
        })
    };
    if let Some(named) = named {
        let born = named
            .born()
            .expect("a parsed handle carries a canonical task stamp");
        let heads = task(board, &born)?;
        if !held_by_caller(heads) {
            return Err(refusal(
                "preparation::design-unheld",
                Some(named.to_string()),
                &claim_design_remedy(Some(named)),
            ));
        }
        return Ok(heads);
    }
    let mut held = board.tasks.iter().filter(|heads| {
        held_by_caller(heads)
            && wire::intent::is_open_design(heads.statement.body.intent, &heads.workflow.body)
    });
    let Some(heads) = held.next() else {
        return Err(refusal(
            "preparation::design-unheld",
            Some(board.principal.to_string()),
            &claim_design_remedy(None),
        ));
    };
    if let Some(other) = held.next() {
        return Err(refusal(
            "preparation::design-ambiguous",
            Some(format!(
                "{} {}",
                heads.statement.body.handle, other.statement.body.handle
            )),
            "name the Design this pass ends with `--design HANDLE`",
        ));
    }
    Ok(heads)
}

/// The proposal `pass` makes under `design`, witnessing the statement and
/// active claim heads the client read so a move between this read and the
/// resident's admission still refuses there.
pub(in crate::board_command) fn derive_proposal(
    design: &BoardTask,
    pass: DesignPassBody,
    candidate: RoomAuthorityStamp,
) -> Result<DesignPassProposal, Outcome<CommandAnswer>> {
    let claim = design.claim.as_ref().ok_or_else(|| {
        refusal(
            "preparation::design-unheld",
            Some(design.statement.body.handle.to_string()),
            &claim_design_remedy(Some(&design.statement.body.handle)),
        )
    })?;
    Ok(DesignPassProposal::witnessing(
        CandidateRef {
            controller: design.statement.body.born.clone(),
            candidate,
        },
        design.statement.position,
        claim.position,
        pass,
    ))
}

/// A fresh candidate identity: the current microsecond, which the resident's
/// absence witness proves unused under the Design.
fn mint_candidate() -> Result<RoomAuthorityStamp, Outcome<CommandAnswer>> {
    RoomAuthorityStamp::for_micros(jiff::Timestamp::now().as_microsecond()).map_err(|error| {
        environment_fault(
            "preparation::candidate-clock-invalid",
            &format!("the current clock mints no candidate identity: {error}"),
        )
    })
}

pub(super) fn client_preparation_decide(
    repository: Host<'_>,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let decision = match DECISION_JSON.argument(matches) {
        Ok(decision) => decision,
        Err(outcome) => return outcome,
    };
    let (candidate, candidate_head) =
        match retained_candidate(repository, decision.outcome.candidate()) {
            Ok(candidate) => candidate,
            Err(outcome) => return outcome,
        };
    let accepts_outputs = decision.outcome.accepted_outputs().next().is_some();
    let design = decision.decision.controller.clone();
    let mut tasks = vec![design.clone()];
    if accepts_outputs {
        tasks.extend(candidate.governed_inputs.iter().cloned());
        tasks.extend(decision.outcome.accepted_outputs().cloned());
    }
    let outcome = submit_with_tasks(repository, contract, tasks, true, |board| {
        let mut expected = transition_witnesses(task(board, &decision.decision.controller)?);
        expected.push(Witness::Proposal {
            proposal: ProposalRef::Decision {
                decision: decision.decision.clone(),
            },
            head: None,
        });
        expected.push(Witness::Proposal {
            proposal: ProposalRef::Candidate {
                candidate: candidate.candidate.clone(),
            },
            head: Some(candidate_head),
        });
        if accepts_outputs {
            for input in &candidate.governed_inputs {
                expected.push(statement_witness(task(board, input)?));
            }
            for output in decision.outcome.accepted_outputs() {
                expected.extend(transition_witnesses(task(board, output)?));
            }
        }
        Ok((
            CommandKind::PreparationDecide,
            expected,
            PreparationDecide { decision },
        ))
    });
    with_cycle(repository, &design, outcome)
}

pub(super) fn client_candidate_withdraw(
    repository: Host<'_>,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let withdrawal = match WITHDRAWAL_JSON.argument(matches) {
        Ok(withdrawal) => withdrawal,
        Err(outcome) => return outcome,
    };
    let candidate = ProposalRef::Candidate {
        candidate: withdrawal.candidate.clone(),
    };
    let withdrawn = ProposalRef::Withdrawal {
        candidate: withdrawal.candidate.clone(),
    };
    let (candidate_head, withdrawn_head) =
        match proposal_heads(repository, &withdrawal.candidate, &candidate, &withdrawn) {
            Ok(heads) => heads,
            Err(outcome) => return outcome,
        };
    let Some(candidate_head) = candidate_head else {
        return refusal(
            "preparation::candidate-missing",
            Some(withdrawal.candidate.candidate.to_string()),
            "the exact proposal is missing or withdrawn",
        );
    };
    let design = withdrawal.candidate.controller.clone();
    let tasks = vec![design.clone()];
    let outcome = submit_with_tasks(repository, contract, tasks, false, |board| {
        let mut expected = task_witnesses(board, &withdrawal.candidate.controller)?;
        expected.push(Witness::Proposal {
            proposal: candidate,
            head: Some(candidate_head),
        });
        expected.push(Witness::Proposal {
            proposal: withdrawn,
            head: withdrawn_head,
        });
        Ok((
            CommandKind::CandidateWithdraw,
            expected,
            CandidateWithdraw { withdrawal },
        ))
    });
    with_cycle(repository, &design, outcome)
}

/// Current heads of two proposal subjects, read at one journal position.
fn proposal_heads(
    repository: Host<'_>,
    candidate: &CandidateRef,
    first: &ProposalRef,
    second: &ProposalRef,
) -> Result<(Option<Seq>, Option<Seq>), Outcome<CommandAnswer>> {
    let reader = journal_reader(repository, &format!("candidate {}", candidate.candidate))?;
    reader
        .read(|read| {
            Ok::<_, journal::GuardError>((
                read.proposal_head(first)?.map(|head| head.position()),
                read.proposal_head(second)?.map(|head| head.position()),
            ))
        })
        .map_err(|error| {
            store_fault(
                "preparation::authority-unreadable",
                &format!("cannot inspect candidate {}: {error}", candidate.candidate),
            )
        })
}

/// The repository journal, read-only, for inspecting `subject`.
fn journal_reader(
    repository: Host<'_>,
    subject: &str,
) -> Result<journal::JournalReader, Outcome<CommandAnswer>> {
    let state = repository
        .admit_state()
        .map_err(|error| repository_state_failure(&error))?;
    journal::JournalReader::open(state.journal()).map_err(|error| {
        store_fault(
            "preparation::journal-unreadable",
            &format!("cannot inspect {subject}: {error}"),
        )
    })
}

fn retained_candidate(
    repository: Host<'_>,
    candidate: &CandidateRef,
) -> Result<(DesignPassProposal, Seq), Outcome<CommandAnswer>> {
    let reader = journal_reader(repository, &format!("candidate {}", candidate.candidate))?;
    let subject = ProposalRef::Candidate {
        candidate: candidate.clone(),
    };
    let head = reader
        .read(|read| read.proposal_head(&subject))
        .map_err(|error| {
            store_fault(
                "preparation::authority-unreadable",
                &format!("cannot inspect candidate {}: {error}", candidate.candidate),
            )
        })?
        .ok_or_else(|| {
            refusal(
                "preparation::candidate-missing",
                Some(candidate.candidate.to_string()),
                "record the exact proposal before deciding it",
            )
        })?;
    let position = head.position();
    match head.into_body() {
        journal::ProposalAuthority::Candidate(candidate) => Ok((candidate, position)),
        _ => Err(store_fault(
            "preparation::candidate-invalid",
            "candidate authority decoded as another proposal family",
        )),
    }
}

pub(super) fn client_output_adopt(
    repository: Host<'_>,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let output = required(matches, "OUTPUT")
        .parse::<TaskHandle>()
        .map_err(|error| invalid_handle("OUTPUT", &error.to_string()));
    let preparation = required(matches, "PREPARATION")
        .parse::<TaskHandle>()
        .map_err(|error| invalid_handle("PREPARATION", &error.to_string()));
    let (output, preparation) = match (output, preparation) {
        (Ok(output), Ok(preparation)) => (output, preparation),
        (Err(outcome), _) | (_, Err(outcome)) => return outcome,
    };
    let stamps = match (output.born(), preparation.born()) {
        (Ok(output), Ok(preparation)) => vec![output, preparation],
        (Err(error), _) | (_, Err(error)) => {
            return invalid_handle("task handle", &error.to_string());
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

/// Append `design`'s pass cycle, read after the command committed, to a
/// text reply, so the reply names the state and action the command left.
/// A failed read leaves the committed reply as the resident wrote it.
fn with_cycle(
    repository: Host<'_>,
    design: &TaskStamp,
    outcome: Outcome<CommandAnswer>,
) -> Outcome<CommandAnswer> {
    let Outcome::Reply(Reply {
        position,
        body: CommandAnswer::Text(TextAnswer { mut text }),
    }) = outcome
    else {
        return outcome;
    };
    let read = BoardRead::Tasks {
        tasks: vec![design.clone()],
        dependencies: false,
        claims: false,
        history: false,
    };
    if let Ok(board) = current_board(repository, read)
        && let Some(task) = board
            .tasks
            .iter()
            .find(|task| task.statement.body.born == *design)
        && let Some(cycle) = design_cycle(&board, task)
    {
        for line in cycle.lines() {
            text.push('\n');
            text.push_str(&line);
        }
        text.push_str(&format!("\nnext: {}", cycle.action));
    }
    Outcome::Reply(Reply {
        position,
        body: CommandAnswer::Text(TextAnswer { text }),
    })
}

fn invalid_handle(name: &str, detail: &str) -> Outcome<CommandAnswer> {
    refusal(
        "preparation::argument-invalid",
        Some(name.to_owned()),
        &format!("name a current task handle as `spice task show` prints it: {detail}"),
    )
}

fn submit_with_tasks<B, F>(
    repository: Host<'_>,
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
            history: false,
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

fn task_witnesses(
    board: &BoardAnswer,
    controller: &TaskStamp,
) -> Result<Vec<Witness>, Outcome<CommandAnswer>> {
    let mut expected = board
        .tasks
        .iter()
        .flat_map(|task| [statement_witness(task), workflow_witness(task)])
        .collect::<Vec<_>>();
    expected.push(claim_witness(task(board, controller)?));
    Ok(expected)
}

fn transition_witnesses(task: &BoardTask) -> Vec<Witness> {
    vec![
        statement_witness(task),
        workflow_witness(task),
        claim_witness(task),
    ]
}
