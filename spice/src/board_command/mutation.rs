struct ValidatedSnapshot {
    cursor: wire::Seq,
    answer: BoardAnswer,
    entries: Vec<Entry>,
}
fn task_add(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    add: &TaskAdd,
    projects: &ProjectPolicy,
) -> Outcome<CommandAnswer> {
    let handles = command
        .expected()
        .iter()
        .filter_map(|witness| match witness {
            Witness::Absence { task } => Some(task.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [task] = handles.as_slice() else {
        return protocol_fault(
            "task::add-witness-invalid",
            "task add requires exactly one intended absent handle",
        );
    };
    let task = task.clone();
    let project = add
        .project
        .clone()
        .unwrap_or_else(|| "task.private".to_owned());
    let flow = match phases(add.flow.as_deref()) {
        Ok(flow) => flow,
        Err(outcome) => return outcome,
    };
    let phase = flow[0].clone();
    let origin = match task_origin(&task) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let statement = TaskHead {
        handle: task.clone(),
        uuid: command.id(),
        title: add.title.clone(),
        project: project.clone(),
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
    if let Err(error) =
        Entry::new(statement.clone(), workflow.clone(), None, None, projects)
    {
        return refusal(
            "task::add-refused",
            Some(error.to_string()),
            "correct the task project, dependencies, or phase flow and retry",
        );
    }
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    let added = match write.fact(
        body::TaskAdded {
            task: task.clone(),
            uuid: command.id(),
            title: add.title.clone(),
            project,
            tags: Vec::new(),
            depends: add.depends.clone(),
            flow,
            phase,
            revision: 1,
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("task::add-build", &error.to_string()),
    };
    let statement_head = statement.clone();
    write.task_head(added, move |position| TaskHead {
        revision: position,
        ..statement_head
    });
    write.workflow_head(task.clone(), added, move |_| workflow);
    if add.project.as_deref() == Some("board.oops")
        && let Some(note) = &add.description
        && let Err(error) = write.fact(
            body::TaskNoted {
                task: task.clone(),
                note: note.clone(),
            },
            FactCause::Pending(added),
        )
    {
        return store_fault("task::oops-note-build", &error.to_string());
    }
    let answer = wire::intent::TaskAnswer {
        task: task.clone(),
        status: "pending".to_owned(),
    };
    if let Err(error) = write.command_result(
        request(command, add.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("task::add-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Task(answer))
}

fn task_note(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    note: &TaskNote,
) -> Outcome<CommandAnswer> {
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

fn task_defer(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    defer: &TaskDefer,
    projects: &ProjectPolicy,
) -> Outcome<CommandAnswer> {
    change_deferral(
        bus,
        command,
        &defer.task,
        Some(defer.wait),
        defer.clone(),
        projects,
    )
}

fn task_wake(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    task: &TaskRef,
    projects: &ProjectPolicy,
) -> Outcome<CommandAnswer> {
    change_deferral(bus, command, &task.task, None, task.clone(), projects)
}

fn change_deferral<B: serde::Serialize + Clone>(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    task: &TaskHandle,
    deferral: Option<Instant>,
    intent: B,
    projects: &ProjectPolicy,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(bus, command.actor().clone(), projects) {
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
    if entry.workflow().status == "completed" {
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
    };
    if let Err(error) =
        write.command_result(request(command, intent), command.epoch(), answer.clone())
    {
        return store_fault("task::deferral-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Task(answer))
}

fn task_depends(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    depends: &TaskDepends,
    projects: &ProjectPolicy,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(bus, command.actor().clone(), projects) {
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
    if let Err(error) = Entry::new(
        statement.clone(),
        entry.workflow().clone(),
        entry.claim().cloned(),
        entry.lease().cloned(),
        projects,
    ) {
        return refusal(
            "task::dependency-refused",
            Some(error.to_string()),
            "submit a valid acyclic dependency edit",
        );
    }
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

fn claim_win(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    win: &ClaimWin,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
) -> Outcome<CommandAnswer> {
    let capture = match capture_rule(win) {
        Ok(capture) => capture,
        Err(outcome) => return outcome,
    };
    let snapshot = match validated_snapshot(
        bus,
        command.actor().clone(),
        projects,
    ) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let selected = match claim_selection(
        &snapshot,
        command,
        capture,
    ) {
        Ok(selected) => selected,
        Err(outcome) => return outcome,
    };
    let task = selected.statement().handle.clone();
    let (lane, occupant) = match claim_residency(command) {
        Ok(seat) => seat,
        Err(outcome) => return outcome,
    };
    let (at, expiry, maximum) = match claim_deadlines() {
        Ok(deadlines) => deadlines,
        Err(outcome) => return outcome,
    };
    let git = match claim_git_boundary(
        repository,
        &snapshot,
        command,
        capture,
    ) {
        Ok(git) => git,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(&task) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(
        command.actor().clone(),
        origin,
        at,
        Producer::CommandDoor,
    );
    guard_claim(
        &mut write,
        &snapshot,
        command,
        capture,
        &task,
        projects,
    );
    if let Err(outcome) = append_claim_heads(
        &mut write,
        command,
        selected,
        &task,
        lane,
        occupant,
        at,
        expiry,
        maximum,
        git,
    ) {
        return outcome;
    }
    let answer = wire::intent::ClaimAnswer {
        task: task.clone(),
        holder: command.actor().clone(),
    };
    if let Err(error) = write.command_result(
        request(command, win.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault(
            "claim::win-receipt",
            &error.to_string(),
        );
    }
    committed(bus, write, CommandAnswer::Claim(answer))
}

fn capture_rule(
    win: &ClaimWin,
) -> Result<bool, Outcome<CommandAnswer>> {
    match win.rule.as_str() {
        "next" => Ok(false),
        "capture" => Ok(true),
        _ => Err(refusal(
            "board::allocator-rule",
            Some(win.rule.to_string()),
            "use the next or capture possession rule",
        )),
    }
}

fn claim_selection<'a>(
    snapshot: &'a ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    capture: bool,
) -> Result<&'a Entry, Outcome<CommandAnswer>> {
    if !capture {
        return board::select(
            &snapshot.entries,
            command.actor(),
        )
        .map(|selected| selected.entry())
        .ok_or_else(|| {
            refusal(
                "board::allocator-empty",
                None,
                concat!(
                    "add or wake eligible work before ",
                    "requesting task next",
                ),
            )
        });
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
    if board::eligible_entry(
        &snapshot.entries,
        entry,
        command.actor(),
    ) {
        Ok(entry)
    } else {
        Err(refusal(
            "claim::capture-ineligible",
            Some(task.to_string()),
            "capture only an unclaimed, ready, pending task",
        ))
    }
}

fn claim_residency(
    command: &wire::AdmittedCommand,
) -> Result<
    (wire::CanonicalUuid, wire::CanonicalUuid),
    Outcome<CommandAnswer>,
> {
    let seat = command
        .expected()
        .iter()
        .find_map(|witness| match witness {
            Witness::Residency {
                lane,
                occupant,
                epoch,
            } if *epoch == command.epoch() => {
                Some((*lane, *occupant))
            }
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

fn claim_deadlines(
) -> Result<(Instant, Instant, Instant), Outcome<CommandAnswer>> {
    let at = current_instant()?;
    let expiry = instant_after(at, 15 * 60)?;
    let maximum = instant_after(at, 60 * 60)?;
    Ok((at, expiry, maximum))
}

fn claim_git_boundary(
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
        spice::task_git::capture(&lane)
            .map(|capture| capture.boundary)
    } else {
        spice::task_git::prepare_claim(&lane)
    };
    result.map(Some).map_err(task_git_outcome)
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
            heads,
            task.clone(),
            command.actor().clone(),
            projects.clone(),
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
) -> Result<(), Outcome<CommandAnswer>> {
    let won = write
        .fact(
            body::ClaimWon {
                task: task.clone(),
                actor: command.actor().clone(),
                lane,
                occupant,
                epoch: command.epoch(),
                until: expiry,
                git,
            },
            FactCause::Root,
        )
        .map_err(|error| {
            store_fault("claim::win-build", &error.to_string())
        })?;
    let actor = command.actor().clone();
    let epoch = command.epoch();
    write.claim_head(task.clone(), won, move |fact| {
        ClaimHead {
            actor,
            fact,
            lane,
            occupant,
            epoch,
            disposition: ClaimDisposition::Active,
        }
    });
    write.lease_head(task.clone(), won, move |_| LeaseHead {
        start: at,
        renewal: at,
        expiry,
        maximum,
        epoch,
    });
    if selected.workflow().phase.as_str() == "review" {
        let mut workflow = selected.workflow().clone();
        workflow.assignment = Some(command.actor().clone());
        workflow.review = "assigned".to_owned();
        write.workflow_head(task.clone(), won, move |_| workflow);
    }
    Ok(())
}
