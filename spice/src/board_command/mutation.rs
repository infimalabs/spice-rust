use super::*;

/// One actor-filtered board snapshot whose cursor and entries were validated together.
pub(super) struct ValidatedSnapshot {
    pub(super) cursor: wire::Seq,
    pub(super) answer: BoardAnswer,
    pub(super) entries: Vec<Entry>,
}

struct PreparedTask {
    task: TaskHandle,
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
        origin: None,
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
    let prepared = match prepare_task(command, add, &project, projects) {
        Ok(prepared) => prepared,
        Err(outcome) => return outcome,
    };
    let snapshot = match validated_snapshot(bus, command.actor().clone(), projects, allocation) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let post = Entry::new(
        prepared.statement.clone(),
        prepared.workflow.clone(),
        None,
        None,
        projects,
    )
    .expect("prepared task already passed entry validation");
    let next = next_action(&snapshot, &post, true);
    let mut write = JournalWrite::new(
        command.actor().clone(),
        prepared.origin,
        prepared.at,
        Producer::CommandDoor,
    );
    guard(&mut write, command.expected().to_vec());
    let added = match write.fact(
        body::TaskAdded {
            task: prepared.task.clone(),
            uuid: command.id(),
            title: add.title.clone(),
            description: add.description.clone(),
            acceptance: add.acceptance.clone(),
            project: prepared.project,
            tags: Vec::new(),
            depends: add.depends.clone(),
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
        revision: position,
        ..statement
    });
    write.workflow_head(prepared.task.clone(), added, move |_| prepared.workflow);
    if let Err(outcome) = add_oops_note(&mut write, &project, add, &prepared.task, added) {
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
            "task add requires exactly one intended absent handle",
        ));
    };
    let task = task.clone();
    let project_text = project.as_str().to_owned();
    let flow = phases(add.flow.as_deref())?;
    let phase = flow[0].clone();
    let origin = match add.origin.clone() {
        Some(origin) => origin,
        None => task_origin(&task)?,
    };
    let at = current_instant()?;
    let statement = TaskHead {
        handle: task.clone(),
        uuid: command.id(),
        title: add.title.clone(),
        description: add.description.clone(),
        acceptance: add.acceptance.clone(),
        project: project_text.clone(),
        tags: Vec::new(),
        dependencies: add.depends.clone(),
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
    task: &TaskHandle,
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
        .find(|entry| entry.statement().handle == note.task)
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
    let origin = match task_origin(&note.task) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    if let Err(error) = write.fact(
        body::TaskNoted {
            task: note.task.clone(),
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
    let Some(entry) = snapshot
        .entries
        .iter()
        .find(|entry| entry.statement().handle == *task)
    else {
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
    let origin = match task_origin(task) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    let fact = if let Some(until) = deferral {
        write.fact(
            body::TaskDeferred {
                task: task.clone(),
                until,
            },
            FactCause::Root,
        )
    } else {
        write.fact(body::TaskWoken { task: task.clone() }, FactCause::Root)
    };
    let fact = match fact {
        Ok(fact) => fact,
        Err(error) => return store_fault("task::deferral-build", &error.to_string()),
    };
    let mut workflow = entry.workflow().clone();
    workflow.deferral = deferral;
    write.workflow_head(task.clone(), fact, move |_| workflow);
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
        .find(|entry| entry.statement().handle == delete.task)
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
    let origin = match task_origin(&delete.task) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    let deleted = match write.fact(
        body::TaskDeleted {
            task: delete.task.clone(),
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
    write.workflow_head(delete.task.clone(), deleted, move |_| workflow);
    if claimed.is_some() {
        let mut claim = entry.claim().expect("a held claim was just read").clone();
        claim.disposition = ClaimDisposition::Freed;
        write.claim_head(delete.task.clone(), deleted, move |fact| ClaimHead {
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
        .find(|entry| entry.statement().handle == depends.task)
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
        if after == &depends.task || statement.dependencies.contains(after) {
            return refusal(
                "task::dependency-refused",
                Some(format!("{} after {after}", depends.task)),
                "name a distinct dependency edge that is not already present",
            );
        }
        if !snapshot
            .entries
            .iter()
            .any(|candidate| candidate.statement().handle == *after)
        {
            return refusal(
                "task::dependency-unknown",
                Some(after.to_string()),
                "read board and name an existing prerequisite",
            );
        }
        statement.dependencies.push(after.clone());
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
    let origin = match task_origin(&depends.task) {
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
            task: depends.task.clone(),
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
        .find(|entry| entry.statement().handle == edit.task)
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
    let origin = match task_origin(&edit.task) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    let edited = match write.fact(
        body::TaskEdited {
            task: edit.task.clone(),
            revision,
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("task::edit-build", &error.to_string()),
    };
    write.task_head(edited, move |position| TaskHead {
        revision: position,
        ..statement
    });
    let answer = wire::intent::TaskAnswer {
        task: edit.task.clone(),
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

/// One decided claim and the board snapshot it was decided against.
///
/// The allocator's re-read guard exists precisely because deciding a claim and
/// writing it down are separated in time. Naming that separation is what makes
/// the hazard expressible: everything `commit_claim` writes, the guard's
/// observation included, is derived from the one snapshot `claim_plan` read,
/// never from a second read taken later. This is the shape `DocPlan` already
/// runs, where the plan carries the snapshot its decision was made against and
/// the later steps consume it.
pub(super) struct ClaimPlan {
    snapshot: ValidatedSnapshot,
    pub(super) selected: Entry,
    grant: ClaimGrant,
    next: Option<Box<str>>,
    possession: Possession,
}

pub(super) fn claim_win(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    win: &ClaimWin,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let plan = match claim_plan(bus, command, win, repository, projects, allocation) {
        Ok(plan) => plan,
        Err(outcome) => return outcome,
    };
    commit_claim(bus, command, win, projects, plan)
}

/// Decide one claim against a single board read.
///
/// Every refusal a caller could have avoided is raised here, in order, and the
/// caller sees the first one only.
pub(super) fn claim_plan(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    win: &ClaimWin,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Result<ClaimPlan, Outcome<CommandAnswer>> {
    let possession = possession_rule(win)?;
    let snapshot = validated_snapshot(bus, command.actor().clone(), projects, allocation)?;
    let selected = claim_selection(&snapshot, command, possession)?.clone();
    let grant = claim_grant(bus, command, repository, &snapshot, possession)?;
    let next = projected_claim_next_action(
        &snapshot,
        &selected,
        command,
        grant.lane,
        grant.occupant,
        grant.at,
        grant.expiry,
        grant.maximum,
        projects,
    );
    Ok(ClaimPlan {
        snapshot,
        selected,
        grant,
        next,
        possession,
    })
}

/// What a claim settles before the journal opens.
///
/// These resolve together because they refuse together, and the order is the
/// contract rather than an accident of writing: each step here can refuse, and
/// the caller sees the first refusal only, so a seat that cannot be read is
/// reported before a boundary that cannot be captured.
pub(super) struct ClaimGrant {
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
    at: Instant,
    expiry: Instant,
    maximum: Instant,
    git: Option<GitBoundary>,
    source: body::SourceBoundary,
}

/// Resolve the seat, the deadlines and the two boundaries one claim records.
pub(super) fn claim_grant(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    repository: Option<&Repository>,
    snapshot: &ValidatedSnapshot,
    possession: Possession,
) -> Result<ClaimGrant, Outcome<CommandAnswer>> {
    let (lane, occupant) = claim_residency(command)?;
    let (at, expiry, maximum) = claim_deadlines()?;
    let git = claim_git_boundary(repository, snapshot, command, possession.captures())?;
    let source = claim_source_boundary(bus, &lane)?;
    Ok(ClaimGrant {
        lane,
        occupant,
        at,
        expiry,
        maximum,
        git,
        source,
    })
}

/// Write the decided claim: one origin, the guards it owes, its two heads, its
/// receipt.
///
/// Nothing here refuses on what the caller asked for; by this point the command
/// is already admitted and the plan already decided, so every failure below is
/// a fault rather than a refusal the caller could have avoided: a validated
/// handle that will not form an origin is a protocol fault, and the mint and the
/// receipt are store faults.
///
/// The plan arrives whole rather than as loose arguments so the guard's
/// observation can only come from the snapshot the decision was made against.
pub(super) fn commit_claim(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    win: &ClaimWin,
    projects: &ProjectPolicy,
    plan: ClaimPlan,
) -> Outcome<CommandAnswer> {
    let ClaimPlan {
        snapshot,
        selected,
        grant,
        next,
        possession,
    } = plan;
    let task = selected.statement().handle.clone();
    let origin = match task_origin(&task) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(
        command.actor().clone(),
        origin,
        grant.at,
        Producer::CommandDoor,
    );
    guard_claim(
        &mut write,
        &snapshot,
        command,
        possession.captures(),
        &task,
        projects,
    );
    if let Err(outcome) = append_claim_heads(
        &mut write,
        command,
        &selected,
        &task,
        grant.lane,
        grant.occupant,
        grant.at,
        grant.expiry,
        grant.maximum,
        grant.git,
        grant.source,
    ) {
        return outcome;
    }
    let answer = wire::intent::ClaimAnswer {
        task,
        holder: command.actor().clone(),
        warnings: board::claim_overlaps(&selected, &snapshot.entries)
            .into_iter()
            .map(|overlap| wire::intent::ClaimWarning {
                task: overlap.task,
                lane: overlap.lane,
                surface: overlap.surface,
            })
            .collect(),
        next,
    };
    if let Err(error) = write.command_result(
        request(command, win.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("claim::win-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Claim(answer))
}

#[allow(
    clippy::too_many_arguments,
    reason = "the projected claim is the exact pair of heads the transaction appends"
)]
fn projected_claim_next_action(
    snapshot: &ValidatedSnapshot,
    selected: &Entry,
    command: &wire::AdmittedCommand,
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
    at: Instant,
    expiry: Instant,
    maximum: Instant,
    projects: &ProjectPolicy,
) -> Option<Box<str>> {
    let mut workflow = selected.workflow().clone();
    if workflow.phase.as_str() == "review" {
        workflow.assignment = Some(command.actor().clone());
        workflow.review = "assigned".to_owned();
    }
    let post = Entry::new(
        selected.statement().clone(),
        workflow,
        Some(ClaimHead {
            actor: command.actor().clone(),
            fact: snapshot.cursor,
            lane,
            occupant,
            epoch: command.epoch(),
            disposition: ClaimDisposition::Active,
        }),
        Some(LeaseHead {
            start: at,
            renewal: at,
            expiry,
            maximum,
            epoch: command.epoch(),
        }),
        projects,
    )
    .expect("a validated selected entry accepts the transaction's claim heads");
    next_action(snapshot, &post, false)
}

/// Which possession law one claim win asks the board to apply.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Possession {
    /// The allocator chooses among tasks nobody holds.
    Next,
    /// The caller names a task nobody holds.
    Capture,
    /// The caller names a task and takes it from its current holder.
    Steal,
}

impl Possession {
    /// Whether the lane's local commits become this claim's boundary.
    const fn captures(self) -> bool {
        !matches!(self, Self::Next)
    }
}

fn possession_rule(win: &ClaimWin) -> Result<Possession, Outcome<CommandAnswer>> {
    match win.rule.as_str() {
        "next" => Ok(Possession::Next),
        "capture" => Ok(Possession::Capture),
        "steal" => Ok(Possession::Steal),
        _ => Err(refusal(
            "board::allocator-rule",
            Some(win.rule.to_string()),
            "use the next, capture or steal possession rule",
        )),
    }
}

fn claim_selection<'a>(
    snapshot: &'a ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    possession: Possession,
) -> Result<&'a Entry, Outcome<CommandAnswer>> {
    if !possession.captures() {
        let subscription =
            allocation_subscription(&snapshot.answer.residency.body, command.actor())?;
        let selected = board::select(
            &snapshot.entries,
            command.actor(),
            subscription,
            snapshot.answer.allocation_band,
        )
        .map(|selected| selected.entry())
        .ok_or_else(|| {
            refusal(
                "board::allocator-empty",
                None,
                concat!("add or wake eligible work before ", "requesting task next",),
            )
        })?;
        let witnessed = command
            .expected()
            .iter()
            .find_map(|witness| match witness {
                Witness::Task { task, .. } => Some(task),
                _ => None,
            })
            .ok_or_else(|| {
                protocol_fault(
                    "board::allocator-task-witness",
                    "task next has no selected task witness",
                )
            })?;
        if witnessed != &selected.statement().handle {
            return Err(refusal(
                "board::witness-moved",
                Some(format!(
                    "witnessed {witnessed}; selected {}",
                    selected.statement().handle
                )),
                "read the current board subscription and retry task next",
            ));
        }
        return Ok(selected);
    }
    let task = command
        .expected()
        .iter()
        .find_map(|witness| match witness {
            Witness::Task { task, .. } => Some(task),
            _ => None,
        })
        .ok_or_else(|| {
            protocol_fault(
                "claim::capture-task-witness",
                "capture possession has no exact task witness",
            )
        })?;
    let entry = snapshot
        .entries
        .iter()
        .find(|entry| entry.statement().handle == *task)
        .ok_or_else(|| {
            refusal(
                "task::unknown",
                Some(task.to_string()),
                "capture into one current pending task",
            )
        })?;
    let (admitted, reason, remedy) = match possession {
        Possession::Steal => (
            board::stealable_entry(&snapshot.entries, entry, command.actor()),
            "claim::steal-ineligible",
            "steal only a ready, pending task",
        ),
        _ => (
            board::eligible_entry(&snapshot.entries, entry, command.actor()),
            "claim::capture-ineligible",
            "capture only an unclaimed, ready, pending task",
        ),
    };
    if admitted {
        Ok(entry)
    } else {
        Err(refusal(reason, Some(task.to_string()), remedy))
    }
}

fn claim_residency(
    command: &wire::AdmittedCommand,
) -> Result<(wire::CanonicalUuid, wire::CanonicalUuid), Outcome<CommandAnswer>> {
    let seat = command
        .expected()
        .iter()
        .find_map(|witness| match witness {
            Witness::Residency {
                lane,
                occupant,
                epoch,
            } if *epoch == command.epoch() => Some((*lane, *occupant)),
            _ => None,
        })
        .ok_or_else(|| {
            protocol_fault(
                "claim::residency-witness",
                "claim win has no current residency witness",
            )
        })?;
    if let Actor::Lane(actor) = command.actor()
        && *actor != seat.0
    {
        return Err(refusal(
            "claim::lane-mismatch",
            Some(seat.0.to_string()),
            concat!(
                "claim through the lane bound to the ",
                "authenticated capability",
            ),
        ));
    }
    Ok(seat)
}

fn claim_deadlines() -> Result<(Instant, Instant, Instant), Outcome<CommandAnswer>> {
    let at = current_instant()?;
    let expiry = instant_after(at, 15 * 60)?;
    let maximum = instant_after(at, 60 * 60)?;
    Ok((at, expiry, maximum))
}

pub(super) fn claim_git_boundary(
    repository: Option<&Repository>,
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    capture: bool,
) -> Result<Option<GitBoundary>, Outcome<CommandAnswer>> {
    let Some(resident) = repository else {
        return Ok(None);
    };
    let lane = task_repository(snapshot, command, resident)?;
    let result = if capture {
        spice::task_git::capture(&lane).map(|capture| capture.boundary)
    } else {
        spice::task_git::prepare_claim(&lane)
    };
    result.map(Some).map_err(task_git_outcome)
}

/// Read the lane's accounted transcript boundary for the claim heads.
///
/// The read goes through the writer queue, so it can fail on a closed or
/// faulted bus rather than merely returning nothing; that failure is a store
/// fault instead of a refusal because no command the caller could have sent
/// would have avoided it.
fn claim_source_boundary(
    bus: &Bus,
    lane: &wire::CanonicalUuid,
) -> Result<body::SourceBoundary, Outcome<CommandAnswer>> {
    bus.source_boundary(lane)
        .map_err(|error| store_fault("claim::boundary-unreadable", &error.to_string()))
}

fn guard_claim(
    write: &mut JournalWrite,
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    capture: bool,
    task: &TaskHandle,
    projects: &ProjectPolicy,
) {
    let expected = command.expected().to_vec();
    let heads = allocation_heads(&snapshot.answer);
    if capture {
        guard_capture(write, expected, heads);
    } else {
        guard_allocation(
            write,
            expected,
            AllocationObservation {
                heads,
                winner: task.clone(),
                actor: command.actor().clone(),
                subscription: allocation_subscription(
                    &snapshot.answer.residency.body,
                    command.actor(),
                )
                .expect("claim selection admitted one durable lineage subscription")
                .clone(),
                residency_position: snapshot.answer.residency.position,
                projects: projects.clone(),
                band: snapshot.answer.allocation_band,
            },
        );
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "one claim fact and its two heads share one boundary"
)]
fn append_claim_heads(
    write: &mut JournalWrite,
    command: &wire::AdmittedCommand,
    selected: &Entry,
    task: &TaskHandle,
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
    at: Instant,
    expiry: Instant,
    maximum: Instant,
    git: Option<GitBoundary>,
    source: body::SourceBoundary,
) -> Result<(), Outcome<CommandAnswer>> {
    board::allocate::mint(
        write,
        board::allocate::Mint {
            task,
            actor: command.actor(),
            workflow: selected.workflow(),
            lane,
            occupant,
            epoch: command.epoch(),
            at,
            expiry,
            maximum,
            git,
            source,
        },
    )
    .map(|_| ())
    .map_err(|error| match error {
        board::allocate::MintError::Assignment(error) => {
            store_fault("review::assignment-build", &error.to_string())
        }
        board::allocate::MintError::Claim(error) => {
            store_fault("claim::win-build", &error.to_string())
        }
    })
}
