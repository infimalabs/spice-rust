use super::*;
use wire::rooms::{
    DecisionOutcome, DesignPassProposal, DesignPropose, DesignProposed, ExecutionAuthorization,
    ExecutionAuthorized, OutputAdopt, OutputControlled, PreparationDecide, PreparationDecided,
    ProposalRef,
};

mod decision;
mod references;
mod withdrawal;

pub(super) fn dispatch_preparation(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    repository: Option<Host<'_>>,
) -> Option<Outcome<CommandAnswer>> {
    Some(match command.body() {
        wire::intent::Intent::DesignPropose(proposal) => {
            propose_design(bus, command, proposal, repository)
        }
        wire::intent::Intent::PreparationDecide(decision) => {
            decision::decide_preparation(bus, command, decision)
        }
        wire::intent::Intent::OutputAdopt(adopt) => adopt_output(bus, command, adopt),
        wire::intent::Intent::CandidateWithdraw(withdraw) => {
            withdrawal::withdraw_candidate(bus, command, withdraw)
        }
        _ => return None,
    })
}

/// How a lane claims the open Design whose pass it means to propose.
///
/// Allocation hands out only a Design ready for proposal, while a Design still
/// gathering input is claimed by name, so the remedy names both routes.
pub(super) fn claim_design_remedy(design: Option<&TaskHandle>) -> String {
    match design {
        Some(design) => format!(
            "claim {design} before proposing its pass: `spice task next` takes it once it is ready for proposal, and `spice task capture {design}` claims it by name while it gathers input"
        ),
        None => "claim the open Design before proposing its pass: `spice task next` takes one ready for proposal, and `spice task capture HANDLE` claims an undeferred Design still gathering input by name".to_owned(),
    }
}

/// Admit the exact claimed Design and every gated output named by one pass.
fn require_design_proposal(
    snapshot: &bus::TaskSnapshot,
    command: &wire::AdmittedCommand,
    proposal: &DesignPassProposal,
) -> Result<(), Outcome<CommandAnswer>> {
    let controller = &proposal.candidate.controller;
    let heads = task_heads(snapshot, controller)?;
    require_open_design(command, heads, controller)?;
    if proposal.statement != heads.statement().position() {
        return Err(refusal(
            "preparation::proposal-statement-moved",
            Some(proposal.statement.to_string()),
            "read the current Design statement, revise the proposal, and retry",
        ));
    }
    let Some(proposal_claim) = proposal.claim else {
        return Err(refusal(
            "preparation::proposal-claim-missing",
            Some(controller.to_string()),
            "new Design pass proposals must name the exact active claim they release",
        ));
    };
    let Some(claim) = heads.claim() else {
        let statement = heads.statement().body();
        let handle = TaskHandle::for_filing(&statement.project, &statement.born, statement.filing);
        return Err(refusal(
            "preparation::proposal-unclaimed",
            Some(controller.to_string()),
            &claim_design_remedy(Some(&handle)),
        ));
    };
    if claim.position() != proposal_claim
        || claim.body().disposition != ClaimDisposition::Active
        || claim.body().actor != *command.actor()
    {
        return Err(refusal(
            "preparation::proposal-claim-moved",
            Some(proposal_claim.to_string()),
            "propose only from the exact active Design claim this capability owns",
        ));
    }
    if claim.body().is_stale(heads.workflow().body()) {
        return Err(refusal(
            "preparation::proposal-claim-stale",
            Some(controller.to_string()),
            "run `spice task next` to read and acknowledge the current Design generation, then retry",
        ));
    }
    for output in &proposal.outputs {
        let output_heads = task_heads(snapshot, output)?;
        require_statement_workflow_witnesses(command, output_heads)?;
        let statement = output_heads.statement().body();
        let gated_output =
            !statement.acceptance.is_empty() && statement.dependencies.contains(controller);
        if !gated_output {
            return Err(refusal(
                "preparation::proposal-output-invalid",
                Some(output.to_string()),
                "name only acceptance-bearing outputs gated by this Design",
            ));
        }
    }
    Ok(())
}

fn require_statement_workflow_witnesses(
    command: &wire::AdmittedCommand,
    heads: &journal::TaskHeads,
) -> Result<(), Outcome<CommandAnswer>> {
    for witness in [
        Witness::Task {
            task: heads.statement().body().born.clone(),
            head: heads.statement().position(),
        },
        Witness::Workflow {
            task: heads.statement().body().born.clone(),
            head: heads.workflow().position(),
            phase: heads.workflow().body().phase.clone(),
        },
    ] {
        require_witness(command, &witness)?;
    }
    Ok(())
}

fn design_controller(
    heads: &journal::TaskHeads,
    controller: &TaskStamp,
) -> Result<(), Outcome<CommandAnswer>> {
    if heads
        .statement()
        .body()
        .intent
        .is_some_and(wire::TaskIntent::is_design)
    {
        return Ok(());
    }
    Err(refusal(
        "preparation::controller-not-design",
        Some(controller.to_string()),
        "propose and decide Design passes only under a Design; a Plan's outputs follow its \
         positive review",
    ))
}

/// Admit a decision's controller as a witnessed, open Design.
fn require_open_design(
    command: &wire::AdmittedCommand,
    heads: &journal::TaskHeads,
    controller: &TaskStamp,
) -> Result<(), Outcome<CommandAnswer>> {
    require_transition_witnesses(command, heads)?;
    design_controller(heads, controller)?;
    if !wire::intent::is_open_design(heads.statement().body().intent, heads.workflow().body()) {
        return Err(refusal(
            "preparation::controller-not-open",
            Some(controller.to_string()),
            "decide after the Design reaches its open design phase",
        ));
    }
    Ok(())
}

fn require_unblocked_selection(
    snapshot: &bus::TaskSnapshot,
    controller: &journal::TaskHeads,
) -> Result<(), Outcome<CommandAnswer>> {
    for dependency in &controller.statement().body().dependencies {
        if task_heads(snapshot, dependency)?
            .workflow()
            .body()
            .status
            .is_pending()
        {
            return Err(refusal(
                "preparation::controller-blocked",
                Some(dependency.to_string()),
                "settle the Design's pending dependencies before selecting its outputs",
            ));
        }
    }
    Ok(())
}

/// Refuse a pass while any current question of its Design lacks an
/// item-specific nonzero charge, naming each uncovered question. Coverage is
/// read from the admitted statement head, which the proposal witnesses, so an
/// edit refuses at the guard and a later charge can only add coverage.
fn require_question_coverage(
    snapshot: &bus::TaskSnapshot,
    controller: &TaskStamp,
) -> Result<(), Outcome<CommandAnswer>> {
    let heads = task_heads(snapshot, controller)?;
    let statement = heads.statement().body();
    let quiet = wire::rooms::DesignItems::default();
    let coverage = snapshot
        .facts()
        .design_items(controller)
        .unwrap_or(&quiet)
        .coverage(&statement.acceptance);
    if coverage.is_full() {
        return Ok(());
    }
    let handle = TaskHandle::for_filing(&statement.project, &statement.born, statement.filing);
    let (mut questions, mut items) = (Vec::new(), Vec::new());
    for question in coverage.missing {
        questions.push(question.to_string());
        items.push(format!(
            "question {question}: {}",
            question.item(&statement.acceptance).unwrap_or_default()
        ));
    }
    Err(refusal(
        "preparation::questions-uncovered",
        Some(questions.join(",")),
        &format!(
            "investigate every uncovered question of {handle} and weigh each with `spice room \
             contribute {handle} --charge=C --question N TEXT` before proposing; uncovered: {}",
            items.join("; ")
        ),
    ))
}

pub(super) fn propose_design(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request_body: &DesignPropose,
    repository: Option<Host<'_>>,
) -> Outcome<CommandAnswer> {
    let proposal = &request_body.candidate;
    let subject = ProposalRef::Candidate {
        candidate: proposal.candidate.clone(),
    };
    if let Err(outcome) = require_proposal_absence(command, &subject) {
        return outcome;
    }
    let observed = match bus.task_snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => return store_fault("preparation::heads-unreadable", &error.to_string()),
    };
    if let Err(outcome) = require_design_proposal(&observed, command, proposal) {
        return outcome;
    }
    if let Err(outcome) = require_question_coverage(&observed, &proposal.candidate.controller) {
        return outcome;
    }
    if let Err(outcome) = references::require_scoped_references(bus, proposal) {
        return outcome;
    }
    let roots = std::iter::once(proposal.candidate.controller.clone())
        .chain(proposal.governed_inputs.iter().cloned())
        .chain(proposal.outputs.iter().cloned())
        .collect::<Vec<_>>();
    let snapshot = match validated_working_snapshot_for_tasks(bus, command.actor().clone(), &roots)
    {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(entry) = snapshot
        .entries
        .iter()
        .find(|entry| entry.born() == &proposal.candidate.controller)
    else {
        return refusal(
            "preparation::controller-missing",
            Some(proposal.candidate.controller.to_string()),
            "read the current board and retry the proposal",
        );
    };
    if let Err(outcome) = require_acceptance(&snapshot, command, entry.born()) {
        return outcome;
    }
    let claim = entry
        .claim()
        .expect("proposal admission proved the controller has an active claim");
    let parked = match prepare_release(
        &snapshot,
        command,
        claim.claimed,
        repository,
        Some("design"),
    ) {
        Ok(parked) => parked,
        Err(outcome) => return outcome,
    };
    commit_proposal(bus, command, request_body, entry, parked)
}

fn commit_proposal(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request_body: &DesignPropose,
    entry: &Entry,
    parked: Option<spice::task_git::ParkedRelease>,
) -> Outcome<CommandAnswer> {
    let proposal = &request_body.candidate;
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return rollback_release(parked, outcome),
    };
    let origin = match task_origin(&proposal.candidate.controller) {
        Ok(origin) => origin,
        Err(outcome) => return rollback_release(parked, outcome),
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    let proposed = match write.fact(
        DesignProposed {
            candidate: proposal.clone(),
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => {
            return rollback_release(
                parked,
                store_fault("preparation::proposal-invalid", &error.to_string()),
            );
        }
    };
    let released = match write.fact(
        body::ClaimFreed {
            task: entry.born().clone(),
            actor: command.actor().clone(),
            parked: parked.as_ref().map(|prepared| prepared.parked().clone()),
        },
        FactCause::Pending(proposed),
    ) {
        Ok(fact) => fact,
        Err(error) => {
            return rollback_release(
                parked,
                store_fault("preparation::proposal-release-invalid", &error.to_string()),
            );
        }
    };
    let mut claim = entry
        .claim()
        .expect("proposal admission proved an active claim")
        .clone();
    claim.disposition = ClaimDisposition::Freed;
    write.claim_head(entry.born().clone(), released, move |fact| ClaimHead {
        fact,
        ..claim
    });
    let recommendation = match proposal.recommendation {
        wire::rooms::DesignPassRecommendation::Work => "work",
        wire::rooms::DesignPassRecommendation::Conclude => "conclude",
        wire::rooms::DesignPassRecommendation::Collapse => "collapse",
    };
    let answer = TextAnswer {
        text: format!(
            "proposed {recommendation} through Design pass {} and released its claim",
            proposal.candidate.candidate
        ),
    };
    if let Err(error) = write.command_result(
        request(command, request_body.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return rollback_release(
            parked,
            store_fault("preparation::proposal-receipt-invalid", &error.to_string()),
        );
    }
    let body = CommandAnswer::Text(answer);
    match parked {
        Some(parked) => committed_with_task_git(bus, write, body, PreparedTaskGit::Release(parked)),
        None => committed(bus, write, body),
    }
}

pub(super) fn adopt_output(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request_body: &OutputAdopt,
) -> Outcome<CommandAnswer> {
    let (output, parent) = match adoption_stamps(request_body) {
        Ok(stamps) => stamps,
        Err(outcome) => return outcome,
    };
    let snapshot = match bus.task_snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => return store_fault("preparation::heads-unreadable", &error.to_string()),
    };
    let output_heads = match task_heads(&snapshot, &output) {
        Ok(heads) => heads,
        Err(outcome) => return outcome,
    };
    let parent_heads = match task_heads(&snapshot, &parent) {
        Ok(heads) => heads,
        Err(outcome) => return outcome,
    };
    for heads in [output_heads, parent_heads] {
        if let Err(outcome) = require_transition_witnesses(command, heads) {
            return outcome;
        }
    }
    if !output_heads.workflow().body().status.is_pending()
        || active_claim(output_heads)
        || output_heads.statement().body().preparation.is_some()
        || output_heads.statement().body().intent.is_some()
    {
        return refusal(
            "preparation::adoption-output-ineligible",
            Some(request_body.output.to_string()),
            "adopt only pending, unclaimed, uncontrolled ordinary work; running or completed work remains reference-only",
        );
    }
    if !parent_heads.workflow().body().status.is_pending()
        || active_claim(parent_heads)
        || !declares_plan(
            parent_heads.statement().body().intent,
            &parent_heads.workflow().body().flow,
        )
    {
        return refusal(
            "preparation::adoption-controller-ineligible",
            Some(request_body.preparation.to_string()),
            "choose a pending, unclaimed Plan or Design context as the controller",
        );
    }
    let preparation = wire::TaskPreparation {
        parent: parent.clone(),
        controller: parent_heads
            .statement()
            .body()
            .preparation
            .as_ref()
            .map_or_else(
                || parent.clone(),
                |preparation| preparation.controller.clone(),
            ),
        construction: output.clone(),
    };
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(&output) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    let fact = match write.fact(
        OutputControlled {
            output: output.clone(),
            preparation: preparation.clone(),
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("preparation::adoption-invalid", &error.to_string()),
    };
    let mut head = output_heads.statement().body().clone();
    head.preparation = Some(preparation);
    if !head.dependencies.contains(&parent) {
        head.dependencies.push(parent);
    }
    write.task_head(fact, move |revision| TaskHead { revision, ..head });
    let answer = TextAnswer {
        text: format!(
            "adopted {output} under controller {}",
            parent_heads.statement().body().born
        ),
    };
    if let Err(error) = write.command_result(
        request(command, request_body.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("preparation::adoption-receipt-invalid", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Text(answer))
}
fn adoption_stamps(
    request_body: &OutputAdopt,
) -> Result<(TaskStamp, TaskStamp), Outcome<CommandAnswer>> {
    let output = request_body.output.born().map_err(|error| {
        refusal(
            "preparation::output-invalid",
            Some(request_body.output.to_string()),
            &format!("name a task handle with a birth stamp: {error}"),
        )
    })?;
    let parent = request_body.preparation.born().map_err(|error| {
        refusal(
            "preparation::controller-invalid",
            Some(request_body.preparation.to_string()),
            &format!("name a preparation task handle with a birth stamp: {error}"),
        )
    })?;
    Ok((output, parent))
}

fn record<B, F>(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request_body: &B,
    controller: &TaskStamp,
    fact_body: F,
    derived: Vec<Witness>,
    text: String,
) -> Outcome<CommandAnswer>
where
    B: serde::Serialize + Clone,
    F: wire::Body,
{
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(controller) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    if !derived.is_empty() {
        guard(&mut write, derived);
    }
    if let Err(error) = write.fact(fact_body, FactCause::Root) {
        return store_fault("preparation::fact-invalid", &error.to_string());
    }
    let answer = TextAnswer { text };
    if let Err(error) = write.command_result(
        request(command, request_body.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("preparation::receipt-invalid", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Text(answer))
}

fn require_proposal_absence(
    command: &wire::AdmittedCommand,
    proposal: &ProposalRef,
) -> Result<(), Outcome<CommandAnswer>> {
    require_witness(
        command,
        &Witness::Proposal {
            proposal: proposal.clone(),
            head: None,
        },
    )
}

fn require_transition_witnesses(
    command: &wire::AdmittedCommand,
    heads: &journal::TaskHeads,
) -> Result<(), Outcome<CommandAnswer>> {
    for witness in [
        Witness::Task {
            task: heads.statement().body().born.clone(),
            head: heads.statement().position(),
        },
        Witness::Workflow {
            task: heads.statement().body().born.clone(),
            head: heads.workflow().position(),
            phase: heads.workflow().body().phase.clone(),
        },
        Witness::Claim {
            task: heads.statement().body().born.clone(),
            head: heads.claim().map(journal::JournalHead::position),
        },
    ] {
        require_witness(command, &witness)?;
    }
    Ok(())
}

fn require_witness(
    command: &wire::AdmittedCommand,
    witness: &Witness,
) -> Result<(), Outcome<CommandAnswer>> {
    if command.expected().contains(witness) {
        return Ok(());
    }
    Err(refusal(
        "preparation::witness-missing",
        Some(format!("{witness:?}")),
        "read the current preparation authority and retry the exact command",
    ))
}

fn task_heads<'a>(
    snapshot: &'a bus::TaskSnapshot,
    task: &TaskStamp,
) -> Result<&'a journal::TaskHeads, Outcome<CommandAnswer>> {
    snapshot
        .tasks()
        .iter()
        .find(|heads| heads.statement().body().born == *task)
        .ok_or_else(|| {
            refusal(
                "preparation::task-missing",
                Some(task.to_string()),
                "read the current board and name an existing task",
            )
        })
}

fn active_claim(heads: &journal::TaskHeads) -> bool {
    heads
        .claim()
        .is_some_and(|claim| claim.body().disposition == ClaimDisposition::Active)
}
