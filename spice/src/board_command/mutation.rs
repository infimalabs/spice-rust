use super::*;

mod claim;
mod edit;
mod requirements;
mod taste_proposal;

#[cfg(test)]
pub(super) use claim::Possession;
pub(super) use claim::claim_win;
#[cfg(test)]
pub(super) use claim::{ClaimPlan, claim_git_boundary, claim_grant, claim_plan, commit_claim};
pub(super) use edit::task_edit;
pub(super) use taste_proposal::taste_proposal_add;

/// One actor-filtered board snapshot whose cursor and entries were validated together.
pub(super) struct ValidatedSnapshot {
    pub(super) cursor: wire::Seq,
    pub(super) facts: bus::TaskFacts,
    pub(super) answer: BoardAnswer,
    pub(super) entries: Vec<Entry>,
    pub(super) review_history: Result<Vec<board::graph::ReviewEdge>, String>,
}

struct PreparedTask {
    task: TaskHandle,
    born: wire::TaskStamp,
    project: String,
    flow: Vec<wire::Phase>,
    phase: wire::Phase,
    origin: wire::Origin,
    creation: TaskCreationCapture,
    at: Instant,
    statement: TaskHead,
    workflow: WorkflowHead,
}

pub(super) fn task_add(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    add: &TaskAdd,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    task_add_with(bus, command, add, repository, projects, allocation, || {})
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn task_add_with(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    add: &TaskAdd,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
    after_validation: impl FnOnce(),
) -> Outcome<CommandAnswer> {
    let requested = add.project.as_deref().unwrap_or("task.private");
    let project = match projects.admit_manual_creation(requested) {
        Ok(project) => project,
        Err(error) => {
            let reason = if matches!(error, ProjectError::SystemReserved(_)) {
                "task::project-system-reserved"
            } else {
                "task::add-refused"
            };
            return refusal(
                reason,
                Some(error.to_string()),
                "use a public project for manual creation or the owning typed system route",
            );
        }
    };
    create_task(
        bus,
        command,
        add,
        project,
        add.clone(),
        repository,
        projects,
        allocation,
        after_validation,
    )
}

pub(super) fn task_oops_add(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    add: &TaskOopsAdd,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let project = match projects.admit_system_creation(&SystemTaskRoute::oops()) {
        Ok(project) => project,
        Err(error) => {
            return protocol_fault(
                "task::oops-route-invalid",
                &format!("resident-owned .oops route is not admitted: {error}"),
            );
        }
    };
    let normalized = TaskAdd {
        title: add.title.clone(),
        project: None,
        description: Some(add.description.clone()),
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        origin: add.origin.clone(),
        flow: None,
        depends: Vec::new(),
    };
    create_task(
        bus,
        command,
        &normalized,
        project,
        add.clone(),
        None,
        projects,
        allocation,
        || {},
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "task creation keeps request identity, repository policy, and board authority explicit"
)]
fn create_task<B: serde::Serialize>(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    add: &TaskAdd,
    project: Project,
    request_body: B,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
    after_validation: impl FnOnce(),
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(bus, command.actor().clone(), projects, allocation) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let parked = referenced_oops(&snapshot.entries, add.depends.iter());
    if !parked.is_empty() {
        return oops_dependency_refusal(&parked);
    }
    let origin =
        match resolve_creation_origin(add.origin.as_ref(), command.actor(), &snapshot, None) {
            Ok(origin) => origin,
            Err(outcome) => return outcome,
        };
    let creation = match task_creation_capture(bus, command, &snapshot) {
        Ok(creation) => creation,
        Err(outcome) => return outcome,
    };
    if let Err(outcome) = validate_acceptance(&add.acceptance) {
        return outcome;
    }
    let (flow, dependency_witnesses) = match task_flow_for_dependencies(add, &snapshot) {
        Ok(observation) => observation,
        Err(outcome) => return outcome,
    };
    let mut prepared = match prepare_task(command, add, &project, projects, origin, creation, flow)
    {
        Ok(prepared) => prepared,
        Err(outcome) => return outcome,
    };
    let taste_review = match task_taste_review(
        repository,
        &snapshot,
        command.actor(),
        &prepared.statement.title,
        prepared.statement.description.as_deref(),
        &prepared.statement.acceptance,
    ) {
        Ok(review) => review,
        Err(outcome) => return outcome,
    };
    let taste_review_required = taste_review.as_ref().is_some_and(TasteReview::required);
    if taste_review_required {
        prepend_plan(&mut prepared.flow);
        prepared.phase = prepared.flow[0].clone();
        prepared.workflow.flow.clone_from(&prepared.flow);
        prepared.workflow.phase.clone_from(&prepared.phase);
    }
    let mut tags = Vec::new();
    if taste_review_required {
        require_taste_review(&mut tags);
    }
    after_validation();
    commit_prepared_task(
        bus,
        command,
        add,
        &project,
        request_body,
        projects,
        &snapshot,
        prepared,
        tags,
        taste_review.as_ref().filter(|review| review.required()),
        dependency_witnesses,
        |_| {},
    )
}

#[allow(clippy::too_many_arguments)]
fn commit_prepared_task<B, F>(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    add: &TaskAdd,
    project: &Project,
    request_body: B,
    projects: &ProjectPolicy,
    snapshot: &ValidatedSnapshot,
    mut prepared: PreparedTask,
    tags: Vec<String>,
    taste_review: Option<&TasteReview>,
    dependency_witnesses: Vec<Witness>,
    extra_guard: F,
) -> Outcome<CommandAnswer>
where
    B: serde::Serialize,
    F: FnOnce(&mut JournalWrite),
{
    prepared.statement.tags.clone_from(&tags);
    let post = Entry::new(
        prepared.statement.clone(),
        prepared.workflow.clone(),
        None,
        None,
        projects,
    )
    .expect("prepared task already passed entry validation")
    .with_creator_team(prepared.creation.creator_team());
    let next = next_action(snapshot, &post);
    let mut write = JournalWrite::new(
        command.actor().clone(),
        prepared.origin,
        prepared.at,
        Producer::CommandDoor,
    );
    let mut expected = command.expected().to_vec();
    expected.extend(dependency_witnesses);
    guard(&mut write, expected);
    prepared.creation.guard(&mut write);
    extra_guard(&mut write);
    let added = match write.fact(
        body::TaskAdded {
            task: prepared.born.clone(),
            title: add.title.clone(),
            description: add.description.clone(),
            acceptance: add.acceptance.clone(),
            requirements: add.requirements.clone(),
            project: prepared.project,
            tags,
            depends: prepared.statement.dependencies.clone(),
            flow: prepared.flow,
            phase: prepared.phase,
            revision: 1,
            creation: prepared.creation.context(),
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("task::add-build", &error.to_string()),
    };
    if let Some(review) = taste_review
        && let Err(outcome) = append_taste_review_audit(
            &mut write,
            &prepared.born,
            review,
            FactCause::Pending(added),
        )
    {
        return outcome;
    }
    let statement = prepared.statement;
    write.task_head(added, move |position| TaskHead {
        queue_origin: position,
        revision: position,
        ..statement
    });
    write.workflow_head(prepared.born.clone(), added, move |_| prepared.workflow);
    if let Err(outcome) = add_oops_note(&mut write, project, add, &prepared.born, added) {
        return outcome;
    }
    let answer = wire::intent::TaskAnswer {
        task: prepared.task,
        status: "pending".to_owned(),
        guidance: None,
        completion: None,
        learning: None,
        next,
    };
    if let Err(error) = write.command_result(
        request(command, request_body),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("task::add-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Task(answer))
}

fn prepare_task(
    command: &wire::AdmittedCommand,
    add: &TaskAdd,
    project: &Project,
    projects: &ProjectPolicy,
    origin: wire::Origin,
    creation: TaskCreationCapture,
    flow: Vec<wire::Phase>,
) -> Result<PreparedTask, Outcome<CommandAnswer>> {
    let handles = command
        .expected()
        .iter()
        .filter_map(|witness| match witness {
            Witness::Absence { task } => Some(task.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [task] = handles.as_slice() else {
        return Err(protocol_fault(
            "task::add-witness-invalid",
            "task add requires exactly one intended absent birth stamp",
        ));
    };
    let born = task.clone();
    if origin == task_origin(&born)? {
        return Err(refusal(
            "task::origin-self",
            Some(origin.to_string()),
            "name the steering directive or existing parent task that caused this task",
        ));
    }
    let project_text = project.as_str().to_owned();
    let task = TaskHandle::for_project(&project_text, &born);
    let phase = flow[0].clone();
    let at = current_instant()?;
    let statement = TaskHead {
        born: born.clone(),
        queue_origin: Seq::new(1).expect("placeholder queue origin is non-zero"),
        title: add.title.clone(),
        description: add.description.clone(),
        acceptance: add.acceptance.clone(),
        requirements: add.requirements.clone(),
        project: project_text.clone(),
        tags: Vec::new(),
        dependencies: add
            .depends
            .iter()
            .map(TaskHandle::born)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                refusal(
                    "task::dependency-invalid",
                    Some(error.to_string()),
                    "name dependencies carrying canonical birth stamps",
                )
            })?,
        provenance: origin.clone(),
        creation: creation.context(),
        revision: Seq::new(1).expect("placeholder revision is non-zero"),
    };
    let workflow = WorkflowHead {
        flow: flow.clone(),
        phase: phase.clone(),
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
    };
    if let Err(error) = Entry::new(statement.clone(), workflow.clone(), None, None, projects) {
        return Err(refusal(
            "task::add-refused",
            Some(error.to_string()),
            "correct the task project, dependencies, or phase flow and retry",
        ));
    }
    Ok(PreparedTask {
        task,
        born,
        project: project_text,
        flow,
        phase,
        origin,
        creation,
        at,
        statement,
        workflow,
    })
}

fn task_flow_for_dependencies(
    add: &TaskAdd,
    snapshot: &ValidatedSnapshot,
) -> Result<(Vec<wire::Phase>, Vec<Witness>), Outcome<CommandAnswer>> {
    let mut flow = task_flow(add.flow.as_deref(), &add.acceptance)?;
    let dependencies = add
        .depends
        .iter()
        .map(TaskHandle::born)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            refusal(
                "task::dependency-invalid",
                Some(error.to_string()),
                "name dependencies carrying canonical birth stamps",
            )
        })?;
    let witnesses = derive_dependency_flow(&mut flow, &dependencies, snapshot)?;
    Ok((flow, witnesses))
}

pub(super) fn derive_dependency_flow(
    flow: &mut Vec<wire::Phase>,
    dependencies: &[TaskStamp],
    snapshot: &ValidatedSnapshot,
) -> Result<Vec<Witness>, Outcome<CommandAnswer>> {
    let mut witnesses = Vec::with_capacity(dependencies.len());
    let mut all_review = !dependencies.is_empty();
    for born in dependencies {
        let Some(heads) = snapshot
            .answer
            .tasks
            .iter()
            .find(|heads| &heads.statement.body.born == born)
        else {
            all_review = false;
            witnesses.push(Witness::Absence { task: born.clone() });
            continue;
        };
        all_review &= heads.workflow.body.phase.as_str() == "review"
            && heads.workflow.body.status == "pending"
            && heads.workflow.body.completion.is_none();
        witnesses.push(workflow_witness(heads));
    }
    omit_review_phase(flow, all_review)?;
    Ok(witnesses)
}

pub(super) fn omit_review_phase(
    flow: &mut Vec<wire::Phase>,
    all_review: bool,
) -> Result<(), Outcome<CommandAnswer>> {
    if all_review {
        flow.retain(|phase| phase.as_str() != "review");
        if flow.is_empty() {
            return Err(refusal(
                "task::flow-empty",
                Some("review".to_owned()),
                "name at least one non-review workflow phase when creation omits review",
            ));
        }
    }
    Ok(())
}

fn add_oops_note(
    write: &mut JournalWrite,
    project: &Project,
    add: &TaskAdd,
    task: &wire::TaskStamp,
    added: PendingFact,
) -> Result<(), Outcome<CommandAnswer>> {
    if project.visibility().hidden_stem() == Some("oops")
        && let Some(note) = &add.description
        && let Err(error) = write.fact(
            body::TaskNoted {
                task: task.clone(),
                note: note.clone(),
            },
            FactCause::Pending(added),
        )
    {
        return Err(store_fault("task::oops-note-build", &error.to_string()));
    }
    Ok(())
}

pub(super) fn task_note(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    note: &TaskNote,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(bus, command.actor().clone(), projects, allocation) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(entry) = task_entry(&snapshot.entries, &note.task) else {
        return refusal(
            "task::unknown",
            Some(note.task.to_string()),
            "read board and name one current task handle",
        );
    };
    let next = next_action(&snapshot, entry);
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(entry.born()) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    if let Err(error) = write.fact(
        body::TaskNoted {
            task: entry.born().clone(),
            note: note.note.clone(),
        },
        FactCause::Root,
    ) {
        return store_fault("task::note-build", &error.to_string());
    }
    let answer = wire::intent::TaskAnswer {
        task: entry.handle().clone(),
        status: "noted".to_owned(),
        guidance: None,
        completion: None,
        learning: None,
        next,
    };
    if let Err(error) = write.command_result(
        request(command, note.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("task::note-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Task(answer))
}

pub(super) fn task_defer(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    defer: &TaskDefer,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    change_deferral(
        bus,
        command,
        &defer.task,
        Some(defer.wait),
        defer.clone(),
        projects,
        allocation,
    )
}

pub(super) fn task_wake(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    wake: &TaskWake,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    if let Some(project) = wake.into.as_deref() {
        return promote_oops(bus, command, wake, project, projects, allocation);
    }
    change_deferral(
        bus,
        command,
        &wake.task,
        None,
        wake.clone(),
        projects,
        allocation,
    )
}

fn validate_promotion_entry(
    entry: &Entry,
    command: &wire::AdmittedCommand,
) -> Result<(), Outcome<CommandAnswer>> {
    let current_statement = Witness::Task {
        task: entry.born().clone(),
        head: entry.statement().revision,
    };
    if !command.expected().contains(&current_statement) {
        return Err(refusal(
            "task::promotion-statement-witness-moved",
            Some(format!("{} current={current_statement:?}", entry.handle())),
            "read the board again and retry with the current target statement witness",
        ));
    }
    if !is_oops(entry) {
        return Err(refusal(
            "task::promotion-refused",
            Some(format!("{} project={}", entry.handle(), entry.project())),
            "use --into only to promote a task currently parked in .oops",
        ));
    }
    if entry.workflow().completion.is_some() {
        return Err(refusal(
            "task::settled",
            Some(entry.handle().to_string()),
            "promote only pending .oops work",
        ));
    }
    Ok(())
}

fn promote_oops(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    wake: &TaskWake,
    requested: &str,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(bus, command.actor().clone(), projects, allocation) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(entry) = task_entry(&snapshot.entries, &wake.task) else {
        return refusal(
            "task::unknown",
            Some(wake.task.to_string()),
            "read board and name one current task handle",
        );
    };
    if let Err(outcome) = validate_promotion_entry(entry, command) {
        return outcome;
    }
    let project = match projects.admit_manual_creation(requested) {
        Ok(project) => project,
        Err(error) => {
            return refusal(
                "task::promotion-project-refused",
                Some(error.to_string()),
                "name a public project this board admits for manual filing",
            );
        }
    };
    let mut statement = entry.statement().clone();
    statement.project = project.as_str().to_owned();
    let mut workflow = entry.workflow().clone();
    workflow.deferral = None;
    let post = match Entry::new(
        statement.clone(),
        workflow.clone(),
        entry.claim().cloned(),
        entry.lease().cloned(),
        projects,
    ) {
        Ok(post) => post,
        Err(error) => {
            return refusal(
                "task::promotion-refused",
                Some(error.to_string()),
                "promote the parked row into a valid public project",
            );
        }
    };
    let next = next_action(&snapshot, &post);
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(entry.born()) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    let woken = match write.fact(
        body::TaskWoken {
            task: entry.born().clone(),
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("task::promotion-wake-build", &error.to_string()),
    };
    let revision = u32::try_from(entry.statement().revision.get())
        .unwrap_or(u32::MAX)
        .saturating_add(1);
    let notice = task_edit_notice(entry, &statement);
    let edited = match write.fact(
        body::TaskEdited {
            task: entry.born().clone(),
            revision,
            notice,
        },
        FactCause::Pending(woken),
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("task::promotion-edit-build", &error.to_string()),
    };
    write.workflow_head(entry.born().clone(), woken, move |_| workflow);
    write.task_head(edited, move |position| TaskHead {
        revision: position,
        ..statement
    });
    let answer = wire::intent::TaskAnswer {
        task: post.handle().clone(),
        status: "promoted".to_owned(),
        guidance: None,
        completion: None,
        learning: None,
        next,
    };
    if let Err(error) = write.command_result(
        request(command, wake.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("task::promotion-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Task(answer))
}

fn change_deferral<B: serde::Serialize + Clone>(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    task: &TaskHandle,
    deferral: Option<Instant>,
    intent: B,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(bus, command.actor().clone(), projects, allocation) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(entry) = task_entry(&snapshot.entries, task) else {
        return refusal(
            "task::unknown",
            Some(task.to_string()),
            "read board and name one current task handle",
        );
    };
    if deferral.is_none() && is_oops(entry) {
        return refusal(
            "task::promotion-required",
            Some(oops_state(entry)),
            &format!(
                "promote the parked row first with spice task wake {} --into PUBLIC_PROJECT",
                entry.handle(),
            ),
        );
    }
    // Settled is the question, not completed. A withdrawn task carries the same
    // completion instant and never becomes workable again, so deferring one is
    // meaningless and waking one would promise a return the allocator cannot
    // make: `workable` admits pending rows only.
    if entry.workflow().completion.is_some() {
        return refusal(
            "task::settled",
            Some(task.to_string()),
            "defer or wake only pending work",
        );
    }
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(entry.born()) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    let fact = if let Some(until) = deferral {
        write.fact(
            body::TaskDeferred {
                task: entry.born().clone(),
                until,
            },
            FactCause::Root,
        )
    } else {
        write.fact(
            body::TaskWoken {
                task: entry.born().clone(),
            },
            FactCause::Root,
        )
    };
    let fact = match fact {
        Ok(fact) => fact,
        Err(error) => return store_fault("task::deferral-build", &error.to_string()),
    };
    let mut workflow = entry.workflow().clone();
    workflow.deferral = deferral;
    let next = if deferral.is_none() {
        let post = match Entry::new(
            entry.statement().clone(),
            workflow.clone(),
            entry.claim().cloned(),
            entry.lease().cloned(),
            projects,
        ) {
            Ok(post) => post,
            Err(error) => {
                return refusal(
                    "task::deferral-refused",
                    Some(error.to_string()),
                    "wake a task into a valid pending state",
                );
            }
        };
        next_action(&snapshot, &post)
    } else {
        None
    };
    write.workflow_head(entry.born().clone(), fact, move |_| workflow);
    let status = if deferral.is_some() {
        "deferred"
    } else {
        "pending"
    };
    let answer = wire::intent::TaskAnswer {
        task: entry.handle().clone(),
        status: status.to_owned(),
        guidance: None,
        completion: None,
        learning: None,
        next,
    };
    if let Err(error) =
        write.command_result(request(command, intent), command.epoch(), answer.clone())
    {
        return store_fault("task::deferral-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Task(answer))
}

/// Withdraw a task from the board with a recorded reason.
///
/// The journal keeps the statement, so this is a terminal workflow status
/// rather than a removal: the row stops being allocatable, stops blocking its
/// dependents, and keeps its history and its reason readable.
///
/// A live claim refuses the withdrawal by default, because taking work away
/// from a lane mid-flight is a decision an operator makes deliberately, not one
/// they discover afterwards. `force_claimed` makes it, and then the fact names
/// the actor whose claim was overridden and the claim is freed in the same
/// transaction -- a lane left holding a withdrawn task would renew its lease
/// against a row nothing can ever advance.
pub(super) fn task_delete(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    delete: &TaskDelete,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(bus, command.actor().clone(), projects, allocation) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(entry) = task_entry(&snapshot.entries, &delete.task) else {
        return refusal(
            "task::unknown",
            Some(delete.task.to_string()),
            "read board and name one current task handle",
        );
    };
    if delete.reason.trim().is_empty() {
        return refusal(
            "task::delete-reason-empty",
            Some(entry.handle().to_string()),
            "state why the task leaves the board",
        );
    }
    match entry.workflow().status.as_str() {
        "completed" => {
            return refusal(
                "task::settled",
                Some(entry.handle().to_string()),
                "a completed task is history and stays on the board",
            );
        }
        "deleted" => {
            return refusal(
                "task::already-deleted",
                Some(entry.handle().to_string()),
                "the task is already withdrawn; read its recorded reason",
            );
        }
        _ => {}
    }
    let held = entry
        .claim()
        .filter(|claim| claim.disposition == ClaimDisposition::Active);
    if let Some(claim) = held
        && !delete.force_claimed
    {
        return refusal(
            "task::delete-claimed",
            Some(claim.actor.to_string()),
            "rerun with --force-claimed to withdraw work a lane is holding",
        );
    }
    let claimed = held.map(|claim| claim.actor.clone());
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(entry.born()) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let post = projected_deleted_entry(entry, at, claimed.is_some(), projects);
    let next = next_action(&snapshot, &post);
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    let deleted = match write.fact(
        body::TaskDeleted {
            task: entry.born().clone(),
            reason: delete.reason.clone(),
            claimed: claimed.clone(),
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("task::delete-build", &error.to_string()),
    };
    let workflow = post.workflow().clone();
    write.workflow_head(entry.born().clone(), deleted, move |_| workflow);
    if claimed.is_some() {
        let claim = post
            .claim()
            .expect("a held claim remains as freed history")
            .clone();
        write.claim_head(entry.born().clone(), deleted, move |fact| ClaimHead {
            fact,
            ..claim
        });
    }
    let answer = wire::intent::TaskAnswer {
        task: entry.handle().clone(),
        status: "deleted".to_owned(),
        guidance: None,
        completion: None,
        learning: None,
        next,
    };
    if let Err(error) = write.command_result(
        request(command, delete.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("task::delete-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Task(answer))
}

fn projected_deleted_entry(
    entry: &Entry,
    at: Instant,
    release_claim: bool,
    projects: &ProjectPolicy,
) -> Entry {
    let mut workflow = entry.workflow().clone();
    workflow.status = "deleted".to_owned();
    workflow.completion = Some(at);
    workflow.assignment = None;
    let mut claim = entry.claim().cloned();
    if let Some(claim) = claim.as_mut()
        && release_claim
    {
        claim.disposition = ClaimDisposition::Freed;
    }
    Entry::new(
        entry.statement().clone(),
        workflow,
        claim,
        entry.lease().cloned(),
        projects,
    )
    .expect("withdrawing an admitted task preserves a valid historical entry")
}

pub(super) fn task_depends(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    depends: &TaskDepends,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    task_depends_with(bus, command, depends, projects, allocation, || {})
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn task_depends_with(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    depends: &TaskDepends,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
    after_validation: impl FnOnce(),
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(bus, command.actor().clone(), projects, allocation) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let entry = match dependency_entry(&snapshot, depends) {
        Ok(entry) => entry,
        Err(outcome) => return outcome,
    };
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let statement = match dependency_statement(&snapshot.entries, entry, depends) {
        Ok(statement) => statement,
        Err(outcome) => return outcome,
    };
    after_validation();
    if let Err(outcome) =
        require_dependency_reachability_witnesses(&snapshot, entry, &statement, command)
    {
        return outcome;
    }
    let post = match Entry::new(
        statement.clone(),
        entry.workflow().clone(),
        entry.claim().cloned(),
        entry.lease().cloned(),
        projects,
    ) {
        Ok(post) => post,
        Err(error) => {
            return refusal(
                "task::dependency-refused",
                Some(error.to_string()),
                "submit a valid acyclic dependency edit",
            );
        }
    };
    let next = next_action(&snapshot, &post);
    commit_dependency_edit(bus, command, depends, entry, statement, next, at)
}

fn require_dependency_reachability_witnesses(
    snapshot: &ValidatedSnapshot,
    entry: &Entry,
    statement: &TaskHead,
    command: &wire::AdmittedCommand,
) -> Result<(), Outcome<CommandAnswer>> {
    let added = statement
        .dependencies
        .iter()
        .filter(|dependency| !entry.statement().dependencies.contains(dependency))
        .cloned()
        .collect::<Vec<_>>();
    let mut required = Vec::new();
    append_dependency_reachability_witnesses(&snapshot.answer, &added, &mut required)?;
    let missing = required
        .iter()
        .find(|witness| !command.expected().contains(witness));
    if let Some(missing) = missing {
        return Err(refusal(
            "task::dependency-witness-missing",
            Some(format!("{missing:?}")),
            "read the complete current prerequisite graph before editing dependencies",
        ));
    }
    Ok(())
}

fn dependency_entry<'a>(
    snapshot: &'a ValidatedSnapshot,
    depends: &TaskDepends,
) -> Result<&'a Entry, Outcome<CommandAnswer>> {
    let parked = referenced_oops(
        &snapshot.entries,
        std::iter::once(&depends.task).chain(depends.after.iter()),
    );
    if !parked.is_empty() {
        return Err(oops_dependency_refusal(&parked));
    }
    let Some(entry) = task_entry(&snapshot.entries, &depends.task) else {
        return Err(refusal(
            "task::unknown",
            Some(depends.task.to_string()),
            "read board and name one current task handle",
        ));
    };
    if entry.workflow().status == "completed" {
        return Err(refusal(
            "task::settled",
            Some(entry.handle().to_string()),
            "edit dependencies only while the task is pending",
        ));
    }
    Ok(entry)
}

fn commit_dependency_edit(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    depends: &TaskDepends,
    entry: &Entry,
    statement: TaskHead,
    next: Option<Box<str>>,
    at: Instant,
) -> Outcome<CommandAnswer> {
    let origin = match task_origin(entry.born()) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    let revision = u32::try_from(entry.statement().revision.get())
        .unwrap_or(u32::MAX)
        .saturating_add(1);
    let notice = task_edit_notice(entry, &statement);
    let edited = match write.fact(
        body::TaskEdited {
            task: entry.born().clone(),
            revision,
            notice,
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("task::depends-build", &error.to_string()),
    };
    write.task_head(edited, move |position| TaskHead {
        revision: position,
        ..statement
    });
    let answer = wire::intent::TaskAnswer {
        task: entry.handle().clone(),
        status: "edited".to_owned(),
        guidance: None,
        completion: None,
        learning: None,
        next,
    };
    if let Err(error) = write.command_result(
        request(command, depends.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("task::depends-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Task(answer))
}

fn dependency_statement(
    entries: &[Entry],
    entry: &Entry,
    depends: &TaskDepends,
) -> Result<TaskHead, Outcome<CommandAnswer>> {
    let prerequisites = depends
        .after
        .iter()
        .map(|after| task_entry(entries, after))
        .collect::<Vec<_>>();
    let mut resolved = Vec::with_capacity(depends.after.len());
    for (after, prerequisite) in depends.after.iter().zip(prerequisites) {
        let Some(prerequisite) = prerequisite else {
            return Err(refusal(
                "task::dependency-unknown",
                Some(after.to_string()),
                "read board and name an existing prerequisite",
            ));
        };
        resolved.push(prerequisite);
    }
    let mut statement = entry.statement().clone();
    let mut added = Vec::new();
    for prerequisite in resolved {
        if prerequisite.born() == entry.born() {
            return Err(refusal(
                "task::dependency-refused",
                Some(format!(
                    "{} after {}",
                    entry.handle(),
                    prerequisite.handle()
                )),
                "a task cannot depend on itself",
            ));
        }
        if statement.dependencies.contains(prerequisite.born())
            || added.contains(prerequisite.born())
        {
            continue;
        }
        if dependency_reaches(entries, prerequisite.born(), entry.born()) {
            return Err(refusal(
                "task::dependency-refused",
                Some(format!(
                    "{} after {}",
                    entry.handle(),
                    prerequisite.handle()
                )),
                "name dependencies that keep the complete task graph acyclic",
            ));
        }
        statement.dependencies.push(prerequisite.born().clone());
        added.push(prerequisite.born().clone());
    }
    Ok(statement)
}

fn dependency_reaches(entries: &[Entry], start: &TaskStamp, target: &TaskStamp) -> bool {
    let mut pending = vec![start.clone()];
    let mut visited = Vec::new();
    while let Some(current) = pending.pop() {
        if &current == target {
            return true;
        }
        if visited.contains(&current) {
            continue;
        }
        visited.push(current.clone());
        if let Some(entry) = entries.iter().find(|entry| entry.born() == &current) {
            pending.extend(entry.statement().dependencies.iter().cloned());
        }
    }
    false
}

pub(super) fn is_oops(entry: &Entry) -> bool {
    entry.project().visibility().hidden_stem() == Some("oops")
}

pub(super) fn oops_state(entry: &Entry) -> String {
    let claim = match entry.claim().map(|claim| claim.disposition) {
        Some(ClaimDisposition::Active) => "active",
        Some(ClaimDisposition::Freed) => "freed",
        Some(ClaimDisposition::Lapsed) => "lapsed",
        None => "none",
    };
    format!(
        "{} project={} status={} claim={claim}",
        entry.handle(),
        entry.project(),
        entry.workflow().status,
    )
}

fn oops_dependency_refusal(entries: &[&Entry]) -> Outcome<CommandAnswer> {
    let offenders = entries
        .iter()
        .map(|entry| oops_state(entry))
        .collect::<Vec<_>>()
        .join("; ");
    refusal(
        "task::dependency-oops-refused",
        Some(offenders.clone()),
        &format!(
            "parked dependency endpoint(s): {offenders}; promote every parked row first with spice task wake HANDLE --into PUBLIC_PROJECT",
        ),
    )
}

fn referenced_oops<'a, 'b>(
    entries: &'a [Entry],
    handles: impl IntoIterator<Item = &'b TaskHandle>,
) -> Vec<&'a Entry> {
    let mut parked = Vec::new();
    for born in handles.into_iter().filter_map(|handle| handle.born().ok()) {
        let Some(entry) = entries.iter().find(|entry| entry.born() == &born) else {
            continue;
        };
        if is_oops(entry)
            && !parked
                .iter()
                .any(|seen: &&Entry| seen.born() == entry.born())
        {
            parked.push(entry);
        }
    }
    parked
}
