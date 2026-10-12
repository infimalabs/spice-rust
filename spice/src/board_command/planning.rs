use super::*;

pub(super) fn default_flow(intent: Option<wire::TaskIntent>, flow: Option<&str>) -> Option<&str> {
    flow.or(match intent {
        Some(wire::TaskIntent::Plan) => Some("plan,review"),
        Some(wire::TaskIntent::Design) => Some("design,review"),
        None => None,
    })
}

pub(super) fn requires_construction(
    intent: Option<wire::TaskIntent>,
    flow: &[wire::Phase],
) -> bool {
    intent == Some(wire::TaskIntent::Plan)
        || (intent.is_none()
            && flow.iter().any(|phase| phase.as_str() == "plan")
            && flow
                .iter()
                .all(|phase| matches!(phase.as_str(), "plan" | "review")))
}

pub(super) fn admit_capture(entry: &Entry) -> Result<(), Outcome<CommandAnswer>> {
    if entry.statement().intent.is_some()
        || requires_construction(entry.statement().intent, &entry.workflow().flow)
        || entry.workflow().phase.as_str() == "design"
    {
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
    if add.origin.is_some()
        || add.intent == Some(wire::TaskIntent::Design)
        || !matches!(board.principal, Actor::Lane(_))
    {
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
        || requires_construction(parent.statement.body.intent, &parent.workflow.body.flow)
        || matches!(parent.workflow.body.phase.as_str(), "plan" | "design"))
    .then_some(parent))
}

pub(super) fn admit_preparation(
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    add: &TaskAdd,
    statement: &mut TaskHead,
) -> Result<(), Outcome<CommandAnswer>> {
    if command.kind() != CommandKind::TaskAdd {
        return Ok(());
    }
    let Some(parent) = preparation_parent(&snapshot.answer, add)? else {
        return Ok(());
    };
    require_expected(command, &statement_witness(parent))?;
    require_expected(command, &workflow_witness(parent))?;
    if add.preparation.is_none() {
        require_expected(command, &claim_witness(parent))?;
    }
    if add.intent == Some(wire::TaskIntent::Design)
        || parent.workflow.body.status != "pending"
        || (parent.statement.body.intent.is_none()
            && !matches!(parent.workflow.body.phase.as_str(), "plan" | "design")
            && !requires_construction(parent.statement.body.intent, &parent.workflow.body.flow))
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
    if add.intent.is_none() {
        let preparation = statement
            .preparation
            .as_ref()
            .expect("association was admitted");
        for prerequisite in [&preparation.parent, &preparation.controller] {
            if !statement.dependencies.contains(prerequisite) {
                statement.dependencies.push(prerequisite.clone());
            }
        }
    }
    Ok(())
}

pub(super) fn correction_preparation(entry: &BoardTask) -> Option<wire::TaskPreparation> {
    let statement = &entry.statement.body;
    if statement.intent.is_none()
        && !requires_construction(statement.intent, &entry.workflow.body.flow)
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
    matches!(task.workflow.body.status.as_str(), "pending" | "completed")
        && task.statement.body.intent.is_none()
        && !planning::requires_construction(task.statement.body.intent, &task.workflow.body.flow)
        && substantive_acceptance(&task.statement.body.acceptance)
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
