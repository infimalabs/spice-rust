use super::*;

/// The one open phase a Design walks until the operator concludes it.
const DESIGN_PHASE: &str = "design";

/// The flow a creation that names none walks. A Design's open questions are
/// its acceptance: without one it is framed in plan intake first, as ordinary
/// work without acceptance gets a plan bookend from [`task_flow`].
pub(super) fn default_flow<'a>(
    intent: Option<wire::TaskIntent>,
    flow: Option<&'a str>,
    acceptance: &[String],
) -> Option<&'a str> {
    flow.or(match intent {
        Some(wire::TaskIntent::Plan) => Some("plan,review"),
        Some(wire::TaskIntent::Design | wire::TaskIntent::OpportunisticDesign)
            if substantive_acceptance(acceptance) =>
        {
            Some(DESIGN_PHASE)
        }
        Some(wire::TaskIntent::Design | wire::TaskIntent::OpportunisticDesign) => {
            Some("plan,design")
        }
        None => None,
    })
}

/// Admit the direct Design flow and either ordinary claimed intake prefix.
pub(super) fn admit_design_flow(
    intent: Option<wire::TaskIntent>,
    flow: &[wire::Phase],
) -> Result<(), Outcome<CommandAnswer>> {
    if !intent.is_some_and(wire::TaskIntent::is_design)
        || matches!(
            flow,
            [design] if design.as_str() == DESIGN_PHASE
        )
        || matches!(
            flow,
            [intake, design]
                if matches!(intake.as_str(), "plan" | "review")
                    && design.as_str() == DESIGN_PHASE
        )
    {
        return Ok(());
    }
    let flow = flow.iter().map(wire::Phase::as_str).collect::<Vec<_>>();
    Err(refusal(
        "task::design-flow",
        Some(flow.join(",")),
        "use one admitted Design route: design, plan,design, or review,design",
    ))
}

/// Whether `phase` constructs work: a parent in it is a planning context.
fn constructs_work(phase: &str) -> bool {
    matches!(phase, "plan" | DESIGN_PHASE)
}

pub(super) fn declares_plan(intent: Option<wire::TaskIntent>, flow: &[wire::Phase]) -> bool {
    intent == Some(wire::TaskIntent::Plan)
        || (intent.is_none()
            && flow.iter().any(|phase| phase.as_str() == "plan")
            && flow
                .iter()
                .all(|phase| matches!(phase.as_str(), "plan" | "review")))
}

/// Whether `entry` delivers planning rather than implementation: loose
/// commits and recovered work never become part of one.
pub(super) fn planning_deliverable(entry: &Entry) -> bool {
    entry.statement().intent.is_some()
        || declares_plan(entry.statement().intent, &entry.workflow().flow)
        || entry.workflow().phase.as_str() == "design"
}

pub(super) fn admit_capture(entry: &Entry) -> Result<(), Outcome<CommandAnswer>> {
    if planning_deliverable(entry) {
        return Err(refusal(
            "task::capture-planning",
            Some(entry.handle().to_string()),
            "capture implementation only into an execution task, not a planning deliverable",
        ));
    }
    Ok(())
}

pub(super) fn construction_owner<'a>(
    born: &'a wire::TaskStamp,
    preparation: Option<&'a wire::TaskPreparation>,
) -> &'a wire::TaskStamp {
    preparation.map_or(born, |preparation| &preparation.construction)
}

pub(super) fn preparation_parent<'a>(
    board: &'a BoardAnswer,
    add: &TaskAdd,
) -> Result<Option<&'a BoardTask>, Outcome<CommandAnswer>> {
    if let Some(parent) = &add.preparation {
        return task_heads(board, parent).map(Some);
    }
    if add.origin.is_some() || !matches!(board.principal, Actor::Lane(_)) {
        return Ok(None);
    }
    let mut held = board.tasks.iter().filter(|task| {
        task.claim.as_ref().is_some_and(|claim| {
            claim.body.actor == board.principal
                && claim.body.disposition == ClaimDisposition::Active
        })
    });
    let Some(parent) = held.next() else {
        return Ok(None);
    };
    if held.next().is_some() {
        return Err(protocol_fault(
            "claim::active-ambiguous",
            "creation has more than one ambient claim",
        ));
    }
    Ok((parent.statement.body.intent.is_some()
        || declares_plan(parent.statement.body.intent, &parent.workflow.body.flow)
        || constructs_work(parent.workflow.body.phase.as_str()))
    .then_some(parent))
}

/// Bind a created statement to its planning parent.
pub(super) fn admit_preparation(
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    add: &TaskAdd,
    statement: &mut TaskHead,
) -> Result<(), Outcome<CommandAnswer>> {
    if command.kind() != CommandKind::TaskAdd {
        return Ok(());
    }
    if let Some(parent) = preparation_parent(&snapshot.answer, add)? {
        if add.preparation.is_none() {
            require_expected(command, &claim_witness(parent))?;
        }
        bind_preparation(command, parent, statement)?;
    }
    Ok(())
}

/// Every construction root retains one ordinary dependency on its immediate
/// gating parent.
pub(super) fn bind_preparation(
    command: &wire::AdmittedCommand,
    parent: &BoardTask,
    statement: &mut TaskHead,
) -> Result<(), Outcome<CommandAnswer>> {
    require_expected(command, &statement_witness(parent))?;
    require_expected(command, &workflow_witness(parent))?;
    if !parent.workflow.body.status.is_pending()
        || (parent.statement.body.intent.is_none()
            && !constructs_work(parent.workflow.body.phase.as_str())
            && !declares_plan(parent.statement.body.intent, &parent.workflow.body.flow))
        || (parent
            .statement
            .body
            .intent
            .is_some_and(wire::TaskIntent::is_design)
            && !wire::intent::is_open_design(parent.statement.body.intent, &parent.workflow.body))
    {
        return Err(refusal(
            "task::preparation-refused",
            Some(parent.statement.body.handle.to_string()),
            "associate a preparation Plan or ordinary output with a pending planning context",
        ));
    }
    statement.preparation = Some(wire::TaskPreparation {
        parent: parent.statement.body.born.clone(),
        controller: parent.statement.body.preparation.as_ref().map_or_else(
            || parent.statement.body.born.clone(),
            |preparation| preparation.controller.clone(),
        ),
        construction: statement.born.clone(),
    });
    if !statement.dependencies.contains(&parent.statement.body.born) {
        statement
            .dependencies
            .push(parent.statement.body.born.clone());
    }
    Ok(())
}

pub(super) fn correction_preparation(entry: &BoardTask) -> Option<wire::TaskPreparation> {
    let statement = &entry.statement.body;
    if statement.intent.is_none()
        && !declares_plan(statement.intent, &entry.workflow.body.flow)
        && statement.preparation.is_none()
    {
        return None;
    }
    Some(wire::TaskPreparation {
        parent: statement.born.clone(),
        controller: statement.preparation.as_ref().map_or_else(
            || statement.born.clone(),
            |preparation| preparation.controller.clone(),
        ),
        construction: construction_owner(&statement.born, statement.preparation.as_ref()).clone(),
    })
}

pub(super) fn accepted_pending_child<'a>(
    snapshot: &'a ValidatedSnapshot,
    entry: &Entry,
) -> Result<Option<&'a BoardTask>, Outcome<CommandAnswer>> {
    let owner = planning::construction_owner(entry.born(), entry.statement().preparation.as_ref());
    let task = visible_task(snapshot, owner)?;
    let dependencies = if owner == entry.born() {
        &entry.statement().dependencies
    } else {
        &task.statement.body.dependencies
    };
    for dependency in dependencies {
        let child = visible_task(snapshot, dependency)?;
        if accepted_construction(child) {
            return Ok(Some(child));
        }
    }
    Ok(snapshot.answer.tasks.iter().find(|child| {
        child.statement.body.dependencies.contains(owner) && accepted_construction(child)
    }))
}

pub(super) fn accepted_construction(task: &BoardTask) -> bool {
    constructs(
        task.statement.body.intent,
        &task.statement.body.acceptance,
        &task.workflow.body,
    )
}

/// Whether work in this state is accepted non-planning output that answers a
/// Plan's need, which contradicts a no-construction disposition.
fn constructs(
    intent: Option<wire::TaskIntent>,
    acceptance: &[String],
    workflow: &wire::WorkflowHead,
) -> bool {
    (workflow.status.is_pending() || workflow.concluded_positively())
        && intent.is_none()
        && !declares_plan(intent, &workflow.flow)
        && substantive_acceptance(acceptance)
}

/// Refuse a created, filed, edited, or relinked statement whose post state
/// would give a pending Plan that already recorded a no-construction
/// disposition accepted output, so the disposition and constructed work never
/// both hold, in either order. Corrections carry preparation without
/// construction and pass.
pub(super) fn refuse_disposed_construction(
    snapshot: &ValidatedSnapshot,
    statement: &TaskHead,
    workflow: &wire::WorkflowHead,
) -> Result<(), Outcome<CommandAnswer>> {
    let disposed = |plan: &BoardTask| {
        plan.workflow.body.status.is_pending()
            && snapshot
                .facts
                .plan_disposition(&plan.statement.body.born)
                .is_some()
    };
    let constructed_under = statement
        .preparation
        .as_ref()
        .filter(|preparation| preparation.construction == statement.born)
        .map(|preparation| &preparation.parent);
    let linked_to = statement
        .dependencies
        .iter()
        .filter(|_| constructs(statement.intent, &statement.acceptance, workflow));
    for plan in constructed_under.into_iter().chain(linked_to) {
        let plan = visible_task(snapshot, plan)?;
        if disposed(plan) {
            return Err(disposed_refusal(plan));
        }
    }
    if workflow.status.is_pending() && snapshot.facts.plan_disposition(&statement.born).is_some() {
        for dependency in &statement.dependencies {
            if accepted_construction(visible_task(snapshot, dependency)?) {
                return Err(disposed_refusal(visible_task(snapshot, &statement.born)?));
            }
        }
    }
    Ok(())
}

fn disposed_refusal(plan: &BoardTask) -> Outcome<CommandAnswer> {
    refusal(
        "task::plan-disposed",
        Some(plan.statement.body.handle.to_string()),
        "this Plan recorded that no work is warranted; construct or link accepted work \
         elsewhere, or let its review find the disposition wrong",
    )
}

pub(super) fn visible_task<'a>(
    snapshot: &'a ValidatedSnapshot,
    task: &wire::TaskStamp,
) -> Result<&'a BoardTask, Outcome<CommandAnswer>> {
    snapshot
        .answer
        .tasks
        .iter()
        .find(|heads| heads.statement.body.born == *task)
        .ok_or_else(|| {
            protocol_fault(
                "task::snapshot-incoherent",
                "validated board snapshot lost a task entry",
            )
        })
}

pub(super) fn require_expected(
    command: &wire::AdmittedCommand,
    witness: &Witness,
) -> Result<(), Outcome<CommandAnswer>> {
    if command
        .expected()
        .iter()
        .any(|expected| expected == witness)
    {
        return Ok(());
    }
    Err(refusal(
        "task::plan-witness-missing",
        Some(format!("{witness:?}")),
        "read the current plan task and its accepted child before closing the plan phase",
    ))
}
