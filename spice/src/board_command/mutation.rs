use super::*;

mod claim;
mod gate_findings;
mod taste_proposal;

pub(super) use claim::claim_win;
#[cfg(test)]
pub(super) use claim::{
    ClaimPlan, Possession, claim_git_boundary, claim_grant, claim_plan, commit_claim,
};
pub(super) use gate_findings::gate_findings;
#[cfg(test)]
pub(super) use gate_findings::{FindingRow, gate_findings_folded};
pub(super) use taste_proposal::taste_proposal_add;

/// One actor-filtered board snapshot whose cursor and entries were validated together.
pub(super) struct ValidatedSnapshot {
    pub(super) cursor: wire::Seq,
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
    at: Instant,
    statement: TaskHead,
    workflow: WorkflowHead,
}

pub(super) fn task_add(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    add: &TaskAdd,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
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
        projects,
        allocation,
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
        projects,
        allocation,
    )
}

fn create_task<B: serde::Serialize>(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    add: &TaskAdd,
    project: Project,
    request_body: B,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(bus, command.actor().clone(), projects, allocation) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let origin =
        match resolve_creation_origin(add.origin.as_ref(), command.actor(), &snapshot, None) {
            Ok(origin) => origin,
            Err(outcome) => return outcome,
        };
    let prepared = match prepare_task(command, add, &project, projects, origin) {
        Ok(prepared) => prepared,
        Err(outcome) => return outcome,
    };
    commit_prepared_task(
        bus,
        command,
        add,
        &project,
        request_body,
        projects,
        &snapshot,
        prepared,
        Vec::new(),
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
    .expect("prepared task already passed entry validation");
    let next = next_action(snapshot, &post, true);
    let mut write = JournalWrite::new(
        command.actor().clone(),
        prepared.origin,
        prepared.at,
        Producer::CommandDoor,
    );
    guard(&mut write, command.expected().to_vec());
    extra_guard(&mut write);
    let added = match write.fact(
        body::TaskAdded {
            task: prepared.born.clone(),
            title: add.title.clone(),
            description: add.description.clone(),
            acceptance: add.acceptance.clone(),
            project: prepared.project,
            tags,
            depends: prepared.statement.dependencies.clone(),
            flow: prepared.flow,
            phase: prepared.phase,
            revision: 1,
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("task::add-build", &error.to_string()),
    };
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
            "task add requires exactly one intended absent inception stamp",
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
    let flow = task_flow(add.flow.as_deref(), &add.acceptance)?;
    let phase = flow[0].clone();
    let at = current_instant()?;
    let statement = TaskHead {
        born: born.clone(),
        queue_origin: Seq::new(1).expect("placeholder queue origin is non-zero"),
        title: add.title.clone(),
        description: add.description.clone(),
        acceptance: add.acceptance.clone(),
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
                    "name dependencies carrying canonical inception stamps",
                )
            })?,
        provenance: origin.clone(),
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
        at,
        statement,
        workflow,
    })
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
    let Some(entry) = snapshot
        .entries
        .iter()
        .find(|entry| entry.handle() == &note.task)
    else {
        return refusal(
            "task::unknown",
            Some(note.task.to_string()),
            "read board and name one current task handle",
        );
    };
    let next = next_action(&snapshot, entry, false);
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
        task: note.task.clone(),
        status: "noted".to_owned(),
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
    task: &TaskRef,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    change_deferral(
        bus,
        command,
        &task.task,
        None,
        task.clone(),
        projects,
        allocation,
    )
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
    let Some(entry) = snapshot.entries.iter().find(|entry| entry.handle() == task) else {
        return refusal(
            "task::unknown",
            Some(task.to_string()),
            "read board and name one current task handle",
        );
    };
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
    write.workflow_head(entry.born().clone(), fact, move |_| workflow);
    let status = if deferral.is_some() {
        "deferred"
    } else {
        "pending"
    };
    let answer = wire::intent::TaskAnswer {
        task: task.clone(),
        status: status.to_owned(),
        learning: None,
        next: None,
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
    let Some(entry) = snapshot
        .entries
        .iter()
        .find(|entry| entry.handle() == &delete.task)
    else {
        return refusal(
            "task::unknown",
            Some(delete.task.to_string()),
            "read board and name one current task handle",
        );
    };
    if delete.reason.trim().is_empty() {
        return refusal(
            "task::delete-reason-empty",
            Some(delete.task.to_string()),
            "state why the task leaves the board",
        );
    }
    match entry.workflow().status.as_str() {
        "completed" => {
            return refusal(
                "task::settled",
                Some(delete.task.to_string()),
                "a completed task is history and stays on the board",
            );
        }
        "deleted" => {
            return refusal(
                "task::already-deleted",
                Some(delete.task.to_string()),
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
    let mut workflow = entry.workflow().clone();
    workflow.status = "deleted".to_owned();
    workflow.completion = Some(at);
    workflow.assignment = None;
    write.workflow_head(entry.born().clone(), deleted, move |_| workflow);
    if claimed.is_some() {
        let mut claim = entry.claim().expect("a held claim was just read").clone();
        claim.disposition = ClaimDisposition::Freed;
        write.claim_head(entry.born().clone(), deleted, move |fact| ClaimHead {
            fact,
            ..claim
        });
    }
    let answer = wire::intent::TaskAnswer {
        // A withdrawn task admits no further transition, so it offers no next
        // action. Naming one would point the caller at a row that can never
        // advance; the work goes on somewhere else, not here.
        task: delete.task.clone(),
        status: "deleted".to_owned(),
        learning: None,
        next: None,
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

pub(super) fn task_depends(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    depends: &TaskDepends,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(bus, command.actor().clone(), projects, allocation) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(entry) = snapshot
        .entries
        .iter()
        .find(|entry| entry.handle() == &depends.task)
    else {
        return refusal(
            "task::unknown",
            Some(depends.task.to_string()),
            "read board and name one current task handle",
        );
    };
    if entry.workflow().status == "completed" {
        return refusal(
            "task::settled",
            Some(depends.task.to_string()),
            "edit dependencies only while the task is pending",
        );
    }
    let mut statement = entry.statement().clone();
    for after in &depends.after {
        let Some(prerequisite) = snapshot
            .entries
            .iter()
            .find(|candidate| candidate.handle() == after)
        else {
            return refusal(
                "task::dependency-unknown",
                Some(after.to_string()),
                "read board and name an existing prerequisite",
            );
        };
        if prerequisite.born() == entry.born()
            || statement.dependencies.contains(prerequisite.born())
        {
            return refusal(
                "task::dependency-refused",
                Some(format!("{} after {after}", depends.task)),
                "name a distinct dependency edge that is not already present",
            );
        }
        statement.dependencies.push(prerequisite.born().clone());
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
    let next = next_action(&snapshot, &post, false);
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
    let revision = u32::try_from(entry.statement().revision.get())
        .unwrap_or(u32::MAX)
        .saturating_add(1);
    let edited = match write.fact(
        body::TaskEdited {
            task: entry.born().clone(),
            revision,
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
        task: depends.task.clone(),
        status: "edited".to_owned(),
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

/// Replace an unsettled task statement in place, at a fresh revision.
///
/// A correction is the same statement-head publication `task depends` performs,
/// so the durable record is a new revision rather than an overwrite: the prior
/// text stays readable in the journal and the edit carries the actor that made
/// it. Only fields the caller named move; the rest ride through untouched.
pub(super) fn task_edit(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    edit: &TaskEdit,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(bus, command.actor().clone(), projects, allocation) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(entry) = snapshot
        .entries
        .iter()
        .find(|entry| entry.handle() == &edit.task)
    else {
        return refusal(
            "task::unknown",
            Some(edit.task.to_string()),
            "read board and name one current task handle",
        );
    };
    // A withdrawn task is settled exactly as a completed one is, and both stamp
    // the same instant. Correcting a row that has already left the board would
    // publish a revision nobody reaches through the working board.
    if entry.workflow().completion.is_some() {
        return refusal(
            "task::settled",
            Some(edit.task.to_string()),
            "correct a statement only while the task is pending",
        );
    }
    let statement = match corrected_statement(entry, edit, projects) {
        Ok(statement) => statement,
        Err(outcome) => return outcome,
    };
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
                "task::edit-refused",
                Some(error.to_string()),
                "submit a statement the board admits",
            );
        }
    };
    // A correction replaces statement text and leaves the workflow where it
    // was, so the next action is whatever it already was, derived from the
    // corrected statement rather than restated here.
    let next = next_action(&snapshot, &post, false);
    let revision = u32::try_from(entry.statement().revision.get())
        .unwrap_or(u32::MAX)
        .saturating_add(1);
    publish_correction(bus, command, edit, statement, revision, next)
}

/// Build the statement a correction publishes, or the refusal it earns.
///
/// Only fields the caller named move; the rest ride through from the filed
/// statement untouched. A field this refuses is refused before anything is
/// written, so a correction that names two fields and gets one wrong publishes
/// neither.
fn corrected_statement(
    entry: &Entry,
    edit: &TaskEdit,
    projects: &ProjectPolicy,
) -> Result<TaskHead, Outcome<CommandAnswer>> {
    if edit.title.is_none()
        && edit.description.is_none()
        && edit.acceptance.is_empty()
        && edit.priority.is_none()
        && edit.project.is_none()
    {
        return Err(refusal(
            "task::edit-empty",
            Some(edit.task.to_string()),
            "name at least one of title, description, acceptance, priority, or project",
        ));
    }
    let mut statement = entry.statement().clone();
    if let Some(title) = &edit.title {
        statement.title = title.clone();
    }
    if let Some(description) = &edit.description {
        statement.description = Some(description.clone());
    }
    if !edit.acceptance.is_empty() {
        statement.acceptance = edit.acceptance.clone();
    }
    if let Some(priority) = &edit.priority {
        // A priority the allocator does not score is refused rather than
        // stored. Storing it would rank the task as if it declared nothing,
        // which is the silent version of the failure this correction exists to
        // repair.
        let Some(declared) = board::priority_from_authored(priority) else {
            return Err(refusal(
                "task::edit-priority-unknown",
                Some(priority.clone()),
                "name one of critical, high, medium, low, or none, or the letter the board prints",
            ));
        };
        // Every prior declaration goes, including a spelling nothing scores, so
        // a corrected task carries exactly one priority tag afterwards.
        statement
            .tags
            .retain(|tag| !tag.starts_with(board::PRIORITY_TAG_PREFIX));
        statement.tags.push(board::priority_tag(declared));
    }
    if let Some(requested) = &edit.project {
        // The destination goes through the same door `task add` uses. A
        // correction that reached further than creation would make this the way
        // to put a manually filed row on a resident-owned route.
        let project = match projects.admit_manual_creation(requested) {
            Ok(project) => project,
            Err(error) => {
                let reason = if matches!(error, ProjectError::SystemReserved(_)) {
                    "task::project-system-reserved"
                } else {
                    "task::edit-project-refused"
                };
                return Err(refusal(
                    reason,
                    Some(error.to_string()),
                    "name a public project this board admits for manual filing",
                ));
            }
        };
        // A move out of the editor's own subscription is a correction, not a
        // refusal: a task filed under the wrong project is visible only to the
        // lanes that project admits, so the lane that can see it has to be able
        // to send it where it belongs even though it loses the row by doing so.
        statement.project = project.as_str().to_owned();
    }
    Ok(statement)
}

/// Write one corrected statement as a fresh revision attributed to its editor.
fn publish_correction(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    edit: &TaskEdit,
    statement: TaskHead,
    revision: u32,
    next: Option<Box<str>>,
) -> Outcome<CommandAnswer> {
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(&statement.born) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    let edited = match write.fact(
        body::TaskEdited {
            task: statement.born.clone(),
            revision,
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("task::edit-build", &error.to_string()),
    };
    let rendered = TaskHandle::for_project(&statement.project, &statement.born);
    write.task_head(edited, move |position| TaskHead {
        revision: position,
        ..statement
    });
    let answer = wire::intent::TaskAnswer {
        task: rendered,
        status: "edited".to_owned(),
        learning: None,
        next,
    };
    if let Err(error) = write.command_result(
        request(command, edit.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("task::edit-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Task(answer))
}
