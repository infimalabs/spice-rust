//! The operator's decision on one Design pass proposal: the accepted outputs
//! it authorizes, the rejected outputs it closes, or the conclusion an
//! outputless recommendation commits.
use super::*;

/// What one decision commits beyond its own immutable record.
enum Consequence {
    /// A work resolution authorizes exactly its accepted outputs and closes
    /// its rejected ones with their pending subtrees.
    Resolve(Resolution),
    /// A return changes no task head.
    Record(&'static str),
    /// An accepted outputless recommendation concludes its Design.
    Conclude {
        design: TaskHandle,
        conclusion: body::DesignConclusion,
        action: &'static str,
    },
}

/// The accepted output heads one decision releases and the authorization
/// that releases them.
type Authorization = (Vec<TaskHead>, ExecutionAuthorization);

/// The heads a work resolution rewrites: accepted outputs lose their edge to
/// the Design under one authorization; rejected outputs close.
struct Resolution {
    authorization: Option<Authorization>,
    rejected: Vec<TaskHandle>,
}

pub(super) fn decide_preparation(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request_body: &PreparationDecide,
) -> Outcome<CommandAnswer> {
    let decision = &request_body.decision;
    let subject = ProposalRef::Decision {
        decision: decision.decision.clone(),
    };
    if let Err(outcome) = require_proposal_absence(command, &subject) {
        return outcome;
    }
    let candidate_ref = decision.outcome.candidate();
    let snapshot = match bus.task_snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => return store_fault("preparation::heads-unreadable", &error.to_string()),
    };
    let candidate = match proposed_candidate(bus, command, &decision.outcome) {
        Ok(candidate) => candidate,
        Err(outcome) => return outcome,
    };
    let controller = match task_heads(&snapshot, &candidate_ref.controller) {
        Ok(heads) => heads,
        Err(outcome) => return outcome,
    };
    if let Err(outcome) = require_open_design(command, controller, &candidate_ref.controller) {
        return outcome;
    }
    let consequence = match &decision.outcome {
        DecisionOutcome::Returned { .. } => Consequence::Record("returned the proposal"),
        DecisionOutcome::Resolved { outputs, .. } => {
            let (authorization, rejected) = match resolve_outputs(
                &snapshot, command, controller, decision, &candidate, outputs,
            ) {
                Ok(resolved) => resolved,
                Err(outcome) => return outcome,
            };
            Consequence::Resolve(Resolution {
                authorization,
                rejected,
            })
        }
        DecisionOutcome::Accepted { .. } => {
            let (conclusion, action) = match candidate.recommendation {
                wire::rooms::DesignPassRecommendation::Work => {
                    return refusal(
                        "preparation::decision-kind-mismatch",
                        Some(candidate_ref.candidate.to_string()),
                        "resolve every output of a work proposal instead of accepting it whole",
                    );
                }
                wire::rooms::DesignPassRecommendation::Conclude => {
                    (body::DesignConclusion::Done {}, "concluded the Design")
                }
                wire::rooms::DesignPassRecommendation::Collapse => (
                    body::DesignConclusion::Collapsed {
                        reason: candidate.justification.clone(),
                    },
                    "collapsed the Design",
                ),
            };
            Consequence::Conclude {
                design: TaskHandle::for_project(
                    &controller.statement().body().project,
                    &candidate_ref.controller,
                ),
                conclusion,
                action,
            }
        }
    };
    commit_decision(bus, command, request_body, candidate, consequence)
}

/// Match a work resolution against its exact proposal and derive the heads
/// it authorizes and the handles it closes.
fn resolve_outputs(
    snapshot: &bus::TaskSnapshot,
    command: &wire::AdmittedCommand,
    controller: &journal::TaskHeads,
    decision: &wire::rooms::PreparationDecision,
    candidate: &DesignPassProposal,
    outputs: &[wire::rooms::OutputDecision],
) -> Result<(Option<Authorization>, Vec<TaskHandle>), Outcome<CommandAnswer>> {
    let candidate_ref = decision.outcome.candidate();
    if candidate.recommendation != wire::rooms::DesignPassRecommendation::Work {
        return Err(refusal(
            "preparation::decision-kind-mismatch",
            Some(candidate_ref.candidate.to_string()),
            "resolve outputs only for a work proposal",
        ));
    }
    if outputs.len() != candidate.outputs.len()
        || outputs
            .iter()
            .zip(&candidate.outputs)
            .any(|(decision, candidate)| decision.output != *candidate)
    {
        return Err(refusal(
            "preparation::decision-output-mismatch",
            Some(candidate_ref.candidate.to_string()),
            "accept or reject every output of the exact proposal in shown order",
        ));
    }
    let accepted = decision
        .outcome
        .accepted_outputs()
        .cloned()
        .collect::<Vec<_>>();
    let rejected = rejected_output_handles(snapshot, &decision.outcome)?;
    let authorization = if accepted.is_empty() {
        None
    } else {
        Some(prepare_selection(
            snapshot, command, controller, decision, candidate, &accepted,
        )?)
    };
    Ok((authorization, rejected))
}

/// The exact retained Design pass proposal a decision disposes of.
fn proposed_candidate(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    outcome: &DecisionOutcome,
) -> Result<DesignPassProposal, Outcome<CommandAnswer>> {
    let candidate = outcome.candidate();
    let subject = ProposalRef::Candidate {
        candidate: candidate.clone(),
    };
    let read_subject = subject.clone();
    let withdrawal = ProposalRef::Withdrawal {
        candidate: candidate.clone(),
    };
    let (head, withdrawn) = bus
        .read(move |read| {
            Ok::<_, GuardError>((
                read.proposal_head(&read_subject)?,
                read.proposal_head(&withdrawal)?.is_some(),
            ))
        })
        .map_err(|error| store_fault("preparation::authority-unreadable", &error.to_string()))?;
    if let Some(head) = head {
        if withdrawn {
            return Err(refusal(
                "preparation::candidate-withdrawn",
                Some(candidate.candidate.to_string()),
                "its author withdrew this proposal; decide the corrected proposal instead",
            ));
        }
        require_witness(
            command,
            &Witness::Proposal {
                proposal: subject,
                head: Some(head.position()),
            },
        )?;
        return match head.into_body() {
            journal::ProposalAuthority::Candidate(candidate) => Ok(candidate),
            _ => Err(store_fault(
                "preparation::candidate-invalid",
                "proposal authority decoded as another proposal family",
            )),
        };
    }
    require_witness(
        command,
        &Witness::Proposal {
            proposal: subject,
            head: None,
        },
    )?;
    Err(refusal(
        "preparation::candidate-missing",
        Some(candidate.candidate.to_string()),
        "record the exact Design pass proposal before deciding it",
    ))
}

fn prepare_selection(
    snapshot: &bus::TaskSnapshot,
    command: &wire::AdmittedCommand,
    controller: &journal::TaskHeads,
    decision: &wire::rooms::PreparationDecision,
    candidate: &DesignPassProposal,
    selected_outputs: &[TaskStamp],
) -> Result<(Vec<TaskHead>, ExecutionAuthorization), Outcome<CommandAnswer>> {
    require_unblocked_selection(snapshot, controller)?;
    require_input_witnesses(snapshot, command, candidate)?;
    let outputs = selected_output_heads(
        snapshot,
        command,
        &candidate.candidate,
        candidate,
        selected_outputs,
    )?;
    require_acyclic_selection(
        snapshot,
        &candidate.candidate.controller,
        &outputs,
        decision,
    )?;
    let authorization = ExecutionAuthorization {
        decision: decision.decision.clone(),
        candidate: candidate.candidate.clone(),
        governed_inputs: candidate.governed_inputs.clone(),
        selected_outputs: selected_outputs.to_vec(),
        coverage: Vec::new(),
    };
    Ok((outputs, authorization))
}

/// A released output becomes the Design's prerequisite, so an accepted output
/// that transitively waits on pending work this Design still gates would
/// wait on the Design that waits on it. Outputs this decision settles, by
/// acceptance or rejection, no longer gate anything; any other gated task on
/// the chain refuses the whole decision and names the chain.
fn require_acyclic_selection(
    snapshot: &bus::TaskSnapshot,
    controller: &TaskStamp,
    outputs: &[TaskHead],
    decision: &wire::rooms::PreparationDecision,
) -> Result<(), Outcome<CommandAnswer>> {
    let settled_here = decision
        .outcome
        .accepted_outputs()
        .chain(decision.outcome.rejected_outputs())
        .collect::<Vec<_>>();
    for output in outputs {
        let mut chain = vec![output.born.clone()];
        let mut pending = output
            .dependencies
            .iter()
            .filter(|dependency| *dependency != controller)
            .map(|dependency| (dependency.clone(), 1usize))
            .collect::<Vec<_>>();
        let mut visited = Vec::new();
        while let Some((task, depth)) = pending.pop() {
            if visited.contains(&task) {
                continue;
            }
            visited.push(task.clone());
            let Some(heads) = snapshot
                .tasks()
                .iter()
                .find(|heads| heads.statement().body().born == task)
            else {
                continue;
            };
            if !heads.workflow().body().status.is_pending() {
                continue;
            }
            chain.truncate(depth);
            chain.push(task.clone());
            let statement = heads.statement().body();
            if statement.dependencies.contains(controller) && !settled_here.contains(&&task) {
                return Err(refusal(
                    "preparation::selection-cycle",
                    Some(
                        chain
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(" -> "),
                    ),
                    "the accepted output waits on work this Design still gates; accept or reject that work in the same decision, or unlink it first",
                ));
            }
            pending.extend(
                statement
                    .dependencies
                    .iter()
                    .map(|dependency| (dependency.clone(), depth + 1)),
            );
        }
    }
    Ok(())
}

/// Every rejected output of a work resolution, as the handle its closure
/// names.
fn rejected_output_handles(
    snapshot: &bus::TaskSnapshot,
    outcome: &DecisionOutcome,
) -> Result<Vec<TaskHandle>, Outcome<CommandAnswer>> {
    outcome
        .rejected_outputs()
        .map(|output| {
            let heads = task_heads(snapshot, output)?;
            Ok(TaskHandle::for_project(
                &heads.statement().body().project,
                output,
            ))
        })
        .collect()
}

fn require_input_witnesses(
    snapshot: &bus::TaskSnapshot,
    command: &wire::AdmittedCommand,
    candidate: &DesignPassProposal,
) -> Result<(), Outcome<CommandAnswer>> {
    for input in &candidate.governed_inputs {
        let heads = task_heads(snapshot, input)?;
        require_witness(
            command,
            &Witness::Task {
                task: input.clone(),
                head: heads.statement().position(),
            },
        )?;
    }
    Ok(())
}

fn selected_output_heads(
    snapshot: &bus::TaskSnapshot,
    command: &wire::AdmittedCommand,
    candidate_ref: &wire::rooms::CandidateRef,
    candidate: &DesignPassProposal,
    selected_outputs: &[TaskStamp],
) -> Result<Vec<TaskHead>, Outcome<CommandAnswer>> {
    if let Some(output) = selected_outputs
        .iter()
        .find(|output| !candidate.outputs.contains(output))
    {
        return Err(refusal(
            "preparation::selection-foreign-output",
            Some(output.to_string()),
            "accept only outputs named by the exact Design pass proposal",
        ));
    }
    selected_outputs
        .iter()
        .map(|output| selected_output_head(snapshot, command, candidate_ref, output))
        .collect()
}

fn selected_output_head(
    snapshot: &bus::TaskSnapshot,
    command: &wire::AdmittedCommand,
    candidate: &wire::rooms::CandidateRef,
    output: &TaskStamp,
) -> Result<TaskHead, Outcome<CommandAnswer>> {
    let heads = task_heads(snapshot, output)?;
    require_transition_witnesses(command, heads)?;
    if !output_is_authorizable(heads, candidate) {
        return Err(refusal(
            "preparation::output-not-authorizable",
            Some(output.to_string()),
            "select a pending, unclaimed output that still depends on this Design",
        ));
    }
    Ok(heads.statement().body().clone())
}

fn output_is_authorizable(
    heads: &journal::TaskHeads,
    candidate: &wire::rooms::CandidateRef,
) -> bool {
    heads.workflow().body().status.is_pending()
        && !active_claim(heads)
        && heads
            .statement()
            .body()
            .dependencies
            .contains(&candidate.controller)
}
fn commit_decision(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request_body: &PreparationDecide,
    candidate: DesignPassProposal,
    consequence: Consequence,
) -> Outcome<CommandAnswer> {
    let decision = &request_body.decision;
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(&decision.decision.controller) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    let decided = match write.fact(
        PreparationDecided {
            decision: decision.clone(),
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("preparation::decision-invalid", &error.to_string()),
    };
    let action = match consequence {
        Consequence::Resolve(resolution) => {
            let validation = format!(
                "Rejected by decision {} of candidate {}",
                decision.decision.decision, candidate.candidate.candidate
            );
            match stage_resolution(
                bus,
                command,
                &mut write,
                decided,
                at,
                &validation,
                resolution,
            ) {
                Ok(action) => action,
                Err(outcome) => return outcome,
            }
        }
        Consequence::Record(action) => action.to_owned(),
        Consequence::Conclude {
            design,
            conclusion,
            action,
        } => {
            let closed = match crate::board_command::mutation::stage_conclusion(
                bus,
                command,
                &TaskConclude {
                    task: design,
                    conclusion,
                },
                crate::board_command::mutation::DecisionWrite {
                    write: &mut write,
                    decided,
                    at,
                },
            ) {
                Ok(closed) => closed,
                Err(outcome) => return outcome,
            };
            if closed.is_empty() {
                action.to_owned()
            } else {
                format!(
                    "{action} and closed {}",
                    closed
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
    };
    let answer = TextAnswer {
        text: format!(
            "{action} through decision {} of candidate {}",
            decision.decision.decision, candidate.candidate.candidate
        ),
    };
    if let Err(error) = write.command_result(
        request(command, request_body.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("preparation::decision-receipt-invalid", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Text(answer))
}

/// Stage a work resolution's head rewrites under its decision fact and
/// describe what it did.
fn stage_resolution(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    write: &mut JournalWrite,
    decided: PendingFact,
    at: wire::Instant,
    validation: &str,
    resolution: Resolution,
) -> Result<String, Outcome<CommandAnswer>> {
    let Resolution {
        authorization,
        rejected,
    } = resolution;
    let mut parts = Vec::with_capacity(2);
    if let Some((outputs, authorization)) = authorization {
        let authorized = write
            .fact(
                ExecutionAuthorized {
                    authorization: authorization.clone(),
                },
                FactCause::Pending(decided),
            )
            .map_err(|error| {
                store_fault("preparation::authorization-invalid", &error.to_string())
            })?;
        parts.push(format!("selected {} output(s)", outputs.len()));
        for mut output in outputs {
            let controller = authorization.decision.controller.clone();
            output
                .dependencies
                .retain(|dependency| dependency != &controller);
            write.task_head(authorized, move |revision| TaskHead { revision, ..output });
        }
    }
    if !rejected.is_empty() {
        let closed = crate::board_command::mutation::stage_rejection(
            bus,
            command,
            &rejected,
            validation,
            crate::board_command::mutation::DecisionWrite { write, decided, at },
        )?;
        if !closed.is_empty() {
            parts.push(format!(
                "closed {}",
                closed
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    Ok(if parts.is_empty() {
        "resolved every output as already settled".to_owned()
    } else {
        parts.join(" and ")
    })
}
