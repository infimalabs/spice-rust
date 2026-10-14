use super::*;

/// The one open phase a Design walks until the operator concludes it.
const DESIGN_PHASE: &str = "design";

pub(super) fn default_flow(intent: Option<wire::TaskIntent>, flow: Option<&str>) -> Option<&str> {
    flow.or(match intent {
        Some(wire::TaskIntent::Plan) => Some("plan,review"),
        Some(wire::TaskIntent::Design) => Some(DESIGN_PHASE),
        None => None,
    })
}

/// Refuse a Design whose flow is anything but its one open phase: no lane
/// step advances or concludes a Design, so another phase could never be
/// reached.
pub(super) fn admit_design_flow(
    intent: Option<wire::TaskIntent>,
    flow: &[wire::Phase],
) -> Result<(), Outcome<CommandAnswer>> {
    if intent != Some(wire::TaskIntent::Design)
        || matches!(flow, [phase] if phase.as_str() == DESIGN_PHASE)
    {
        return Ok(());
    }
    let flow = flow.iter().map(wire::Phase::as_str).collect::<Vec<_>>();
    Err(refusal(
        "task::design-flow",
        Some(flow.join(",")),
        "a Design walks the one open `design` phase until the operator concludes it; omit --flow",
    ))
}

/// Whether `phase` constructs work: a parent in it is a planning context.
fn constructs_work(phase: &str) -> bool {
    matches!(phase, "plan" | DESIGN_PHASE)
}

/// The Design a new statement was constructed directly under, if any.
pub(super) fn constructing_design<'a>(
    board: &'a BoardAnswer,
    statement: &TaskHead,
) -> Option<&'a BoardTask> {
    let preparation = statement
        .preparation
        .as_ref()
        .filter(|preparation| preparation.construction == statement.born)?;
    board.tasks.iter().find(|heads| {
        heads.statement.body.born == preparation.parent
            && heads.statement.body.intent == Some(wire::TaskIntent::Design)
    })
}

/// Refuse a gather task outside an open Design the working-group partition
/// holds: it exists only to gather evidence for an active Design.
pub(super) fn admit_gather_design(
    intent: Option<wire::TaskIntent>,
    flow: &[wire::Phase],
    design: Option<&BoardTask>,
) -> Result<(), Outcome<CommandAnswer>> {
    if !board::gather_task(intent, flow) {
        return Ok(());
    }
    if design.is_some_and(|design| {
        design.grouped
            && design.workflow.body.status == "pending"
            && design.workflow.body.completion.is_none()
    }) {
        return Ok(());
    }
    Err(refusal(
        "task::gather-design",
        design.map(|design| design.statement.body.handle.to_string()),
        "create a gather task directly under an open Design in a working group, while holding \
         it or by naming it with --preparation",
    ))
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

/// Whether `entry` delivers planning rather than implementation: loose
/// commits and recovered work never become part of one.
pub(super) fn planning_deliverable(entry: &Entry) -> bool {
    entry.statement().intent.is_some()
        || requires_construction(entry.statement().intent, &entry.workflow().flow)
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
        || constructs_work(parent.workflow.body.phase.as_str()))
    .then_some(parent))
}

/// Bind a created statement to its planning parent, then admit a gather task
/// only directly under an active open Design.
pub(super) fn admit_preparation(
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    add: &TaskAdd,
    flow: &[wire::Phase],
    statement: &mut TaskHead,
) -> Result<(), Outcome<CommandAnswer>> {
    if command.kind() != CommandKind::TaskAdd {
        return Ok(());
    }
    if let Some(parent) = preparation_parent(&snapshot.answer, add)? {
        if add.preparation.is_none() {
            require_expected(command, &claim_witness(parent))?;
        }
        bind_preparation(&snapshot.answer, command, parent, statement)?;
    }
    admit_gather_design(
        statement.intent,
        flow,
        constructing_design(&snapshot.answer, statement),
    )
}

/// Bind a new statement to its pending planning parent under current policy.
///
/// An ordinary output also follows its parent and controller; a Design never
/// becomes a preparation output.
pub(super) fn bind_preparation(
    board: &BoardAnswer,
    command: &wire::AdmittedCommand,
    parent: &BoardTask,
    statement: &mut TaskHead,
) -> Result<(), Outcome<CommandAnswer>> {
    require_expected(command, &statement_witness(parent))?;
    require_expected(command, &workflow_witness(parent))?;
    if statement.intent == Some(wire::TaskIntent::Design)
        || parent.workflow.body.status != "pending"
        || (parent.statement.body.intent.is_none()
            && !constructs_work(parent.workflow.body.phase.as_str())
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
    if statement.intent.is_none() {
        let preparation = statement
            .preparation
            .as_ref()
            .expect("association was admitted");
        // Selection, not conclusion, releases work a Design controls, and a
        // Design stays open after selecting it, so an ordinary edge to an open
        // Design would hold its output until the Design concluded.
        for prerequisite in [&preparation.parent, &preparation.controller] {
            if !statement.dependencies.contains(prerequisite) && !is_design(board, prerequisite) {
                statement.dependencies.push(prerequisite.clone());
            }
        }
    }
    Ok(())
}

fn is_design(board: &BoardAnswer, task: &TaskStamp) -> bool {
    board.tasks.iter().any(|heads| {
        heads.statement.body.born == *task
            && heads.statement.body.intent == Some(wire::TaskIntent::Design)
    })
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
        && task.workflow.body.review != "closed"
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
