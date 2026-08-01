struct ValidatedSnapshot {
    cursor: wire::Seq,
    answer: BoardAnswer,
    entries: Vec<Entry>,
}

fn task_add(bus: &Bus, command: &wire::AdmittedCommand, add: &TaskAdd) -> Outcome<CommandAnswer> {
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
    if let Err(error) = Entry::new(statement.clone(), workflow.clone(), None, None) {
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
) -> Outcome<CommandAnswer> {
    change_deferral(bus, command, &defer.task, Some(defer.wait), defer.clone())
}

fn task_wake(bus: &Bus, command: &wire::AdmittedCommand, task: &TaskRef) -> Outcome<CommandAnswer> {
    change_deferral(bus, command, &task.task, None, task.clone())
}

fn change_deferral<B: serde::Serialize + Clone>(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    task: &TaskHandle,
    deferral: Option<Instant>,
    intent: B,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(bus, command.actor().clone()) {
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
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(bus, command.actor().clone()) {
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

fn claim_win(bus: &Bus, command: &wire::AdmittedCommand, win: &ClaimWin) -> Outcome<CommandAnswer> {
    if win.rule.as_str() != "next" {
        return refusal(
            "board::allocator-rule",
            Some(win.rule.to_string()),
            "use the sole current next allocator rule",
        );
    }
    let snapshot = match validated_snapshot(bus, command.actor().clone()) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(selected) = board::select(&snapshot.entries, command.actor()) else {
        return refusal(
            "board::allocator-empty",
            None,
            "add or wake eligible work before requesting task next",
        );
    };
    let task = selected.entry().statement().handle.clone();
    let Some((lane, occupant)) = command.expected().iter().find_map(|witness| match witness {
        Witness::Residency {
            lane,
            occupant,
            epoch,
        } if *epoch == command.epoch() => Some((*lane, *occupant)),
        _ => None,
    }) else {
        return protocol_fault(
            "claim::residency-witness",
            "claim win has no current residency witness",
        );
    };
    if let Actor::Lane(actor) = command.actor()
        && *actor != lane
    {
        return refusal(
            "claim::lane-mismatch",
            Some(lane.to_string()),
            "claim through the lane bound to the authenticated capability",
        );
    }
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let expiry = match instant_after(at, 15 * 60) {
        Ok(expiry) => expiry,
        Err(outcome) => return outcome,
    };
    let maximum = match instant_after(at, 60 * 60) {
        Ok(maximum) => maximum,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(&task) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard_allocation(
        &mut write,
        command.expected().to_vec(),
        allocation_heads(&snapshot.answer),
        task.clone(),
        command.actor().clone(),
    );
    let won = match write.fact(
        body::ClaimWon {
            task: task.clone(),
            actor: command.actor().clone(),
            lane,
            occupant,
            epoch: command.epoch(),
            until: expiry,
        },
        FactCause::Root,
    ) {
        Ok(fact) => fact,
        Err(error) => return store_fault("claim::win-build", &error.to_string()),
    };
    let actor = command.actor().clone();
    let epoch = command.epoch();
    write.claim_head(task.clone(), won, move |fact| ClaimHead {
        actor,
        fact,
        lane,
        occupant,
        epoch,
        disposition: ClaimDisposition::Active,
    });
    write.lease_head(task.clone(), won, move |_| LeaseHead {
        start: at,
        renewal: at,
        expiry,
        maximum,
        epoch,
    });
    if selected.entry().workflow().phase.as_str() == "review" {
        let mut workflow = selected.entry().workflow().clone();
        workflow.assignment = Some(command.actor().clone());
        workflow.review = "assigned".to_owned();
        write.workflow_head(task.clone(), won, move |_| workflow);
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
        return store_fault("claim::win-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Claim(answer))
}
